//! The automatic backup before an upgrade.
//!
//! At start, before any table is touched, the server compares its version with the one that last started (a small
//! file in the config folder) and asks the database which migration steps are waiting. When the version changed or a
//! step waits, it writes a backup first: `pg_dump --format=custom` for PostgreSQL (checked with `pg_restore --list`),
//! `VACUUM INTO` for SQLite. It writes to a `.partial` name and renames it when the copy is complete. When the backup
//! fails the server stops and the database is left as it was, unless `backup.on_upgrade = "skip"`.
//!
//! The password goes to `pg_dump` only through its `PGPASSWORD` environment entry; it is never an argument and never
//! logged.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sqlx::Connection;

use crate::config::{BackupSettings, OnUpgrade, Settings};
use crate::db::{Db, Location};

/// The start of every automatic backup's file name. Pruning only ever looks at files named this way.
pub const PREFIX: &str = "borealis_party-";

/// The extension of a backup being written.
const PARTIAL: &str = ".partial";

/// The file in the config folder holding the version that last started.
pub const VERSION_FILE: &str = "last-started-version";

/// Why no backup could be taken.
#[derive(Debug)]
pub struct BackupError(String);

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackupError {}

fn fail(what: impl std::fmt::Display) -> BackupError {
    BackupError(what.to_string())
}

/// What the start-up check decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Nothing changes: no new version, no waiting step, or a database with nothing in it yet.
    Nothing,
    /// Back up, then upgrade.
    Backup {
        /// Why, for the log.
        reason: String,
    },
}

/// Back up when the version changed or a step waits. `pending` is `None` for a database without tables.
#[must_use]
pub fn decide(previous: Option<&str>, current: &str, pending: Option<&[i64]>) -> Decision {
    let Some(pending) = pending else {
        return Decision::Nothing;
    };
    let mut why = Vec::new();
    match previous {
        Some(old) if old == current => {}
        Some(old) => why.push(format!("version {old} to {current}")),
        None => why.push(format!("first start of {current} with backups")),
    }
    if !pending.is_empty() {
        let steps: Vec<String> = pending.iter().map(ToString::to_string).collect();
        why.push(format!("database steps {} waiting", steps.join(", ")));
    }
    if why.is_empty() {
        Decision::Nothing
    } else {
        Decision::Backup {
            reason: why.join("; "),
        }
    }
}

/// The version that last started from `config_dir`, if it says.
#[must_use]
pub fn previous_version(config_dir: &Path) -> Option<String> {
    std::fs::read_to_string(config_dir.join(VERSION_FILE))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Record that `version` started: the tables are up to date for it.
///
/// # Errors
/// When the file can't be written.
pub fn record_started(config_dir: &Path, version: &str) -> std::io::Result<()> {
    let file = config_dir.join(VERSION_FILE);
    let temp = config_dir.join(format!("{VERSION_FILE}{PARTIAL}"));
    std::fs::write(&temp, format!("{version}\n"))?;
    std::fs::rename(&temp, &file)
}

/// `YYYYMMDDTHHMMSSZ` for seconds since the epoch, in UTC.
#[must_use]
pub fn stamp(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rest = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// A version as it may appear in a file name.
fn safe(version: &str) -> String {
    version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `borealis_party-<old>-to-<new>-<stamp>.<dump|sqlite>`.
#[must_use]
pub fn file_name(old: Option<&str>, new: &str, secs: u64, postgres: bool) -> String {
    format!(
        "{PREFIX}{}-to-{}-{}.{}",
        safe(old.unwrap_or("unknown")),
        safe(new),
        stamp(secs),
        if postgres { "dump" } else { "sqlite" }
    )
}

/// The stamp of an automatic backup's file name, or `None` for any other file.
fn automatic(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(PREFIX)?;
    let rest = rest
        .strip_suffix(".dump")
        .or_else(|| rest.strip_suffix(".sqlite"))?;
    let (versions, stamp) = rest.rsplit_once('-')?;
    let ok = stamp.len() == 16
        && stamp.ends_with('Z')
        && stamp.as_bytes()[8] == b'T'
        && stamp
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 8 || i == 15 || b.is_ascii_digit());
    (ok && versions.contains("-to-")).then_some(stamp)
}

/// The automatic backups in `dir`, oldest first.
fn backups(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stamp = automatic(&name)?.to_string();
            Some((format!("{stamp} {name}"), e.path()))
        })
        .collect();
    found.sort();
    found
}

/// Delete the oldest automatic backups so `keep` are left (0 keeps all). Other files are never touched.
///
/// # Errors
/// When one can't be deleted.
pub fn prune(dir: &Path, keep: usize) -> std::io::Result<Vec<PathBuf>> {
    if keep == 0 {
        return Ok(Vec::new());
    }
    let found = backups(dir);
    let extra = found.len().saturating_sub(keep);
    let mut removed = Vec::new();
    for (_, path) in found.into_iter().take(extra) {
        std::fs::remove_file(&path)?;
        removed.push(path);
    }
    Ok(removed)
}

/// Remove `.partial` files an interrupted backup left (automatic names only).
fn clear_partials(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(base) = name.strip_suffix(PARTIAL) {
            if automatic(base).is_some() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// The PostgreSQL connection, from the server's URL.
struct Conn {
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    database: String,
    sslmode: Option<String>,
}

impl Conn {
    fn parse(url: &str) -> Result<Self, BackupError> {
        let parsed =
            url::Url::parse(url).map_err(|_| fail("the PostgreSQL address is not valid"))?;
        let password = parsed.password().map(percent_decode);
        Ok(Self {
            host: parsed.host_str().unwrap_or("localhost").to_string(),
            port: parsed.port().unwrap_or(5432),
            user: percent_decode(parsed.username()),
            password,
            database: percent_decode(parsed.path().trim_start_matches('/')),
            sslmode: parsed
                .query_pairs()
                .find(|(k, _)| k == "sslmode")
                .map(|(_, v)| v.into_owned()),
        })
    }
}

/// `%xx` escapes undone.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%')
            .then(|| text.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        if let Some(b) = hex {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A `pg_*` program, from `postgres_bin` when set.
fn program(settings: &BackupSettings, name: &str) -> PathBuf {
    settings
        .postgres_bin
        .as_ref()
        .map_or_else(|| PathBuf::from(name), |dir| dir.join(name))
}

/// The `pg_dump` command for `url` writing to `out`. Everything about the connection goes in `PG*` environment
/// entries, so the arguments are only the format and the file.
pub(crate) fn pg_dump_command(
    settings: &BackupSettings,
    url: &str,
    out: &Path,
) -> Result<tokio::process::Command, BackupError> {
    let conn = Conn::parse(url)?;
    let mut cmd = tokio::process::Command::new(program(settings, "pg_dump"));
    cmd.arg("--format=custom")
        .arg("--no-password")
        .arg("--file")
        .arg(out)
        .env("PGHOST", &conn.host)
        .env("PGPORT", conn.port.to_string())
        .env("PGUSER", &conn.user)
        .env("PGDATABASE", &conn.database)
        .env("PGAPPNAME", "borealis-party-backup")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(password) = &conn.password {
        cmd.env("PGPASSWORD", password);
    }
    if let Some(mode) = &conn.sslmode {
        cmd.env("PGSSLMODE", mode);
    }
    Ok(cmd)
}

/// A child's error output, with the password taken out should it ever appear.
fn tell(output: &std::process::Output, password: Option<&str>) -> String {
    let mut text = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if let Some(p) = password.filter(|p| !p.is_empty()) {
        text = text.replace(p, "<redacted>");
    }
    if text.is_empty() {
        format!("exit status {}", output.status)
    } else {
        text
    }
}

async fn dump_postgres(
    settings: &BackupSettings,
    url: &str,
    out: &Path,
) -> Result<(), BackupError> {
    let password = Conn::parse(url)?.password;
    let dump = pg_dump_command(settings, url, out)?
        .output()
        .await
        .map_err(|e| {
            fail(format!(
                "cannot run pg_dump ({e}); install the PostgreSQL client or set backup.postgres_bin"
            ))
        })?;
    if !dump.status.success() {
        return Err(fail(format!(
            "pg_dump failed: {}",
            tell(&dump, password.as_deref())
        )));
    }
    let list = tokio::process::Command::new(program(settings, "pg_restore"))
        .arg("--list")
        .arg(out)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| fail(format!("cannot run pg_restore to check the backup ({e})")))?;
    if !list.status.success() || list.stdout.is_empty() {
        return Err(fail(format!(
            "pg_restore --list does not accept the backup: {}",
            tell(&list, password.as_deref())
        )));
    }
    Ok(())
}

async fn dump_sqlite(db: &Db, out: &Path) -> Result<(), BackupError> {
    let target = out.to_string_lossy().replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{target}'"))
        .execute(db.pool())
        .await
        .map_err(|e| fail(format!("VACUUM INTO failed: {e}")))?;
    let mut copy = sqlx::AnyConnection::connect(&format!("sqlite://{}?mode=ro", out.display()))
        .await
        .map_err(|e| fail(format!("cannot open the backup: {e}")))?;
    let check: String = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_one(&mut copy)
        .await
        .map_err(|e| fail(format!("cannot check the backup: {e}")))?;
    let _ = copy.close().await;
    if check != "ok" {
        return Err(fail(format!("the backup fails its check: {check}")));
    }
    Ok(())
}

/// A backup that was written.
#[derive(Debug, Clone)]
pub struct Written {
    /// Where.
    pub path: PathBuf,
    /// Its size.
    pub bytes: u64,
}

/// Take a backup of `db` into `dir` now, whatever the version.
///
/// # Errors
/// When the copy can't be made or checked; nothing is left behind then.
pub async fn write(
    db: &Db,
    location: &Location,
    settings: &BackupSettings,
    dir: &Path,
    old: Option<&str>,
    new: &str,
) -> Result<Written, BackupError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| fail(format!("cannot create {}: {e}", dir.display())))?;
    clear_partials(dir);
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let postgres = matches!(location, Location::Postgres(_));
    let path = dir.join(file_name(old, new, secs, postgres));
    let temp = PathBuf::from(format!("{}{PARTIAL}", path.display()));
    let done = match location {
        Location::Postgres(url) => dump_postgres(settings, url, &temp).await,
        _ => dump_sqlite(db, &temp).await,
    };
    if let Err(e) = done {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    std::fs::rename(&temp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        fail(format!("cannot rename the backup into place: {e}"))
    })?;
    let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
    Ok(Written { path, bytes })
}

/// The start-up check: back up when needed, before [`Db::upgrade`]. `Ok(None)` when no backup was needed or it was
/// skipped on purpose.
///
/// # Errors
/// When a backup was needed and could not be written: the caller must not upgrade.
pub async fn before_upgrade(
    db: &Db,
    location: &Location,
    settings: &BackupSettings,
    config_dir: &Path,
    version: &str,
) -> Result<Option<Written>, BackupError> {
    let pending = db
        .pending_steps()
        .await
        .map_err(|e| fail(format!("cannot read the schema version: {e}")))?;
    let previous = previous_version(config_dir);
    let Decision::Backup { reason } = decide(previous.as_deref(), version, pending.as_deref())
    else {
        return Ok(None);
    };
    if settings.on_upgrade == OnUpgrade::Skip {
        tracing::warn!("!!! ============================================================ !!!");
        tracing::warn!(
            "!!! NO BACKUP before this upgrade ({reason}): backup.on_upgrade = skip (BOREALIS__BACKUP__ON_UPGRADE). \
             Remove the setting once the server has started."
        );
        tracing::warn!("!!! ============================================================ !!!");
        return Ok(None);
    }
    let dir = Settings::resolve(config_dir, &settings.dir);
    tracing::info!(
        "backing up the database before the upgrade ({reason}) into {}",
        dir.display()
    );
    let written = write(db, location, settings, &dir, previous.as_deref(), version).await?;
    tracing::info!(
        "backup written: {} ({} bytes). Copying it off this host is recommended",
        written.path.display(),
        written.bytes
    );
    match prune(&dir, settings.keep) {
        Ok(removed) => {
            for path in removed {
                tracing::info!("removed the old backup {}", path.display());
            }
        }
        Err(e) => tracing::warn!("cannot remove old backups: {e}"),
    }
    Ok(Some(written))
}

/// The whole start-up sequence: [`before_upgrade`], then [`Db::upgrade`] only when that succeeded, then record the
/// version as started.
///
/// # Errors
/// When the backup or the upgrade fails. After a failed backup no table has been changed.
pub async fn upgrade_with_backup(
    db: &Db,
    location: &Location,
    settings: &BackupSettings,
    config_dir: &Path,
    version: &str,
) -> Result<Option<Written>, String> {
    let written = before_upgrade(db, location, settings, config_dir, version)
        .await
        .map_err(|e| {
            format!(
                "the automatic backup before the upgrade failed, so the database was not changed: {e}. Fix the cause and start again, or set BOREALIS__BACKUP__ON_UPGRADE=skip once to start without a backup"
            )
        })?;
    db.upgrade().await.map_err(|e| e.to_string())?;
    if let Err(e) = record_started(config_dir, version) {
        tracing::warn!(
            "cannot record the started version in {}: {e}",
            config_dir.display()
        );
    }
    Ok(written)
}

static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Where the admin page looks for backups; set once at start.
pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

/// For the admin page: the newest automatic backup's time and size, and how many there are. No file contents.
#[must_use]
pub fn status() -> Value {
    let Some(dir) = DIR.get() else {
        return Value::Null;
    };
    let found = backups(dir);
    let last = found.last().and_then(|(_, path)| {
        let meta = std::fs::metadata(path).ok()?;
        let at = meta
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_secs();
        Some(json!({ "at": at, "bytes": meta.len() }))
    });
    json!({ "last": last, "count": found.len() })
}
