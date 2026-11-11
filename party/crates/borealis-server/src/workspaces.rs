//! One Borealis workspace per place a person works in.
//!
//! Layout under the data folder (ids are server-made UUIDs):
//!
//! ```text
//! users/<player>/personal/            a player's characters that are in no group: bundled content only
//! users/<dm>/library/                 the DM's content packs
//! users/<dm>/studio/                  the DM's Library screen (library -> ../library)
//! groups/<g>/dm/                      the DM's view of a group; its content-prefs.json says which packs the group uses
//! groups/<g>/members/<player>/        a member's character (library -> DM's, content-prefs.json -> the DM's above)
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

/// A place to work.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WsKey {
    /// A player's characters outside any group.
    Personal {
        /// The player.
        player: String,
    },
    /// A DM's content library.
    Library {
        /// The DM.
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
}

impl Workspace {
    /// Run a command, off the async threads.
    pub async fn call(&self, command: &str, args: Map<String, Value>) -> ApiResult<Value> {
        self.last_used.store(now(), Ordering::Relaxed);
        let handle = self.handle.clone();
        let name = command.to_string();
        let outcome = tokio::task::spawn_blocking(move || headless::invoke(&handle, &name, args))
            .await
            .map_err(|e| ApiError::Internal(format!("command task: {e}")))?;
        self.last_used.store(now(), Ordering::Relaxed);
        match outcome {
            Some(Ok(value)) => Ok(value),
            Some(Err(error)) => Err(ApiError::Command(error)),
            None => Err(ApiError::NotFound("No such command.".into())),
        }
    }

    /// What a command requires, or `None` when the server does not offer it.
    pub fn access_of(&self, command: &str) -> Option<headless::Access> {
        headless::access_of(&self.handle, command)
    }
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

    /// Forget a deleted group's workspaces and delete its folder.
    pub async fn remove_group(&self, group: &str) -> ApiResult<()> {
        if let Ok(mut open) = self.open.lock() {
            open.retain(|key, _| {
                !matches!(key, WsKey::GroupDm { group: g, .. } | WsKey::GroupMember { group: g, .. } if g == group)
            });
        }
        let dir = self.data.join("groups").join(group);
        tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ApiError::Internal(format!("removing a group folder: {e}"))),
        })
        .await
        .map_err(|e| ApiError::Internal(format!("remove task: {e}")))?
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
