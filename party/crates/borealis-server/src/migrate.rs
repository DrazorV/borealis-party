//! Moving an instance from SQLite to PostgreSQL.
//!
//! Only the database moves: characters and content are files in the data folder and stay where they are. The copy
//! runs in one transaction on the target, which must have no accounts yet, and the source is only read.

use std::path::Path;

use sqlx::Row;

use crate::db::{Db, Location, OpenError};

#[derive(Clone, Copy)]
enum Column {
    /// Text, possibly NULL.
    Text,
    /// A number, never NULL.
    Int,
    /// A number, possibly NULL.
    OptInt,
}

/// Every table, parents before children, with its columns.
const TABLES: &[(&str, &[(&str, Column)])] = &[
    (
        "users",
        &[
            ("id", Column::Text),
            ("username", Column::Text),
            ("password_hash", Column::Text),
            ("created_at", Column::Int),
            ("email", Column::Text),
            ("status", Column::Text),
            ("is_admin", Column::Int),
            ("must_change_password", Column::Int),
            ("email_verified_at", Column::OptInt),
            ("verify_token_hash", Column::Text),
            ("verify_expires_at", Column::OptInt),
        ],
    ),
    (
        "sessions",
        &[
            ("token_hash", Column::Text),
            ("user_id", Column::Text),
            ("expires_at", Column::Int),
        ],
    ),
    (
        "server_settings",
        &[("key", Column::Text), ("value", Column::Text)],
    ),
    (
        "activity",
        &[
            ("id", Column::Text),
            ("at", Column::Int),
            ("kind", Column::Text),
            ("actor_id", Column::Text),
            ("actor_name", Column::Text),
            ("target_id", Column::Text),
            ("target_name", Column::Text),
            ("detail", Column::Text),
            ("ip", Column::Text),
        ],
    ),
    (
        "shares",
        &[
            ("id", Column::Text),
            ("from_id", Column::Text),
            ("code_hash", Column::Text),
            ("to_id", Column::Text),
            ("to_name", Column::Text),
            ("file_name", Column::Text),
            ("characters", Column::Int),
            ("packs", Column::Int),
            ("links", Column::Text),
            ("bytes", Column::Int),
            ("created_at", Column::Int),
            ("expires_at", Column::Int),
            ("claimed_at", Column::OptInt),
            ("ended_at", Column::OptInt),
            ("status", Column::Text),
        ],
    ),
    (
        "share_blocks",
        &[
            ("user_id", Column::Text),
            ("blocked_id", Column::Text),
            ("created_at", Column::Int),
        ],
    ),
    // M-parties.
    (
        "account_roles",
        &[
            ("user_id", Column::Text),
            ("role", Column::Text),
            ("granted_at", Column::Int),
            ("granted_by", Column::Text),
        ],
    ),
    (
        "parties",
        &[
            ("id", Column::Text),
            ("name", Column::Text),
            ("dm_id", Column::Text),
            ("ruleset", Column::Text),
            ("created_at", Column::Int),
            ("updated_at", Column::Int),
        ],
    ),
    (
        "party_invites",
        &[
            ("id", Column::Text),
            ("party_id", Column::Text),
            ("invited_username_lower", Column::Text),
            ("code_hash", Column::Text),
            ("created_by", Column::Text),
            ("created_at", Column::Int),
            ("expires_at", Column::Int),
            ("uses_left", Column::Int),
            ("declined_at", Column::OptInt),
        ],
    ),
    (
        "party_invite_holds",
        &[
            ("user_id", Column::Text),
            ("invite_id", Column::Text),
            ("created_at", Column::Int),
        ],
    ),
    (
        "party_offers",
        &[
            ("id", Column::Text),
            ("invite_id", Column::Text),
            ("owner_id", Column::Text),
            ("character_id", Column::Text),
            ("created_at", Column::Int),
        ],
    ),
    (
        "party_members",
        &[
            ("owner_id", Column::Text),
            ("character_id", Column::Text),
            ("party_id", Column::Text),
            ("joined_at", Column::Int),
        ],
    ),
    (
        "monster_meta",
        &[
            ("owner_id", Column::Text),
            ("element_id", Column::Text),
            ("pack_id", Column::Text),
            ("pack_rev", Column::Text),
            ("environment_json", Column::Text),
            ("page", Column::OptInt),
            ("extras_json", Column::Text),
            ("captured_at", Column::Int),
        ],
    ),
    (
        "bestiary_imports",
        &[
            ("owner_id", Column::Text),
            ("pack_id", Column::Text),
            ("pack_rev", Column::Text),
            ("source", Column::Text),
            ("in_data", Column::Int),
            ("skipped_json", Column::Text),
            ("partial_json", Column::Text),
            ("captured_at", Column::Int),
        ],
    ),
];

/// Tests: every table the copy moves, with its columns as `text`, `int` (never NULL) or `optint`.
#[cfg(test)]
pub(crate) fn tables() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    TABLES
        .iter()
        .map(|(table, columns)| {
            (
                *table,
                columns
                    .iter()
                    .map(|(name, kind)| {
                        let kind = match kind {
                            Column::Text => "text",
                            Column::Int => "int",
                            Column::OptInt => "optint",
                        };
                        (*name, kind)
                    })
                    .collect(),
            )
        })
        .collect()
}

/// Rows copied per table.
pub type Report = Vec<(&'static str, i64)>;

/// Why nothing was copied.
#[derive(Debug)]
pub enum MigrateError {
    /// The target already has accounts.
    TargetNotEmpty,
    /// A database failed.
    Database(sqlx::Error),
    /// The SQLite file can't be opened.
    Source(OpenError),
    /// The copy doesn't match the source.
    Mismatch(&'static str, i64, i64),
}

impl std::fmt::Display for MigrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TargetNotEmpty => f.write_str("the PostgreSQL database already has accounts"),
            Self::Database(e) => write!(f, "database: {e}"),
            Self::Source(e) => write!(f, "the SQLite file: {e}"),
            Self::Mismatch(table, want, got) => {
                write!(f, "{table}: copied {got} rows of {want}; nothing was saved")
            }
        }
    }
}

impl std::error::Error for MigrateError {}

impl From<sqlx::Error> for MigrateError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

async fn count(db: &Db, table: &str) -> sqlx::Result<i64> {
    sqlx::query(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(db.pool())
        .await?
        .try_get(0)
}

/// True when `db` has no accounts.
///
/// # Errors
/// When the database can't be read.
pub async fn is_empty(db: &Db) -> sqlx::Result<bool> {
    Ok(count(db, "users").await? == 0)
}

/// Copy everything from `source` into `target`, all or nothing.
///
/// # Errors
/// When the target isn't empty, a database fails, or the copy comes out different.
pub async fn copy(source: &Db, target: &Db) -> Result<Report, MigrateError> {
    if !is_empty(target).await? {
        return Err(MigrateError::TargetNotEmpty);
    }
    let mut tx = target.pool().begin().await?;
    let mut report = Report::new();
    for (table, columns) in TABLES {
        let names: Vec<&str> = columns.iter().map(|(name, _)| *name).collect();
        let list = names.join(", ");
        let rows = sqlx::query(&format!("SELECT {list} FROM {table} ORDER BY {}", names[0]))
            .fetch_all(source.pool())
            .await?;
        let marks: Vec<String> = (1..=columns.len()).map(|i| format!("${i}")).collect();
        let insert = format!("INSERT INTO {table} ({list}) VALUES ({})", marks.join(", "));
        for row in &rows {
            let mut query = sqlx::query(&insert);
            for (index, (_, kind)) in columns.iter().enumerate() {
                query = match kind {
                    Column::Text => query.bind(row.try_get::<Option<String>, _>(index)?),
                    Column::Int => query.bind(row.try_get::<i64, _>(index)?),
                    Column::OptInt => query.bind(row.try_get::<Option<i64>, _>(index)?),
                };
            }
            query.execute(&mut *tx).await?;
        }
        let want = i64::try_from(rows.len()).unwrap_or(i64::MAX);
        let got: i64 = sqlx::query(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&mut *tx)
            .await?
            .try_get(0)?;
        if got != want {
            return Err(MigrateError::Mismatch(table, want, got));
        }
        report.push((table, want));
    }
    tx.commit().await?;
    Ok(report)
}

/// On a PostgreSQL instance with no accounts yet, bring in the SQLite file it ran on before, if there is one.
///
/// Returns what was copied, or `None` when there was nothing to do.
///
/// # Errors
/// When the copy fails; nothing is saved then.
pub async fn adopt_sqlite(target: &Db, sqlite: &Path) -> Result<Option<Report>, MigrateError> {
    if !sqlite.is_file() {
        return Ok(None);
    }
    if !is_empty(target).await? {
        tracing::info!(
            "{} is still here, but PostgreSQL already has accounts: not copying it again",
            sqlite.display()
        );
        return Ok(None);
    }
    let source = Db::open(&Location::Sqlite(sqlite.to_path_buf()))
        .await
        .map_err(MigrateError::Source)?;
    if is_empty(&source).await? {
        return Ok(None);
    }
    tracing::info!("copying {} into PostgreSQL", sqlite.display());
    let report = copy(&source, target).await?;
    source.pool().close().await;
    Ok(Some(report))
}

/// `users 3, sessions 4, ...`
#[must_use]
pub fn describe(report: &Report) -> String {
    report
        .iter()
        .map(|(table, rows)| format!("{table} {rows}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Each table in [`TABLES`] with its columns, and whether a column is a number that is never NULL (schema test).
#[cfg(test)]
pub fn table_columns() -> Vec<(&'static str, Vec<(&'static str, bool)>)> {
    TABLES
        .iter()
        .map(|(table, columns)| {
            (
                *table,
                columns
                    .iter()
                    .map(|(name, kind)| (*name, matches!(kind, Column::Int)))
                    .collect(),
            )
        })
        .collect()
}
