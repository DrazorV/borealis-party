//! Who is who and who belongs where, in SQLite or PostgreSQL. Character and content files stay in the Borealis
//! workspaces on disk.
//!
//! Every query is written once in SQL that both databases accept: `$n` placeholders in ascending order of first use,
//! `lower(...)` instead of collations, `ON CONFLICT DO NOTHING`, and no booleans read back from SQL.

use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::any::{AnyPoolOptions, AnyRow};
use sqlx::{AnyPool, Row};

use crate::auth::{Status, User};
use crate::error::{ApiError, ApiResult};

/// Applied in order; each runs once. Never edit a step that has shipped, add a new one.
const MIGRATIONS: &[&[&str]] = &[
    SCHEMA_1,
    // 2: account settings.
    &[
        // Each account's own Application Settings (settings.rs), as a JSON object.
        "CREATE TABLE IF NOT EXISTS user_settings (
        user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
        settings TEXT NOT NULL
    )",
    ],
    // 3: every account can be DM and player (users.role stays, unused); email, approval, admins, runtime settings
    // and the activity log. Accounts that exist already are active, and the oldest is the admin.
    &[
        "ALTER TABLE users ADD COLUMN email TEXT",
        "ALTER TABLE users ADD COLUMN status TEXT NOT NULL DEFAULT 'active'",
        "ALTER TABLE users ADD COLUMN is_admin BIGINT NOT NULL DEFAULT 0",
        "ALTER TABLE users ADD COLUMN must_change_password BIGINT NOT NULL DEFAULT 0",
        "ALTER TABLE users ADD COLUMN email_verified_at BIGINT",
        "ALTER TABLE users ADD COLUMN verify_token_hash TEXT",
        "ALTER TABLE users ADD COLUMN verify_expires_at BIGINT",
        "CREATE UNIQUE INDEX IF NOT EXISTS users_email ON users (lower(email))",
        "CREATE INDEX IF NOT EXISTS users_verify ON users (verify_token_hash)",
        "UPDATE users SET is_admin = 1 WHERE id = (SELECT id FROM users ORDER BY created_at, id LIMIT 1)",
        "CREATE TABLE IF NOT EXISTS server_settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        // No foreign keys: the log outlives the accounts and groups it names.
        "CREATE TABLE IF NOT EXISTS activity (
            id TEXT PRIMARY KEY,
            at BIGINT NOT NULL,
            kind TEXT NOT NULL,
            actor_id TEXT,
            actor_name TEXT,
            target_id TEXT,
            target_name TEXT,
            detail TEXT,
            ip TEXT
        )",
    ],
];

/// Rows the activity log keeps; older ones are pruned as new ones arrive.
pub const ACTIVITY_KEPT: i64 = 10_000;

/// The schema as first shipped.
pub(crate) const SCHEMA_1: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS users (
        id TEXT PRIMARY KEY,
        username TEXT NOT NULL,
        password_hash TEXT NOT NULL,
        role TEXT NOT NULL CHECK (role IN ('dm', 'player')),
        created_at BIGINT NOT NULL
    )",
    "CREATE UNIQUE INDEX IF NOT EXISTS users_username ON users (lower(username))",
    "CREATE TABLE IF NOT EXISTS sessions (
        token_hash TEXT PRIMARY KEY,
        user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        expires_at BIGINT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS sessions_user ON sessions (user_id)",
    "CREATE TABLE IF NOT EXISTS party_groups (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        dm_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        created_at BIGINT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS party_groups_dm ON party_groups (dm_id)",
    "CREATE TABLE IF NOT EXISTS invites (
        id TEXT PRIMARY KEY,
        group_id TEXT NOT NULL REFERENCES party_groups(id) ON DELETE CASCADE,
        player_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        created_at BIGINT NOT NULL,
        UNIQUE (group_id, player_id)
    )",
    "CREATE INDEX IF NOT EXISTS invites_player ON invites (player_id)",
    // One row per player in a group, so one character per group; a character is in at most one group.
    "CREATE TABLE IF NOT EXISTS members (
        group_id TEXT NOT NULL REFERENCES party_groups(id) ON DELETE CASCADE,
        player_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        character_id TEXT UNIQUE,
        joined_at BIGINT NOT NULL,
        PRIMARY KEY (group_id, player_id)
    )",
    "CREATE INDEX IF NOT EXISTS members_player ON members (player_id)",
];

/// The account columns [`user_from`] reads, from `users u`.
const USER_COLUMNS: &str = "u.id, u.username, u.email, u.is_admin, u.must_change_password";

/// The columns [`account_from`] reads, from `users u`.
const ACCOUNT_COLUMNS: &str =
    "u.id, u.username, u.email, u.is_admin, u.must_change_password, u.status, \
     u.created_at, u.email_verified_at, u.verify_token_hash, \
     (SELECT COUNT(*) FROM sessions s WHERE s.user_id = u.id AND s.expires_at > $1)";

/// A condition on `users` that leaves the last active admin alone: true when the row is not one, or another active
/// admin exists. `id` is the placeholder holding the row's id.
fn not_last_admin(id: &str) -> String {
    format!(
        "(is_admin = 0 OR status <> 'active' OR \
         (SELECT COUNT(*) FROM users o WHERE o.is_admin = 1 AND o.status = 'active' AND o.id <> {id}) > 0)"
    )
}

/// Where the database is.
#[derive(Debug, Clone)]
pub enum Location {
    /// A SQLite file.
    Sqlite(std::path::PathBuf),
    /// SQLite in memory (tests).
    #[cfg(test)]
    Memory,
    /// A PostgreSQL connection URL.
    Postgres(String),
}

/// A group in a list.
#[derive(Debug, Clone)]
pub struct GroupRow {
    /// Id.
    pub id: String,
    /// Name.
    pub name: String,
    /// The DM's username.
    pub dm_name: String,
    /// The DM's account id.
    pub dm_id: String,
    /// How many players.
    pub member_count: i64,
    /// The caller's character in it.
    pub character_id: Option<String>,
}

/// A player in a group.
#[derive(Debug, Clone)]
pub struct MemberRow {
    /// Account id.
    pub player: String,
    /// Username.
    pub username: String,
    /// Their character in the group.
    pub character: Option<String>,
}

/// What an invitation attempt found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteOutcome {
    /// Invited.
    Sent,
    /// No such username, or not an active account.
    NoAccount,
    /// The account runs the group.
    OwnGroup,
    /// Already in the group.
    AlreadyMember,
    /// Already invited.
    AlreadyInvited,
}

/// An account to add.
pub struct NewAccount<'a> {
    /// Id.
    pub id: &'a str,
    /// Username.
    pub username: &'a str,
    /// Email address, already checked.
    pub email: Option<&'a str>,
    /// Argon2 hash.
    pub password_hash: &'a str,
    /// Where it starts.
    pub status: Status,
    /// Whether it is an admin.
    pub admin: bool,
    /// The hash of the emailed link's token, while the address is unconfirmed.
    pub verify_token_hash: Option<&'a str>,
    /// When that link stops working.
    pub verify_expires_at: Option<i64>,
}

/// Why an account wasn't added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    /// Added.
    Added,
    /// The username is taken.
    UsernameTaken,
    /// The email address is taken.
    EmailTaken,
}

/// An account as an admin sees it.
#[derive(Debug, Clone)]
pub struct AccountRow {
    /// The account.
    pub user: User,
    /// Status.
    pub status: Status,
    /// When it was created.
    pub created_at: i64,
    /// When the email address was confirmed.
    pub email_verified_at: Option<i64>,
    /// Still waiting for the emailed link to be opened.
    pub awaiting_email: bool,
    /// Live sessions.
    pub sessions: i64,
}

/// What a sign-in found for a username or email address.
pub struct Login {
    /// The account.
    pub user: User,
    /// Its password hash.
    pub password_hash: String,
    /// Whether it may sign in.
    pub status: Status,
    /// Still waiting for the emailed link to be opened.
    pub awaiting_email: bool,
}

/// One activity log entry.
#[derive(Debug, Clone, Default)]
pub struct Activity {
    /// When.
    pub at: i64,
    /// What happened, for example `sign_in` or `approve`.
    pub kind: String,
    /// Who did it.
    pub actor_id: Option<String>,
    /// Their username then.
    pub actor_name: Option<String>,
    /// Whom or what it was done to.
    pub target_id: Option<String>,
    /// Its name then.
    pub target_name: Option<String>,
    /// More words.
    pub detail: Option<String>,
    /// The client's address.
    pub ip: Option<String>,
}

/// Counts for the admin page.
#[derive(Debug, Clone, Default)]
pub struct Counts {
    /// Accounts by status.
    pub accounts: Vec<(String, i64)>,
    /// Admins.
    pub admins: i64,
    /// Groups.
    pub groups: i64,
    /// Characters in groups.
    pub group_characters: i64,
    /// Pending invitations.
    pub invites: i64,
    /// Unexpired sessions.
    pub sessions: i64,
}

/// The database.
#[derive(Clone)]
pub struct Db {
    pool: AnyPool,
    kind: &'static str,
}

/// Seconds since the epoch.
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::Internal(format!("database: {e}"))
    }
}

fn user_from(row: &AnyRow) -> sqlx::Result<User> {
    Ok(User {
        id: row.try_get(0)?,
        username: row.try_get(1)?,
        email: row.try_get(2)?,
        admin: row.try_get::<i64, _>(3)? != 0,
        must_change_password: row.try_get::<i64, _>(4)? != 0,
    })
}

fn account_from(row: &AnyRow) -> sqlx::Result<AccountRow> {
    Ok(AccountRow {
        user: user_from(row)?,
        status: Status::parse(&row.try_get::<String, _>(5)?),
        created_at: row.try_get(6)?,
        email_verified_at: row.try_get(7)?,
        awaiting_email: row.try_get::<Option<String>, _>(8)?.is_some(),
        sessions: row.try_get(9)?,
    })
}

fn flag(value: bool) -> i64 {
    i64::from(value)
}

impl Db {
    /// The connection pool, for the migration between databases.
    pub(crate) fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// `sqlite` or `postgres`.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// Connect, and create or update the tables.
    ///
    /// # Errors
    /// When the database can't be reached or its tables can't be created.
    pub async fn open(location: &Location) -> sqlx::Result<Self> {
        sqlx::any::install_default_drivers();
        let (url, sqlite, max) = match location {
            Location::Sqlite(path) => (format!("sqlite://{}?mode=rwc", path.display()), true, 4),
            #[cfg(test)]
            Location::Memory => ("sqlite::memory:".to_string(), true, 1),
            Location::Postgres(url) => (url.clone(), false, 10),
        };
        let pool = AnyPoolOptions::new()
            .max_connections(max)
            .after_connect(move |conn, _| {
                Box::pin(async move {
                    if sqlite {
                        sqlx::query("PRAGMA foreign_keys = ON")
                            .execute(&mut *conn)
                            .await?;
                        sqlx::query("PRAGMA busy_timeout = 5000")
                            .execute(&mut *conn)
                            .await?;
                    }
                    Ok(())
                })
            })
            .connect(&url)
            .await?;
        if sqlite && !matches!(location, Location::Postgres(_)) {
            // Persistent per file; memory databases ignore it.
            sqlx::query("PRAGMA journal_mode = WAL")
                .execute(&pool)
                .await?;
        }
        let db = Self {
            pool,
            kind: if sqlite { "sqlite" } else { "postgres" },
        };
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> sqlx::Result<()> {
        sqlx::query("CREATE TABLE IF NOT EXISTS schema_version (version BIGINT NOT NULL)")
            .execute(&self.pool)
            .await?;
        let current: i64 = sqlx::query("SELECT COALESCE(MAX(version), 0) FROM schema_version")
            .fetch_one(&self.pool)
            .await?
            .try_get(0)?;
        for (index, steps) in MIGRATIONS.iter().enumerate() {
            let version = i64::try_from(index).unwrap_or(i64::MAX) + 1;
            if version <= current {
                continue;
            }
            let mut tx = self.pool.begin().await?;
            for step in *steps {
                sqlx::query(step).execute(&mut *tx).await?;
            }
            sqlx::query("INSERT INTO schema_version (version) VALUES ($1)")
                .bind(version)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Ok(())
    }

    // Accounts and sessions.

    /// Add an account.
    pub async fn insert_user(&self, account: &NewAccount<'_>) -> ApiResult<InsertOutcome> {
        let mut tx = self.pool.begin().await?;
        let taken = sqlx::query("SELECT 1 FROM users WHERE lower(username) = lower($1)")
            .bind(account.username)
            .fetch_optional(&mut *tx)
            .await?;
        if taken.is_some() {
            return Ok(InsertOutcome::UsernameTaken);
        }
        if let Some(email) = account.email {
            let taken = sqlx::query("SELECT 1 FROM users WHERE lower(email) = lower($1)")
                .bind(email)
                .fetch_optional(&mut *tx)
                .await?;
            if taken.is_some() {
                return Ok(InsertOutcome::EmailTaken);
            }
        }
        // `role` is from when accounts were either a DM or a player; it is no longer read.
        let done = sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, created_at, email, status, is_admin,
                must_change_password, verify_token_hash, verify_expires_at)
             VALUES ($1, $2, $3, 'player', $4, $5, $6, $7, 0, $8, $9)
             ON CONFLICT DO NOTHING",
        )
        .bind(account.id)
        .bind(account.username)
        .bind(account.password_hash)
        .bind(now())
        .bind(account.email.map(str::to_string))
        .bind(account.status.as_str())
        .bind(flag(account.admin))
        .bind(account.verify_token_hash.map(str::to_string))
        .bind(account.verify_expires_at)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 0 {
            return Ok(InsertOutcome::UsernameTaken);
        }
        tx.commit().await?;
        Ok(InsertOutcome::Added)
    }

    /// How many accounts exist.
    pub async fn user_count(&self) -> ApiResult<i64> {
        Ok(sqlx::query("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?
            .try_get(0)?)
    }

    /// An account by username or email address (any case), for signing in.
    pub async fn find_login(&self, name: &str) -> ApiResult<Option<Login>> {
        let row = sqlx::query(&format!(
            "SELECT {USER_COLUMNS}, u.password_hash, u.status, u.verify_token_hash FROM users u
             WHERE lower(u.username) = lower($1) OR lower(u.email) = lower($1)
             ORDER BY u.created_at LIMIT 1"
        ))
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        let Some(r) = row else {
            return Ok(None);
        };
        Ok(Some(Login {
            user: user_from(&r)?,
            password_hash: r.try_get(5)?,
            status: Status::parse(&r.try_get::<String, _>(6)?),
            awaiting_email: r.try_get::<Option<String>, _>(7)?.is_some(),
        }))
    }

    /// An account's password hash.
    pub async fn password_hash(&self, user: &str) -> ApiResult<Option<String>> {
        let row = sqlx::query("SELECT password_hash FROM users WHERE id = $1")
            .bind(user)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.try_get(0)).transpose()?)
    }

    /// Store a new session and forget expired ones.
    pub async fn insert_session(
        &self,
        token_hash: &str,
        user: &str,
        expires_at: i64,
    ) -> ApiResult<()> {
        sqlx::query("DELETE FROM sessions WHERE expires_at < $1")
            .bind(now())
            .execute(&self.pool)
            .await?;
        sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, $3)")
            .bind(token_hash)
            .bind(user)
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The active account behind a live session.
    pub async fn session_user(&self, token_hash: &str) -> ApiResult<Option<User>> {
        let row = sqlx::query(&format!(
            "SELECT {USER_COLUMNS} FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1 AND s.expires_at > $2 AND u.status = 'active'"
        ))
        .bind(token_hash)
        .bind(now())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| user_from(&r)).transpose()?)
    }

    /// Sign a session out.
    pub async fn delete_session(&self, token_hash: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Sign an account out everywhere; how many sessions ended.
    pub async fn delete_sessions_of(&self, user: &str) -> ApiResult<u64> {
        let done = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected())
    }

    /// Sign an account out everywhere but one session.
    pub async fn delete_other_sessions(&self, user: &str, keep: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND token_hash <> $2")
            .bind(user)
            .bind(keep)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Confirm the email address an emailed link belongs to; the account, or `None` for an unknown or old link.
    pub async fn confirm_email(&self, token_hash: &str) -> ApiResult<Option<AccountRow>> {
        let mut tx = self.pool.begin().await?;
        let found = sqlx::query(
            "SELECT id FROM users WHERE verify_token_hash = $1 AND verify_expires_at > $2",
        )
        .bind(token_hash)
        .bind(now())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(found) = found else {
            return Ok(None);
        };
        let id: String = found.try_get(0)?;
        sqlx::query(
            "UPDATE users SET email_verified_at = $1, verify_token_hash = NULL, verify_expires_at = NULL
             WHERE id = $2",
        )
        .bind(now())
        .bind(&id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.account(&id).await
    }

    /// One account, as an admin sees it.
    pub async fn account(&self, user: &str) -> ApiResult<Option<AccountRow>> {
        let row = sqlx::query(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM users u WHERE u.id = $2"
        ))
        .bind(now())
        .bind(user)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| account_from(&r)).transpose()?)
    }

    /// Accounts whose username or email contains `search`, optionally with one status: a page of them, and how many
    /// match in all. Pending accounts come first, then the newest.
    pub async fn accounts(
        &self,
        search: &str,
        status: Option<Status>,
        limit: i64,
        offset: i64,
    ) -> ApiResult<(Vec<AccountRow>, i64)> {
        let pattern = format!(
            "%{}%",
            search
                .trim()
                .to_lowercase()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let status = status.map_or("", Status::as_str);
        let filter = "(lower(u.username) LIKE $2 ESCAPE '\\' OR lower(COALESCE(u.email, '')) LIKE $2 ESCAPE '\\')
             AND ($3 = '' OR u.status = $3)";
        let rows = sqlx::query(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM users u WHERE {filter}
             ORDER BY CASE WHEN u.status = 'pending' THEN 0 ELSE 1 END, u.created_at DESC, u.id
             LIMIT $4 OFFSET $5"
        ))
        .bind(now())
        .bind(&pattern)
        .bind(status)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        // The same filter with the placeholders it needs renumbered from 1.
        let total: i64 = sqlx::query(&format!(
            "SELECT COUNT(*) FROM users u WHERE {}",
            filter.replace("$2", "$1").replace("$3", "$2")
        ))
        .bind(&pattern)
        .bind(status)
        .fetch_one(&self.pool)
        .await?
        .try_get(0)?;
        let rows = rows.iter().map(account_from).collect::<sqlx::Result<_>>()?;
        Ok((rows, total))
    }

    /// Change an account's status; `false` when it is the last active admin (or doesn't exist) and the change would
    /// take them away.
    pub async fn set_status(&self, user: &str, status: Status) -> ApiResult<bool> {
        let guard = if status == Status::Active {
            "1 = 1".to_string()
        } else {
            not_last_admin("$2")
        };
        let done = sqlx::query(&format!(
            "UPDATE users SET status = $1, verify_token_hash = NULL, verify_expires_at = NULL
             WHERE id = $2 AND {guard}"
        ))
        .bind(status.as_str())
        .bind(user)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Make an account an admin or not; `false` when that would leave no active admin (or it doesn't exist).
    pub async fn set_admin(&self, user: &str, admin: bool) -> ApiResult<bool> {
        let guard = if admin {
            "1 = 1".to_string()
        } else {
            not_last_admin("$2")
        };
        let done = sqlx::query(&format!(
            "UPDATE users SET is_admin = $1 WHERE id = $2 AND {guard}"
        ))
        .bind(flag(admin))
        .bind(user)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Replace a password hash, and say whether it has to be changed at the next sign-in.
    pub async fn set_password(
        &self,
        user: &str,
        hash: Option<&str>,
        must_change: bool,
    ) -> ApiResult<bool> {
        let done = sqlx::query(
            "UPDATE users SET password_hash = COALESCE($1, password_hash), must_change_password = $2
             WHERE id = $3",
        )
        .bind(hash.map(str::to_string))
        .bind(flag(must_change))
        .bind(user)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Set an account's email address; `false` when another account has it.
    pub async fn set_email(&self, user: &str, email: Option<&str>) -> ApiResult<bool> {
        let mut tx = self.pool.begin().await?;
        if let Some(email) = email {
            let taken =
                sqlx::query("SELECT 1 FROM users WHERE lower(email) = lower($1) AND id <> $2")
                    .bind(email)
                    .bind(user)
                    .fetch_optional(&mut *tx)
                    .await?;
            if taken.is_some() {
                return Ok(false);
            }
        }
        sqlx::query("UPDATE users SET email = $1 WHERE id = $2")
            .bind(email.map(str::to_string))
            .bind(user)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Delete an account with its sessions, invitations, memberships and the groups it runs; `false` when it is the
    /// last active admin or doesn't exist. Characters and groups' files are the caller's to move first.
    pub async fn delete_user(&self, user: &str) -> ApiResult<bool> {
        let done = sqlx::query(&format!(
            "DELETE FROM users WHERE id = $1 AND {}",
            not_last_admin("$1")
        ))
        .bind(user)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// How many active admins there are.
    pub async fn active_admins(&self) -> ApiResult<i64> {
        Ok(
            sqlx::query("SELECT COUNT(*) FROM users WHERE is_admin = 1 AND status = 'active'")
                .fetch_one(&self.pool)
                .await?
                .try_get(0)?,
        )
    }

    /// Where active admins with an address get email.
    pub async fn admin_emails(&self) -> ApiResult<Vec<String>> {
        let rows = sqlx::query(
            "SELECT email FROM users WHERE is_admin = 1 AND status = 'active' AND email IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| Ok(r.try_get(0)?))
            .collect::<ApiResult<_>>()
    }

    /// The groups a user runs: `(id, name)`.
    pub async fn groups_run_by(&self, user: &str) -> ApiResult<Vec<(String, String)>> {
        let rows = sqlx::query("SELECT id, name FROM party_groups WHERE dm_id = $1")
            .bind(user)
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|r| Ok((r.try_get(0)?, r.try_get(1)?)))
            .collect()
    }

    /// The groups a user plays in: `(group id, DM id, character)`.
    pub async fn memberships_of(
        &self,
        user: &str,
    ) -> ApiResult<Vec<(String, String, Option<String>)>> {
        let rows = sqlx::query(
            "SELECT g.id, g.dm_id, m.character_id FROM members m JOIN party_groups g ON g.id = m.group_id
             WHERE m.player_id = $1",
        )
        .bind(user)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| Ok((r.try_get(0)?, r.try_get(1)?, r.try_get(2)?)))
            .collect()
    }

    // Admin page.

    /// Counts for the statistics.
    pub async fn counts(&self) -> ApiResult<Counts> {
        let one = |sql: &'static str| async move {
            sqlx::query(sql)
                .fetch_one(&self.pool)
                .await?
                .try_get::<i64, _>(0)
        };
        let accounts = sqlx::query("SELECT status, COUNT(*) FROM users GROUP BY status")
            .fetch_all(&self.pool)
            .await?
            .iter()
            .map(|r| Ok((r.try_get(0)?, r.try_get(1)?)))
            .collect::<sqlx::Result<_>>()?;
        Ok(Counts {
            accounts,
            admins: one("SELECT COUNT(*) FROM users WHERE is_admin = 1").await?,
            groups: one("SELECT COUNT(*) FROM party_groups").await?,
            group_characters: one("SELECT COUNT(*) FROM members WHERE character_id IS NOT NULL")
                .await?,
            invites: one("SELECT COUNT(*) FROM invites").await?,
            sessions: sqlx::query("SELECT COUNT(*) FROM sessions WHERE expires_at > $1")
                .bind(now())
                .fetch_one(&self.pool)
                .await?
                .try_get(0)?,
        })
    }

    /// A setting saved on the admin page.
    pub async fn setting(&self, key: &str) -> ApiResult<Option<String>> {
        let row = sqlx::query("SELECT value FROM server_settings WHERE key = $1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.try_get(0)).transpose()?)
    }

    /// Save a setting from the admin page.
    pub async fn set_setting(&self, key: &str, value: &str) -> ApiResult<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM server_settings WHERE key = $1")
            .bind(key)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO server_settings (key, value) VALUES ($1, $2)")
            .bind(key)
            .bind(value)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Add to the activity log, and prune it to [`ACTIVITY_KEPT`] rows.
    pub async fn log(&self, entry: &Activity) -> ApiResult<()> {
        // Version 7 ids sort by time, so the newest rows have the largest ids.
        let id = uuid::Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO activity (id, at, kind, actor_id, actor_name, target_id, target_name, detail, ip)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&id)
        .bind(entry.at)
        .bind(&entry.kind)
        .bind(entry.actor_id.clone())
        .bind(entry.actor_name.clone())
        .bind(entry.target_id.clone())
        .bind(entry.target_name.clone())
        .bind(entry.detail.clone())
        .bind(entry.ip.clone())
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "DELETE FROM activity WHERE id <= (SELECT id FROM activity ORDER BY id DESC LIMIT 1 OFFSET $1)",
        )
        .bind(ACTIVITY_KEPT)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// A page of the activity log, newest first, and how many entries there are.
    pub async fn activity(&self, limit: i64, offset: i64) -> ApiResult<(Vec<Activity>, i64)> {
        let rows = sqlx::query(
            "SELECT at, kind, actor_id, actor_name, target_id, target_name, detail, ip FROM activity
             ORDER BY id DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        let total: i64 = sqlx::query("SELECT COUNT(*) FROM activity")
            .fetch_one(&self.pool)
            .await?
            .try_get(0)?;
        let rows = rows
            .iter()
            .map(|r| {
                Ok(Activity {
                    at: r.try_get(0)?,
                    kind: r.try_get(1)?,
                    actor_id: r.try_get(2)?,
                    actor_name: r.try_get(3)?,
                    target_id: r.try_get(4)?,
                    target_name: r.try_get(5)?,
                    detail: r.try_get(6)?,
                    ip: r.try_get(7)?,
                })
            })
            .collect::<sqlx::Result<_>>()?;
        Ok((rows, total))
    }

    // Groups.

    /// The groups a user runs or plays in.
    pub async fn groups_of(&self, user: &str) -> ApiResult<Vec<GroupRow>> {
        let rows = sqlx::query(
            "SELECT g.id, g.name, d.username,
                (SELECT COUNT(*) FROM members m WHERE m.group_id = g.id),
                (SELECT m.character_id FROM members m WHERE m.group_id = g.id AND m.player_id = $1),
                g.dm_id
             FROM party_groups g JOIN users d ON d.id = g.dm_id
             WHERE g.dm_id = $1 OR EXISTS (SELECT 1 FROM members m WHERE m.group_id = g.id AND m.player_id = $1)
             ORDER BY lower(g.name)",
        )
        .bind(user)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(GroupRow {
                    id: r.try_get(0)?,
                    name: r.try_get(1)?,
                    dm_name: r.try_get(2)?,
                    member_count: r.try_get(3)?,
                    character_id: r.try_get(4)?,
                    dm_id: r.try_get(5)?,
                })
            })
            .collect()
    }

    /// Add a group.
    pub async fn insert_group(&self, id: &str, name: &str, dm: &str) -> ApiResult<()> {
        sqlx::query(
            "INSERT INTO party_groups (id, name, dm_id, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(name)
        .bind(dm)
        .bind(now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// A group's name and its DM's username.
    pub async fn group_header(&self, group: &str) -> ApiResult<Option<(String, String)>> {
        let row = sqlx::query(
            "SELECT g.name, d.username FROM party_groups g JOIN users d ON d.id = g.dm_id WHERE g.id = $1",
        )
        .bind(group)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row
            .map(|r| Ok::<_, sqlx::Error>((r.try_get(0)?, r.try_get(1)?)))
            .transpose()?)
    }

    /// A group's DM.
    pub async fn group_dm(&self, group: &str) -> ApiResult<Option<String>> {
        let row = sqlx::query("SELECT dm_id FROM party_groups WHERE id = $1")
            .bind(group)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.try_get(0)).transpose()?)
    }

    /// `None`: not a member. `Some(None)`: a member without a character.
    pub async fn membership(&self, group: &str, player: &str) -> ApiResult<Option<Option<String>>> {
        let row =
            sqlx::query("SELECT character_id FROM members WHERE group_id = $1 AND player_id = $2")
                .bind(group)
                .bind(player)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|r| r.try_get(0)).transpose()?)
    }

    /// Rename a group.
    pub async fn rename_group(&self, group: &str, name: &str) -> ApiResult<()> {
        sqlx::query("UPDATE party_groups SET name = $1 WHERE id = $2")
            .bind(name)
            .bind(group)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete a group with its invitations and memberships.
    pub async fn delete_group(&self, group: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM party_groups WHERE id = $1")
            .bind(group)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// A group's players, by username.
    pub async fn members(&self, group: &str) -> ApiResult<Vec<MemberRow>> {
        let rows = sqlx::query(
            "SELECT u.id, u.username, m.character_id FROM members m JOIN users u ON u.id = m.player_id
             WHERE m.group_id = $1 ORDER BY lower(u.username)",
        )
        .bind(group)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(MemberRow {
                    player: r.try_get(0)?,
                    username: r.try_get(1)?,
                    character: r.try_get(2)?,
                })
            })
            .collect()
    }

    /// A group's pending invitations: `(invite id, username)`.
    pub async fn group_invites(&self, group: &str) -> ApiResult<Vec<(String, String)>> {
        let rows = sqlx::query(
            "SELECT i.id, u.username FROM invites i JOIN users u ON u.id = i.player_id
             WHERE i.group_id = $1 ORDER BY lower(u.username)",
        )
        .bind(group)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| Ok((r.try_get(0)?, r.try_get(1)?)))
            .collect()
    }

    /// Invite a player by username.
    pub async fn invite(&self, id: &str, group: &str, username: &str) -> ApiResult<InviteOutcome> {
        let mut tx = self.pool.begin().await?;
        let player = sqlx::query(
            "SELECT id FROM users WHERE lower(username) = lower($1) AND status = 'active'",
        )
        .bind(username)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(player) = player else {
            return Ok(InviteOutcome::NoAccount);
        };
        let player: String = player.try_get(0)?;
        let runs = sqlx::query("SELECT 1 FROM party_groups WHERE id = $1 AND dm_id = $2")
            .bind(group)
            .bind(&player)
            .fetch_optional(&mut *tx)
            .await?;
        if runs.is_some() {
            return Ok(InviteOutcome::OwnGroup);
        }
        let member = sqlx::query("SELECT 1 FROM members WHERE group_id = $1 AND player_id = $2")
            .bind(group)
            .bind(&player)
            .fetch_optional(&mut *tx)
            .await?;
        if member.is_some() {
            return Ok(InviteOutcome::AlreadyMember);
        }
        let added = sqlx::query(
            "INSERT INTO invites (id, group_id, player_id, created_at) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(group)
        .bind(&player)
        .bind(now())
        .execute(&mut *tx)
        .await?;
        if added.rows_affected() == 0 {
            return Ok(InviteOutcome::AlreadyInvited);
        }
        tx.commit().await?;
        Ok(InviteOutcome::Sent)
    }

    /// Withdraw an invitation.
    pub async fn cancel_invite(&self, invite: &str, group: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM invites WHERE id = $1 AND group_id = $2")
            .bind(invite)
            .bind(group)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// A player's invitations: `(invite id, group name, DM's username)`.
    pub async fn invites_for(&self, player: &str) -> ApiResult<Vec<(String, String, String)>> {
        let rows = sqlx::query(
            "SELECT i.id, g.name, d.username FROM invites i
             JOIN party_groups g ON g.id = i.group_id JOIN users d ON d.id = g.dm_id
             WHERE i.player_id = $1 ORDER BY i.created_at",
        )
        .bind(player)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| Ok((r.try_get(0)?, r.try_get(1)?, r.try_get(2)?)))
            .collect()
    }

    /// Accept an invitation; the group joined, or `None` when there is no such invitation.
    pub async fn accept_invite(&self, invite: &str, player: &str) -> ApiResult<Option<String>> {
        let mut tx = self.pool.begin().await?;
        let group = sqlx::query("SELECT group_id FROM invites WHERE id = $1 AND player_id = $2")
            .bind(invite)
            .bind(player)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(group) = group else {
            return Ok(None);
        };
        let group: String = group.try_get(0)?;
        sqlx::query("DELETE FROM invites WHERE id = $1")
            .bind(invite)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO members (group_id, player_id, character_id, joined_at) VALUES ($1, $2, NULL, $3)
             ON CONFLICT DO NOTHING",
        )
        .bind(&group)
        .bind(player)
        .bind(now())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(group))
    }

    /// Refuse an invitation.
    pub async fn decline_invite(&self, invite: &str, player: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM invites WHERE id = $1 AND player_id = $2")
            .bind(invite)
            .bind(player)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Take a player out of a group.
    pub async fn delete_member(&self, group: &str, player: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM members WHERE group_id = $1 AND player_id = $2")
            .bind(group)
            .bind(player)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Set or clear a member's character.
    pub async fn set_member_character(
        &self,
        character: Option<&str>,
        group: &str,
        player: &str,
    ) -> ApiResult<()> {
        sqlx::query("UPDATE members SET character_id = $1 WHERE group_id = $2 AND player_id = $3")
            .bind(character.map(str::to_string))
            .bind(group)
            .bind(player)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The groups where a player has a character: `(group id, name, DM id)`.
    pub async fn character_groups(&self, player: &str) -> ApiResult<Vec<(String, String, String)>> {
        let rows = sqlx::query(
            "SELECT g.id, g.name, g.dm_id FROM members m JOIN party_groups g ON g.id = m.group_id
             WHERE m.player_id = $1 AND m.character_id IS NOT NULL",
        )
        .bind(player)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| Ok((r.try_get(0)?, r.try_get(1)?, r.try_get(2)?)))
            .collect()
    }

    // Account settings.

    /// An account's own settings (a JSON object), if it has saved any.
    pub async fn user_settings(&self, user: &str) -> ApiResult<Option<String>> {
        let row = sqlx::query("SELECT settings FROM user_settings WHERE user_id = $1")
            .bind(user)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.try_get(0)).transpose()?)
    }

    /// Store an account's own settings; with `replace` false, a row already there is kept.
    pub async fn set_user_settings(
        &self,
        user: &str,
        settings: &str,
        replace: bool,
    ) -> ApiResult<()> {
        let sql = if replace {
            "INSERT INTO user_settings (user_id, settings) VALUES ($1, $2)
             ON CONFLICT (user_id) DO UPDATE SET settings = excluded.settings"
        } else {
            "INSERT INTO user_settings (user_id, settings) VALUES ($1, $2) ON CONFLICT DO NOTHING"
        };
        sqlx::query(sql)
            .bind(user)
            .bind(settings)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
