//! One Borealis workspace per place a person works in.
//!
//! Layout under the data folder (ids are server-made UUIDs):
//!
//! ```text
//! users/<account>/personal/           an account's characters that are in no group: bundled content only
//! users/<account>/library/            the account's content packs, used by the groups it runs
//! users/<account>/studio/             its Library screen (library -> ../library)
//! groups/<g>/dm/                      the DM's view of a group; its content-prefs.json says which packs the group uses
//! groups/<g>/members/<player>/        a member's character (library -> DM's; content-prefs.json and settings.json,
//!                                     the house rules, -> the DM's above)
//! ```
//!
//! Which content a character sees is therefore decided purely by the folder its file sits in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::backend as headless;
use serde_json::{json, Map, Value};
use tauri::AppHandle;
use tokio::sync::{broadcast, OnceCell};

use crate::db::now;
use crate::error::{ApiError, ApiResult};

const IDLE_SECONDS: i64 = 30 * 60;

/// What the data folder holds.
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    /// Bytes in files.
    pub bytes: u64,
    /// Character files.
    pub characters: u64,
    /// Content packs in libraries.
    pub packs: u64,
}

/// A place to work. Every account has a `Personal` place and a `Library`, both keyed by its id; in a group it is
/// `GroupDm` when it runs the group and `GroupMember` when it plays in it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WsKey {
    /// An account's characters outside any group.
    Personal {
        /// The account.
        player: String,
    },
    /// An account's content library.
    Library {
        /// The account.
        dm: String,
    },
    /// A DM inside one of their groups.
    GroupDm {
        /// The group.
        group: String,
        /// Its DM.
        dm: String,
    },
    /// A player inside a group.
    GroupMember {
        /// The group.
        group: String,
        /// Its DM, whose library it uses.
        dm: String,
        /// The player.
        player: String,
    },
}

/// An event from a workspace to its clients.
#[derive(Debug, Clone)]
pub struct WorkspaceEvent {
    /// The Tauri event name.
    pub name: String,
    /// Its payload.
    pub payload: Value,
}

/// An open workspace.
pub struct Workspace {
    handle: AppHandle,
    /// The workspace's event stream.
    pub events: broadcast::Sender<WorkspaceEvent>,
    last_used: AtomicI64,
    /// A group member's: the house rules are the DM's, so its characters keep none of their own.
    group_rules: bool,
}

impl Workspace {
    /// Run a command, off the async threads.
    pub async fn call(&self, command: &str, args: Map<String, Value>) -> ApiResult<Value> {
        self.last_used.store(now(), Ordering::Relaxed);
        let handle = self.handle.clone();
        let name = command.to_string();
        let group_rules = self.group_rules;
        let mut args = args;
        let outcome = tokio::task::spawn_blocking(move || -> ApiResult<_> {
            if group_rules {
                house_rules_first(&handle, &name, &mut args)?;
            }
            let outcome = headless::invoke(&handle, &name, args);
            Ok(match outcome {
                Some(Ok(value)) if group_rules => {
                    Some(Ok(house_rules_after(&handle, &name, value)?))
                }
                other => other,
            })
        })
        .await
        .map_err(|e| ApiError::Internal(format!("command task: {e}")))??;
        self.last_used.store(now(), Ordering::Relaxed);
        match outcome {
            Some(Ok(value)) => Ok(value),
            Some(Err(error)) => Err(ApiError::Command(error)),
            None => Err(ApiError::NotFound("No such command.".into())),
        }
    }

    /// Replace some settings in memory only (an account's own settings).
    pub fn overlay_settings(&self, fields: &Map<String, Value>) -> ApiResult<()> {
        headless::overlay_settings(&self.handle, fields).map_err(ApiError::Internal)
    }

    /// What a command requires, or `None` when the server does not offer it.
    pub fn access_of(&self, command: &str) -> Option<headless::Access> {
        headless::access_of(&self.handle, command)
    }
}

const SET_BY_DM: &str = "Your Dungeon Master sets this group's house rules.";

/// In a group the house rules are the Dungeon Master's: a character's own can't be switched on, its ability method is
/// the group's, and a new character starts that way.
fn house_rules_first(
    handle: &AppHandle,
    command: &str,
    args: &mut Map<String, Value>,
) -> ApiResult<()> {
    let id = args
        .get("characterId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let refused = match command {
        "set_character_options" => {
            let options = args.get("options").cloned().unwrap_or(Value::Null);
            !headless::changed_house_rules(handle, &id, &options)
                .map_err(ApiError::Internal)?
                .is_empty()
        }
        "set_ability_method" => args.get("method") != Some(&headless::group_ability_method(handle)),
        "create_character" => {
            if let Some(request) = args.get_mut("request").and_then(Value::as_object_mut) {
                request.insert(
                    "abilityMethod".into(),
                    headless::group_ability_method(handle),
                );
                if let Some(options) = request.get_mut("options").and_then(Value::as_object_mut) {
                    for key in headless::HOUSE_RULE_OPTIONS {
                        options.remove(key);
                    }
                }
            }
            false
        }
        _ => false,
    };
    if refused {
        Err(ApiError::Forbidden(SET_BY_DM.into()))
    } else {
        Ok(())
    }
}

/// A character that comes into a group (made here, imported, moved in from outside any group, or there before the DM
/// changed the rules) is brought in line with the group's rules the moment it opens, whoever opens it.
fn house_rules_after(handle: &AppHandle, command: &str, value: Value) -> ApiResult<Value> {
    if !matches!(command, "load_character" | "create_character") {
        return Ok(value);
    }
    let Some(id) = value.get("id").and_then(Value::as_str) else {
        return Ok(value);
    };
    let cleared = headless::apply_group_rules(handle, id).map_err(ApiError::Internal)?;
    Ok(cleared.unwrap_or(value))
}

type Cell = Arc<OnceCell<Arc<Workspace>>>;

/// All open workspaces.
pub struct Workspaces {
    data: PathBuf,
    content: PathBuf,
    open: Mutex<HashMap<WsKey, Cell>>,
}

fn invalid_id(id: &str) -> bool {
    id.is_empty()
        || id.len() > 128
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Workspaces {
    /// Workspaces under `data`, with the bundled packs in `content`.
    pub fn new(data: PathBuf, content: PathBuf) -> Self {
        Self {
            data,
            content,
            open: Mutex::new(HashMap::new()),
        }
    }

    fn dm_library(&self, dm: &str) -> PathBuf {
        self.data.join("users").join(dm).join("library")
    }

    /// A workspace's data folder.
    pub fn dir(&self, key: &WsKey) -> PathBuf {
        match key {
            WsKey::Personal { player } => self.data.join("users").join(player).join("personal"),
            WsKey::Library { dm } => self.data.join("users").join(dm).join("studio"),
            WsKey::GroupDm { group, .. } => self.data.join("groups").join(group).join("dm"),
            WsKey::GroupMember { group, player, .. } => self
                .data
                .join("groups")
                .join(group)
                .join("members")
                .join(player),
        }
    }

    fn prepare(&self, key: &WsKey) -> std::io::Result<PathBuf> {
        let dir = self.dir(key);
        std::fs::create_dir_all(dir.join("characters"))?;
        let dm = match key {
            WsKey::Personal { .. } => None,
            WsKey::Library { dm } | WsKey::GroupDm { dm, .. } | WsKey::GroupMember { dm, .. } => {
                Some(dm)
            }
        };
        if let Some(dm) = dm {
            let library = self.dm_library(dm);
            std::fs::create_dir_all(&library)?;
            link(&library, &dir.join("library"))?;
        }
        if let WsKey::GroupMember { group, dm, .. } = key {
            let owner = self.dir(&WsKey::GroupDm {
                group: group.clone(),
                dm: dm.clone(),
            });
            std::fs::create_dir_all(&owner)?;
            link(
                &owner.join("content-prefs.json"),
                &dir.join("content-prefs.json"),
            )?;
            // The group's house rules: the DM's settings, never the member's own (an older member folder has one).
            relink(&owner.join("settings.json"), &dir.join("settings.json"))?;
        }
        Ok(dir)
    }

    fn cell(&self, key: &WsKey) -> ApiResult<Cell> {
        let mut open = self
            .open
            .lock()
            .map_err(|_| ApiError::Internal("workspace lock poisoned".into()))?;
        Ok(Arc::clone(open.entry(key.clone()).or_default()))
    }

    /// The workspace, opened if need be (the first open of a large library takes seconds).
    pub async fn get(self: &Arc<Self>, key: &WsKey) -> ApiResult<Arc<Workspace>> {
        let cell = self.cell(key)?;
        let this = Arc::clone(self);
        let owned = key.clone();
        let workspace = cell
            .get_or_try_init(|| async move {
                tokio::task::spawn_blocking(move || this.open_now(&owned))
                    .await
                    .map_err(|e| ApiError::Internal(format!("open task: {e}")))?
            })
            .await?;
        workspace.last_used.store(now(), Ordering::Relaxed);
        Ok(Arc::clone(workspace))
    }

    fn open_now(&self, key: &WsKey) -> ApiResult<Arc<Workspace>> {
        let dir = self
            .prepare(key)
            .map_err(|e| ApiError::Internal(format!("preparing a workspace: {e}")))?;
        let (events, _) = broadcast::channel(256);
        let sender = events.clone();
        let handle = headless::open_workspace(dir, self.content.clone(), move |name, payload| {
            let _ = sender.send(WorkspaceEvent {
                name: name.to_string(),
                payload,
            });
        })
        .map_err(|e| ApiError::Internal(format!("opening a workspace: {e}")))?;
        Ok(Arc::new(Workspace {
            handle,
            events,
            last_used: AtomicI64::new(now()),
            group_rules: matches!(key, WsKey::GroupMember { .. }),
        }))
    }

    fn loaded(&self) -> Vec<(WsKey, Arc<Workspace>)> {
        self.open.lock().map_or_else(
            |_| Vec::new(),
            |open| {
                open.iter()
                    .filter_map(|(key, cell)| Some((key.clone(), Arc::clone(cell.get()?))))
                    .collect()
            },
        )
    }

    /// Reload the open workspaces that `matches` picks (in the background, the caller's request is already done).
    pub fn reload_where(&self, reason: &'static str, matches: impl Fn(&WsKey) -> bool) {
        for (key, workspace) in self.loaded() {
            if !matches(&key) {
                continue;
            }
            tokio::task::spawn_blocking(move || {
                if let Err(e) = headless::reload_shared_content(&workspace.handle, reason) {
                    tracing::warn!("reloading {key:?}: {e}");
                }
            });
        }
    }

    /// Move a character's file between workspaces, saving and closing it first.
    pub async fn move_character(
        self: &Arc<Self>,
        from: &WsKey,
        to: &WsKey,
        character: &str,
    ) -> ApiResult<()> {
        if invalid_id(character) {
            return Err(ApiError::Invalid("That is not a character id.".into()));
        }
        let source = self.get(from).await?;
        let mut args = Map::new();
        args.insert("characterId".into(), json!(character));
        args.insert("discard".into(), json!(false));
        source.call("close_character", args).await?;
        let file = format!("{character}.json");
        let from_file = self.dir(from).join("characters").join(&file);
        let to_dir = self
            .prepare(to)
            .map_err(|e| ApiError::Internal(format!("preparing a workspace: {e}")))?
            .join("characters");
        tokio::task::spawn_blocking(move || -> ApiResult<()> {
            if !from_file.is_file() {
                return Err(ApiError::NotFound("That character does not exist.".into()));
            }
            std::fs::rename(&from_file, to_dir.join(&file))
                .map_err(|e| ApiError::Internal(format!("moving a character: {e}")))
        })
        .await
        .map_err(|e| ApiError::Internal(format!("move task: {e}")))?
    }

    /// Close the workspaces `matches` picks and wait, for a while, until none of them is still building its search
    /// index, which writes into the workspace's folder from a thread of its own.
    async fn close_where(&self, matches: impl Fn(&WsKey) -> bool) {
        let mut closed = Vec::new();
        if let Ok(mut open) = self.open.lock() {
            open.retain(|key, cell| {
                if !matches(key) {
                    return true;
                }
                if let Some(workspace) = cell.get() {
                    closed.push(Arc::clone(workspace));
                }
                false
            });
        }
        for workspace in closed {
            for _ in 0..300 {
                let status = workspace
                    .call("search_index_status", Map::new())
                    .await
                    .unwrap_or_default();
                if status.get("state").and_then(Value::as_str) != Some("building") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }

    /// Forget a deleted group's workspaces and delete its folder.
    pub async fn remove_group(&self, group: &str) -> ApiResult<()> {
        self.close_where(|key| {
            matches!(key, WsKey::GroupDm { group: g, .. } | WsKey::GroupMember { group: g, .. } if g == group)
        })
        .await;
        let dir = self.data.join("groups").join(group);
        tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ApiError::Internal(format!("removing a group folder: {e}"))),
        })
        .await
        .map_err(|e| ApiError::Internal(format!("remove task: {e}")))?
    }

    /// Forget a deleted account's workspaces and delete its folders: its own, and its member folders in groups (which
    /// hold no characters any more by then).
    pub async fn remove_user(&self, user: &str) -> ApiResult<()> {
        self.close_where(|key| match key {
            WsKey::Personal { player } => player == user,
            WsKey::Library { dm } | WsKey::GroupDm { dm, .. } => dm == user,
            WsKey::GroupMember { dm, player, .. } => dm == user || player == user,
        })
        .await;
        let data = self.data.clone();
        let user = user.to_string();
        tokio::task::spawn_blocking(move || -> ApiResult<()> {
            let mut dirs = vec![data.join("users").join(&user)];
            if let Ok(groups) = std::fs::read_dir(data.join("groups")) {
                for group in groups.flatten() {
                    dirs.push(group.path().join("members").join(&user));
                }
            }
            for dir in dirs {
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(ApiError::Internal(format!(
                            "removing {}: {e}",
                            dir.display()
                        )))
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| ApiError::Internal(format!("remove task: {e}")))?
    }

    /// What the data folder holds, for the admin page. Links are not followed, so shared libraries count once.
    pub async fn usage(&self) -> ApiResult<Usage> {
        let data = self.data.clone();
        tokio::task::spawn_blocking(move || {
            let mut usage = Usage::default();
            let mut stack = vec![(data, false)];
            while let Some((dir, in_library)) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                let in_characters = dir.file_name().is_some_and(|n| n == "characters");
                for entry in entries.flatten() {
                    let Ok(meta) = entry.path().symlink_metadata() else {
                        continue;
                    };
                    let name = entry.file_name();
                    if meta.is_dir() {
                        stack.push((entry.path(), in_library || name == "library"));
                    } else if meta.is_file() {
                        usage.bytes += meta.len();
                        let name = name.to_string_lossy();
                        if in_characters && name.ends_with(".json") {
                            usage.characters += 1;
                        }
                        if in_library && name == "pack.json" {
                            usage.packs += 1;
                        }
                    }
                }
            }
            usage
        })
        .await
        .map_err(|e| ApiError::Internal(format!("usage task: {e}")))
    }

    /// Close one member's workspace after they leave.
    pub fn forget(&self, key: &WsKey) {
        if let Ok(mut open) = self.open.lock() {
            open.remove(key);
        }
    }

    /// Close workspaces nobody has used for a while. In-flight users hold their own `Arc`, and unsaved edits keep a
    /// workspace open.
    pub fn evict_idle(&self) {
        let cutoff = now() - IDLE_SECONDS;
        if let Ok(mut open) = self.open.lock() {
            open.retain(|_, cell| {
                cell.get().is_none_or(|w| {
                    w.last_used.load(Ordering::Relaxed) > cutoff
                        || w.events.receiver_count() > 0
                        || headless::has_unsaved_changes(&w.handle)
                })
            });
        }
    }

    /// Write every unsaved character to disk (service stop).
    pub fn save_all(&self) {
        for (key, workspace) in self.loaded() {
            if let Err(e) = headless::save_all(&workspace.handle) {
                tracing::error!("saving {key:?} at shutdown: {e}");
            }
        }
    }

    /// Run [`Workspaces::evict_idle`] forever.
    pub async fn evict_loop(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(5 * 60));
        loop {
            tick.tick().await;
            self.evict_idle();
        }
    }
}

/// `to` as seen from the folder `from`, so links keep working when the data folder moves (a direct install's data
/// mounted into the container, say).
fn relative(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut path = PathBuf::new();
    for _ in shared..from.len() {
        path.push("..");
    }
    for part in &to[shared..] {
        path.push(part);
    }
    path
}

/// Make `link_path` a symlink to `target`, if nothing is there yet.
#[cfg(unix)]
fn link(target: &Path, link_path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(link_path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let from = link_path.parent().unwrap_or(Path::new("."));
            std::os::unix::fs::symlink(relative(from, target), link_path)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
#[test]
fn links_are_relative() {
    assert_eq!(
        relative(
            Path::new("/config/data/groups/g/members/p"),
            Path::new("/config/data/users/dm/library")
        ),
        Path::new("../../../../users/dm/library")
    );
    assert_eq!(
        relative(
            Path::new("/d/users/dm/studio"),
            Path::new("/d/users/dm/library")
        ),
        Path::new("../library")
    );
}

#[cfg(not(unix))]
fn link(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("the server runs on Linux"))
}

/// Like [`link`], replacing a file that is there instead of the link.
fn relink(target: &Path, link_path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(link_path) {
        Ok(meta) if !meta.file_type().is_symlink() => std::fs::remove_file(link_path)?,
        _ => {}
    }
    link(target, link_path)
}
