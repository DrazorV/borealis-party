//! Background jobs (plan I1): the commands that can run longer than a proxy waits for an answer.
//!
//! Through Cloudflare a request that takes over about 100 s gets a 524 while the server keeps working, and trying again
//! starts the same work twice. A first download of a set of indexes takes over three minutes, so the few commands in
//! [`JOBS`] can also run as a job: `job_start { command, args }` answers `{ jobId, command, started, same }` at once, and
//! `job_status { jobId }` answers `{ jobId, command, state, result?, error?, progress? }` until `state` is `done` (with
//! the command's own result) or `failed` (with its own `{ code, message }` error). The client is
//! `ui/party/api/overrides/jobs.ts`.
//!
//! - A job runs on the caller's own workspace, through the same call path as `POST /api/rpc/<command>`, so it answers
//!   what the plain call would, and every download it makes passes [`crate::net_guard`] as before.
//! - One job per account at a time: a start while one runs answers the running job (`started: false`; `same` when it
//!   runs the same command with the same args, so its result is this call's too).
//! - A running job keeps its workspace open ([`crate::workspaces::Workspace::busy`]); it goes on when the client that
//!   started it is gone.
//! - Progress still comes as the command's own events; `progress` is the last of them. [`JOB_DONE`] tells the account's
//!   clients a job ended.
//! - Each job has a record, `users/<account>/sync/jobs/<id>.json`. A record still running when the server starts is
//!   from a server that stopped during the job: it is marked failed with [`RESTARTED`].
//! - Job ids are random (122 bits), and a job is found only among the caller's own: another account's id answers
//!   the same "not found" as an unknown one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::db::now;
use crate::error::{ApiError, ApiResult};
use crate::workspaces::Workspace;

/// The job commands, under `POST /api/rpc/<name>`.
pub const COMMANDS: [&str; 2] = ["job_start", "job_status"];

/// The commands that may run as a job: the downloads that can outlast a proxy's timeout (party/README.md,
/// Measurements). Everything else stays one request.
pub const JOBS: [&str; 3] = ["update_indexes", "stage_5etools_url", "update_5etools_url"];

/// Sent when a job ends: `{ jobId, command, state }`.
pub const JOB_DONE: &str = "party://job-done";

/// The error of a job the server stopped during.
pub const RESTARTED: &str = "The server restarted during this import; run it again.";

/// The events a job's `progress` follows.
const PROGRESS_EVENTS: [&str; 2] = [
    "borealis://content-progress",
    "borealis://import_5etools_progress",
];

/// How long an ended job can still be asked about.
const KEEP_SECONDS: i64 = 24 * 60 * 60;

/// Where a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    /// Still working.
    Running,
    /// Ended with a result.
    Done,
    /// Ended with an error.
    Failed,
}

/// A job, as `job_status` answers it and as its record keeps it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    /// The job's id.
    pub job_id: String,
    /// The command it runs.
    pub command: String,
    /// Where it is.
    pub state: JobState,
    /// When it started (seconds).
    pub started_at: i64,
    /// When it ended (seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// The command's result, once done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// The command's error, once failed: `{ code, message, details? }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    /// The last progress event while running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Value>,
    /// The command's arguments, while it runs (kept in memory only).
    #[serde(skip)]
    pub args: Value,
}

impl Record {
    fn fail(&mut self, error: Value) {
        self.state = JobState::Failed;
        self.finished_at = Some(now());
        self.result = None;
        self.error = Some(error);
        self.progress = None;
    }
}

#[derive(Default)]
struct Inner {
    /// The running job of each account.
    running: HashMap<String, String>,
    /// Jobs by id, with their account: the running ones and those ended since this start.
    jobs: HashMap<String, (String, Record)>,
}

/// The accounts' jobs.
pub struct Jobs {
    data: PathBuf,
    inner: Mutex<Inner>,
}

fn not_found() -> ApiError {
    ApiError::NotFound("That job is not on the server any more. Run it again.".into())
}

fn lock(inner: &Mutex<Inner>) -> std::sync::MutexGuard<'_, Inner> {
    inner
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A job id: 32 lowercase hex digits, never a path.
fn is_job_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn read_record(path: &Path) -> Option<Record> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Write a record in full or not at all.
fn write_record(dir: &Path, record: &Record) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", record.job_id));
    let temp = dir.join(format!(".{}.tmp", record.job_id));
    std::fs::write(&temp, serde_json::to_vec_pretty(record)?)?;
    std::fs::rename(&temp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// The `{ code, message }` body a failed command would have answered over HTTP.
async fn wire(error: ApiError) -> Value {
    if let ApiError::Command(value) = error {
        return value;
    }
    let body = error.into_response().into_body();
    match axum::body::to_bytes(body, 64 * 1024).await {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| internal_error()),
        Err(_) => internal_error(),
    }
}

fn internal_error() -> Value {
    json!({ "code": "internal", "message": "Something went wrong on the server." })
}

impl Jobs {
    /// The jobs of the accounts under `data`. Records left running by the server's last run are marked failed with
    /// [`RESTARTED`], and records of jobs that ended over a day ago are deleted.
    pub fn new(data: PathBuf) -> Self {
        let jobs = Self {
            data,
            inner: Mutex::new(Inner::default()),
        };
        jobs.sweep();
        jobs
    }

    fn dir(&self, account: &str) -> PathBuf {
        self.data
            .join("users")
            .join(account)
            .join("sync")
            .join("jobs")
    }

    fn sweep(&self) {
        let Ok(accounts) = std::fs::read_dir(self.data.join("users")) else {
            return;
        };
        for account in accounts.flatten() {
            let dir = account.path().join("sync").join("jobs");
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(mut record) = read_record(&path) else {
                    continue;
                };
                if record.state == JobState::Running {
                    record.fail(json!({ "code": "interrupted", "message": RESTARTED }));
                    if let Err(e) = write_record(&dir, &record) {
                        tracing::error!("marking job {} as interrupted: {e}", record.job_id);
                    }
                } else if record
                    .finished_at
                    .is_some_and(|at| at < now() - KEEP_SECONDS)
                {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    /// Start `command` with `args` on the account's workspace, or answer the account's running job:
    /// `(record, started)`. `work` is the command itself; it runs to its end whatever happens to the request that
    /// started it.
    pub fn start<F>(
        self: &Arc<Self>,
        account: &str,
        workspace: &Arc<Workspace>,
        command: &str,
        args: Value,
        work: F,
    ) -> ApiResult<(Record, bool)>
    where
        F: std::future::Future<Output = ApiResult<Value>> + Send + 'static,
    {
        let mut inner = lock(&self.inner);
        if let Some(running) = inner.running.get(account) {
            if let Some((_, record)) = inner.jobs.get(running) {
                return Ok((record.clone(), false));
            }
        }
        let cutoff = now() - KEEP_SECONDS;
        inner
            .jobs
            .retain(|_, (_, r)| r.finished_at.is_none_or(|at| at > cutoff));
        let record = Record {
            job_id: uuid::Uuid::new_v4().simple().to_string(),
            command: command.to_string(),
            state: JobState::Running,
            started_at: now(),
            finished_at: None,
            result: None,
            error: None,
            progress: None,
            args,
        };
        let dir = self.dir(account);
        write_record(&dir, &record)
            .map_err(|e| ApiError::Internal(format!("writing a job record: {e}")))?;
        inner
            .running
            .insert(account.to_string(), record.job_id.clone());
        inner
            .jobs
            .insert(record.job_id.clone(), (account.to_string(), record.clone()));
        drop(inner);

        let busy = workspace.busy();
        let jobs = Arc::clone(self);
        let account = account.to_string();
        let id = record.job_id.clone();
        tokio::spawn(async move {
            let mut events = busy.workspace().events.subscribe();
            // Its own task, so a panic in the command fails the job instead of leaving it running.
            let mut task = tokio::spawn(work);
            let outcome = loop {
                tokio::select! {
                    joined = &mut task => break joined,
                    event = events.recv() => match event {
                        Ok(event) if PROGRESS_EVENTS.contains(&event.name.as_str()) => {
                            if let Some((_, r)) = lock(&jobs.inner).jobs.get_mut(&id) {
                                r.progress = Some(event.payload);
                            }
                        }
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break (&mut task).await,
                    },
                }
            };
            let ended = match outcome {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(error)) => Err(wire(error).await),
                Err(e) => {
                    tracing::error!("job {id} stopped: {e}");
                    Err(internal_error())
                }
            };
            let mut inner = lock(&jobs.inner);
            let Some((_, record)) = inner.jobs.get_mut(&id) else {
                return;
            };
            match ended {
                Ok(value) => {
                    record.state = JobState::Done;
                    record.finished_at = Some(now());
                    record.result = Some(value);
                    record.progress = None;
                }
                Err(error) => record.fail(error),
            }
            let record = record.clone();
            if let Err(e) = write_record(&jobs.dir(&account), &record) {
                // The answer is still kept here until the server stops.
                tracing::error!("writing the record of job {id}: {e}");
            }
            if inner.running.get(&account) == Some(&id) {
                inner.running.remove(&account);
            }
            drop(inner);
            busy.workspace().emit(
                JOB_DONE,
                json!({ "jobId": id, "command": record.command, "state": record.state }),
            );
            drop(busy);
        });
        Ok((record, true))
    }

    /// The account's job `id`.
    pub fn status(&self, account: &str, id: &str) -> ApiResult<Record> {
        if !is_job_id(id) {
            return Err(not_found());
        }
        if let Some((owner, record)) = lock(&self.inner).jobs.get(id) {
            return if owner == account {
                Ok(record.clone())
            } else {
                Err(not_found())
            };
        }
        let mut record =
            read_record(&self.dir(account).join(format!("{id}.json"))).ok_or_else(not_found)?;
        if record.state == JobState::Running {
            // Not running here, so it ran on a server that stopped.
            record.fail(json!({ "code": "interrupted", "message": RESTARTED }));
        }
        Ok(record)
    }
}

/// Run a job command for `account` on its own `workspace`.
pub async fn call(
    jobs: &Arc<Jobs>,
    state: &crate::AppState,
    account: &str,
    workspace: &Arc<Workspace>,
    command: &str,
    args: &Map<String, Value>,
) -> ApiResult<Value> {
    match command {
        "job_start" => {
            let name = args
                .get("command")
                .and_then(Value::as_str)
                .filter(|name| JOBS.contains(name))
                .ok_or_else(|| ApiError::Invalid("That command does not run as a job.".into()))?
                .to_string();
            let call_args = match args.get("args") {
                None | Some(Value::Null) => Map::new(),
                Some(Value::Object(map)) => map.clone(),
                Some(_) => return Err(ApiError::Invalid("A job's args are an object.".into())),
            };
            let asked = Value::Object(call_args.clone());
            let on = Arc::clone(workspace);
            let run = name.clone();
            let app = state.clone();
            let owner = account.to_string();
            let (record, started) =
                jobs.start(account, workspace, &name, asked.clone(), async move {
                    let ended = crate::sync::call_upstream(&on, &run, call_args).await;
                    // A failed update may still have changed some packs: the lists are fetched again either way.
                    if let Some(kind) = crate::content_events::kind_of(&run) {
                        crate::content_events::changed(&app.hub, &owner, kind, &run);
                    }
                    // 0.3.3: the party cards are read again when the import ENDS (the RPC's refresh ran at its start).
                    crate::parties::after_command(
                        &app,
                        &owner,
                        &on,
                        crate::parties::CardTarget::All,
                    )
                    .await;
                    ended
                })?;
            // `same`: the running job is this very call (a start sent again after no answer came, say), so its
            // result is this call's. Otherwise the client waits for it to end and starts its own.
            let same = started || (record.command == name && record.args == asked);
            Ok(json!({
                "jobId": record.job_id,
                "command": record.command,
                "started": started,
                "same": same,
            }))
        }
        "job_status" => {
            let id = args.get("jobId").and_then(Value::as_str).unwrap_or("");
            let record = jobs.status(account, id)?;
            serde_json::to_value(record).map_err(|e| ApiError::Internal(e.to_string()))
        }
        _ => Err(ApiError::NotFound("The server does not offer that.".into())),
    }
}
