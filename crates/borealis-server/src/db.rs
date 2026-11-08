//! SQLite holds who is who and who belongs where; the character and content files stay in the Borealis workspaces.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use crate::error::{ApiError, ApiResult};

const SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL COLLATE NOCASE UNIQUE,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('dm', 'player')),
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_user ON sessions(user_id);
CREATE TABLE IF NOT EXISTS groups (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    dm_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS groups_dm ON groups(dm_id);
CREATE TABLE IF NOT EXISTS invites (
    id TEXT PRIMARY KEY,
    group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    player_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    UNIQUE (group_id, player_id)
);
CREATE INDEX IF NOT EXISTS invites_player ON invites(player_id);
-- One row per player in a group, so one character per group. A character is in at most one group.
CREATE TABLE IF NOT EXISTS members (
    group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    player_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    character_id TEXT UNIQUE,
    joined_at INTEGER NOT NULL,
    PRIMARY KEY (group_id, player_id)
);
CREATE INDEX IF NOT EXISTS members_player ON members(player_id);
";

/// The database. Calls are short, so one connection behind a mutex is enough for a table's worth of friends.
#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    /// Open or create the database at `path`.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        Self::setup(Connection::open(path)?)
    }

    /// A database that disappears with the process (tests).
    #[cfg(test)]
    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::setup(Connection::open_in_memory()?)
    }

    fn setup(conn: Connection) -> rusqlite::Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    /// Run `work` on a blocking thread with the connection.
    pub async fn run<T, F>(&self, work: F) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> ApiResult<T> + Send + 'static,
    {
        let db = Arc::clone(&self.0);
        tokio::task::spawn_blocking(move || {
            let mut conn = db
                .lock()
                .map_err(|_| ApiError::Internal("database lock poisoned".into()))?;
            work(&mut conn)
        })
        .await
        .map_err(|e| ApiError::Internal(format!("database task: {e}")))?
    }
}

/// Seconds since the epoch.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
