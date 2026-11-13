//! Live combats: the server around the pure engine in `party-combat`. The API is `party/docs/combat-api.md`.
//!
//! * **State.** A combat is one row in `combats`: the engine's [`CombatState`] and die as JSON, and `seq`, the
//!   compare-and-set counter every write moves inside one transaction (`UPDATE ... WHERE seq = <the seq read>`).
//!   `combatants` says who each combatant id is (a creature with the stat block it fights with, or a PC by owner and
//!   character); `combat_journal` holds the undo steps and `combat_log` the DM-only log.
//! * **Who.** The DM is `parties.dm_id` and a player owns a `party_members` row of the party, both read in the
//!   write's own transaction. Players act only on their own PC; anything else they try is the one generic 404.
//! * **What players see** is built by [`view_for`] only: every player answer and every player event comes from it.
//!   Hidden creatures are absent from it, and each player's projection is hashed after every write, so a player's
//!   own `pseq` (and their event) moves only when what that player sees changed.
//! * **Retries.** Every write carries a client request id; the same id twice does the write once (D44).
//!
//! Nothing here goes to the admin page's activity log (D40); the server log gets ids and kinds only.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use party_combat::combat::{Author, JournalEntry};
use party_combat::party_dice::{self, DieRoller, SeededDie};
use party_combat::visibility::{is_visible, player_view_of};
use party_combat::{
    apply, derive, export_json, import_json, turn_pointer, visible_order, Actor, Boundary, Combat,
    CombatError, CombatState, Combatant, CombatantId, Command, Controller, DamageTaken, DmEvent,
    Duration, Edition, EffectSpec, EncounterRule, Hp, Life, Modifier, PlayerEvent, RuleFlag,
    RuleTarget, ViewerKind, UNDO_LIMIT,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::{AnyConnection, Row};

use crate::auth::User;
use crate::db::{now, Db};
use crate::error::ApiError;
use crate::hub::{Card, CardCache, COMBAT_EVENT};
use crate::AppState;
use party_encounter::combatant::{creature_of, fighter_from_snapshot, legendary_pool, Fighter};

/// Writes one account may make a minute, over all its combats.
const WRITES_PER_MINUTE: usize = 240;
/// Blocking jobs (start, import, export) at a time: per account, and on the server.
const JOBS_PER_ACCOUNT: usize = 2;
const JOBS_TOTAL: usize = 16;
/// Request ids are kept this long, in seconds.
const REQUEST_KEPT: i64 = 24 * 60 * 60;
/// Compare-and-set attempts before a write gives up.
const ATTEMPTS: usize = 8;
/// Largest import, in bytes (the engine refuses larger too).
const MAX_IMPORT: usize = 8 * 1024 * 1024;
/// Copies one `add_creature` may add.
const MAX_COPIES: u32 = 10;
const DAY: i64 = 24 * 60 * 60;
/// Roll results a player's projection shows per combatant.
const ROLLS_KEPT: usize = 10;

/// The one answer for anything a player or an outsider may not see or do, and for anything that doesn't exist.
fn not_found() -> ApiError {
    ApiError::NotFound("There is no such combat here.".into())
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

/// Why a combat request failed.
#[derive(Debug)]
enum Fail {
    /// The server's usual errors.
    Api(ApiError),
    /// `expectSeq` was not the current `seq` (DM only): 409 with the current one.
    Stale(i64),
    /// The engine refused a DM's command: 422 with its reason.
    Refused(String),
    /// A refusal about a player's own PC, with a fixed message.
    Player(StatusCode, &'static str),
    /// A refusal with its own code, for the client to act on.
    Coded(StatusCode, &'static str, String),
    /// The write raced another one: try again.
    Retry,
}

impl From<ApiError> for Fail {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

impl From<sqlx::Error> for Fail {
    fn from(e: sqlx::Error) -> Self {
        if retryable(&e) {
            Self::Retry
        } else {
            Self::Api(e.into())
        }
    }
}

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        match self {
            Self::Api(e) => e.into_response(),
            Self::Stale(seq) => (
                StatusCode::CONFLICT,
                Json(json!({
                    "code": "stale",
                    "message": "The combat changed. Refresh and try again.",
                    "seq": seq,
                })),
            )
                .into_response(),
            Self::Refused(message) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "code": "refused", "message": message })),
            )
                .into_response(),
            Self::Player(status, message) => {
                let code = if status == StatusCode::BAD_REQUEST {
                    "invalid_argument"
                } else {
                    "conflict"
                };
                (status, Json(json!({ "code": code, "message": message }))).into_response()
            }
            Self::Coded(status, code, message) => {
                (status, Json(json!({ "code": code, "message": message }))).into_response()
            }
            Self::Retry => {
                ApiError::Conflict("The combat is busy. Try again.".into()).into_response()
            }
        }
    }
}

type Res<T> = Result<T, Fail>;

/// A lost race in SQLite (a lock it couldn't take) or PostgreSQL (a serialization failure): the write runs again.
fn retryable(e: &sqlx::Error) -> bool {
    let text = e.to_string().to_lowercase();
    text.contains("database is locked")
        || text.contains("busy")
        || text.contains("could not serialize")
}

/// The routes, unless the server config switched parties off (then they are unknown routes: 404).
pub fn routes() -> axum::Router<AppState> {
    routes_if(crate::config::features().parties)
}

/// The combat routes when `on`; none otherwise.
pub(crate) fn routes_if(on: bool) -> axum::Router<AppState> {
    use axum::routing::{get, post};
    if !on {
        return axum::Router::new();
    }
    axum::Router::new()
        .route("/api/encounters/{encounter}/start", post(start))
        .route("/api/parties/{party}/combat", get(party_combat))
        .route("/api/parties/{party}/combats/import", post(import))
        .route("/api/combats/{combat}", get(read))
        .route("/api/combats/{combat}/log", get(log))
        .route("/api/combats/{combat}/{action}", post(act))
}

// Process-wide state: which parties are in a combat, limits, and one writer per combat at a time.

static LIVE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);
static WINDOWS: LazyLock<Mutex<HashMap<String, VecDeque<i64>>>> = LazyLock::new(Mutex::default);
static JOBS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(JOBS_TOTAL)));
static ACCOUNT_JOBS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>> =
    LazyLock::new(Mutex::default);
static LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Mutex::default);

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether a party has a live combat: its other members then see each other's health band (D6).
pub(crate) fn party_in_combat(party: &str) -> bool {
    guard(&LIVE).contains(party)
}

fn set_live(party: &str, live: bool) {
    let mut set = guard(&LIVE);
    if live {
        set.insert(party.to_string());
    } else {
        set.remove(party);
    }
}

/// At start-up: learn which parties are in a combat, then delete old logs once an hour (D32 a).
pub async fn startup(db: Db) {
    match sqlx::query("SELECT party_id FROM combats WHERE status = 'live'")
        .fetch_all(db.pool())
        .await
    {
        Ok(rows) => {
            for row in rows {
                if let Ok(party) = row.try_get::<String, _>(0) {
                    set_live(&party, true);
                }
            }
        }
        Err(e) => tracing::warn!("combat: reading the live combats: {e}"),
    }
    let mut every = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
    loop {
        every.tick().await;
        if let Err(e) = sweep(&db, now()).await {
            tracing::warn!("combat: deleting old logs: {e}");
        }
    }
}

/// Delete the log, undo journal and prompts of combats that ended more than 90 days before `at`, and request ids
/// older than a day.
pub(crate) async fn sweep(db: &Db, at: i64) -> sqlx::Result<()> {
    let cutoff = at - i64::from(party_combat::LOG_RETENTION_DAYS) * DAY;
    for table in ["combat_log", "combat_journal", "combat_prompts"] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE combat_id IN
                (SELECT id FROM combats WHERE status = 'ended' AND ended_at IS NOT NULL AND ended_at < $1)"
        ))
        .bind(cutoff)
        .execute(db.pool())
        .await?;
    }
    sqlx::query("DELETE FROM combat_requests WHERE created_at < $1")
        .bind(at - REQUEST_KEPT)
        .execute(db.pool())
        .await?;
    Ok(())
}

/// Count a write of `account`'s; false (and not counted) when it made [`WRITES_PER_MINUTE`] in the last minute.
fn take_write(account: &str) -> bool {
    let at = now();
    let mut map = guard(&WINDOWS);
    map.retain(|_, times| {
        while times.front().is_some_and(|t| *t <= at - 60) {
            times.pop_front();
        }
        !times.is_empty()
    });
    let times = map.entry(account.to_string()).or_default();
    if times.len() >= WRITES_PER_MINUTE {
        return false;
    }
    times.push_back(at);
    true
}

/// A slot for blocking work: at most [`JOBS_PER_ACCOUNT`] per account (else 429) and [`JOBS_TOTAL`] on the server.
async fn job(
    account: &str,
) -> Res<(
    tokio::sync::OwnedSemaphorePermit,
    tokio::sync::OwnedSemaphorePermit,
)> {
    let mine = Arc::clone(
        guard(&ACCOUNT_JOBS)
            .entry(account.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(JOBS_PER_ACCOUNT))),
    );
    let mine = mine.try_acquire_owned().map_err(|_| ApiError::TooMany)?;
    let all = Arc::clone(&JOBS).acquire_owned().await.map_err(internal)?;
    Ok((mine, all))
}

async fn combat_lock(id: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = Arc::clone(guard(&LOCKS).entry(id.to_string()).or_default());
    lock.lock_owned().await
}

// Checks of what clients send.

/// The server's ids are UUIDs; anything else is the same 404 as an id nobody has.
/// A job slot for the encounter simulation (`simulate.rs`): the same per-account and server-wide limits as combat
/// work. `None` when the account already runs its share.
pub(crate) async fn job_slot(
    account: &str,
) -> Option<(
    tokio::sync::OwnedSemaphorePermit,
    tokio::sync::OwnedSemaphorePermit,
)> {
    job(account).await.ok()
}

fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn request_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn seed() -> u32 {
    rand::random()
}

// Reading a combat.

/// How the caller relates to a party.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// Its DM.
    Dm,
    /// Owns a member character.
    Player,
}

async fn role(conn: &mut AnyConnection, user: &str, party: &str) -> sqlx::Result<Option<Role>> {
    let Some(row) = sqlx::query("SELECT dm_id FROM parties WHERE id = $1")
        .bind(party)
        .fetch_optional(&mut *conn)
        .await?
    else {
        return Ok(None);
    };
    if row.try_get::<String, _>(0)? == user {
        return Ok(Some(Role::Dm));
    }
    let member = sqlx::query("SELECT 1 FROM party_members WHERE party_id = $1 AND owner_id = $2")
        .bind(party)
        .bind(user)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(member.map(|_| Role::Player))
}

/// The caller's characters that are members of the party now.
async fn my_characters(
    conn: &mut AnyConnection,
    user: &str,
    party: &str,
) -> sqlx::Result<BTreeSet<String>> {
    let rows =
        sqlx::query("SELECT character_id FROM party_members WHERE party_id = $1 AND owner_id = $2")
            .bind(party)
            .bind(user)
            .fetch_all(&mut *conn)
            .await?;
    rows.iter().map(|r| r.try_get(0)).collect()
}

/// A combat row.
#[derive(Debug, Clone)]
pub(crate) struct Loaded {
    pub id: String,
    pub party: String,
    pub encounter: Option<String>,
    /// `live` or `ended`.
    pub status: String,
    pub seq: i64,
    pub state: CombatState,
    pub die: SeededDie,
    pub started_at: i64,
    pub ended_at: Option<i64>,
}

async fn load(conn: &mut AnyConnection, id: &str) -> Res<Option<Loaded>> {
    let Some(row) = sqlx::query(
        "SELECT id, party_id, encounter_id, status, seq, state_json, die_json, started_at, ended_at
         FROM combats WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let state: String = row.try_get(5)?;
    let die: String = row.try_get(6)?;
    Ok(Some(Loaded {
        id: row.try_get(0)?,
        party: row.try_get(1)?,
        encounter: row.try_get(2)?,
        status: row.try_get(3)?,
        seq: row.try_get(4)?,
        state: serde_json::from_str(&state).map_err(internal)?,
        die: serde_json::from_str(&die).map_err(internal)?,
        started_at: row.try_get(7)?,
        ended_at: row.try_get(8)?,
    }))
}

/// Who a combatant id is.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Member {
    /// `creature` or `character`.
    pub kind: String,
    pub encounter_creature: Option<String>,
    pub owner: Option<String>,
    pub character: Option<String>,
    /// The stat block a creature fights with (overrides applied).
    pub block: Option<Value>,
    /// The element's name (a creature's; DM only).
    pub name: Option<String>,
    /// Where a creature came from: `encounter`, `copy`, `element`, `companion:<key>` or `summon`.
    pub origin: Option<String>,
}

async fn roster(conn: &mut AnyConnection, combat: &str) -> Res<BTreeMap<String, Member>> {
    let rows = sqlx::query(
        "SELECT id, kind, encounter_creature_id, owner_id, character_id, statblock_json, name, origin
         FROM combatants WHERE combat_id = $1",
    )
    .bind(combat)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = BTreeMap::new();
    for row in rows {
        let block: Option<String> = row.try_get(5)?;
        out.insert(
            row.try_get(0)?,
            Member {
                kind: row.try_get(1)?,
                encounter_creature: row.try_get(2)?,
                owner: row.try_get(3)?,
                character: row.try_get(4)?,
                block: block
                    .map(|b| serde_json::from_str(&b))
                    .transpose()
                    .map_err(internal)?,
                name: row.try_get(6)?,
                origin: row.try_get(7)?,
            },
        );
    }
    Ok(out)
}

async fn insert_member(
    conn: &mut AnyConnection,
    combat: &str,
    id: &str,
    m: &Member,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO combatants (combat_id, id, kind, encounter_creature_id, owner_id, character_id, statblock_json,
            name, origin, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(combat)
    .bind(id)
    .bind(&m.kind)
    .bind(m.encounter_creature.clone())
    .bind(m.owner.clone())
    .bind(m.character.clone())
    .bind(m.block.as_ref().map(Value::to_string))
    .bind(m.name.clone())
    .bind(m.origin.clone())
    .bind(now())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// An open damage or heal prompt for a PC's owner (D7 a).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Prompt {
    pub id: String,
    pub combatant: String,
    pub owner: String,
    /// `damage` or `heal`.
    pub kind: String,
    /// `{ amount, parts, criticalHit, seq }`.
    pub payload: Value,
    pub at: i64,
}

async fn open_prompts(conn: &mut AnyConnection, combat: &str) -> Res<Vec<Prompt>> {
    let rows = sqlx::query(
        "SELECT id, combatant_id, owner_id, kind, payload_json, created_at FROM combat_prompts
         WHERE combat_id = $1 AND resolved_at IS NULL ORDER BY created_at, id",
    )
    .bind(combat)
    .fetch_all(&mut *conn)
    .await?;
    rows.iter()
        .map(|row| {
            let payload: String = row.try_get(4)?;
            Ok(Prompt {
                id: row.try_get(0)?,
                combatant: row.try_get(1)?,
                owner: row.try_get(2)?,
                kind: row.try_get(3)?,
                payload: serde_json::from_str(&payload).map_err(internal)?,
                at: row.try_get(5)?,
            })
        })
        .collect()
}

/// Each PC's sheet companions that aren't in the fight yet (from the roster cards, which the owner's play keeps
/// current; no workspace is opened).
fn offers_of(
    state: &CombatState,
    roster: &BTreeMap<String, Member>,
    cards: &CardCache,
) -> Vec<Value> {
    let mut out = Vec::new();
    for (cid, m) in roster {
        let (Some(owner), Some(character)) = (&m.owner, &m.character) else {
            continue;
        };
        if m.kind != "character"
            || state
                .combatant(&CombatantId::new(cid.clone()))
                .is_none_or(|c| c.removed)
        {
            continue;
        }
        let Some(card) = cards.get(owner, character) else {
            continue;
        };
        for comp in card.companions {
            let origin = format!("companion:{cid}:{}", comp.key);
            let summoned = roster.iter().any(|(id, r)| {
                r.origin.as_deref() == Some(origin.as_str())
                    && state
                        .combatant(&CombatantId::new(id.clone()))
                        .is_some_and(|c| !c.removed)
            });
            if !summoned {
                out.push(json!({
                    "pc": cid,
                    "key": comp.key,
                    "name": comp.name,
                    "hp": comp.hp,
                    "ac": comp.ac,
                    "initiative": comp.initiative,
                    "elementId": comp.element_id,
                }));
            }
        }
    }
    out
}

/// Roll results players see for their own PCs and creatures: the newest [`ROLLS_KEPT`] per combatant.
async fn recent_rolls(conn: &mut AnyConnection, combat: &str) -> Res<Vec<Prompt>> {
    let rows = sqlx::query(
        "SELECT id, combatant_id, owner_id, kind, payload_json, created_at FROM combat_prompts
         WHERE combat_id = $1 AND kind = 'roll' ORDER BY created_at DESC, id DESC LIMIT 400",
    )
    .bind(combat)
    .fetch_all(&mut *conn)
    .await?;
    let mut per: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for row in rows {
        let combatant: String = row.try_get(1)?;
        let n = per.entry(combatant.clone()).or_default();
        if *n >= ROLLS_KEPT {
            continue;
        }
        *n += 1;
        let payload: String = row.try_get(4)?;
        out.push(Prompt {
            id: row.try_get(0)?,
            combatant,
            owner: row.try_get(2)?,
            kind: row.try_get(3)?,
            payload: serde_json::from_str(&payload).map_err(internal)?,
            at: row.try_get(5)?,
        });
    }
    Ok(out)
}

/// The encounter a combat came from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EncounterInfo {
    pub id: String,
    pub name: String,
    pub public_name: String,
}

async fn encounter_info(conn: &mut AnyConnection, id: Option<&str>) -> Res<Option<EncounterInfo>> {
    let Some(id) = id else { return Ok(None) };
    let row = sqlx::query("SELECT id, name, public_name FROM encounters WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(r) = row else { return Ok(None) };
    Ok(Some(EncounterInfo {
        id: r.try_get(0)?,
        name: r.try_get(1)?,
        public_name: r.try_get(2)?,
    }))
}

/// Everything a view is built from.
#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub combat: Loaded,
    pub roster: BTreeMap<String, Member>,
    pub prompts: Vec<Prompt>,
    /// The latest roll results for players' PCs and their creatures (D21), newest first.
    pub rolls: Vec<Prompt>,
    /// Companions on the PCs' sheets not yet in the fight: `{ pc, key, name, hp, ac, initiative, elementId }`.
    pub offers: Vec<Value>,
    pub encounter: Option<EncounterInfo>,
}

async fn snapshot(conn: &mut AnyConnection, combat: Loaded, cards: &CardCache) -> Res<Snapshot> {
    let roster = roster(conn, &combat.id).await?;
    let offers = offers_of(&combat.state, &roster, cards);
    let prompts = open_prompts(conn, &combat.id).await?;
    let rolls = recent_rolls(conn, &combat.id).await?;
    let encounter = encounter_info(conn, combat.encounter.as_deref()).await?;
    Ok(Snapshot {
        combat,
        roster,
        prompts,
        rolls,
        offers,
        encounter,
    })
}

// The projection.

/// Who a view is for.
pub(crate) enum Viewer<'a> {
    /// The party's DM, with the undo labels (newest first), the party's member characters now and who is online.
    Dm {
        undo: Vec<String>,
        members: &'a BTreeSet<(String, String)>,
        online: &'a dyn Fn(&str) -> bool,
    },
    /// A player, with their characters that are members of the party now.
    Player {
        account: &'a str,
        characters: &'a BTreeSet<String>,
    },
}

/// The one serializer of a combat for a viewer. Every player answer, error and event is built from what this gives
/// a player: hidden creatures are absent, and nothing of the DM's (labels, notes, `seq`, tiebreaks, element ids, the
/// log) is in it. A player's view has no `pseq`; the caller adds it.
pub(crate) fn view_for(viewer: &Viewer<'_>, snap: &Snapshot) -> Value {
    match viewer {
        Viewer::Dm {
            undo,
            members,
            online,
        } => dm_view(snap, undo, members, *online),
        Viewer::Player {
            account,
            characters,
        } => player_view(snap, account, characters),
    }
}

/// A stat block as players see it once revealed (D20 b): all of it but the XP.
fn public_block(block: Option<&Value>) -> Value {
    let Some(Value::Object(map)) = block else {
        return Value::Null;
    };
    let mut map = map.clone();
    map.remove("xp");
    map.remove("xp_lair");
    Value::Object(map)
}

fn prompt_json(p: &Prompt, dm: bool) -> Value {
    let mut out = json!({
        "id": p.id,
        "kind": p.kind,
        "amount": p.payload.get("amount").cloned().unwrap_or(Value::Null),
        "parts": p.payload.get("parts").cloned().unwrap_or_else(|| json!([])),
        "criticalHit": p.payload.get("criticalHit").cloned().unwrap_or(Value::Bool(false)),
        "at": p.at,
    });
    if dm {
        out["combatantId"] = json!(p.combatant);
    }
    out
}

/// Fields of `player_view_of` a player never needs from someone else's combatant or a creature: the account behind
/// a PC, and a creature's initiative (a follower's would give away its hidden leader's).
const NOT_FOR_OTHERS: [&str; 3] = ["kind", "initiative", "initiative_bonus"];

fn player_view(snap: &Snapshot, account: &str, characters: &BTreeSet<String>) -> Value {
    let s = &snap.combat.state;
    // My PCs: mine, and still members of the party.
    let my_pc = |id: &CombatantId| {
        s.combatant(id).is_some_and(|c| {
            !c.removed
                && c.owner() == Some(account)
                && snap
                    .roster
                    .get(&c.id.0)
                    .and_then(|m| m.character.as_ref())
                    .is_some_and(|ch| characters.contains(ch))
        })
    };
    // And the creatures they control (summons, pets, companions).
    let mine = |c: &Combatant| {
        if c.is_character() {
            my_pc(&c.id)
        } else {
            matches!(&c.controller, Controller::Player { pc } if my_pc(pc))
        }
    };
    let order: Vec<Value> = visible_order(s, ViewerKind::Player)
        .iter()
        .filter_map(|id| s.combatant(id))
        .map(|c| {
            let own = mine(c);
            let mut entry = json!({
                "id": c.id.0,
                "kind": if c.is_character() { "character" } else { "creature" },
                "name": c.public_name,
                "mine": own,
                "band": c.hp.band(),
                "life": c.life,
                "conditions": derive::conditions(s, &c.id),
                "exhaustion": c.exhaustion,
                "concentration": c.concentration.as_ref().map(|k| k.name.clone()),
            });
            if c.is_character() {
                entry["initiative"] = json!(c.initiative);
            }
            if let Some(owner) = c
                .owner_id
                .as_ref()
                .filter(|o| is_visible(s, o, ViewerKind::Player))
            {
                entry["ownerId"] = json!(owner.0);
            }
            // D6: another player's PC is a band, never its numbers. Everything else seen is in full (D20 b).
            if own || !c.is_character() {
                if let Some(Value::Object(mut v)) = player_view_of(s, &c.id) {
                    if !own {
                        for key in NOT_FOR_OTHERS {
                            v.remove(key);
                        }
                    }
                    entry["view"] = Value::Object(v);
                }
            }
            if !c.is_character() {
                entry["statBlock"] =
                    public_block(snap.roster.get(&c.id.0).and_then(|m| m.block.as_ref()));
            }
            entry
        })
        .collect();
    let me: Vec<Value> = s
        .combatants
        .iter()
        .filter(|c| !c.removed && mine(c))
        .map(|c| {
            json!({
                "id": c.id.0,
                "kind": if c.is_character() { "character" } else { "creature" },
                "characterId": if c.is_character() {
                    snap.roster.get(&c.id.0).and_then(|m| m.character.clone())
                } else {
                    None
                },
                "offers": snap.offers.iter()
                    .filter(|o| o["pc"] == c.id.0.as_str())
                    .map(|o| json!({ "key": o["key"], "name": o["name"], "hp": o["hp"], "ac": o["ac"], "initiative": o["initiative"] }))
                    .collect::<Vec<_>>(),
                "hp": { "current": c.hp.current, "max": c.hp.max, "temp": c.hp.temp },
                "life": c.life,
                "absent": c.absent,
                "concentration": c.concentration.as_ref().map(|k| k.name.clone()),
                "concentrationChecks": c.pending_concentration.iter().map(|k| json!({ "dc": k.dc })).collect::<Vec<_>>(),
                "prompts": snap.prompts.iter()
                    .filter(|p| p.combatant == c.id.0 && p.owner == account)
                    .map(|p| prompt_json(p, false))
                    .collect::<Vec<_>>(),
                "rolls": snap.rolls.iter()
                    .filter(|p| p.combatant == c.id.0 && p.owner == account)
                    .map(|p| p.payload.clone())
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "id": snap.combat.id,
        "partyId": snap.combat.party,
        "encounter": snap.encounter.as_ref().map(|e| json!({ "id": e.id, "publicName": e.public_name })),
        "status": s.status,
        "turn": turn_pointer(s, ViewerKind::Player),
        "showEnemyTurn": s.settings.show_enemy_turn,
        "order": order,
        "me": me,
    })
}

fn dm_view(
    snap: &Snapshot,
    undo: &[String],
    members: &BTreeSet<(String, String)>,
    online: &dyn Fn(&str) -> bool,
) -> Value {
    let mut creatures = Map::new();
    let mut characters = Map::new();
    for (id, m) in &snap.roster {
        if m.kind == "character" {
            let owner = m.owner.clone().unwrap_or_default();
            let character = m.character.clone().unwrap_or_default();
            characters.insert(
                id.clone(),
                json!({
                    "ownerId": owner,
                    "characterId": character,
                    "member": members.contains(&(owner.clone(), character)),
                    "online": online(&owner),
                }),
            );
        } else {
            creatures.insert(
                id.clone(),
                json!({
                    "encounterCreatureId": m.encounter_creature,
                    "statBlock": m.block,
                    "name": m.name,
                    "origin": m.origin.as_deref().map(|o| o.split(':').next().unwrap_or(o)),
                }),
            );
        }
    }
    json!({
        "id": snap.combat.id,
        "partyId": snap.combat.party,
        "encounter": snap.encounter.as_ref().map(|e| json!({ "id": e.id, "name": e.name, "publicName": e.public_name })),
        "seq": snap.combat.seq,
        "status": snap.combat.state.status,
        "live": snap.combat.status == "live",
        "state": snap.combat.state,
        "creatures": creatures,
        "characters": characters,
        "prompts": snap.prompts.iter().map(|p| prompt_json(p, true)).collect::<Vec<_>>(),
        "offers": snap.offers,
        "undo": undo,
        "startedAt": snap.combat.started_at,
        "endedAt": snap.combat.ended_at,
    })
}

/// The hash a player's projection is compared by.
pub(crate) fn view_hash(view: &Value) -> String {
    let bytes = serde_json::to_vec(view).unwrap_or_default();
    Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn members_of(
    conn: &mut AnyConnection,
    party: &str,
) -> sqlx::Result<BTreeSet<(String, String)>> {
    let rows = sqlx::query("SELECT owner_id, character_id FROM party_members WHERE party_id = $1")
        .bind(party)
        .fetch_all(&mut *conn)
        .await?;
    rows.iter()
        .map(|r| sqlx::Result::Ok((r.try_get(0)?, r.try_get(1)?)))
        .collect()
}

async fn undo_labels(conn: &mut AnyConnection, combat: &str) -> sqlx::Result<Vec<String>> {
    let rows =
        sqlx::query("SELECT label FROM combat_journal WHERE combat_id = $1 ORDER BY seq DESC")
            .bind(combat)
            .fetch_all(&mut *conn)
            .await?;
    rows.iter().map(|r| r.try_get(0)).collect()
}

async fn dm_id(conn: &mut AnyConnection, party: &str) -> sqlx::Result<Option<String>> {
    sqlx::query("SELECT dm_id FROM parties WHERE id = $1")
        .bind(party)
        .fetch_optional(&mut *conn)
        .await?
        .map(|r| r.try_get(0))
        .transpose()
}

/// Compare each player's projection with the hash kept for them, and move the `pseq` of those whose view changed.
/// Answers the players to tell, with their new `pseq`.
async fn refresh_views(conn: &mut AnyConnection, snap: &Snapshot) -> Res<Vec<(String, i64)>> {
    let Some(dm) = dm_id(conn, &snap.combat.party).await? else {
        return Ok(vec![]);
    };
    let mut audience: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (owner, character) in members_of(conn, &snap.combat.party).await? {
        if owner != dm {
            audience.entry(owner).or_default().insert(character);
        }
    }
    let mut tell = Vec::new();
    for (owner, characters) in audience {
        let hash = view_hash(&player_view(snap, &owner, &characters));
        let kept = sqlx::query(
            "SELECT seq, hash FROM combat_views WHERE combat_id = $1 AND viewer_id = $2",
        )
        .bind(&snap.combat.id)
        .bind(&owner)
        .fetch_optional(&mut *conn)
        .await?;
        let (seq, old): (i64, Option<String>) = match kept {
            Some(r) => (r.try_get(0)?, Some(r.try_get(1)?)),
            None => (0, None),
        };
        if old.as_deref() == Some(hash.as_str()) {
            continue;
        }
        sqlx::query(
            "INSERT INTO combat_views (combat_id, viewer_id, seq, hash) VALUES ($1, $2, $3, $4)
             ON CONFLICT (combat_id, viewer_id) DO UPDATE SET seq = excluded.seq, hash = excluded.hash",
        )
        .bind(&snap.combat.id)
        .bind(&owner)
        .bind(seq + 1)
        .bind(&hash)
        .execute(&mut *conn)
        .await?;
        tell.push((owner, seq + 1));
    }
    Ok(tell)
}

async fn pseq_of(conn: &mut AnyConnection, combat: &str, viewer: &str) -> sqlx::Result<i64> {
    Ok(
        sqlx::query("SELECT seq FROM combat_views WHERE combat_id = $1 AND viewer_id = $2")
            .bind(combat)
            .bind(viewer)
            .fetch_optional(&mut *conn)
            .await?
            .map(|r| r.try_get(0))
            .transpose()?
            .unwrap_or(0),
    )
}

/// The caller's view of a combat now: the DM view, or the player's projection with their `pseq`. 404 for anyone
/// else.
async fn view_now(state: &AppState, user: &User, combat: &str) -> Res<Value> {
    if !id_ok(combat) {
        return Err(not_found().into());
    }
    let mut conn = state.db.pool().acquire().await?;
    let Some(loaded) = load(&mut conn, combat).await? else {
        return Err(not_found().into());
    };
    let party = loaded.party.clone();
    match role(&mut conn, &user.id, &party).await? {
        Some(Role::Dm) => {
            let undo = undo_labels(&mut conn, combat).await?;
            let members = members_of(&mut conn, &party).await?;
            let snap = snapshot(&mut conn, loaded, &state.hub.cards).await?;
            let hub = Arc::clone(&state.hub);
            let online = move |u: &str| hub.online(u);
            Ok(view_for(
                &Viewer::Dm {
                    undo,
                    members: &members,
                    online: &online,
                },
                &snap,
            ))
        }
        Some(Role::Player) => {
            let characters = my_characters(&mut conn, &user.id, &party).await?;
            let pseq = pseq_of(&mut conn, combat, &user.id).await?;
            let snap = snapshot(&mut conn, loaded, &state.hub.cards).await?;
            let mut view = view_for(
                &Viewer::Player {
                    account: &user.id,
                    characters: &characters,
                },
                &snap,
            );
            view["pseq"] = json!(pseq);
            Ok(view)
        }
        None => Err(not_found().into()),
    }
}

// Writes.

/// What a write does.
#[derive(Debug, Clone)]
enum Op {
    /// One engine command.
    Engine(Box<Command>),
    /// C7: copies of a creature.
    Add {
        source: AddSource,
        count: u32,
        roll: bool,
        hidden: Option<bool>,
    },
    /// C8.
    Undo { force: bool },
    /// The end of the combat.
    End,
    /// A player answered a prompt (D7 a).
    Resolve {
        prompt: String,
        applied: bool,
        hp: Option<HpIn>,
    },
    /// A PC's roster card changed: its HP comes into the combat.
    Sync { id: String, hp: Hp },
}

#[derive(Debug, Clone)]
enum AddSource {
    Copy(String),
    Encounter(String),
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct HpIn {
    current: i32,
    max: i32,
    #[serde(default)]
    temp: i32,
}

/// Who writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    Dm,
    Player,
    System,
}

#[derive(Debug, Clone)]
struct Write {
    account: String,
    writer: Writer,
    request: Option<String>,
    expect: Option<i64>,
    op: Op,
    /// The PC a player acts on (checked again in the transaction).
    own: Option<String>,
    /// Who a summoned creature is (its stat block, name and origin), when the command is a summon.
    summoned: Option<Member>,
}

/// What a write did.
#[derive(Debug, Default)]
struct Done {
    party: String,
    seq: i64,
    replayed: bool,
    ended: bool,
    nothing: bool,
    tell: Vec<(String, i64)>,
    dm: Option<String>,
}

fn author(w: Writer) -> Author {
    match w {
        Writer::Dm => Author::Dm,
        Writer::Player => Author::Player,
        Writer::System => Author::System,
    }
}

fn author_str(a: Author) -> &'static str {
    match a {
        Author::Dm => "dm",
        Author::Player => "player",
        Author::System => "system",
    }
}

/// An engine refusal, as the writer may see it. Players get a fixed message about their own PC, or the 404.
fn refusal(writer: Writer, e: &CombatError) -> Fail {
    if writer != Writer::Player {
        if matches!(e, CombatError::UndoWouldRevertPlayer) {
            return Fail::Coded(StatusCode::CONFLICT, "undo_player", e.to_string());
        }
        return Fail::Refused(e.to_string());
    }
    match e {
        CombatError::NotDying => Fail::Player(StatusCode::CONFLICT, "Not making death saves."),
        CombatError::Dead => Fail::Player(StatusCode::CONFLICT, "Dead: set HP to revive."),
        CombatError::NoPendingCheck => {
            Fail::Player(StatusCode::CONFLICT, "No concentration check is waiting.")
        }
        CombatError::NotActive => Fail::Player(StatusCode::CONFLICT, "The combat isn't running."),
        CombatError::Ended => Fail::Player(StatusCode::CONFLICT, "The combat has ended."),
        CombatError::Dice(_) | CombatError::Invalid(_) => {
            Fail::Player(StatusCode::BAD_REQUEST, "That roll isn't valid.")
        }
        _ => Fail::Api(not_found()),
    }
}

/// The new state of a write, before it is stored.
struct Applied {
    state: CombatState,
    die: SeededDie,
    dm: Vec<DmEvent>,
    players: Vec<PlayerEvent>,
    /// The undo step to keep, by label (none for the server's own syncs, and for undo itself).
    journal: Option<&'static str>,
    /// The undo step undone.
    popped: Option<i64>,
    new_members: Vec<(String, Member)>,
    resolved: Option<(String, bool)>,
}

impl Applied {
    fn plain(c: &Loaded) -> Self {
        Self {
            state: c.state.clone(),
            die: c.die,
            dm: vec![],
            players: vec![],
            journal: None,
            popped: None,
            new_members: vec![],
            resolved: None,
        }
    }
}

fn actor_of(w: &Write) -> Actor {
    match w.writer {
        Writer::Dm => Actor::Dm,
        Writer::Player => Actor::Player {
            account: w.account.clone(),
        },
        Writer::System => Actor::System,
    }
}

/// The PCs of `account`'s a command acts through: each combatant it names that the account plays (its PC, or a
/// creature its PC controls), as that PC.
fn acting_pcs(state: &CombatState, cmd: &Command, account: &str) -> BTreeSet<CombatantId> {
    fn strings(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(list) => list.iter().for_each(|x| strings(x, out)),
            Value::Object(map) => map.values().for_each(|x| strings(x, out)),
            _ => {}
        }
    }
    let mut named = Vec::new();
    strings(
        &serde_json::to_value(cmd).unwrap_or(Value::Null),
        &mut named,
    );
    let mut out = BTreeSet::new();
    for id in named {
        let id = CombatantId::new(id);
        let Some(c) = state.combatant(&id) else {
            continue;
        };
        if party_combat::engine::account_of(state, c).as_deref() != Some(account) {
            continue;
        }
        if c.is_character() {
            out.insert(id);
        } else if let Controller::Player { pc } = &c.controller {
            out.insert(pc.clone());
        }
    }
    out
}

/// Run a write: one transaction, compare-and-set on `seq`, tried again when another write won the race. The caller
/// holds the combat's lock (tests leave it out to race two writers on purpose).
async fn run_write(state: &AppState, combat: &str, w: &Write) -> Res<Done> {
    for _ in 0..ATTEMPTS {
        match attempt(state, combat, w).await {
            Err(Fail::Retry) => {
                tokio::time::sleep(std::time::Duration::from_millis(
                    5 + u64::from(rand::random::<u8>() % 20),
                ))
                .await;
            }
            other => return other,
        }
    }
    Err(Fail::Retry)
}

#[allow(clippy::too_many_lines)]
async fn attempt(state: &AppState, combat: &str, w: &Write) -> Res<Done> {
    let mut tx = state.db.pool().begin().await?;
    let Some(c) = load(&mut tx, combat).await? else {
        return Err(not_found().into());
    };
    let party = c.party.clone();
    // Who, again, inside this transaction.
    match w.writer {
        Writer::Dm => {
            if role(&mut tx, &w.account, &party).await? != Some(Role::Dm) {
                return Err(not_found().into());
            }
        }
        Writer::Player => {
            if role(&mut tx, &w.account, &party).await? != Some(Role::Player) {
                return Err(not_found().into());
            }
        }
        Writer::System => {}
    }
    let roster_now = roster(&mut tx, combat).await?;
    // A player's own PC: theirs, and a member of the party now.
    let mut own = w.own.clone();
    if let Op::Resolve { prompt, .. } = &w.op {
        let row = sqlx::query(
            "SELECT combatant_id FROM combat_prompts
             WHERE id = $1 AND combat_id = $2 AND owner_id = $3 AND resolved_at IS NULL",
        )
        .bind(prompt)
        .bind(combat)
        .bind(&w.account)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(not_found)?;
        own = Some(row.try_get(0)?);
    }
    if w.writer == Writer::Player {
        // The engine decides what an account may do (its own PC and the creatures it controls); the server adds
        // that each PC acting is still a member of the party, read in this transaction.
        let characters = my_characters(&mut tx, &w.account, &party).await?;
        let member_pc = |pc: &CombatantId| {
            roster_now.get(&pc.0).is_some_and(|m| {
                m.kind == "character"
                    && m.owner.as_deref() == Some(w.account.as_str())
                    && m.character
                        .as_ref()
                        .is_some_and(|ch| characters.contains(ch))
            }) && c.state.combatant(pc).is_some_and(|x| !x.removed)
        };
        let ok = match (&w.op, own.as_deref()) {
            (Op::Resolve { .. }, Some(id)) => member_pc(&CombatantId::new(id)),
            (Op::Engine(cmd), _) => {
                let pcs = acting_pcs(&c.state, cmd, &w.account);
                party_combat::engine::player_may(&c.state, &w.account, cmd)
                    && !pcs.is_empty()
                    && pcs.iter().all(member_pc)
            }
            _ => false,
        };
        if !ok {
            return Err(not_found().into());
        }
    }
    if c.status != "live" {
        return Err(match w.writer {
            Writer::Player => Fail::Player(StatusCode::CONFLICT, "The combat has ended."),
            _ => Fail::Api(ApiError::Conflict("The combat has ended.".into())),
        });
    }
    if let Some(request) = &w.request {
        let done = sqlx::query(
            "INSERT INTO combat_requests (scope, account_id, request_id, result, created_at)
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
        )
        .bind(combat)
        .bind(&w.account)
        .bind(request)
        .bind(combat)
        .bind(now())
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 0 {
            return Ok(Done {
                party,
                seq: c.seq,
                replayed: true,
                ..Done::default()
            });
        }
    }
    if let Some(expect) = w.expect {
        if expect != c.seq {
            return Err(Fail::Stale(c.seq));
        }
    }
    let actor = actor_of(w);
    let mut out = Applied::plain(&c);
    match &w.op {
        Op::Engine(cmd) => {
            let mut die = c.die;
            let o = apply(&c.state, &actor, cmd, &mut die).map_err(|e| refusal(w.writer, &e))?;
            if let Command::Summon { combatant, .. } = cmd.as_ref() {
                let member = w.summoned.clone().unwrap_or_else(|| Member {
                    kind: "creature".into(),
                    origin: Some("summon".into()),
                    ..Member::default()
                });
                out.new_members.push((combatant.id.0.clone(), member));
            }
            out.state = o.state;
            out.die = die;
            out.dm = o.events.dm;
            out.players = o.events.players;
            out.journal = Some(cmd.label());
        }
        Op::End => {
            let mut die = c.die;
            let o = apply(&c.state, &actor, &Command::End, &mut die)
                .map_err(|e| refusal(w.writer, &e))?;
            out.state = o.state;
            out.die = die;
            out.dm = o.events.dm;
            out.players = o.events.players;
            out.journal = Some(Command::End.label());
        }
        Op::Undo { force } => {
            let row = sqlx::query(
                "SELECT seq, author, label, before_json FROM combat_journal WHERE combat_id = $1
                 ORDER BY seq DESC LIMIT 1",
            )
            .bind(combat)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = row else {
                return Err(Fail::Refused(CombatError::NothingToUndo.to_string()));
            };
            let seq: i64 = row.try_get(0)?;
            let who: String = row.try_get(1)?;
            let before: String = row.try_get(3)?;
            let entry = JournalEntry {
                seq: u64::try_from(seq).unwrap_or(0),
                author: match who.as_str() {
                    "player" => Author::Player,
                    "system" => Author::System,
                    _ => Author::Dm,
                },
                label: row.try_get(2)?,
                before: serde_json::from_str(&before).map_err(internal)?,
            };
            let mut engine = Combat {
                state: c.state.clone(),
                die: c.die,
                journal: VecDeque::from([entry]),
                log: vec![],
                seq: u64::try_from(c.seq).unwrap_or(0),
            };
            let events = engine
                .undo(&Actor::Dm, *force)
                .map_err(|e| refusal(w.writer, &e))?;
            out.state = engine.state;
            out.dm = events.dm;
            out.players = events.players;
            out.popped = Some(seq);
        }
        Op::Add {
            source,
            count,
            roll,
            hidden,
        } => {
            // A fresh template at full HP: a copy of a creature in this fight, or of an encounter creature.
            let (base_member, template) = match source {
                AddSource::Copy(id) => {
                    let m = roster_now
                        .get(id)
                        .filter(|m| m.kind == "creature")
                        .ok_or_else(|| {
                            ApiError::Invalid("There is no such creature in this combat.".into())
                        })?;
                    let x = c
                        .state
                        .combatant(&CombatantId::new(id.clone()))
                        .filter(|x| !x.is_character())
                        .ok_or_else(|| {
                            ApiError::Invalid("There is no such creature in this combat.".into())
                        })?;
                    let mut t = creature_of(
                        "template",
                        &x.label,
                        &x.public_name,
                        &Fighter {
                            block: m.block.clone().unwrap_or(Value::Null),
                            data: m
                                .block
                                .as_ref()
                                .and_then(|b| serde_json::from_value(b.clone()).ok()),
                            name: m.name.clone().unwrap_or_default(),
                            ac: x.ac,
                            average: x.hp.max.max(1),
                            formula: None,
                        },
                        x.hp.max.max(1),
                    );
                    t.ac = x.ac;
                    t.initiative_bonus = x.initiative_bonus;
                    t.visible = x.visible;
                    t.owner_id.clone_from(&x.owner_id);
                    t.controller = x.controller.clone();
                    t.initiative_mode = x.initiative_mode;
                    let mut member = m.clone();
                    member.origin = Some("copy".into());
                    (member, t)
                }
                AddSource::Encounter(id) => {
                    let enc = c
                        .encounter
                        .clone()
                        .ok_or_else(|| ApiError::Invalid("This combat has no encounter.".into()))?;
                    let row = sqlx::query(
                        "SELECT dm_label, public_name, statblock_json, overrides_json, visibility, owner_creature_id,
                            init_mode
                         FROM encounter_creatures WHERE id = $1 AND encounter_id = $2",
                    )
                    .bind(id)
                    .bind(&enc)
                    .fetch_optional(&mut *tx)
                    .await?
                    .ok_or_else(|| {
                        ApiError::Invalid("There is no such creature in the encounter.".into())
                    })?;
                    let f =
                        fighter_of(&row.try_get::<String, _>(2)?, &row.try_get::<String, _>(3)?)?;
                    let label: String = row.try_get(0)?;
                    let public: String = row.try_get(1)?;
                    let visibility: String = row.try_get(4)?;
                    let owner: Option<String> = row.try_get(5)?;
                    let mode: String = row.try_get(6)?;
                    let mut t = creature_of("template", &label, &public, &f, f.average);
                    t.visible = visibility != "hidden";
                    // Its owner: the first combatant made from the owning encounter creature, if it's in the fight.
                    if let Some(owner) = owner {
                        t.owner_id = roster_now
                            .iter()
                            .find(|(cid, m)| {
                                m.encounter_creature.as_deref() == Some(owner.as_str())
                                    && c.state
                                        .combatant(&CombatantId::new((*cid).clone()))
                                        .is_some_and(|x| !x.removed)
                            })
                            .map(|(cid, _)| CombatantId::new(cid.clone()));
                        if t.owner_id.is_some() {
                            t.initiative_mode =
                                serde_json::from_value(json!(mode)).unwrap_or_default();
                        }
                    }
                    (
                        Member {
                            kind: "creature".into(),
                            encounter_creature: Some(id.clone()),
                            block: Some(f.block.clone()),
                            name: Some(f.name.clone()),
                            origin: Some("encounter".into()),
                            ..Member::default()
                        },
                        t,
                    )
                }
            };
            let visible = hidden.map_or(template.visible, |h| !h);
            let mut st = c.state.clone();
            let mut die = c.die;
            let taken: BTreeSet<String> = st.combatants.iter().map(|x| x.label.clone()).collect();
            let mut n = 2;
            for _ in 0..*count {
                let mut name = format!("{} {n}", template.label);
                while taken.contains(&name) {
                    n += 1;
                    name = format!("{} {n}", template.label);
                }
                n += 1;
                let id = new_id();
                let mut x = template.clone();
                x.id = CombatantId::new(id.clone());
                x.label = name;
                x.visible = visible;
                x.ever_revealed = visible;
                let cmd = Command::AddCombatant {
                    combatant: Box::new(x),
                    roll_initiative: *roll,
                };
                let o = apply(&st, &actor, &cmd, &mut die).map_err(|e| refusal(w.writer, &e))?;
                st = o.state;
                out.dm.extend(o.events.dm);
                out.players.extend(o.events.players);
                out.new_members.push((id, base_member.clone()));
            }
            out.state = st;
            out.die = die;
            out.journal = Some("add creature");
        }
        Op::Resolve {
            prompt,
            applied,
            hp,
        } => {
            let id = own.clone().unwrap_or_default();
            out.resolved = Some((prompt.clone(), *applied));
            if *applied {
                let row =
                    sqlx::query("SELECT kind, payload_json FROM combat_prompts WHERE id = $1")
                        .bind(prompt)
                        .fetch_one(&mut *tx)
                        .await?;
                let kind: String = row.try_get(0)?;
                let payload: Value =
                    serde_json::from_str(&row.try_get::<String, _>(1)?).map_err(internal)?;
                let hp = match hp {
                    Some(h) => Some(Hp {
                        current: h.current,
                        max: h.max,
                        temp: h.temp,
                    }),
                    None => {
                        let m = roster_now.get(&id);
                        let owner = m.and_then(|m| m.owner.clone()).unwrap_or_default();
                        let character = m.and_then(|m| m.character.clone()).unwrap_or_default();
                        let hub = Arc::clone(&state.hub);
                        tokio::task::spawn_blocking(move || hub.cards.get(&owner, &character))
                            .await
                            .map_err(internal)?
                            .as_ref()
                            .and_then(card_hp)
                    }
                };
                if let Some(hp) = hp {
                    let damage = (kind == "damage").then(|| DamageTaken {
                        amount: payload
                            .get("amount")
                            .and_then(Value::as_i64)
                            .and_then(|a| i32::try_from(a).ok())
                            .unwrap_or(0),
                        critical_hit: payload
                            .get("criticalHit")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    });
                    let cmd = Command::SyncPlayerHp {
                        id: CombatantId::new(id.clone()),
                        current: hp.current,
                        max: hp.max,
                        temp: hp.temp,
                        damage,
                    };
                    let mut die = c.die;
                    let o = apply(&c.state, &Actor::System, &cmd, &mut die)
                        .map_err(|e| refusal(w.writer, &e))?;
                    out.state = o.state;
                    out.die = die;
                    out.dm = o.events.dm;
                    out.players = o.events.players;
                    out.journal = Some("apply prompt");
                }
            }
        }
        Op::Sync { id, hp } => {
            let cid = CombatantId::new(id.clone());
            let Some(x) = c.state.combatant(&cid) else {
                return Ok(Done {
                    party,
                    nothing: true,
                    ..Done::default()
                });
            };
            if x.hp == *hp {
                return Ok(Done {
                    party,
                    nothing: true,
                    ..Done::default()
                });
            }
            let cmd = Command::SyncPlayerHp {
                id: cid,
                current: hp.current,
                max: hp.max,
                temp: hp.temp,
                damage: None,
            };
            let mut die = c.die;
            let o = apply(&c.state, &Actor::System, &cmd, &mut die)
                .map_err(|e| Fail::Refused(e.to_string()))?;
            out.state = o.state;
            out.die = die;
            out.dm = o.events.dm;
            out.players = o.events.players;
        }
    }
    let ended = matches!(w.op, Op::End);
    let seq = c.seq + 1;
    let at = now();
    let updated = sqlx::query(
        "UPDATE combats SET seq = $1, state_json = $2, die_json = $3, updated_at = $4, status = $5, ended_at = $6
         WHERE id = $7 AND seq = $8",
    )
    .bind(seq)
    .bind(serde_json::to_string(&out.state).map_err(internal)?)
    .bind(serde_json::to_string(&out.die).map_err(internal)?)
    .bind(at)
    .bind(if ended { "ended" } else { "live" })
    .bind(if ended { Some(at) } else { None })
    .bind(combat)
    .bind(c.seq)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        // Another write won: start again from its state.
        return Err(Fail::Retry);
    }
    let who = author(w.writer);
    if let Some(label) = out.journal {
        sqlx::query(
            "INSERT INTO combat_journal (combat_id, seq, author, label, before_json) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(combat)
        .bind(seq)
        .bind(author_str(who))
        .bind(label)
        .bind(serde_json::to_string(&c.state).map_err(internal)?)
        .execute(&mut *tx)
        .await?;
        let keep = i64::try_from(UNDO_LIMIT).unwrap_or(50);
        sqlx::query(
            "DELETE FROM combat_journal WHERE combat_id = $1 AND seq NOT IN
                (SELECT seq FROM combat_journal WHERE combat_id = $1 ORDER BY seq DESC LIMIT $2)",
        )
        .bind(combat)
        .bind(keep)
        .execute(&mut *tx)
        .await?;
    }
    if let Some(popped) = out.popped {
        sqlx::query("DELETE FROM combat_journal WHERE combat_id = $1 AND seq = $2")
            .bind(combat)
            .bind(popped)
            .execute(&mut *tx)
            .await?;
        // Prompts the undone write made go with it.
        for p in open_prompts(&mut tx, combat).await? {
            if p.payload.get("seq").and_then(Value::as_i64) == Some(popped) {
                sqlx::query("DELETE FROM combat_prompts WHERE id = $1")
                    .bind(&p.id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    for (n, event) in out.dm.iter().enumerate() {
        sqlx::query(
            "INSERT INTO combat_log (combat_id, seq, n, at, author, event_json) VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(combat)
        .bind(seq)
        .bind(i64::try_from(n).unwrap_or(0))
        .bind(at)
        .bind(author_str(who))
        .bind(serde_json::to_string(event).map_err(internal)?)
        .execute(&mut *tx)
        .await?;
    }
    for event in &out.players {
        let (id, owner, kind, payload) = match event {
            PlayerEvent::DamagePrompt {
                id,
                owner,
                amount,
                parts,
                critical_hit,
            } => (
                id,
                owner,
                "damage",
                json!({ "amount": amount, "parts": parts, "criticalHit": critical_hit, "seq": seq }),
            ),
            PlayerEvent::RollResult { id, owner, .. } => {
                let mut payload = serde_json::to_value(event).map_err(internal)?;
                if let Some(map) = payload.as_object_mut() {
                    map.remove("owner");
                    map.remove("event");
                    // `name`, not `label`: nothing in a player's view is called a label.
                    if let Some(label) = map.remove("label") {
                        map.insert("name".into(), label);
                    }
                }
                sqlx::query(
                    "INSERT INTO combat_prompts (id, combat_id, combatant_id, owner_id, kind, payload_json, created_at, resolved_at)
                     VALUES ($1, $2, $3, $4, 'roll', $5, $6, $6)",
                )
                .bind(new_id())
                .bind(combat)
                .bind(&id.0)
                .bind(owner)
                .bind(payload.to_string())
                .bind(at)
                .execute(&mut *tx)
                .await?;
                continue;
            }
            PlayerEvent::HealPrompt { id, owner, amount } => (
                id,
                owner,
                "heal",
                json!({ "amount": amount, "parts": [], "criticalHit": false, "seq": seq }),
            ),
            _ => continue,
        };
        sqlx::query(
            "INSERT INTO combat_prompts (id, combat_id, combatant_id, owner_id, kind, payload_json, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(new_id())
        .bind(combat)
        .bind(&id.0)
        .bind(owner)
        .bind(kind)
        .bind(payload.to_string())
        .bind(at)
        .execute(&mut *tx)
        .await?;
    }
    if let Some((prompt, applied)) = &out.resolved {
        sqlx::query("UPDATE combat_prompts SET resolved_at = $1, applied = $2 WHERE id = $3")
            .bind(at)
            .bind(i64::from(*applied))
            .bind(prompt)
            .execute(&mut *tx)
            .await?;
    }
    for (id, m) in &out.new_members {
        insert_member(&mut tx, combat, id, m).await?;
    }
    if ended {
        if let Some(enc) = &c.encounter {
            sqlx::query("UPDATE encounters SET status = 'ended', updated_at = $1 WHERE id = $2 AND status = 'live'")
                .bind(at)
                .bind(enc)
                .execute(&mut *tx)
                .await?;
        }
    }
    let after = Loaded {
        seq,
        state: out.state,
        die: out.die,
        status: if ended { "ended".into() } else { "live".into() },
        ended_at: if ended { Some(at) } else { c.ended_at },
        ..c
    };
    let snap = snapshot(&mut tx, after, &state.hub.cards).await?;
    let tell = refresh_views(&mut tx, &snap).await?;
    let dm = dm_id(&mut tx, &party).await?;
    tx.commit().await?;
    Ok(Done {
        party,
        seq,
        replayed: false,
        ended,
        nothing: false,
        tell,
        dm,
    })
}

/// Tell the DM and the players whose view changed.
async fn announce(state: &AppState, combat: &str, done: &Done) {
    if done.replayed || done.nothing {
        return;
    }
    if let Some(dm) = &done.dm {
        state.hub.deliver(
            dm,
            COMBAT_EVENT,
            &json!({ "party": done.party, "combat": combat, "seq": done.seq }),
        );
    }
    for (owner, pseq) in &done.tell {
        state.hub.deliver(
            owner,
            COMBAT_EVENT,
            &json!({ "party": done.party, "combat": combat, "pseq": pseq }),
        );
    }
    if done.ended {
        set_live(&done.party, false);
        guard(&LOCKS).remove(combat);
        crate::parties::publish(state, &done.party, "combat", Some(combat), true).await;
    }
}

/// A member character's roster card changed while its party is in a combat: bring its HP into the combat (D7 a).
/// Not while a damage prompt is open for it: the prompt's answer brings the HP in, with the damage.
pub(crate) async fn card_changed(state: &AppState, party: &str, owner: &str, character: &str) {
    if !party_in_combat(party) {
        return;
    }
    let found = sqlx::query(
        "SELECT c.id, m.id FROM combats c JOIN combatants m ON m.combat_id = c.id
         WHERE c.party_id = $1 AND c.status = 'live' AND m.owner_id = $2 AND m.character_id = $3",
    )
    .bind(party)
    .bind(owner)
    .bind(character)
    .fetch_optional(state.db.pool())
    .await;
    let Ok(Some(row)) = found else { return };
    let (Ok(combat), Ok(id)) = (row.try_get::<String, _>(0), row.try_get::<String, _>(1)) else {
        return;
    };
    let open = sqlx::query(
        "SELECT 1 FROM combat_prompts WHERE combat_id = $1 AND combatant_id = $2 AND kind = 'damage' AND resolved_at IS NULL",
    )
    .bind(&combat)
    .bind(&id)
    .fetch_optional(state.db.pool())
    .await;
    if !matches!(open, Ok(None)) {
        return;
    }
    let Some(hp) = state
        .hub
        .cards
        .get(owner, character)
        .as_ref()
        .and_then(card_hp)
    else {
        return;
    };
    let w = Write {
        account: owner.to_string(),
        writer: Writer::System,
        request: None,
        expect: None,
        op: Op::Sync { id, hp },
        own: None,
        summoned: None,
    };
    let _lock = combat_lock(&combat).await;
    match run_write(state, &combat, &w).await {
        Ok(done) => announce(state, &combat, &done).await,
        Err(e) => tracing::warn!("combat: syncing a PC's HP into {combat}: {e:?}"),
    }
}

fn card_hp(card: &Card) -> Option<Hp> {
    let s = card.stats.as_ref()?;
    Some(Hp {
        current: i32::try_from(s.hp).unwrap_or(i32::MAX),
        max: i32::try_from(s.hp_max).unwrap_or(i32::MAX),
        temp: i32::try_from(s.temp_hp).unwrap_or(i32::MAX),
    })
}

// Building combatants from stat blocks.

/// A stat block from a combat file without what a creature is worth in XP: an edited file never brings XP back into
/// the DM's view or an export (D16).
fn without_xp(mut block: Value) -> Value {
    if let Some(map) = block.as_object_mut() {
        for key in party_encounter::creature::HIDDEN_KEYS {
            map.remove(key);
        }
    }
    block
}

/// The fighter of an encounter creature's copy (`statblock_json`) with its overrides (`party_encounter::combatant`).
fn fighter_of(statblock: &str, overrides: &str) -> Res<Fighter> {
    Ok(party_encounter::combatant::fighter_of(statblock, overrides)
        .map_err(|e| internal(format!("a stat block: {e}")))?)
}

fn rolled_hp(formula: Option<&str>, die: &mut dyn DieRoller) -> Option<i32> {
    let expr = party_dice::try_parse_notation(formula?)?;
    party_dice::roll_expr(&expr, die, &party_dice::RollOptions::default())
        .ok()
        .map(|r| r.total.max(1))
}

// Routes.

#[derive(Debug, Deserialize)]
struct StartBody {
    #[serde(rename = "requestId")]
    request_id: String,
}

/// The combat a done request made, if this account already sent it.
async fn done_before(
    state: &AppState,
    scope: &str,
    account: &str,
    request: &str,
) -> sqlx::Result<Option<String>> {
    sqlx::query("SELECT result FROM combat_requests WHERE scope = $1 AND account_id = $2 AND request_id = $3")
        .bind(scope)
        .bind(account)
        .bind(request)
        .fetch_optional(state.db.pool())
        .await?
        .map(|r| r.try_get(0))
        .transpose()
}

/// A new combat, stored with its roster, its first player views and the request that made it.
struct NewCombat {
    party: String,
    encounter: Option<String>,
    state: CombatState,
    die: SeededDie,
    members: Vec<(String, Member)>,
    scope: String,
    request: String,
}

async fn insert_combat(state: &AppState, user: &User, new: NewCombat) -> Res<String> {
    let id = new_id();
    let at = now();
    let mut tx = state.db.pool().begin().await?;
    if role(&mut tx, &user.id, &new.party).await? != Some(Role::Dm) {
        return Err(not_found().into());
    }
    let live = sqlx::query("SELECT 1 FROM combats WHERE party_id = $1 AND status = 'live'")
        .bind(&new.party)
        .fetch_optional(&mut *tx)
        .await?;
    if live.is_some() {
        return Err(ApiError::Conflict("This party already has a live combat.".into()).into());
    }
    let done = sqlx::query(
        "INSERT INTO combat_requests (scope, account_id, request_id, result, created_at)
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(&new.scope)
    .bind(&user.id)
    .bind(&new.request)
    .bind(&id)
    .bind(at)
    .execute(&mut *tx)
    .await?;
    if done.rows_affected() == 0 {
        return Err(Fail::Retry);
    }
    let inserted = sqlx::query(
        "INSERT INTO combats (id, party_id, encounter_id, status, seq, state_json, die_json, started_at, updated_at)
         VALUES ($1, $2, $3, 'live', 0, $4, $5, $6, $6)",
    )
    .bind(&id)
    .bind(&new.party)
    .bind(new.encounter.clone())
    .bind(serde_json::to_string(&new.state).map_err(internal)?)
    .bind(serde_json::to_string(&new.die).map_err(internal)?)
    .bind(at)
    .execute(&mut *tx)
    .await;
    if let Err(e) = inserted {
        // The one-live-combat index: another start won.
        if e.to_string().to_lowercase().contains("unique") {
            return Err(ApiError::Conflict("This party already has a live combat.".into()).into());
        }
        return Err(e.into());
    }
    for (cid, m) in &new.members {
        insert_member(&mut tx, &id, cid, m).await?;
    }
    if let Some(enc) = &new.encounter {
        sqlx::query("UPDATE encounters SET status = 'live', updated_at = $1 WHERE id = $2")
            .bind(at)
            .bind(enc)
            .execute(&mut *tx)
            .await?;
    }
    let loaded = load(&mut tx, &id)
        .await?
        .ok_or_else(|| internal("the new combat is gone"))?;
    let snap = snapshot(&mut tx, loaded, &state.hub.cards).await?;
    refresh_views(&mut tx, &snap).await?;
    tx.commit().await?;
    set_live(&new.party, true);
    crate::parties::publish(state, &new.party, "combat", Some(&id), true).await;
    Ok(id)
}

fn edition_of(ruleset: &str) -> Edition {
    if ruleset == "dnd2014" {
        Edition::Srd51
    } else {
        Edition::Srd521
    }
}

/// `POST /api/encounters/{encounter}/start` (DM): a combat from the encounter, in setup.
#[allow(clippy::too_many_lines)]
pub async fn start(
    State(state): State<AppState>,
    user: User,
    Path(encounter): Path<String>,
    body: Bytes,
) -> Response {
    match start_inner(&state, &user, &encounter, &body).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

#[allow(clippy::too_many_lines)]
async fn start_inner(state: &AppState, user: &User, encounter: &str, body: &[u8]) -> Res<Value> {
    if !id_ok(encounter) {
        return Err(not_found().into());
    }
    let Some(row) = sqlx::query(
        "SELECT e.party_id, e.hp_mode, p.dm_id, p.ruleset FROM encounters e JOIN parties p ON p.id = e.party_id
         WHERE e.id = $1",
    )
    .bind(encounter)
    .fetch_optional(state.db.pool())
    .await?
    else {
        return Err(not_found().into());
    };
    let party: String = row.try_get(0)?;
    let hp_mode: String = row.try_get(1)?;
    let dm: String = row.try_get(2)?;
    let ruleset: String = row.try_get(3)?;
    if dm != user.id {
        return Err(not_found().into());
    }
    let body: StartBody =
        serde_json::from_slice(body).map_err(|_| ApiError::Invalid("Send a requestId.".into()))?;
    if !request_ok(&body.request_id) {
        return Err(ApiError::Invalid("The requestId isn't valid.".into()).into());
    }
    let scope = format!("start:{encounter}");
    if let Some(done) = done_before(state, &scope, &user.id, &body.request_id).await? {
        let mut view = view_now(state, user, &done).await?;
        view["replayed"] = json!(true);
        return Ok(view);
    }
    if !take_write(&user.id) {
        return Err(ApiError::TooMany.into());
    }
    let _job = job(&user.id).await?;
    // One connection for the reads (a single-connection pool must never wait on a second one).
    let mut conn = state.db.pool().acquire().await?;
    let mut die = SeededDie::new(seed());
    let mut st = CombatState::new(edition_of(&ruleset));
    let mut members = Vec::new();
    let mut by_creature: HashMap<String, String> = HashMap::new();
    let rows = sqlx::query(
        "SELECT id, dm_label, public_name, statblock_json, overrides_json, hp_mode, hp_max, visibility,
            owner_creature_id, init_mode
         FROM encounter_creatures WHERE encounter_id = $1 ORDER BY sort, id",
    )
    .bind(encounter)
    .fetch_all(&mut *conn)
    .await?;
    // Owners before the creatures they control: the engine refuses an owner that isn't in the fight yet.
    let mut rows: Vec<_> = rows.into_iter().collect();
    rows.sort_by_key(|r| r.try_get::<Option<String>, _>(8).ok().flatten().is_some());
    for row in rows {
        let ec: String = row.try_get(0)?;
        let label: String = row.try_get(1)?;
        let public: String = row.try_get(2)?;
        let f = fighter_of(&row.try_get::<String, _>(3)?, &row.try_get::<String, _>(4)?)?;
        let own_mode: String = row.try_get(5)?;
        let fixed: Option<i64> = row.try_get(6)?;
        let visibility: String = row.try_get(7)?;
        let owner: Option<String> = row.try_get(8)?;
        let init_mode: String = row.try_get(9)?;
        let hp = match (own_mode.as_str(), fixed) {
            ("fixed", Some(n)) => i32::try_from(n).unwrap_or(1).max(1),
            _ if hp_mode == "rolled" => {
                rolled_hp(f.formula.as_deref(), &mut die).unwrap_or(f.average)
            }
            _ => f.average,
        };
        let id = new_id();
        let mut c = creature_of(&id, &label, &public, &f, hp);
        let visible = visibility != "hidden";
        c.visible = visible;
        c.ever_revealed = visible;
        if let Some(owner_id) = owner.as_ref().and_then(|o| by_creature.get(o)) {
            c.owner_id = Some(CombatantId::new(owner_id.clone()));
            c.initiative_mode = serde_json::from_value(json!(init_mode)).unwrap_or_default();
        }
        let cmd = Command::AddCombatant {
            combatant: Box::new(c),
            roll_initiative: false,
        };
        match apply(&st, &Actor::System, &cmd, &mut die) {
            Ok(o) => st = o.state,
            Err(e) => {
                return Err(ApiError::Invalid(format!(
                    "A creature of the encounter can't join the fight: {e}"
                ))
                .into())
            }
        }
        if let Some(pool) = legendary_pool(&id, f.data.as_ref()) {
            st.legendary_pools.insert(id.clone(), pool);
        }
        by_creature.insert(ec.clone(), id.clone());
        members.push((
            id,
            Member {
                kind: "creature".into(),
                encounter_creature: Some(ec),
                block: Some(f.block),
                name: Some(f.name),
                origin: Some("encounter".into()),
                ..Member::default()
            },
        ));
    }
    // The party's characters, from their roster cards.
    let pcs = sqlx::query(
        "SELECT owner_id, character_id FROM party_members WHERE party_id = $1 ORDER BY joined_at, character_id",
    )
    .bind(&party)
    .fetch_all(&mut *conn)
    .await?;
    for row in pcs {
        let owner: String = row.try_get(0)?;
        let character: String = row.try_get(1)?;
        let hub = Arc::clone(&state.hub);
        let (o, ch) = (owner.clone(), character.clone());
        let card = tokio::task::spawn_blocking(move || hub.cards.get(&o, &ch))
            .await
            .map_err(internal)?
            .unwrap_or_default();
        let hp = card_hp(&card).unwrap_or_default();
        let stats = card.stats.clone().unwrap_or_default();
        let id = new_id();
        let mut c = Combatant::character(
            &id,
            &owner,
            card.name.clone().unwrap_or_else(|| "Character".into()),
            hp,
            stats.armor_class,
            stats.initiative,
        );
        if hp.max == 0 {
            // HP not known yet: the first card refresh brings it.
            c.life = Life::Alive;
        }
        c.speed.walk = u32::try_from(stats.speed).unwrap_or(30);
        let cmd = Command::AddCombatant {
            combatant: Box::new(c),
            roll_initiative: false,
        };
        st = apply(&st, &Actor::System, &cmd, &mut die)
            .map_err(|e| internal(format!("a PC: {e}")))?
            .state;
        members.push((
            id,
            Member {
                kind: "character".into(),
                encounter_creature: None,
                owner: Some(owner),
                character: Some(character),
                block: None,
                ..Member::default()
            },
        ));
    }
    // Starting effects (C19) and encounter rules (C20), when the encounter has them.
    if let Ok(rows) = sqlx::query(
        "SELECT creature_id, name, detail, source_creature_id, duration_turns, charges, concentration, modifiers_json
         FROM encounter_effects WHERE encounter_id = $1 ORDER BY sort, id",
    )
    .bind(encounter)
    .fetch_all(&mut *conn)
    .await
    {
        for row in rows {
            let Some(target) = row.try_get::<String, _>(0).ok().and_then(|t| by_creature.get(&t).cloned()) else {
                continue;
            };
            let mut spec = EffectSpec::new(row.try_get::<String, _>(1).unwrap_or_default(), target);
            spec.detail = row.try_get(2).unwrap_or_default();
            spec.source = row
                .try_get::<Option<String>, _>(3)
                .ok()
                .flatten()
                .and_then(|s| by_creature.get(&s).cloned())
                .map(CombatantId::new);
            if let Some(turns) = row.try_get::<Option<i64>, _>(4).ok().flatten() {
                spec.duration = Duration::SourceTurns {
                    remaining: u32::try_from(turns).unwrap_or(1),
                    at: Boundary::End,
                };
            }
            spec.charges = row
                .try_get::<Option<i64>, _>(5)
                .ok()
                .flatten()
                .and_then(|n| u32::try_from(n).ok());
            spec.concentration = row.try_get::<i64, _>(6).unwrap_or(0) != 0;
            let mods: Value = row
                .try_get::<String, _>(7)
                .ok()
                .and_then(|m| serde_json::from_str(&m).ok())
                .unwrap_or(Value::Null);
            if let Some(ac) = mods.get("ac").and_then(Value::as_i64) {
                spec.modifiers.push(Modifier::Ac(i32::try_from(ac).unwrap_or(0)));
            }
            if let Some(add) = mods.get("speedAdd").and_then(Value::as_i64) {
                spec.modifiers.push(Modifier::SpeedAdd(i32::try_from(add).unwrap_or(0)));
            }
            if let Some(mult) = mods.get("speedMult").and_then(Value::as_u64) {
                spec.modifiers.push(Modifier::SpeedMultiply(u32::try_from(mult).unwrap_or(1)));
            }
            if mods.get("speedZero").and_then(Value::as_bool) == Some(true) {
                spec.modifiers.push(Modifier::SpeedZero);
            }
            let cmd = Command::AddEffect { effect: Box::new(spec) };
            match apply(&st, &Actor::System, &cmd, &mut die) {
                Ok(o) => st = o.state,
                Err(e) => tracing::warn!("combat: a starting effect of {encounter} didn't apply: {e}"),
            }
        }
    }
    if let Ok(rows) = sqlx::query(
        "SELECT id, name, text, creature_id, ends_when, active, flags_json FROM encounter_rules
         WHERE encounter_id = $1 ORDER BY sort, id",
    )
    .bind(encounter)
    .fetch_all(&mut *conn)
    .await
    {
        for row in rows {
            let linked = row
                .try_get::<Option<String>, _>(3)
                .ok()
                .flatten()
                .and_then(|c| by_creature.get(&c).cloned());
            let ends: String = row.try_get(4).unwrap_or_default();
            let flags: Value = row
                .try_get::<String, _>(6)
                .ok()
                .and_then(|f| serde_json::from_str(&f).ok())
                .unwrap_or(Value::Null);
            let flags = flags
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(|f| {
                            let target = by_creature.get(f.get("creatureId")?.as_str()?)?;
                            Some(RuleFlag {
                                target: RuleTarget::Combatant(CombatantId::new(target.clone())),
                                ability: f.get("ability")?.as_str()?.to_string(),
                                while_active: f.get("whileActive").and_then(Value::as_str)
                                    != Some("off"),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let rule = EncounterRule {
                id: new_id(),
                name: row.try_get(1).unwrap_or_default(),
                text_active: row.try_get(2).unwrap_or_default(),
                text_ended: String::new(),
                linked: if ends == "manual" {
                    None
                } else {
                    linked.map(CombatantId::new)
                },
                active: row.try_get::<i64, _>(5).unwrap_or(1) != 0,
                flags,
            };
            match apply(&st, &Actor::System, &Command::SetRule { rule }, &mut die) {
                Ok(o) => st = o.state,
                Err(e) => tracing::warn!("combat: a rule of {encounter} didn't apply: {e}"),
            }
        }
    }
    drop(conn);
    let request = body.request_id.clone();
    let id = insert_combat(
        state,
        user,
        NewCombat {
            party,
            encounter: Some(encounter.to_string()),
            state: st,
            die,
            members,
            scope: scope.clone(),
            request: request.clone(),
        },
    )
    .await;
    match id {
        Ok(id) => view_now(state, user, &id).await,
        Err(Fail::Retry) => {
            // The same request raced itself: answer what the first one made.
            let done = done_before(state, &scope, &user.id, &request)
                .await
                .ok()
                .flatten();
            match done {
                Some(id) => view_now(state, user, &id).await,
                None => {
                    Err(ApiError::Conflict("This party already has a live combat.".into()).into())
                }
            }
        }
        Err(e) => Err(e),
    }
}

/// `GET /api/parties/{party}/combat`: the party's live combat, for its DM and players.
pub async fn party_combat(
    State(state): State<AppState>,
    user: User,
    Path(party): Path<String>,
) -> Response {
    let run = async {
        if !id_ok(&party) {
            return Err(Fail::Api(ApiError::NotFound(
                "There is no such party, invite or character here.".into(),
            )));
        }
        let mut conn = state.db.pool().acquire().await?;
        if role(&mut conn, &user.id, &party).await?.is_none() {
            return Err(Fail::Api(ApiError::NotFound(
                "There is no such party, invite or character here.".into(),
            )));
        }
        let id: Option<String> =
            sqlx::query("SELECT id FROM combats WHERE party_id = $1 AND status = 'live'")
                .bind(&party)
                .fetch_optional(&mut *conn)
                .await?
                .map(|r| r.try_get(0))
                .transpose()?;
        Ok(json!({ "combat": id }))
    };
    match run.await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/combats/{combat}`.
pub async fn read(
    State(state): State<AppState>,
    user: User,
    Path(combat): Path<String>,
) -> Response {
    match view_now(&state, &user, &combat).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct LogQuery {
    before: Option<i64>,
    limit: Option<i64>,
}

/// `GET /api/combats/{combat}/log` (DM): the DM-only log, newest first.
pub async fn log(
    State(state): State<AppState>,
    user: User,
    Path(combat): Path<String>,
    Query(q): Query<LogQuery>,
) -> Response {
    let run = async {
        if !id_ok(&combat) {
            return Err(Fail::Api(not_found()));
        }
        let mut conn = state.db.pool().acquire().await?;
        let party: Option<String> = sqlx::query("SELECT party_id FROM combats WHERE id = $1")
            .bind(&combat)
            .fetch_optional(&mut *conn)
            .await?
            .map(|r| r.try_get(0))
            .transpose()?;
        let Some(party) = party else {
            return Err(Fail::Api(not_found()));
        };
        if role(&mut conn, &user.id, &party).await? != Some(Role::Dm) {
            return Err(Fail::Api(not_found()));
        }
        let limit = q.limit.unwrap_or(100).clamp(1, 200);
        let rows = sqlx::query(
            "SELECT seq, at, author, event_json FROM combat_log WHERE combat_id = $1 AND seq < $2
             ORDER BY seq DESC, n DESC LIMIT $3",
        )
        .bind(&combat)
        .bind(q.before.unwrap_or(i64::MAX))
        .bind(limit + 1)
        .fetch_all(&mut *conn)
        .await?;
        let more = i64::try_from(rows.len()).unwrap_or(0) > limit;
        let mut entries = Vec::new();
        for row in rows.iter().take(usize::try_from(limit).unwrap_or(100)) {
            let event: String = row.try_get(3)?;
            entries.push(json!({
                "seq": row.try_get::<i64, _>(0)?,
                "at": row.try_get::<i64, _>(1)?,
                "author": row.try_get::<String, _>(2)?,
                "event": serde_json::from_str::<Value>(&event).map_err(internal)?,
            }));
        }
        Ok(json!({ "entries": entries, "more": more }))
    };
    match run.await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Actions only the server issues, or that need the server's own handling.
const NOT_OFFERED: [&str; 2] = ["add_combatant", "sync_player_hp"];
/// The server's own actions only the DM may take. What else a player may do is the engine's call (their PC and the
/// creatures it controls), checked again by the server for membership.
const DM_ACTIONS: [&str; 5] = ["undo", "end", "add_creature", "remove_creature", "export"];

fn engine_tag(action: &str) -> &str {
    match action {
        "prev_turn" => "previous_turn",
        "condition_add" => "add_condition",
        "condition_remove" => "remove_condition",
        "remove_creature" => "remove_combatant",
        other => other,
    }
}

/// An action name: lower-case letters and `_`.
fn action_ok(action: &str) -> bool {
    !action.is_empty()
        && action.len() <= 40
        && action.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

/// `POST /api/combats/{combat}/{action}`.
pub async fn act(
    State(state): State<AppState>,
    user: User,
    Path((combat, action)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    match act_inner(&state, &user, &combat, &action, &body).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

#[allow(clippy::too_many_lines)]
async fn act_inner(
    state: &AppState,
    user: &User,
    combat: &str,
    action: &str,
    body: &[u8],
) -> Res<Value> {
    if !id_ok(combat) || !action_ok(action) {
        return Err(not_found().into());
    }
    if !take_write(&user.id) {
        return Err(ApiError::TooMany.into());
    }
    let who = {
        let mut conn = state.db.pool().acquire().await?;
        let party: Option<String> = sqlx::query("SELECT party_id FROM combats WHERE id = $1")
            .bind(combat)
            .fetch_optional(&mut *conn)
            .await?
            .map(|r| r.try_get(0))
            .transpose()?;
        match party {
            Some(p) => role(&mut conn, &user.id, &p).await?,
            None => None,
        }
    };
    let Some(who) = who else {
        return Err(not_found().into());
    };
    // Players: one 404 for anything they may not do, whatever the body.
    let player = who == Role::Player;
    let bad = |message: &str| -> Fail {
        if player {
            Fail::Api(not_found())
        } else {
            Fail::Api(ApiError::Invalid(message.to_string()))
        }
    };
    if player && DM_ACTIONS.contains(&action) {
        return Err(not_found().into());
    }
    if action == "export" {
        return export(state, user, combat).await;
    }
    let body: Value = serde_json::from_slice(body).map_err(|_| bad("The body isn't JSON."))?;
    let Some(body) = body.as_object() else {
        return Err(bad("The body must be an object."));
    };
    let request = body
        .get("requestId")
        .and_then(Value::as_str)
        .filter(|r| request_ok(r))
        .ok_or_else(|| bad("Send a requestId."))?
        .to_string();
    let expect = if player {
        None
    } else {
        match body.get("expectSeq") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_i64()
                    .ok_or_else(|| bad("expectSeq must be a number."))?,
            ),
        }
    };
    let args = match body.get("args") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(bad("args must be an object.")),
    };
    let mut summoned = None;
    let (op, own) = match action {
        "undo" => (
            Op::Undo {
                force: args.get("force").and_then(Value::as_bool).unwrap_or(false),
            },
            None,
        ),
        "end" => (Op::End, None),
        "add_creature" => {
            let source = match (
                args.get("copyOf").and_then(Value::as_str),
                args.get("encounterCreatureId").and_then(Value::as_str),
            ) {
                (Some(id), None) if id_ok(id) => AddSource::Copy(id.to_string()),
                (None, Some(id)) if id_ok(id) => AddSource::Encounter(id.to_string()),
                _ => return Err(bad("Send copyOf or encounterCreatureId.")),
            };
            let count = match args.get("count") {
                None => 1,
                Some(v) => v
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|n| (1..=MAX_COPIES).contains(n))
                    .ok_or_else(|| bad("count is 1 to 10."))?,
            };
            (
                Op::Add {
                    source,
                    count,
                    roll: args
                        .get("rollInitiative")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                    hidden: args.get("hidden").and_then(Value::as_bool),
                },
                None,
            )
        }
        "resolve_prompt" => {
            let prompt = args
                .get("promptId")
                .and_then(Value::as_str)
                .filter(|p| id_ok(p))
                .ok_or_else(|| bad("Send a promptId."))?
                .to_string();
            let applied = args
                .get("applied")
                .and_then(Value::as_bool)
                .ok_or_else(|| bad("Send applied."))?;
            let hp = match args.get("hp") {
                None | Some(Value::Null) => None,
                Some(v) => Some(
                    serde_json::from_value::<HpIn>(v.clone())
                        .map_err(|_| bad("hp isn't valid."))?,
                ),
            };
            (
                Op::Resolve {
                    prompt,
                    applied,
                    hp,
                },
                None,
            )
        }
        _ => {
            let tag = engine_tag(action);
            if NOT_OFFERED.contains(&tag) {
                return Err(not_found().into());
            }
            let mut cmd = args.clone();
            cmd.insert("command".into(), json!(tag));
            // A summoned creature is built by the server, with a random id, whatever the client sent.
            if tag == "summon" {
                let (combatant, member) =
                    summon_combatant(state, user, combat, &args, player).await?;
                cmd.insert("combatant".into(), combatant);
                for key in ["companion", "elementId", "copyOf", "name", "publicName"] {
                    cmd.remove(key);
                }
                summoned = Some(member);
            }
            let command: Command = serde_json::from_value(Value::Object(cmd)).map_err(|e| {
                if player {
                    Fail::Api(not_found())
                } else {
                    Fail::Api(ApiError::Invalid(format!("Not a valid {action}: {e}")))
                }
            })?;
            (Op::Engine(Box::new(command)), None)
        }
    };
    let w = Write {
        account: user.id.clone(),
        writer: if player { Writer::Player } else { Writer::Dm },
        request: Some(request),
        expect,
        op,
        own,
        summoned,
    };
    let done = {
        let _lock = combat_lock(combat).await;
        run_write(state, combat, &w).await?
    };
    if done.replayed {
        // The answer the first try got (D44: a retry never applies twice and sees what the write did).
        let kept: Option<String> = sqlx::query(
            "SELECT response_json FROM combat_requests WHERE scope = $1 AND account_id = $2 AND request_id = $3",
        )
        .bind(combat)
        .bind(&user.id)
        .bind(&w.request)
        .fetch_optional(state.db.pool())
        .await?
        .and_then(|r| r.try_get::<Option<String>, _>(0).ok().flatten());
        let mut view = match kept.and_then(|k| serde_json::from_str::<Value>(&k).ok()) {
            Some(v) => v,
            None => view_now(state, user, combat).await?,
        };
        view["replayed"] = json!(true);
        return Ok(view);
    }
    announce(state, combat, &done).await;
    let view = view_now(state, user, combat).await?;
    sqlx::query(
        "UPDATE combat_requests SET response_json = $1 WHERE scope = $2 AND account_id = $3 AND request_id = $4",
    )
    .bind(view.to_string())
    .bind(combat)
    .bind(&user.id)
    .bind(&w.request)
    .execute(state.db.pool())
    .await?;
    Ok(view)
}

/// The combatant a `summon` brings in, built by the server with a random id, and who it is:
/// - `{ owner, companion: <key> }`: a companion on the owning PC's sheet (from its roster card);
/// - `{ owner, elementId }`: a Monster or Aurora Companion element of the caller's own content;
/// - `{ owner, copyOf }` (DM): a fresh copy of a creature in this fight;
/// - `{ owner, combatant }`: an engine-shaped block as the client typed it.
#[allow(clippy::too_many_lines)]
async fn summon_combatant(
    state: &AppState,
    user: &User,
    combat: &str,
    args: &Map<String, Value>,
    player: bool,
) -> Res<(Value, Member)> {
    let bad = |message: &str| -> Fail {
        if player {
            Fail::Api(not_found())
        } else {
            Fail::Api(ApiError::Invalid(message.to_string()))
        }
    };
    let owner = args
        .get("owner")
        .and_then(Value::as_str)
        .filter(|o| id_ok(o))
        .ok_or_else(|| bad("Send the owner."))?
        .to_string();
    let id = new_id();
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(|t| t.trim().chars().take(80).collect::<String>())
            .filter(|t| !t.is_empty())
    };
    if let Some(key) = args.get("companion").and_then(Value::as_str) {
        let mut conn = state.db.pool().acquire().await?;
        let roster = roster(&mut conn, combat).await?;
        drop(conn);
        let pc = roster
            .get(&owner)
            .filter(|m| m.kind == "character")
            .ok_or_else(|| bad("Companions belong to a character."))?;
        let card = state
            .hub
            .cards
            .get(
                pc.owner.as_deref().unwrap_or_default(),
                pc.character.as_deref().unwrap_or_default(),
            )
            .ok_or_else(|| bad("That character has no such companion."))?;
        let comp = card
            .companions
            .iter()
            .find(|c| c.key == key)
            .ok_or_else(|| bad("That character has no such companion."))?;
        let mut x = Combatant::creature(&id, &comp.name, comp.hp.max(0), comp.ac, comp.initiative);
        x.public_name.clone_from(&comp.name);
        x.speed.walk = u32::try_from(comp.speed).unwrap_or(0);
        let member = Member {
            kind: "creature".into(),
            block: serde_json::to_value(comp).ok(),
            name: Some(comp.name.clone()),
            origin: Some(format!("companion:{owner}:{key}")),
            ..Member::default()
        };
        return Ok((serde_json::to_value(x).map_err(internal)?, member));
    }
    if let Some(element) = args.get("elementId").and_then(Value::as_str) {
        let _job = job(&user.id).await?;
        let found = crate::encounters::copy_creature(state, &user.id, element)
            .await
            .map_err(|e| {
                if player {
                    Fail::Api(not_found())
                } else {
                    Fail::Api(e)
                }
            })?;
        let (snap, _pack) = found.ok_or_else(|| bad("That creature isn't in your content."))?;
        let f = fighter_from_snapshot(&snap, &party_encounter::overrides::Overrides::default());
        let label = text("name").unwrap_or_else(|| f.name.clone());
        let public = text("publicName").unwrap_or_else(|| label.clone());
        let x = creature_of(&id, &label, &public, &f, f.average);
        let member = Member {
            kind: "creature".into(),
            block: Some(f.block.clone()),
            name: Some(f.name.clone()),
            origin: Some("element".into()),
            ..Member::default()
        };
        return Ok((serde_json::to_value(x).map_err(internal)?, member));
    }
    if let Some(source) = args.get("copyOf").and_then(Value::as_str) {
        if player {
            return Err(not_found().into());
        }
        let mut conn = state.db.pool().acquire().await?;
        let roster = roster(&mut conn, combat).await?;
        let loaded = load(&mut conn, combat).await?.ok_or_else(not_found)?;
        drop(conn);
        let m = roster
            .get(source)
            .filter(|m| m.kind == "creature")
            .ok_or_else(|| bad("There is no such creature in this combat."))?;
        let x = loaded
            .state
            .combatant(&CombatantId::new(source))
            .ok_or_else(|| bad("There is no such creature in this combat."))?;
        let f = Fighter {
            block: m.block.clone().unwrap_or(Value::Null),
            data: m
                .block
                .as_ref()
                .and_then(|b| serde_json::from_value(b.clone()).ok()),
            name: m.name.clone().unwrap_or_default(),
            ac: x.ac,
            average: x.hp.max.max(1),
            formula: None,
        };
        let label = text("name").unwrap_or_else(|| x.label.clone());
        let mut y = creature_of(&id, &label, &x.public_name, &f, x.hp.max.max(1));
        y.ac = x.ac;
        y.initiative_bonus = x.initiative_bonus;
        let mut member = m.clone();
        member.origin = Some("copy".into());
        return Ok((serde_json::to_value(y).map_err(internal)?, member));
    }
    let Some(Value::Object(mut block)) = args.get("combatant").cloned() else {
        return Err(bad(
            "Send a companion, an elementId, copyOf or a combatant.",
        ));
    };
    block.insert("id".into(), json!(id));
    let name = block
        .get("label")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok((
        Value::Object(block),
        Member {
            kind: "creature".into(),
            name,
            origin: Some("summon".into()),
            ..Member::default()
        },
    ))
}

/// The export (D45): the engine's file plus who each combatant is, for the DM.
async fn export(state: &AppState, user: &User, combat: &str) -> Res<Value> {
    let _job = job(&user.id).await?;
    let mut conn = state.db.pool().acquire().await?;
    let Some(c) = load(&mut conn, combat).await? else {
        return Err(not_found().into());
    };
    if role(&mut conn, &user.id, &c.party).await? != Some(Role::Dm) {
        return Err(not_found().into());
    }
    let rows = sqlx::query(
        "SELECT seq, author, event_json FROM combat_log WHERE combat_id = $1 ORDER BY seq, n",
    )
    .bind(combat)
    .fetch_all(&mut *conn)
    .await?;
    let mut log = Vec::new();
    for row in rows {
        let event: String = row.try_get(2)?;
        log.push(party_combat::combat::LogEntry {
            seq: u64::try_from(row.try_get::<i64, _>(0)?).unwrap_or(0),
            author: match row.try_get::<String, _>(1)?.as_str() {
                "player" => Author::Player,
                "system" => Author::System,
                _ => Author::Dm,
            },
            event: serde_json::from_str(&event).map_err(internal)?,
        });
    }
    let roster = roster(&mut conn, combat).await?;
    let engine = Combat {
        state: c.state,
        die: c.die,
        journal: VecDeque::new(),
        log,
        seq: u64::try_from(c.seq).unwrap_or(0),
    };
    let mut file: Value = serde_json::from_str(&export_json(&engine)).map_err(internal)?;
    let mut creatures = Map::new();
    let mut characters = Map::new();
    for (id, m) in roster {
        if m.kind == "character" {
            characters.insert(
                id,
                json!({ "ownerId": m.owner, "characterId": m.character }),
            );
        } else {
            creatures.insert(
                id,
                json!({ "encounterCreatureId": m.encounter_creature, "statBlock": m.block, "name": m.name, "origin": m.origin }),
            );
        }
    }
    file["server"] = json!({ "creatures": creatures, "characters": characters });
    Ok(file)
}

/// Every string in `value` equal to an old id becomes its new one.
fn remap(value: &mut Value, ids: &HashMap<String, String>) {
    match value {
        Value::String(s) => {
            if let Some(new) = ids.get(s.as_str()) {
                s.clone_from(new);
            }
        }
        Value::Array(list) => list.iter_mut().for_each(|v| remap(v, ids)),
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                if let Some(mut v) = map.remove(&key) {
                    remap(&mut v, ids);
                    let key = ids.get(&key).cloned().unwrap_or(key);
                    map.insert(key, v);
                }
            }
        }
        _ => {}
    }
}

/// `POST /api/parties/{party}/combats/import` (DM, D45): always a new combat.
pub async fn import(
    State(state): State<AppState>,
    user: User,
    Path(party): Path<String>,
    body: Bytes,
) -> Response {
    match import_inner(&state, &user, &party, &body).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn import_inner(state: &AppState, user: &User, party: &str, body: &[u8]) -> Res<Value> {
    if !id_ok(party) {
        return Err(not_found().into());
    }
    {
        let mut conn = state.db.pool().acquire().await?;
        if role(&mut conn, &user.id, party).await? != Some(Role::Dm) {
            return Err(not_found().into());
        }
    }
    if body.len() > MAX_IMPORT + 4096 {
        return Err(ApiError::Invalid("The file is too large.".into()).into());
    }
    let body: Value = serde_json::from_slice(body)
        .map_err(|_| ApiError::Invalid("The body isn't JSON.".into()))?;
    let request = body
        .get("requestId")
        .and_then(Value::as_str)
        .filter(|r| request_ok(r))
        .ok_or_else(|| ApiError::Invalid("Send a requestId.".into()))?
        .to_string();
    let scope = format!("import:{party}");
    if let Some(done) = done_before(state, &scope, &user.id, &request).await? {
        let mut view = view_now(state, user, &done).await?;
        view["replayed"] = json!(true);
        return Ok(view);
    }
    if !take_write(&user.id) {
        return Err(ApiError::TooMany.into());
    }
    let _job = job(&user.id).await?;
    let mut file = match body.get("file") {
        Some(Value::String(text)) => serde_json::from_str::<Value>(text)
            .map_err(|_| ApiError::Invalid("Can't import: not JSON".into()))?,
        Some(v @ Value::Object(_)) => v.clone(),
        _ => return Err(ApiError::Invalid("Send the exported file.".into()).into()),
    };
    let server = file
        .as_object_mut()
        .and_then(|m| m.remove("server"))
        .unwrap_or(Value::Null);
    let text = file.to_string();
    let imported = import_json(&text, seed()).map_err(|e| ApiError::Invalid(e.to_string()))?;
    let mut st = imported.state;
    // New random ids for everyone, references included.
    let ids: HashMap<String, String> = st
        .combatants
        .iter()
        .map(|c| (c.id.0.clone(), new_id()))
        .collect();
    let mut value = serde_json::to_value(&st).map_err(internal)?;
    remap(&mut value, &ids);
    st = serde_json::from_value(value).map_err(internal)?;
    let mut conn = state.db.pool().acquire().await?;
    let members = members_of(&mut conn, party).await?;
    drop(conn);
    let mut roster = Vec::new();
    let back: HashMap<&String, &String> = ids.iter().map(|(old, new)| (new, old)).collect();
    for c in &mut st.combatants {
        let old = back.get(&c.id.0).map_or("", |s| s.as_str());
        if c.is_character() {
            let info = server.get("characters").and_then(|m| m.get(old));
            let owner = c.owner().unwrap_or_default().to_string();
            let character = info
                .and_then(|i| i.get("characterId"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let member = character
                .as_ref()
                .is_some_and(|ch| members.contains(&(owner.clone(), ch.clone())));
            if !member {
                c.removed = true;
            }
            roster.push((
                c.id.0.clone(),
                Member {
                    kind: "character".into(),
                    encounter_creature: None,
                    owner: Some(owner),
                    character,
                    block: None,
                    ..Member::default()
                },
            ));
        } else {
            let info = server.get("creatures").and_then(|m| m.get(old));
            roster.push((
                c.id.0.clone(),
                Member {
                    kind: "creature".into(),
                    encounter_creature: None,
                    owner: None,
                    character: None,
                    block: info
                        .and_then(|i| i.get("statBlock"))
                        .filter(|b| b.is_object())
                        .cloned()
                        .map(without_xp),
                    name: info
                        .and_then(|i| i.get("name"))
                        .and_then(Value::as_str)
                        .map(|n| n.chars().take(200).collect()),
                    // A companion's origin names the PC's old id; the import keeps only its kind.
                    origin: info
                        .and_then(|i| i.get("origin"))
                        .and_then(Value::as_str)
                        .and_then(|o| o.split(':').next())
                        .filter(|o| {
                            ["encounter", "copy", "element", "companion", "summon"].contains(o)
                        })
                        .map(str::to_string),
                },
            ));
        }
    }
    let id = insert_combat(
        state,
        user,
        NewCombat {
            party: party.to_string(),
            encounter: None,
            state: st,
            die: imported.die,
            members: roster,
            scope,
            request,
        },
    )
    .await?;
    view_now(state, user, &id).await
}

/// Tests: a DM's damage write without the combat's in-process lock, so two writers really race on `seq`.
#[cfg(test)]
pub(crate) async fn test_write_damage(
    state: &AppState,
    combat: &str,
    account: &str,
    target: &str,
    amount: i32,
) -> Result<i64, String> {
    let command = Command::Damage {
        target: CombatantId::new(target),
        parts: vec![party_combat::DamagePart::typed(amount, None)],
        magical: false,
        apply_qualified: false,
        critical_hit: false,
    };
    let w = Write {
        account: account.to_string(),
        writer: Writer::Dm,
        request: Some(new_id()),
        expect: None,
        op: Op::Engine(Box::new(command)),
        own: None,
        summoned: None,
    };
    match run_write(state, combat, &w).await {
        Ok(done) => Ok(done.seq),
        Err(e) => Err(format!("{e:?}")),
    }
}

/// Tests: forget an account's recent writes, so a suite that sends hundreds of requests isn't stopped by the limit.
#[cfg(test)]
pub(crate) fn test_forget_writes(account: &str) {
    guard(&WINDOWS).remove(account);
}
