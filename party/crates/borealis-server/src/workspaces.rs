//! One Borealis workspace per account.
//!
//! Each account has an ordinary Borealis data folder, `users/<account id>/` under the server's data folder, holding
//! its `characters/`, its content `library/`, `settings.json` and whatever else Borealis keeps there. Nothing is
//! shared between accounts.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::backend as headless;
use serde_json::{Map, Value};
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
    /// Held by the sync commands and by the commands that delete or replace character files, so a sync never sees
    /// one half done. `call` alone holds nothing between commands.
    pub sync_lock: tokio::sync::Mutex<()>,
    last_used: AtomicI64,
    /// Background jobs running on the workspace ([`crate::jobs`]). A busy workspace is never evicted, so the account
    /// never gets a second backend on the same folder while a job still holds the first.
    active_jobs: AtomicUsize,
}

/// Marks a workspace busy for as long as it lives ([`Workspace::busy`]).
pub struct Busy(Arc<Workspace>);

impl Busy {
    /// The busy workspace.
    pub fn workspace(&self) -> &Arc<Workspace> {
        &self.0
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.active_jobs.fetch_sub(1, Ordering::SeqCst);
        self.0.last_used.store(now(), Ordering::Relaxed);
    }
}

impl Workspace {
    /// Mark the workspace busy with a job until the returned value is dropped.
    pub fn busy(self: &Arc<Self>) -> Busy {
        self.active_jobs.fetch_add(1, Ordering::SeqCst);
        Busy(Arc::clone(self))
    }

    /// True while a job runs on the workspace.
    pub fn is_busy(&self) -> bool {
        self.active_jobs.load(Ordering::SeqCst) > 0
    }

    /// Run `work` on the workspace's backend, off the async threads.
    pub async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&AppHandle) -> ApiResult<T> + Send + 'static,
    ) -> ApiResult<T> {
        self.last_used.store(now(), Ordering::Relaxed);
        let handle = self.handle.clone();
        let outcome = tokio::task::spawn_blocking(move || work(&handle))
            .await
            .map_err(|e| ApiError::Internal(format!("command task: {e}")))?;
        self.last_used.store(now(), Ordering::Relaxed);
        outcome
    }

    /// True when the character is open with unsaved edits.
    pub fn is_dirty(&self, character: &str) -> bool {
        headless::is_dirty(&self.handle, character)
    }

    /// Send an event to the account's clients (none listening is fine).
    pub fn emit(&self, name: &str, payload: Value) {
        let _ = self.events.send(WorkspaceEvent {
            name: name.to_string(),
            payload,
        });
    }

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
            // The web app matches on these words.
            None => Err(ApiError::NotFound("The server does not offer that.".into())),
        }
    }
}

type Cell = Arc<OnceCell<Arc<Workspace>>>;

/// The open workspaces, by account id.
pub struct Workspaces {
    data: PathBuf,
    content: PathBuf,
    open: Mutex<HashMap<String, Cell>>,
    jobs: Arc<crate::jobs::Jobs>,
}

impl Workspaces {
    /// Workspaces under `data`, with the bundled packs in `content`.
    /// Job records left running by a previous start are marked failed here ([`crate::jobs::Jobs::new`]).
    pub fn new(data: PathBuf, content: PathBuf) -> Self {
        Self {
            jobs: Arc::new(crate::jobs::Jobs::new(data.clone())),
            data,
            content,
            open: Mutex::new(HashMap::new()),
        }
    }

    /// The accounts' background jobs.
    pub fn jobs(&self) -> &Arc<crate::jobs::Jobs> {
        &self.jobs
    }

    /// An account's data folder. Account ids are made by the server (UUIDs), never taken from a client.
    pub fn dir(&self, account: &str) -> PathBuf {
        self.data.join("users").join(account)
    }

    /// The account's workspace, opened if need be (the first open loads the content, which takes seconds).
    pub async fn get(&self, account: &str) -> ApiResult<Arc<Workspace>> {
        let cell = {
            let mut open = self
                .open
                .lock()
                .map_err(|_| ApiError::Internal("workspace lock poisoned".into()))?;
            Arc::clone(open.entry(account.to_string()).or_default())
        };
        let dir = self.dir(account);
        let content = self.content.clone();
        let workspace = cell
            .get_or_try_init(|| async move {
                tokio::task::spawn_blocking(move || open_now(dir, content))
                    .await
                    .map_err(|e| ApiError::Internal(format!("open task: {e}")))?
            })
            .await?;
        workspace.last_used.store(now(), Ordering::Relaxed);
        Ok(Arc::clone(workspace))
    }

    fn loaded(&self) -> Vec<(String, Arc<Workspace>)> {
        self.open.lock().map_or_else(
            |_| Vec::new(),
            |open| {
                open.iter()
                    .filter_map(|(account, cell)| Some((account.clone(), Arc::clone(cell.get()?))))
                    .collect()
            },
        )
    }

    /// Close a deleted account's workspace and delete its folder.
    pub async fn remove(&self, account: &str) -> ApiResult<()> {
        let cell = self
            .open
            .lock()
            .ok()
            .and_then(|mut open| open.remove(account));
        // An open that is under way writes into the folder: this waits for it to end.
        let closed = match &cell {
            Some(cell) => cell.get_or_try_init(|| async { Err(()) }).await.ok(),
            None => None,
        };
        if let Some(workspace) = closed {
            // A job writes into the folder too: wait, for a while, until it ends.
            for _ in 0..3000 {
                if !workspace.is_busy() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            // The search index is built on a thread of its own, which writes into the folder: wait, for a while,
            // until it is done.
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
        let dir = self.dir(account);
        tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ApiError::Internal(format!(
                "removing {}: {e}",
                dir.display()
            ))),
        })
        .await
        .map_err(|e| ApiError::Internal(format!("remove task: {e}")))?
    }

    /// What the data folder holds, for the admin page.
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
                    // Not following links: nothing outside the data folder is counted.
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

    /// Close workspaces nobody has used for a while. In-flight users hold their own `Arc`; unsaved edits and a running
    /// job keep a workspace open (a later `get` would otherwise open a second backend on the same folder).
    pub fn evict_idle(&self) {
        let cutoff = now() - IDLE_SECONDS;
        if let Ok(mut open) = self.open.lock() {
            open.retain(|_, cell| {
                cell.get().is_none_or(|w| {
                    w.is_busy()
                        || w.last_used.load(Ordering::Relaxed) > cutoff
                        || w.events.receiver_count() > 0
                        || headless::has_unsaved_changes(&w.handle)
                })
            });
        }
    }

    /// Write every unsaved character to disk (service stop).
    pub fn save_all(&self) {
        for (account, workspace) in self.loaded() {
            if let Err(e) = headless::save_all(&workspace.handle) {
                tracing::error!("saving the characters of {account} at shutdown: {e}");
            }
        }
    }

    /// Tests: the account's open workspace, if any, without opening it.
    #[cfg(test)]
    pub fn loaded_now(&self, account: &str) -> Option<Arc<Workspace>> {
        let open = self.open.lock().ok()?;
        open.get(account)
            .and_then(|cell| cell.get())
            .map(Arc::clone)
    }

    /// Tests: make the account's workspace look unused for longer than the idle time.
    #[cfg(test)]
    pub fn age(&self, account: &str) {
        if let Some(w) = self.loaded_now(account) {
            w.last_used
                .store(now() - IDLE_SECONDS - 60, Ordering::Relaxed);
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

fn open_now(dir: PathBuf, content: PathBuf) -> ApiResult<Arc<Workspace>> {
    let (events, _) = broadcast::channel(256);
    let sender = events.clone();
    let handle = headless::open_workspace(dir, content, move |name, payload| {
        let _ = sender.send(WorkspaceEvent {
            name: name.to_string(),
            payload,
        });
    })
    .map_err(|e| ApiError::Internal(format!("opening a workspace: {e}")))?;
    Ok(Arc::new(Workspace {
        handle,
        events,
        sync_lock: tokio::sync::Mutex::new(()),
        last_used: AtomicI64::new(now()),
        active_jobs: AtomicUsize::new(0),
    }))
}
