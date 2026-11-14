//! "Update all content" and the automatic content update (0.3.3).
//!
//! Upstream checks the user's index files at the desktop app's start (`content_auto.rs`), and only the index files. On a
//! server that never ran: no account's workspace started it, so `content_update_status` stayed `idle`. Here:
//!
//! - `update_all_content` (a job command, [`crate::jobs::JOBS`]) updates the account's index files
//!   (`update_indexes`) and then each of its 5etools sources (`update_5etools_url`), through the same address checks
//!   ([`crate::net_guard`]), and answers one summary: `{ indexes, sources, updated, failed, message, checkedAt }`.
//!   A failure of one part is reported in `failed` and the others still run.
//! - When an account's workspace opens, [`schedule`] starts it as the account's job after [`AUTO_DELAY`], when the
//!   account's "Automatic Updates" setting (upstream's `autoContentUpdate`, on by default) is on, the last
//!   check is at least [`AUTO_EVERY`] old, and there is something to check. It never blocks a request: it is a
//!   background job like a user's own, and a job the user already runs is left alone.
//! - The last check is kept in the account's folder ([`STAMP`]), so the 24 hours count across restarts.
//! - `content_update_status` answers upstream's shape from that, so upstream's status bar shows "Checking…" and the
//!   outcome.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use borealis_lib::content_fivetools_remote::RemoteFetcher;
use borealis_lib::content_index::{Fetcher, HttpsFetcher};
use borealis_lib::content_packs::{progress, EventSink};
use borealis_lib::error::ApiResult;
use borealis_lib::settings_dto::{ContentUpdateState, ContentUpdateStatus};
use borealis_lib::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map};

use crate::hub::PartyHub;
use crate::jobs::Jobs;
use crate::workspaces::Workspace;

/// The job command.
pub const COMMAND: &str = "update_all_content";

/// The automatic check runs at most this often per account (seconds).
pub const AUTO_EVERY: i64 = 24 * 60 * 60;

/// How long after a workspace opens the automatic check starts, so it never competes with the first page load.
pub const AUTO_DELAY: Duration = Duration::from_secs(10);

/// The last check, in the account's folder.
pub const STAMP: &str = "party-content-update.json";

/// What `update_all_content` answers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAll {
    /// Index files checked.
    pub indexes: usize,
    /// 5etools sources checked.
    pub sources: usize,
    /// Files or sources that changed.
    pub updated: usize,
    /// What could not be updated, one line each.
    pub failed: Vec<String>,
    /// The one-line summary.
    pub message: String,
    /// When it finished (RFC 3339).
    pub checked_at: String,
    /// Started by the automatic check, not the user.
    #[serde(default)]
    pub auto: bool,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "2 index files and 1 5etools source checked · 1 updated", with "· N could not be updated" when any failed.
#[must_use]
pub fn summary(indexes: usize, sources: usize, updated: usize, failed: usize) -> String {
    if indexes == 0 && sources == 0 {
        return "Nothing to update: add an index file or a 5etools source first.".into();
    }
    let mut text = format!(
        "{} and {} checked · {} updated",
        plural(indexes, "index file", "index files"),
        plural(sources, "5etools source", "5etools sources"),
        updated
    );
    if failed > 0 {
        text.push_str(&format!(" · {failed} could not be updated"));
    }
    text
}

/// Whether the automatic check is due: on, and the last one at least [`AUTO_EVERY`] before `now` (or none yet).
#[must_use]
pub fn due(on: bool, last: Option<i64>, now: i64) -> bool {
    on && last.is_none_or(|at| now - at >= AUTO_EVERY || at > now)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The account's last check, as kept in its folder.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Stamp {
    /// Seconds.
    at: i64,
    result: UpdateAll,
}

fn read_stamp(data: &Path) -> Option<Stamp> {
    serde_json::from_slice(&std::fs::read(data.join(STAMP)).ok()?).ok()
}

fn write_stamp(data: &Path, stamp: &Stamp) {
    let Ok(bytes) = serde_json::to_vec_pretty(stamp) else {
        return;
    };
    let tmp = data.join(format!("{STAMP}.tmp"));
    if let Err(e) =
        std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, data.join(STAMP)))
    {
        tracing::warn!("writing {STAMP}: {e}");
    }
}

/// The last check's time (seconds), for the throttle and the page.
#[must_use]
pub fn last_check(data: &Path) -> Option<i64> {
    read_stamp(data).map(|s| s.at)
}

/// Where each account's automatic check is, by data folder: only this run of the server.
#[derive(Debug, Clone)]
enum Phase {
    Pending,
    Running,
    Done(UpdateAll),
}

fn phases() -> &'static Mutex<HashMap<PathBuf, Phase>> {
    static PHASES: OnceLock<Mutex<HashMap<PathBuf, Phase>>> = OnceLock::new();
    PHASES.get_or_init(Mutex::default)
}

fn set_phase(data: &Path, phase: Option<Phase>) {
    let mut map = phases().lock().unwrap_or_else(PoisonError::into_inner);
    match phase {
        Some(p) => map.insert(data.to_path_buf(), p),
        None => map.remove(data),
    };
}

/// Update the account's index files and every 5etools source; see the module.
///
/// # Errors
/// None of its own: a part that fails is reported in `failed`.
pub fn run(state: &AppState, sink: &dyn EventSink, auto: bool) -> ApiResult<UpdateAll> {
    run_with(
        state,
        &crate::net_guard::Guarded::new(HttpsFetcher),
        sink,
        auto,
    )
}

/// [`run`] with its downloads through `fetch` (tests).
///
/// # Errors
/// None of its own: a part that fails is reported in `failed`.
pub fn run_with<F: Fetcher + RemoteFetcher>(
    state: &AppState,
    fetch: &F,
    sink: &dyn EventSink,
    auto: bool,
) -> ApiResult<UpdateAll> {
    let data = state.paths.data.clone();
    set_phase(&data, Some(Phase::Running));
    let indexes = state.content_prefs().indexes.len();
    let urls: Vec<String> = state
        .list_5etools_urls()
        .into_iter()
        .map(|u| u.url)
        .collect();
    let total = indexes.min(1) + urls.len();
    let mut updated = 0;
    let mut failed = Vec::new();
    if indexes > 0 {
        sink.content_progress(&progress("checking", 0, total, "Checking the index files…"));
        match state.update_indexes(fetch, sink) {
            Ok(r) => {
                updated += r.updated;
                failed.extend(r.failed);
            }
            Err(e) => failed.push(format!("Index files: {}", e.message)),
        }
    }
    for (i, url) in urls.iter().enumerate() {
        sink.content_progress(&progress(
            "checking",
            indexes.min(1) + i,
            total,
            &format!("Checking the 5etools source {url}…"),
        ));
        match crate::backend::update_5etools_with(state, fetch, sink, url) {
            Ok(u) if !u.up_to_date => updated += 1,
            Ok(_) => {}
            Err(e) => failed.push(format!("{url}: {}", e.message)),
        }
    }
    let result = UpdateAll {
        indexes,
        sources: urls.len(),
        updated,
        message: summary(indexes, urls.len(), updated, failed.len()),
        failed,
        checked_at: borealis_lib::ids::rfc3339(SystemTime::now()),
        auto,
    };
    sink.content_progress(&progress("done", total, total, &result.message));
    write_stamp(
        &data,
        &Stamp {
            at: unix_now(),
            result: result.clone(),
        },
    );
    set_phase(&data, Some(Phase::Done(result.clone())));
    Ok(result)
}

/// `content_update_status` in upstream's shape (the status bar's).
#[must_use]
pub fn status(state: &AppState) -> ContentUpdateStatus {
    let data = &state.paths.data;
    let last = read_stamp(data);
    let checked_at = last.as_ref().map(|s| s.result.checked_at.clone());
    let phase = phases()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(data)
        .cloned();
    match phase {
        Some(Phase::Pending) => ContentUpdateStatus {
            state: ContentUpdateState::Pending,
            checked_at,
            ..ContentUpdateStatus::default()
        },
        Some(Phase::Running) => ContentUpdateStatus {
            state: ContentUpdateState::Running,
            message: "Checking for content updates...".into(),
            checked_at,
            ..ContentUpdateStatus::default()
        },
        // An automatic check that ran since the workspace opened: its outcome, once, in the status bar.
        Some(Phase::Done(r)) if r.auto => ContentUpdateStatus {
            state: ContentUpdateState::Done,
            message: r.message,
            updated: r.updated,
            failed: r.failed,
            checked_at: Some(r.checked_at),
        },
        // Nothing running: done, with nothing to tell (the status bar keeps its idle line).
        _ => ContentUpdateStatus {
            state: if state.settings().is_ok_and(|s| s.auto_content_update) {
                ContentUpdateState::Done
            } else {
                ContentUpdateState::Disabled
            },
            checked_at,
            ..ContentUpdateStatus::default()
        },
    }
}

/// `content_update_info`: the Update all line's facts.
#[must_use]
pub fn info(state: &AppState) -> serde_json::Value {
    let last = read_stamp(&state.paths.data);
    json!({
        "auto": state.settings().is_ok_and(|s| s.auto_content_update),
        "everyHours": AUTO_EVERY / 3600,
        "indexes": state.content_prefs().indexes.len(),
        "sources": state.list_5etools_urls().len(),
        "last": last.map(|s| s.result),
    })
}

/// The hub the automatic check tells about changed content; set by main.rs. Without it (tests that build the
/// routes by hand) no automatic check runs.
static HUB: OnceLock<Arc<PartyHub>> = OnceLock::new();

/// Turn the automatic check on, with the hub its content events go through.
pub fn install(hub: Arc<PartyHub>) {
    let _ = HUB.set(hub);
}

/// When the account's workspace has just opened: start the automatic check after `delay` if it is due.
pub fn schedule(jobs: Arc<Jobs>, account: String, workspace: Arc<Workspace>, delay: Duration) {
    let Some(hub) = HUB.get().cloned() else {
        return;
    };
    tokio::spawn(async move {
        if let Err(e) = start_if_due(&jobs, &hub, &account, &workspace, delay).await {
            tracing::warn!("automatic content update for {account}: {e:?}");
        }
    });
}

/// Check, wait, check again, and start the job. Public for the tests.
///
/// # Errors
/// When the workspace could not be read or the job not started.
pub async fn start_if_due(
    jobs: &Arc<Jobs>,
    hub: &Arc<PartyHub>,
    account: &str,
    workspace: &Arc<Workspace>,
    delay: Duration,
) -> crate::error::ApiResult<bool> {
    let check = || {
        workspace.blocking(|app| {
            use tauri::Manager;
            let state = app.state::<AppState>();
            let on = state.settings().is_ok_and(|s| s.auto_content_update);
            let something =
                !state.content_prefs().indexes.is_empty() || !state.list_5etools_urls().is_empty();
            let data = state.paths.data.clone();
            Ok((due(on, last_check(&data), unix_now()) && something, data))
        })
    };
    let (go, data) = check().await?;
    if !go {
        return Ok(false);
    }
    set_phase(&data, Some(Phase::Pending));
    tokio::time::sleep(delay).await;
    // The setting or the sources may have changed meanwhile.
    let (go, _) = check().await?;
    if !go {
        set_phase(&data, None);
        return Ok(false);
    }
    let on = Arc::clone(workspace);
    let hub = Arc::clone(hub);
    let owner = account.to_string();
    let mut args = Map::new();
    args.insert("auto".into(), json!(true));
    let (_, started) = jobs.start(
        account,
        workspace,
        COMMAND,
        json!({ "auto": true }),
        async move {
            let ended = crate::sync::call_upstream(&on, COMMAND, args).await;
            crate::content_events::changed(&hub, &owner, "packs", COMMAND);
            ended
        },
    )?;
    if !started {
        // The account runs a job of its own already: try again on the next open.
        set_phase(&data, None);
    }
    Ok(started)
}
