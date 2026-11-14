//! "Recently deleted" (0.3.6): what an account deleted in the last 30 days, with a Restore and a "Delete forever".
//!
//! - **Parties and encounters** (their DM's): the delete moves the rows into one JSON snapshot in `deleted_items`
//!   (migration step 11) in the same transaction that removes them, so every other query keeps seeing only live
//!   rows. A party's snapshot holds its members, invites, encounters (with their creatures, effects, rules and
//!   precasts) and combats (with their log, journal, views and prompts); join codes people entered and pending
//!   character offers are not kept. An encounter's holds only that encounter. A restore puts the rows back: a member
//!   whose character joined another party meanwhile, or whose account is gone, stays out; a combat that was live
//!   comes back ended. An encounter comes back only into its party (restore the party first).
//! - **Characters**: as before, a deleted character's file is in the account's sync trash (`admin_trash.rs`); this
//!   lists those too and restores them the same way. "Delete forever" leaves its delete record as a sheetless
//!   tombstone until the sweep, so sync keeps telling devices it is deleted.
//!
//! Rows older than [`RETENTION_DAYS`] go in the sweep ([`sweep_loop`], and on every list); "Delete forever" removes
//! one at once (a party takes its deleted encounters with it).

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::{Path, State};
use axum::Json;
use borealis_bundle::{is_plain_key, trash_dir, TRASH_RETENTION_DAYS};
use borealis_lib::state::AppState as Backend;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::{AnyConnection, Row};
use tauri::Manager;

use crate::admin::trash::{deleted_in, name_of, now_ms, restore_now, TOMBSTONE};
use crate::auth::User;
use crate::db::{now, Db};
use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// How long a deleted party, encounter or character can be restored. The characters' trash has its own constant
/// (upstream's); both are 30.
pub const RETENTION_DAYS: i64 = 30;
const DAY_SECS: i64 = 24 * 60 * 60;
const _: () = assert!(RETENTION_DAYS as u64 == TRASH_RETENTION_DAYS);

/// How often the sweep runs.
const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(60 * 60);

pub const PARTY: &str = "party";
pub const ENCOUNTER: &str = "encounter";
const CHARACTER: &str = "character";

const IN_PARTY_ENCOUNTERS: &str = "encounter_id IN (SELECT id FROM encounters WHERE party_id = $1)";
const IN_PARTY_COMBATS: &str = "combat_id IN (SELECT id FROM combats WHERE party_id = $1)";

/// What a party's snapshot holds, parents first (the order rows go back in), with the condition on `$1` (the
/// party's id) that picks each table's rows.
const PARTY_TABLES: &[(&str, &str)] = &[
    ("parties", "id = $1"),
    ("party_invites", "party_id = $1"),
    ("party_members", "party_id = $1"),
    ("encounters", "party_id = $1"),
    ("encounter_creatures", IN_PARTY_ENCOUNTERS),
    ("encounter_effects", IN_PARTY_ENCOUNTERS),
    ("encounter_rules", IN_PARTY_ENCOUNTERS),
    ("encounter_casts", IN_PARTY_ENCOUNTERS),
    ("combats", "party_id = $1"),
    ("combatants", IN_PARTY_COMBATS),
    ("combat_journal", IN_PARTY_COMBATS),
    ("combat_log", IN_PARTY_COMBATS),
    ("combat_views", IN_PARTY_COMBATS),
    ("combat_prompts", IN_PARTY_COMBATS),
];

/// What an encounter's snapshot holds, with `$1` its id.
const ENCOUNTER_TABLES: &[(&str, &str)] = &[
    ("encounters", "id = $1"),
    ("encounter_creatures", "encounter_id = $1"),
    ("encounter_effects", "encounter_id = $1"),
    ("encounter_rules", "encounter_id = $1"),
    ("encounter_casts", "encounter_id = $1"),
];

/// Columns that name another account: a row whose account is gone stays out of a restore.
const ACCOUNT_COLUMNS: &[(&str, &str)] = &[
    ("party_invites", "created_by"),
    ("party_members", "owner_id"),
    ("combat_views", "viewer_id"),
];

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(format!("recently deleted: {e}"))
}

fn not_found() -> ApiError {
    ApiError::NotFound("That isn't in Recently deleted (any more).".into())
}

/// A table's columns now, each with whether it holds integers (every other column is text).
async fn columns(
    conn: &mut AnyConnection,
    sqlite: bool,
    table: &str,
) -> sqlx::Result<BTreeMap<String, bool>> {
    let rows = sqlx::query(if sqlite {
        "SELECT name, upper(type) LIKE '%INT%' FROM pragma_table_info($1)"
    } else {
        "SELECT column_name::text, data_type IN ('bigint', 'integer', 'smallint') FROM information_schema.columns
         WHERE table_schema = current_schema() AND table_name = $1"
    })
    .bind(table)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter()
        .map(|r| {
            // SQLite answers the comparison as an integer, PostgreSQL as a boolean.
            let int = r
                .try_get::<bool, _>(1)
                .or_else(|_| r.try_get::<i64, _>(1).map(|v| v != 0))?;
            Ok((r.try_get::<String, _>(0)?, int))
        })
        .collect()
}

/// The rows of `tables` for `id`, as `[{ table, rows: [{ column: value }] }]`.
async fn snapshot(
    conn: &mut AnyConnection,
    sqlite: bool,
    tables: &[(&str, &str)],
    id: &str,
) -> sqlx::Result<Value> {
    let mut out = Vec::new();
    for (table, filter) in tables {
        let cols = columns(conn, sqlite, table).await?;
        let rows = sqlx::query(&format!("SELECT * FROM {table} WHERE {filter}"))
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;
        let mut kept = Vec::new();
        for row in &rows {
            let mut obj = Map::new();
            for (name, int) in &cols {
                let value = if *int {
                    row.try_get::<Option<i64>, _>(name.as_str())?
                        .map_or(Value::Null, Value::from)
                } else {
                    row.try_get::<Option<String>, _>(name.as_str())?
                        .map_or(Value::Null, Value::from)
                };
                obj.insert(name.clone(), value);
            }
            kept.push(Value::Object(obj));
        }
        out.push(json!({ "table": table, "rows": kept }));
    }
    Ok(Value::Array(out))
}

/// Put a snapshot's rows back, in `tables`' order (the only tables it may write). A row that would clash with one
/// there now (a character in another party, an invite code taken) or whose account is gone stays out, and so does a
/// member whose character is no longer a live file (`live_character(owner, character)`): deleted meanwhile. A live
/// combat comes back ended, and an encounter that was live comes back ended unless a live combat still uses it (an
/// encounter deleted mid-fight and restored while its fight goes on).
async fn put_back(
    conn: &mut AnyConnection,
    sqlite: bool,
    tables: &[(&str, &str)],
    snapshot: &Value,
    live_character: &(dyn Fn(&str, &str) -> bool + Sync),
) -> ApiResult<()> {
    let at = now();
    for (table, _) in tables {
        let Some(rows) = snapshot
            .as_array()
            .and_then(|t| t.iter().find(|t| t["table"] == *table))
            .and_then(|t| t["rows"].as_array())
        else {
            continue;
        };
        let cols = columns(conn, sqlite, table).await?;
        for row in rows {
            let Some(obj) = row.as_object() else { continue };
            let mut obj = obj.clone();
            if let Some((_, column)) = ACCOUNT_COLUMNS.iter().find(|(t, _)| t == table) {
                let account = obj.get(*column).and_then(Value::as_str).unwrap_or_default();
                let there = sqlx::query("SELECT 1 FROM users WHERE id = $1")
                    .bind(account)
                    .fetch_optional(&mut *conn)
                    .await?;
                if there.is_none() {
                    continue;
                }
            }
            if *table == "party_members" {
                let owner = obj
                    .get("owner_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let character = obj
                    .get("character_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !live_character(owner, character) {
                    continue;
                }
            }
            if obj.get("status").and_then(Value::as_str) == Some("live") {
                match *table {
                    "combats" => {
                        obj.insert("status".into(), json!("ended"));
                        obj.insert("ended_at".into(), json!(at));
                    }
                    "encounters" => {
                        let id = obj.get("id").and_then(Value::as_str).unwrap_or_default();
                        let fighting = sqlx::query(
                            "SELECT 1 FROM combats WHERE encounter_id = $1 AND status = 'live'",
                        )
                        .bind(id)
                        .fetch_optional(&mut *conn)
                        .await?;
                        if fighting.is_none() {
                            obj.insert("status".into(), json!("ended"));
                        }
                    }
                    _ => {}
                }
            }
            // Only the columns the table has now: one added since gets its default, one dropped is left out.
            let names: Vec<&String> = obj.keys().filter(|k| cols.contains_key(*k)).collect();
            if names.is_empty() {
                continue;
            }
            let marks: Vec<String> = (1..=names.len()).map(|n| format!("${n}")).collect();
            let sql = format!(
                "INSERT INTO {table} ({}) VALUES ({}) ON CONFLICT DO NOTHING",
                names
                    .iter()
                    .map(|n| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                marks.join(", ")
            );
            let mut query = sqlx::query(&sql);
            for name in names {
                let value = &obj[name.as_str()];
                query = if cols[name] {
                    query.bind(value.as_i64())
                } else {
                    query.bind(value.as_str().map(str::to_string))
                };
            }
            query.execute(&mut *conn).await?;
        }
    }
    Ok(())
}

/// `FOR UPDATE` on PostgreSQL: it conflicts with the key-share lock every child row's insert takes on its parent, so
/// nothing is added under a parent between its snapshot and its cascade.
fn lock(sqlite: bool) -> &'static str {
    if sqlite {
        ""
    } else {
        " FOR UPDATE"
    }
}

/// On SQLite, make the transaction the writer before it reads: a no-op update takes the database's write lock, so
/// the snapshot and the delete see the same rows. (PostgreSQL locks rows instead, [`lock`].)
async fn hold(
    conn: &mut AnyConnection,
    sqlite: bool,
    table: &str,
    filter: &str,
    id: &str,
) -> sqlx::Result<()> {
    if sqlite {
        sqlx::query(&format!(
            "UPDATE {table} SET updated_at = updated_at WHERE {filter}"
        ))
        .bind(id)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Delete a party its DM runs, keeping it in Recently deleted. False when `dm` runs no such party (nothing done).
///
/// # Errors
/// When the database fails.
pub async fn delete_party(db: &Db, dm: &str, party: &str) -> ApiResult<bool> {
    let sqlite = db.kind() == "sqlite";
    let mut tx = db.pool().begin().await?;
    hold(&mut tx, sqlite, "parties", "id = $1", party).await?;
    let Some(row) = sqlx::query(&format!(
        "SELECT name FROM parties WHERE id = $1 AND dm_id = $2{}",
        lock(sqlite)
    ))
    .bind(party)
    .bind(dm)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(false);
    };
    let name: String = row.try_get(0)?;
    // The rows the other tables' children point at: no new log line, member or creature lands between the
    // snapshot and the cascade (it waits, then fails on the missing parent).
    if !sqlite {
        for table in ["encounters", "combats"] {
            sqlx::query(&format!(
                "SELECT id FROM {table} WHERE party_id = $1 FOR UPDATE"
            ))
            .bind(party)
            .fetch_all(&mut *tx)
            .await?;
        }
    }
    let snap = snapshot(&mut tx, sqlite, PARTY_TABLES, party).await?;
    keep(&mut tx, PARTY, party, dm, None, &name, "", &snap).await?;
    sqlx::query("DELETE FROM parties WHERE id = $1 AND dm_id = $2")
        .bind(party)
        .bind(dm)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Delete an encounter (its DM checked by the caller), keeping it in Recently deleted for `dm`.
///
/// # Errors
/// When the database fails.
pub async fn delete_encounter(db: &Db, dm: &str, encounter: &str) -> ApiResult<()> {
    let sqlite = db.kind() == "sqlite";
    let mut tx = db.pool().begin().await?;
    hold(&mut tx, sqlite, "encounters", "id = $1", encounter).await?;
    let Some(row) = sqlx::query(&format!(
        "SELECT e.name, e.party_id, p.name FROM encounters e JOIN parties p ON p.id = e.party_id WHERE e.id = $1{}",
        if sqlite { "" } else { " FOR UPDATE OF e" }
    ))
    .bind(encounter)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(());
    };
    let (name, party, party_name): (String, String, String) =
        (row.try_get(0)?, row.try_get(1)?, row.try_get(2)?);
    let snap = snapshot(&mut tx, sqlite, ENCOUNTER_TABLES, encounter).await?;
    keep(
        &mut tx,
        ENCOUNTER,
        encounter,
        dm,
        Some(&party),
        &name,
        &party_name,
        &snap,
    )
    .await?;
    sqlx::query("DELETE FROM encounters WHERE id = $1")
        .bind(encounter)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn keep(
    conn: &mut AnyConnection,
    kind: &str,
    id: &str,
    owner: &str,
    party: Option<&str>,
    name: &str,
    detail: &str,
    snap: &Value,
) -> sqlx::Result<()> {
    // A second delete of the same id (restored, then deleted again) replaces the older copy.
    sqlx::query("DELETE FROM deleted_items WHERE kind = $1 AND id = $2")
        .bind(kind)
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO deleted_items (kind, id, owner_id, party_id, name, detail, snapshot_json, deleted_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(kind)
    .bind(id)
    .bind(owner)
    .bind(party.map(str::to_string))
    .bind(name)
    .bind(detail)
    .bind(snap.to_string())
    .bind(now())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Remove what was deleted more than [`RETENTION_DAYS`] before `at` (seconds). How many rows went.
///
/// # Errors
/// When the database fails.
pub async fn sweep(db: &Db, at: i64) -> sqlx::Result<u64> {
    Ok(
        sqlx::query("DELETE FROM deleted_items WHERE deleted_at < $1")
            .bind(at - RETENTION_DAYS * DAY_SECS)
            .execute(db.pool())
            .await?
            .rows_affected(),
    )
}

/// The sweep, every hour while the server runs.
pub async fn sweep_loop(db: Db) {
    loop {
        match sweep(&db, now()).await {
            Ok(0) => {}
            Ok(n) => {
                tracing::info!("recently deleted: {n} removed for good after {RETENTION_DAYS} days")
            }
            Err(e) => tracing::warn!("recently deleted: sweep failed: {e:?}"),
        }
        tokio::time::sleep(SWEEP_EVERY).await;
    }
}

/// The routes.
pub fn routes() -> axum::Router<AppState> {
    use axum::routing::{delete, get, post};
    axum::Router::new()
        .route("/api/me/deleted", get(list))
        .route("/api/me/deleted/restore", post(restore))
        .route("/api/me/deleted/{kind}/{id}", delete(forget))
}

/// The caller's deleted characters (newest trash copy per character), from its own folder.
async fn my_characters(
    state: &AppState,
    user: &str,
) -> ApiResult<Vec<(String, u64, Option<String>)>> {
    let dir = state.workspaces.dir(user);
    tokio::task::spawn_blocking(move || {
        deleted_in(&trash_dir(&dir), &dir.join("characters"), now_ms())
            .into_iter()
            .map(|(id, (ms, path))| (id, ms, name_of(&path)))
            .collect()
    })
    .await
    .map_err(internal)
}

/// `GET /api/me/deleted`: `{ items: [{ kind, id, name, detail, deletedAtMs, purgeAtMs }], retentionDays }`, newest
/// first. `detail` is an encounter's party. Never a snapshot.
pub async fn list(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    sweep(&state.db, now()).await?;
    let day_ms = u64::try_from(DAY_SECS).unwrap_or(0) * 1000;
    let keep_ms = u64::try_from(RETENTION_DAYS).unwrap_or(0) * day_ms;
    let mut items = Vec::new();
    let rows = sqlx::query(
        "SELECT kind, id, name, detail, deleted_at FROM deleted_items WHERE owner_id = $1",
    )
    .bind(&user.id)
    .fetch_all(state.db.pool())
    .await?;
    for row in &rows {
        let at = u64::try_from(row.try_get::<i64, _>(4)?).unwrap_or(0) * 1000;
        items.push(json!({
            "kind": row.try_get::<String, _>(0)?,
            "id": row.try_get::<String, _>(1)?,
            "name": row.try_get::<String, _>(2)?,
            "detail": row.try_get::<String, _>(3)?,
            "deletedAtMs": at,
            "purgeAtMs": at + keep_ms,
        }));
    }
    for (id, ms, name) in my_characters(&state, &user.id).await? {
        items.push(json!({
            "kind": CHARACTER,
            "id": id,
            "name": name.unwrap_or_default(),
            "detail": "",
            "deletedAtMs": ms,
            "purgeAtMs": ms + keep_ms,
        }));
    }
    items.sort_by(|a, b| b["deletedAtMs"].as_u64().cmp(&a["deletedAtMs"].as_u64()));
    Ok(Json(
        json!({ "items": items, "retentionDays": RETENTION_DAYS }),
    ))
}

/// What to restore.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Item {
    kind: String,
    id: String,
}

/// One of the caller's rows: (name, party id, snapshot).
async fn mine(
    state: &AppState,
    user: &str,
    kind: &str,
    id: &str,
) -> ApiResult<(String, Option<String>, Value)> {
    let row = sqlx::query(
        "SELECT name, party_id, snapshot_json FROM deleted_items WHERE kind = $1 AND id = $2 AND owner_id = $3
         AND deleted_at >= $4",
    )
    .bind(kind)
    .bind(id)
    .bind(user)
    .bind(now() - RETENTION_DAYS * DAY_SECS)
    .fetch_optional(state.db.pool())
    .await?
    .ok_or_else(not_found)?;
    let snap: String = row.try_get(2)?;
    Ok((
        row.try_get(0)?,
        row.try_get(1)?,
        serde_json::from_str(&snap).map_err(internal)?,
    ))
}

/// `POST /api/me/deleted/restore` `{ kind, id }`: it is back where it was. `{ kind, id, name }`.
pub async fn restore(
    State(state): State<AppState>,
    user: User,
    Json(form): Json<Item>,
) -> ApiResult<Json<Value>> {
    let name = match form.kind.as_str() {
        PARTY => restore_party(&state, &user, &form.id).await?,
        ENCOUNTER => restore_encounter(&state, &user, &form.id).await?,
        CHARACTER => restore_character(&state, &user, &form.id).await?,
        _ => return Err(not_found()),
    };
    tracing::info!(
        "recently deleted: {} {} restored by {}",
        form.kind,
        form.id,
        user.id
    );
    Ok(Json(
        json!({ "kind": form.kind, "id": form.id, "name": name }),
    ))
}

/// Whether an account's character is a live file now (not deleted, nor deleted forever).
fn live_characters(state: &AppState) -> impl Fn(&str, &str) -> bool + Sync + '_ {
    move |owner: &str, character: &str| {
        is_plain_key(owner)
            && is_plain_key(character)
            && state
                .workspaces
                .dir(owner)
                .join("characters")
                .join(format!("{character}.json"))
                .is_file()
    }
}

async fn restore_party(state: &AppState, user: &User, id: &str) -> ApiResult<String> {
    let (name, _, snap) = mine(state, &user.id, PARTY, id).await?;
    crate::parties::may_restore(state, &user.id).await?;
    let sqlite = state.db.kind() == "sqlite";
    let mut tx = state.db.pool().begin().await?;
    let there = sqlx::query("SELECT 1 FROM parties WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if there.is_some() {
        return Err(ApiError::Conflict("That party is there already.".into()));
    }
    let live = live_characters(state);
    put_back(&mut tx, sqlite, PARTY_TABLES, &snap, &live).await?;
    sqlx::query("DELETE FROM deleted_items WHERE kind = $1 AND id = $2")
        .bind(PARTY)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    crate::parties::restored(state, &user.id, id).await;
    Ok(name)
}

async fn restore_encounter(state: &AppState, user: &User, id: &str) -> ApiResult<String> {
    let (name, party, snap) = mine(state, &user.id, ENCOUNTER, id).await?;
    let party = party.unwrap_or_default();
    let dm: Option<String> = sqlx::query("SELECT dm_id FROM parties WHERE id = $1")
        .bind(&party)
        .fetch_optional(state.db.pool())
        .await?
        .map(|r| r.try_get(0))
        .transpose()?;
    match dm {
        Some(dm) if dm == user.id => {}
        Some(_) => return Err(not_found()),
        None => {
            let waiting = sqlx::query(
                "SELECT name FROM deleted_items WHERE kind = $1 AND id = $2 AND owner_id = $3",
            )
            .bind(PARTY)
            .bind(&party)
            .bind(&user.id)
            .fetch_optional(state.db.pool())
            .await?;
            return Err(ApiError::Conflict(match waiting {
                Some(row) => format!(
                    "Its party \u{201c}{}\u{201d} is deleted too. Restore the party first.",
                    row.try_get::<String, _>(0)?
                ),
                None => "Its party was deleted for good, so the encounter can't come back.".into(),
            }));
        }
    }
    crate::encounters::may_restore(state, &party).await?;
    let sqlite = state.db.kind() == "sqlite";
    let mut tx = state.db.pool().begin().await?;
    let there = sqlx::query("SELECT 1 FROM encounters WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    if there.is_some() {
        return Err(ApiError::Conflict(
            "That encounter is there already.".into(),
        ));
    }
    let live = live_characters(state);
    put_back(&mut tx, sqlite, ENCOUNTER_TABLES, &snap, &live).await?;
    sqlx::query("DELETE FROM deleted_items WHERE kind = $1 AND id = $2")
        .bind(ENCOUNTER)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    crate::encounters::restored(state, &party, id).await;
    Ok(name)
}

async fn restore_character(state: &AppState, user: &User, id: &str) -> ApiResult<String> {
    if !is_plain_key(id) {
        return Err(not_found());
    }
    let workspace = state.workspaces.get(&user.id).await?;
    let name = {
        let _sync = workspace.sync_lock.lock().await;
        let id = id.to_string();
        workspace
            .blocking(move |app| restore_now(&app.state::<Backend>(), &id))
            .await?
    };
    workspace.emit(crate::sync::CHARACTERS_CHANGED, json!({ "ids": [id] }));
    Ok(name.unwrap_or_default())
}

/// `DELETE /api/me/deleted/{kind}/{id}`: "Delete forever". A party takes its deleted encounters with it; a character
/// loses every trash copy. `{ deleted: true }`.
pub async fn forget(
    State(state): State<AppState>,
    user: User,
    Path((kind, id)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    match kind.as_str() {
        PARTY | ENCOUNTER => {
            let mut tx = state.db.pool().begin().await?;
            let done = sqlx::query(
                "DELETE FROM deleted_items WHERE kind = $1 AND id = $2 AND owner_id = $3",
            )
            .bind(&kind)
            .bind(&id)
            .bind(&user.id)
            .execute(&mut *tx)
            .await?;
            if done.rows_affected() == 0 {
                return Err(not_found());
            }
            if kind == PARTY {
                sqlx::query(
                    "DELETE FROM deleted_items WHERE kind = $1 AND party_id = $2 AND owner_id = $3",
                )
                .bind(ENCOUNTER)
                .bind(&id)
                .bind(&user.id)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
        }
        CHARACTER => forget_character(&state, &user, &id).await?,
        _ => return Err(not_found()),
    }
    tracing::info!(
        "recently deleted: {kind} {id} deleted for good by {}",
        user.id
    );
    Ok(Json(json!({ "deleted": true })))
}

async fn forget_character(state: &AppState, user: &User, id: &str) -> ApiResult<()> {
    if !is_plain_key(id) {
        return Err(not_found());
    }
    let workspace = state.workspaces.get(&user.id).await?;
    let _sync = workspace.sync_lock.lock().await;
    let id = id.to_string();
    workspace
        .blocking(move |app| {
            let backend = app.state::<Backend>();
            let characters = crate::sync::characters_dir(&backend)?;
            let trash = trash_dir(&backend.paths.data);
            let Some((_, record)) = deleted_in(&trash, &characters, now_ms()).remove(&id) else {
                return Err(not_found());
            };
            // The newest delete record stays, by name, as a tombstone until the sweep: sync still reports the
            // character deleted, so a device that has it drops it rather than pushing it back. Its sheet goes.
            let temp = trash.join(format!(
                ".party-tombstone-{}.tmp",
                uuid::Uuid::new_v4().simple()
            ));
            if let Err(e) =
                std::fs::write(&temp, TOMBSTONE).and_then(|()| std::fs::rename(&temp, &record))
            {
                let _ = std::fs::remove_file(&temp);
                return Err(internal(e));
            }
            // Every other copy: older replaced versions and device copies.
            let suffixes: BTreeSet<String> =
                [format!("-{id}.json"), format!("-{id}.device.json")].into();
            for entry in std::fs::read_dir(&trash).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let ours = suffixes.iter().any(|s| {
                    name.strip_suffix(s.as_str())
                        .is_some_and(|ms| !ms.is_empty() && ms.bytes().all(|b| b.is_ascii_digit()))
                });
                if ours
                    && entry.path() != record
                    && entry.path().symlink_metadata().is_ok_and(|m| m.is_file())
                {
                    std::fs::remove_file(entry.path()).map_err(internal)?;
                }
            }
            Ok(())
        })
        .await
}
