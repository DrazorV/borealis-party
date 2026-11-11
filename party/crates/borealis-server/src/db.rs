//! Who is who and who belongs where, in SQLite or PostgreSQL. Character and content files stay in the Borealis
//! workspaces on disk.
//!
//! Every query is written once in SQL that both databases accept: `$n` placeholders in ascending order of first use,
//! `lower(...)` instead of collations, `ON CONFLICT DO NOTHING`, and no booleans read back from SQL.

use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::any::{AnyPoolOptions, AnyRow};
use sqlx::{AnyPool, Row};

use crate::auth::{Role, User};
use crate::error::{ApiError, ApiResult};

/// Applied in order; each runs once. Never edit a step that has shipped, add a new one.
const MIGRATIONS: &[&[&str]] = &[&[
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
]];

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
    /// No such username.
    NoAccount,
    /// The account is a DM.
    NotAPlayer,
    /// Already in the group.
    AlreadyMember,
    /// Already invited.
    AlreadyInvited,
}

/// The database.
#[derive(Clone)]
pub struct Db(AnyPool);

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
        role: Role::parse(&row.try_get::<String, _>(2)?),
    })
}

impl Db {
    /// The connection pool, for the migration between databases.
    pub(crate) fn pool(&self) -> &AnyPool {
        &self.0
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
        let db = Self(pool);
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> sqlx::Result<()> {
        sqlx::query("CREATE TABLE IF NOT EXISTS schema_version (version BIGINT NOT NULL)")
            .execute(&self.0)
            .await?;
        let current: i64 = sqlx::query("SELECT COALESCE(MAX(version), 0) FROM schema_version")
            .fetch_one(&self.0)
            .await?
            .try_get(0)?;
        for (index, steps) in MIGRATIONS.iter().enumerate() {
            let version = i64::try_from(index).unwrap_or(i64::MAX) + 1;
            if version <= current {
                continue;
            }
            let mut tx = self.0.begin().await?;
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

    /// Add an account; `false` when the username is taken.
    pub async fn insert_user(&self, user: &User, password_hash: &str) -> ApiResult<bool> {
        let done = sqlx::query(
            "INSERT INTO users (id, username, password_hash, role, created_at) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT DO NOTHING",
        )
        .bind(&user.id)
        .bind(&user.username)
        .bind(password_hash)
        .bind(user.role.as_str())
        .bind(now())
        .execute(&self.0)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// How many accounts exist.
    pub async fn user_count(&self) -> ApiResult<i64> {
        Ok(sqlx::query("SELECT COUNT(*) FROM users")
            .fetch_one(&self.0)
            .await?
            .try_get(0)?)
    }

    /// An account and its password hash, by username (any case).
    pub async fn find_login(&self, username: &str) -> ApiResult<Option<(User, String)>> {
        let row = sqlx::query(
            "SELECT id, username, role, password_hash FROM users WHERE lower(username) = lower($1)",
        )
        .bind(username)
        .fetch_optional(&self.0)
        .await?;
        row.map(|r| Ok((user_from(&r)?, r.try_get(3)?))).transpose()
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
            .execute(&self.0)
            .await?;
        sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, $3)")
            .bind(token_hash)
            .bind(user)
            .bind(expires_at)
            .execute(&self.0)
            .await?;
        Ok(())
    }

    /// The account behind a live session.
    pub async fn session_user(&self, token_hash: &str) -> ApiResult<Option<User>> {
        let row = sqlx::query(
            "SELECT u.id, u.username, u.role FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = $1 AND s.expires_at > $2",
        )
        .bind(token_hash)
        .bind(now())
        .fetch_optional(&self.0)
        .await?;
        Ok(row.map(|r| user_from(&r)).transpose()?)
    }

    /// Sign a session out.
    pub async fn delete_session(&self, token_hash: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&self.0)
            .await?;
        Ok(())
    }

    // Groups.

    /// The groups a user runs or plays in.
    pub async fn groups_of(&self, user: &str) -> ApiResult<Vec<GroupRow>> {
        let rows = sqlx::query(
            "SELECT g.id, g.name, d.username,
                (SELECT COUNT(*) FROM members m WHERE m.group_id = g.id),
                (SELECT m.character_id FROM members m WHERE m.group_id = g.id AND m.player_id = $1)
             FROM party_groups g JOIN users d ON d.id = g.dm_id
             WHERE g.dm_id = $1 OR EXISTS (SELECT 1 FROM members m WHERE m.group_id = g.id AND m.player_id = $1)
             ORDER BY lower(g.name)",
        )
        .bind(user)
        .fetch_all(&self.0)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(GroupRow {
                    id: r.try_get(0)?,
                    name: r.try_get(1)?,
                    dm_name: r.try_get(2)?,
                    member_count: r.try_get(3)?,
                    character_id: r.try_get(4)?,
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
        .execute(&self.0)
        .await?;
        Ok(())
    }

    /// A group's name and its DM's username.
    pub async fn group_header(&self, group: &str) -> ApiResult<Option<(String, String)>> {
        let row = sqlx::query(
            "SELECT g.name, d.username FROM party_groups g JOIN users d ON d.id = g.dm_id WHERE g.id = $1",
        )
        .bind(group)
        .fetch_optional(&self.0)
        .await?;
        Ok(row
            .map(|r| Ok::<_, sqlx::Error>((r.try_get(0)?, r.try_get(1)?)))
            .transpose()?)
    }

    /// A group's DM.
    pub async fn group_dm(&self, group: &str) -> ApiResult<Option<String>> {
        let row = sqlx::query("SELECT dm_id FROM party_groups WHERE id = $1")
            .bind(group)
            .fetch_optional(&self.0)
            .await?;
        Ok(row.map(|r| r.try_get(0)).transpose()?)
    }

    /// `None`: not a member. `Some(None)`: a member without a character.
    pub async fn membership(&self, group: &str, player: &str) -> ApiResult<Option<Option<String>>> {
        let row =
            sqlx::query("SELECT character_id FROM members WHERE group_id = $1 AND player_id = $2")
                .bind(group)
                .bind(player)
                .fetch_optional(&self.0)
                .await?;
        Ok(row.map(|r| r.try_get(0)).transpose()?)
    }

    /// Rename a group.
    pub async fn rename_group(&self, group: &str, name: &str) -> ApiResult<()> {
        sqlx::query("UPDATE party_groups SET name = $1 WHERE id = $2")
            .bind(name)
            .bind(group)
            .execute(&self.0)
            .await?;
        Ok(())
    }

    /// Delete a group with its invitations and memberships.
    pub async fn delete_group(&self, group: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM party_groups WHERE id = $1")
            .bind(group)
            .execute(&self.0)
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
        .fetch_all(&self.0)
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
        .fetch_all(&self.0)
        .await?;
        rows.iter()
            .map(|r| Ok((r.try_get(0)?, r.try_get(1)?)))
            .collect()
    }

    /// Invite a player by username.
    pub async fn invite(&self, id: &str, group: &str, username: &str) -> ApiResult<InviteOutcome> {
        let mut tx = self.0.begin().await?;
        let player = sqlx::query("SELECT id, role FROM users WHERE lower(username) = lower($1)")
            .bind(username)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(player) = player else {
            return Ok(InviteOutcome::NoAccount);
        };
        let (player, role): (String, String) = (player.try_get(0)?, player.try_get(1)?);
        if role != "player" {
            return Ok(InviteOutcome::NotAPlayer);
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
            .execute(&self.0)
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
        .fetch_all(&self.0)
        .await?;
        rows.iter()
            .map(|r| Ok((r.try_get(0)?, r.try_get(1)?, r.try_get(2)?)))
            .collect()
    }

    /// Accept an invitation; the group joined, or `None` when there is no such invitation.
    pub async fn accept_invite(&self, invite: &str, player: &str) -> ApiResult<Option<String>> {
        let mut tx = self.0.begin().await?;
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
            .execute(&self.0)
            .await?;
        Ok(())
    }

    /// Take a player out of a group.
    pub async fn delete_member(&self, group: &str, player: &str) -> ApiResult<()> {
        sqlx::query("DELETE FROM members WHERE group_id = $1 AND player_id = $2")
            .bind(group)
            .bind(player)
            .execute(&self.0)
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
            .execute(&self.0)
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
        .fetch_all(&self.0)
        .await?;
        rows.iter()
            .map(|r| Ok((r.try_get(0)?, r.try_get(1)?, r.try_get(2)?)))
            .collect()
    }
}
