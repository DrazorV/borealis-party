//! Moving an instance from SQLite to PostgreSQL.
//!
//! Only the database moves: characters and content are files in the data folder and stay where they are. The copy
//! runs in one transaction on the target, which must have no accounts or groups yet, and the source is only read.

use std::path::Path;

use sqlx::Row;

use crate::db::{Db, Location};

#[derive(Clone, Copy)]
enum Column {
    Text,
    Int,
}

/// Every table, parents before children, with its columns.
const TABLES: &[(&str, &[(&str, Column)])] = &[
    (
        "users",
        &[
            ("id", Column::Text),
            ("username", Column::Text),
            ("password_hash", Column::Text),
            ("role", Column::Text),
            ("created_at", Column::Int),
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
        "party_groups",
        &[
            ("id", Column::Text),
            ("name", Column::Text),
            ("dm_id", Column::Text),
            ("created_at", Column::Int),
        ],
    ),
    (
        "invites",
        &[
            ("id", Column::Text),
            ("group_id", Column::Text),
            ("player_id", Column::Text),
            ("created_at", Column::Int),
        ],
    ),
    (
        "members",
        &[
            ("group_id", Column::Text),
            ("player_id", Column::Text),
            ("character_id", Column::Text),
            ("joined_at", Column::Int),
        ],
    ),
];

/// Rows copied per table.
pub type Report = Vec<(&'static str, i64)>;

/// Why nothing was copied.
#[derive(Debug)]
pub enum MigrateError {
    /// The target already has accounts or groups.
    TargetNotEmpty,
    /// A database failed.
    Database(sqlx::Error),
    /// The copy doesn't match the source.
    Mismatch(&'static str, i64, i64),
}

impl std::fmt::Display for MigrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TargetNotEmpty => {
                f.write_str("the PostgreSQL database already has accounts or groups")
            }
            Self::Database(e) => write!(f, "database: {e}"),
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

/// True when `db` has no accounts and no groups.
///
/// # Errors
/// When the database can't be read.
pub async fn is_empty(db: &Db) -> sqlx::Result<bool> {
    Ok(count(db, "users").await? == 0 && count(db, "party_groups").await? == 0)
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
    let source = Db::open(&Location::Sqlite(sqlite.to_path_buf())).await?;
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
