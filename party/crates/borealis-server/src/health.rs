//! `GET /api/health`: whether this server is well, for an uptime checker (party/docs/deploy.md, "Health monitoring").
//!
//! It reports the database (reachable, how long a query takes), the migrations (at this build's step, none pending),
//! the free space where the config folder is, how many event streams are open, and how many of the recent API answers
//! were server errors. No secrets: no paths, names, addresses, settings or account data. No sign-in, so a checker
//! needs no token.
//!
//! The answer is 503 when the server can't do its job (the database can't be reached, or its schema isn't the one
//! this build expects) and 200 otherwise, with `status` `ok` or `degraded` (low disk, or many recent server errors),
//! so a plain HTTP check alerts on outages and a keyword check on `"status":"ok"` also on warnings.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::AppState;

/// How far back the server-error rate looks.
pub const WINDOW: Duration = Duration::from_secs(300);
/// Below this many API answers in the window, the rate never makes the server "degraded".
const MIN_ANSWERS: u64 = 20;
/// The share of server errors (5xx) in the window from which the server is "degraded".
const ERROR_RATE_WARN: f64 = 0.05;
/// Free space (share of the disk) below which the server is "degraded".
const DISK_FREE_WARN: f64 = 0.05;
/// A database check slower than this counts as unreachable.
const DB_TIMEOUT: Duration = Duration::from_secs(5);

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/health", get(health))
}

// ----- the config folder -----

static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The folder whose disk is reported (the config folder, where the database, data and backups live); set once at start.
pub fn set_config_dir(dir: PathBuf) {
    let _ = CONFIG_DIR.set(dir);
}

// ----- open event streams -----

static SUBSCRIBERS: AtomicUsize = AtomicUsize::new(0);

/// Held by an open `GET /api/events` stream: counted while it lives.
pub struct Subscriber(());

impl Subscriber {
    #[must_use]
    pub fn new() -> Self {
        SUBSCRIBERS.fetch_add(1, Ordering::Relaxed);
        Self(())
    }
}

impl Default for Subscriber {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Subscriber {
    fn drop(&mut self) {
        SUBSCRIBERS.fetch_sub(1, Ordering::Relaxed);
    }
}

#[must_use]
pub fn subscribers() -> usize {
    SUBSCRIBERS.load(Ordering::Relaxed)
}

// ----- recent server errors -----

/// API answers by minute: (minute, answers, server errors), oldest first, for the last [`WINDOW`].
#[derive(Default)]
pub struct Window {
    minutes: VecDeque<(u64, u64, u64)>,
}

impl Window {
    const MINUTES: u64 = WINDOW.as_secs() / 60;

    #[must_use]
    pub const fn new() -> Self {
        Self {
            minutes: VecDeque::new(),
        }
    }

    /// Counts one answer in `minute`.
    pub fn record(&mut self, minute: u64, server_error: bool) {
        match self.minutes.back_mut() {
            Some((last, n, errors)) if *last == minute => {
                *n += 1;
                *errors += u64::from(server_error);
            }
            _ => self.minutes.push_back((minute, 1, u64::from(server_error))),
        }
        while self
            .minutes
            .front()
            .is_some_and(|(first, _, _)| first + Self::MINUTES < minute)
        {
            self.minutes.pop_front();
        }
    }

    /// (answers, server errors) in the window that ends with `minute`.
    #[must_use]
    pub fn recent(&self, minute: u64) -> (u64, u64) {
        self.minutes
            .iter()
            .filter(|(first, _, _)| first + Self::MINUTES >= minute)
            .fold((0, 0), |(n, e), (_, a, b)| (n + a, e + b))
    }
}

static ANSWERS: Mutex<Window> = Mutex::new(Window::new());
static START: OnceLock<Instant> = OnceLock::new();

fn minute(at: Instant) -> u64 {
    at.saturating_duration_since(*START.get_or_init(Instant::now))
        .as_secs()
        / 60
}

/// Counts one API answer.
pub fn record(server_error: bool, at: Instant) {
    ANSWERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record(minute(at), server_error);
}

/// (answers, server errors) in the last [`WINDOW`].
#[must_use]
pub fn recent(at: Instant) -> (u64, u64) {
    ANSWERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .recent(minute(at))
}

/// Middleware over `/api`: counts each answer, and whether it was a server error. The health check itself and the
/// event stream (which answers once and then stays open) are not counted.
pub async fn count(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let counted = path != "/api/health" && path != "/api/events";
    let response = next.run(req).await;
    if counted {
        record(response.status().is_server_error(), Instant::now());
    }
    response
}

// ----- disk -----

/// (free, total) bytes of the disk holding `path`, where the platform tells.
#[cfg(unix)]
fn disk(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut s = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c` is a valid NUL-terminated path and `s` is written by statvfs before it is read (only on success).
    let s = unsafe {
        if libc::statvfs(c.as_ptr(), s.as_mut_ptr()) != 0 {
            return None;
        }
        s.assume_init()
    };
    #[allow(clippy::useless_conversion)] // the field types differ between platforms
    let unit = u64::from(s.f_frsize);
    #[allow(clippy::useless_conversion)]
    Some((u64::from(s.f_bavail) * unit, u64::from(s.f_blocks) * unit))
}

#[cfg(not(unix))]
fn disk(_path: &Path) -> Option<(u64, u64)> {
    None
}

/// What [`health`] reports about the disk of `dir`: free and total bytes, the free share, and whether that's enough.
#[must_use]
pub fn disk_report(dir: &Path) -> Value {
    match disk(dir) {
        Some((free, total)) if total > 0 => {
            #[allow(clippy::cast_precision_loss)]
            let share = free as f64 / total as f64;
            json!({ "freeBytes": free, "totalBytes": total, "freeShare": (share * 1000.0).round() / 1000.0,
                    "ok": share >= DISK_FREE_WARN })
        }
        _ => Value::Null,
    }
}

// ----- the route -----

async fn health(State(state): State<AppState>) -> Response {
    let at = Instant::now();
    let db_start = Instant::now();
    let pending = tokio::time::timeout(DB_TIMEOUT, state.db.pending_steps()).await;
    let latency = u64::try_from(db_start.elapsed().as_millis()).unwrap_or(u64::MAX);
    let expected = crate::db::SCHEMA_VERSION;
    let (reachable, migrations) = match pending {
        Ok(Ok(Some(pending))) => (
            true,
            json!({ "expected": expected, "pending": pending, "ok": pending.is_empty() }),
        ),
        // Reachable, but no schema at all: nothing ran yet.
        Ok(Ok(None)) => (
            true,
            json!({ "expected": expected, "pending": [], "ok": false }),
        ),
        Ok(Err(e)) => {
            tracing::warn!("health: the database didn't answer: {e}");
            (false, Value::Null)
        }
        Err(_) => {
            tracing::warn!("health: the database didn't answer within {DB_TIMEOUT:?}");
            (false, Value::Null)
        }
    };
    let migrations_ok = migrations["ok"].as_bool().unwrap_or(false);
    let disk = CONFIG_DIR.get().map_or(Value::Null, |d| disk_report(d));
    let (answers, errors) = recent(at);
    #[allow(clippy::cast_precision_loss)]
    let rate = if answers == 0 {
        0.0
    } else {
        errors as f64 / answers as f64
    };
    let errors_ok = answers < MIN_ANSWERS || rate < ERROR_RATE_WARN;
    let disk_ok = disk["ok"].as_bool().unwrap_or(true);
    let status = if !reachable || !migrations_ok {
        "down"
    } else if !disk_ok || !errors_ok {
        "degraded"
    } else {
        "ok"
    };
    let body = json!({
        "status": status,
        "version": env!("CARGO_PKG_VERSION"),
        "uptimeSeconds": state.started.elapsed().as_secs(),
        "database": { "reachable": reachable, "kind": state.db.kind(), "latencyMs": latency },
        "migrations": migrations,
        "disk": disk,
        "events": { "subscribers": subscribers() },
        "errors": { "windowSeconds": WINDOW.as_secs(), "answers": answers, "serverErrors": errors,
                    "rate": (rate * 1000.0).round() / 1000.0, "ok": errors_ok },
    });
    let code = if status == "down" {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    let mut response = (code, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
