//! Parties: a DM's table of other players' characters. The API is `party/docs/parties-api.md`.
//!
//! A party is database rows only. Characters never leave their owner's folder (`data/users/<owner>/characters/`):
//! a party names them by `(owner, character id)`, and the roster reads the owner's **saved file** for its card,
//! without opening a workspace or a session, so a DM never sees a player's unsaved edits and nobody but the owner
//! writes the file.
//!
//! Authorization is checked here on every request, against the database:
//! * the DM capability is a row in `account_roles` the account writes itself (D2); it is needed to create parties;
//! * the DM of a party P is `parties.dm_id`; a player in P owns a character with a `party_members` row for P;
//! * anything a caller may not see answers the same 404 as anything that doesn't exist, never 403;
//! * the owner of a membership always comes from the signed-in account, and a character id is checked against a
//!   strict pattern before it is joined into a path.
//!
//! Nothing here goes to the admin page's activity log (D40): the server log gets ids and kinds only.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use axum::extract::{Path, State};
use axum::Json;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;

use crate::auth::{token_hash, ClientIp, User};
use crate::db::now;
use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// The role row that makes an account DM-capable.
const DM: &str = "dm";
/// Parties one account may run.
pub const MAX_PARTIES: i64 = 20;
/// Invites one account may send a day, over all its parties; and from one client address.
const INVITES_PER_DAY: usize = 50;
const INVITES_PER_DAY_PER_IP: usize = 150;
/// Wrong join codes one account may try, and code previews one client address may ask for, in ten minutes.
const CODE_TRIES: usize = 20;
const PREVIEWS: usize = 30;
const TEN_MINUTES: i64 = 10 * 60;
const DAY: i64 = 24 * 60 * 60;
/// Longest an invite lasts, in days, and most uses of a join code.
const MAX_DAYS: i64 = 7;
const MAX_USES: i64 = 50;
/// Join codes: 80 random bits in 16 characters of Crockford base32.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_LEN: usize = 16;
const RULESETS: [&str; 2] = ["dnd2024", "dnd2014"];

/// The one answer for anything the caller may not see or that doesn't exist.
fn not_found() -> ApiError {
    ApiError::NotFound("There is no such party, invite or character here.".into())
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

/// The party routes, unless the server config switched parties off (then they are unknown routes: 404).
pub fn routes() -> axum::Router<AppState> {
    routes_if(crate::config::features().parties)
}

/// The party routes when `on`; none otherwise.
pub(crate) fn routes_if(on: bool) -> axum::Router<AppState> {
    use axum::routing::{delete, get, post};
    if !on {
        return axum::Router::new();
    }
    axum::Router::new()
        .route("/api/me/roles", get(roles))
        .route("/api/me/roles/dm", post(dm_on).delete(dm_off))
        .route("/api/parties", get(list).post(create))
        .route(
            "/api/parties/{party}",
            get(detail).patch(update).delete(remove),
        )
        .route("/api/parties/{party}/roster", get(roster))
        .route("/api/parties/{party}/invites", post(invite))
        .route("/api/parties/{party}/invites/{invite}", delete(revoke))
        .route("/api/parties/{party}/members", post(add_own))
        .route(
            "/api/parties/{party}/members/{owner}/{character}",
            delete(remove_member),
        )
        .route(
            "/api/parties/{party}/offers/{offer}/accept",
            post(accept_offer),
        )
        .route("/api/parties/{party}/offers/{offer}", delete(decline_offer))
        .route("/api/invites", get(my_invites))
        .route("/api/invites/{invite}/accept", post(accept))
        .route(
            "/api/invites/{invite}/offer",
            post(offer).delete(withdraw_offer),
        )
        .route("/api/invites/{invite}/decline", post(decline))
        .route("/api/join", post(join))
        .route("/api/join/preview", post(preview))
}

// Limits.

/// Attempts per key in a sliding window, for this process.
static WINDOWS: LazyLock<Mutex<HashMap<String, Vec<i64>>>> = LazyLock::new(Mutex::default);

/// Count an attempt under `key`; false (and not counted) when `allowed` were made in the last `window` seconds.
fn take(key: &str, allowed: usize, window: i64) -> bool {
    let Ok(mut map) = WINDOWS.lock() else {
        return false;
    };
    let at = now();
    map.retain(|_, times| {
        times.retain(|t| *t > at - DAY.max(window));
        !times.is_empty()
    });
    let times = map.entry(key.to_string()).or_default();
    if times.iter().filter(|t| **t > at - window).count() >= allowed {
        return false;
    }
    times.push(at);
    true
}

/// True while `key` has had fewer than `allowed` attempts in `window` seconds (counts nothing).
fn open(key: &str, allowed: usize, window: i64) -> bool {
    let at = now();
    WINDOWS.lock().is_ok_and(|map| {
        map.get(key)
            .is_none_or(|times| times.iter().filter(|t| **t > at - window).count() < allowed)
    })
}

// Checks of what clients send.

/// A character id as upstream makes them: `c_` and 32 lower-case hex digits. Nothing else is ever joined into a path.
/// An id from a path, before it goes near the database: the server's ids are UUIDs, and anything else (a NUL byte
/// that PostgreSQL refuses, say) is the same 404 as an id nobody has.
fn id_arg(id: &str) -> ApiResult<()> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(not_found())
    }
}

pub(crate) fn valid_character_id(id: &str) -> bool {
    id.len() == 34
        && id.starts_with("c_")
        && id[2..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn character_arg(id: Option<&str>) -> ApiResult<String> {
    id.filter(|id| valid_character_id(id))
        .map(str::to_string)
        .ok_or_else(|| ApiError::Invalid("Choose one of your characters.".into()))
}

fn party_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if (1..=60).contains(&name.chars().count()) && !name.chars().any(char::is_control) {
        Ok(name.to_string())
    } else {
        Err(ApiError::Invalid(
            "Party names are 1 to 60 characters.".into(),
        ))
    }
}

fn ruleset_arg(ruleset: Option<&str>) -> ApiResult<Option<String>> {
    match ruleset {
        None => Ok(None),
        Some(r) if RULESETS.contains(&r) => Ok(Some(r.to_string())),
        Some(_) => Err(ApiError::Invalid(
            "The ruleset is dnd2024 or dnd2014.".into(),
        )),
    }
}

/// A join code as typed: case, spaces and dashes don't matter, and O, I and L read as 0, 1 and 1.
pub(crate) fn normal_code(code: &str) -> Option<String> {
    let code: String = code
        .chars()
        .filter(|c| !matches!(c, '-' | ' ' | '\t'))
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    (code.len() == CODE_LEN && code.bytes().all(|b| CODE_ALPHABET.contains(&b))).then_some(code)
}

fn new_code() -> String {
    let mut bytes = [0u8; 10];
    rand::rng().fill_bytes(&mut bytes);
    let mut bits = 0u128;
    for b in bytes {
        bits = (bits << 8) | u128::from(b);
    }
    (0..CODE_LEN)
        .rev()
        .map(|i| char::from(CODE_ALPHABET[usize::try_from((bits >> (i * 5)) & 31).unwrap_or(0)]))
        .collect()
}

fn grouped(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

// Roster cards, from the owner's saved file.

/// What the roster shows of a character, read from its saved file.
#[derive(Debug, Clone)]
struct Card {
    name: Option<String>,
    level: Option<u32>,
    ruleset: Option<Value>,
    state: &'static str,
}

fn character_file(state: &AppState, owner: &str, character: &str) -> PathBuf {
    state
        .workspaces
        .dir(owner)
        .join("characters")
        .join(format!("{character}.json"))
}

fn read_card(path: &std::path::Path) -> Card {
    let missing = Card {
        name: None,
        level: None,
        ruleset: None,
        state: "missing",
    };
    // A regular file only: no links out of the folder.
    match path.symlink_metadata() {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => {
            return Card {
                state: "unreadable",
                ..missing
            }
        }
        Err(_) => return missing,
    }
    match borealis_character::Character::load(path) {
        Ok(c) => Card {
            name: Some(c.meta.name.clone()),
            level: Some(c.build.level()),
            ruleset: serde_json::to_value(c.ruleset()).ok(),
            state: "ok",
        },
        Err(_) => Card {
            state: "unreadable",
            ..missing
        },
    }
}

/// Cards for `(owner, character)` pairs, off the async threads.
async fn cards(state: &AppState, pairs: &[(String, String)]) -> ApiResult<Vec<Card>> {
    let files: Vec<PathBuf> = pairs
        .iter()
        .map(|(owner, character)| character_file(state, owner, character))
        .collect();
    tokio::task::spawn_blocking(move || files.iter().map(|f| read_card(f)).collect())
        .await
        .map_err(internal)
}

/// The signed-in account's own saved character, or a 400 that says to pick one of theirs.
async fn own_character(state: &AppState, user: &User, id: Option<&str>) -> ApiResult<String> {
    let id = character_arg(id)?;
    let card = cards(state, &[(user.id.clone(), id.clone())]).await?;
    if card.first().is_some_and(|c| c.state == "ok") {
        Ok(id)
    } else {
        Err(ApiError::Invalid("Choose one of your characters.".into()))
    }
}

// Rows.

#[derive(Debug, Clone)]
struct Party {
    id: String,
    name: String,
    dm_id: String,
    dm_name: String,
    ruleset: String,
    created_at: i64,
    updated_at: i64,
}

const PARTY_COLUMNS: &str =
    "p.id, p.name, p.dm_id, u.username, p.ruleset, p.created_at, p.updated_at, \
     (SELECT COUNT(*) FROM party_members c WHERE c.party_id = p.id)";

fn party_from(row: &sqlx::any::AnyRow) -> sqlx::Result<(Party, i64)> {
    Ok((
        Party {
            id: row.try_get(0)?,
            name: row.try_get(1)?,
            dm_id: row.try_get(2)?,
            dm_name: row.try_get(3)?,
            ruleset: row.try_get(4)?,
            created_at: row.try_get(5)?,
            updated_at: row.try_get(6)?,
        },
        row.try_get(7)?,
    ))
}

/// How the caller sees a party.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Dm,
    Player,
}

/// The party, with its member count, when the caller is its DM or a player in it; else 404.
async fn viewed(state: &AppState, user: &User, party: &str) -> ApiResult<(Party, i64, Role)> {
    id_arg(party)?;
    let row = sqlx::query(&format!(
        "SELECT {PARTY_COLUMNS} FROM parties p JOIN users u ON u.id = p.dm_id
         WHERE p.id = $1 AND (p.dm_id = $2
             OR EXISTS (SELECT 1 FROM party_members m WHERE m.party_id = p.id AND m.owner_id = $2))"
    ))
    .bind(party)
    .bind(&user.id)
    .fetch_optional(state.db.pool())
    .await?
    .ok_or_else(not_found)?;
    let (party, count) = party_from(&row)?;
    let role = if party.dm_id == user.id {
        Role::Dm
    } else {
        Role::Player
    };
    Ok((party, count, role))
}

/// The party when the caller is its DM; 404 otherwise, players included.
async fn run_by(state: &AppState, user: &User, party: &str) -> ApiResult<(Party, i64)> {
    match viewed(state, user, party).await? {
        (party, count, Role::Dm) => Ok((party, count)),
        _ => Err(not_found()),
    }
}

async fn is_dm_capable(state: &AppState, user: &str) -> ApiResult<bool> {
    Ok(
        sqlx::query("SELECT 1 FROM account_roles WHERE user_id = $1 AND role = $2")
            .bind(user)
            .bind(DM)
            .fetch_optional(state.db.pool())
            .await?
            .is_some(),
    )
}

/// The caller's characters in each party, by party id.
async fn my_characters(state: &AppState, user: &str) -> ApiResult<BTreeMap<String, Vec<String>>> {
    let rows = sqlx::query(
        "SELECT party_id, character_id FROM party_members WHERE owner_id = $1 ORDER BY joined_at, character_id",
    )
    .bind(user)
    .fetch_all(state.db.pool())
    .await?;
    let mut mine: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows {
        mine.entry(row.try_get(0)?)
            .or_default()
            .push(row.try_get(1)?);
    }
    Ok(mine)
}

fn summary(party: &Party, count: i64, role: Role, mine: &[String]) -> Value {
    json!({
        "id": party.id,
        "name": party.name,
        "ruleset": party.ruleset,
        "role": if role == Role::Dm { "dm" } else { "player" },
        "dmName": party.dm_name,
        "memberCount": count,
        "myCharacters": mine,
        "createdAt": party.created_at,
        "updatedAt": party.updated_at,
    })
}

fn card_json(out: &mut serde_json::Map<String, Value>, card: &Card) {
    out.insert("name".into(), json!(card.name));
    out.insert("level".into(), json!(card.level));
    out.insert(
        "ruleset".into(),
        card.ruleset.clone().unwrap_or(Value::Null),
    );
    out.insert("state".into(), json!(card.state));
}

/// The roster as `user` may see it.
async fn members(
    state: &AppState,
    user: &User,
    party: &Party,
    role: Role,
) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT m.owner_id, u.username, m.character_id, m.joined_at FROM party_members m
         JOIN users u ON u.id = m.owner_id WHERE m.party_id = $1 ORDER BY m.joined_at, m.character_id",
    )
    .bind(&party.id)
    .fetch_all(state.db.pool())
    .await?;
    let mut found = Vec::new();
    for row in &rows {
        let owner: String = row.try_get(0)?;
        let owner_name: String = row.try_get(1)?;
        let character: String = row.try_get(2)?;
        let joined: i64 = row.try_get(3)?;
        found.push((owner, owner_name, character, joined));
    }
    let pairs: Vec<(String, String)> = found
        .iter()
        .filter(|(_, _, c, _)| valid_character_id(c))
        .map(|(o, _, c, _)| (o.clone(), c.clone()))
        .collect();
    let mut read = cards(state, &pairs).await?.into_iter();
    let mut out = Vec::new();
    for (owner, owner_name, character, joined) in found {
        let card = if valid_character_id(&character) {
            read.next().unwrap_or(Card {
                name: None,
                level: None,
                ruleset: None,
                state: "unreadable",
            })
        } else {
            Card {
                name: None,
                level: None,
                ruleset: None,
                state: "unreadable",
            }
        };
        let mine = owner == user.id;
        let mut member = serde_json::Map::new();
        member.insert("characterId".into(), json!(character));
        member.insert("ownerName".into(), json!(owner_name));
        if mine || role == Role::Dm {
            member.insert("ownerId".into(), json!(owner));
        }
        member.insert("mine".into(), json!(mine));
        member.insert("dmCharacter".into(), json!(owner == party.dm_id));
        card_json(&mut member, &card);
        member.insert("build".into(), Value::Null);
        member.insert("joinedAt".into(), json!(joined));
        out.push(Value::Object(member));
    }
    Ok(out)
}

/// The DM's list of open invites. Declined ones stay listed, so a username invite looks the same whatever happened.
async fn dm_invites(state: &AppState, party: &str) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT id, invited_username_lower, code_hash, uses_left, created_at, expires_at FROM party_invites
         WHERE party_id = $1 AND expires_at > $2 AND uses_left > 0 ORDER BY created_at DESC, id",
    )
    .bind(party)
    .bind(now())
    .fetch_all(state.db.pool())
    .await?;
    rows.iter()
        .map(|r| {
            let code: Option<String> = r.try_get(2)?;
            let id: String = r.try_get(0)?;
            let username: Option<String> = r.try_get(1)?;
            let uses: i64 = r.try_get(3)?;
            let created: i64 = r.try_get(4)?;
            let expires: i64 = r.try_get(5)?;
            Ok(if code.is_some() {
                json!({ "id": id, "kind": "code", "usesLeft": uses, "createdAt": created, "expiresAt": expires })
            } else {
                json!({ "id": id, "kind": "username", "username": username, "createdAt": created, "expiresAt": expires })
            })
        })
        .collect()
}

/// The DM's list of offers on open invites.
async fn dm_offers(state: &AppState, party: &str) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT o.id, o.invite_id, u.username, o.owner_id, o.character_id, o.created_at FROM party_offers o
         JOIN party_invites i ON i.id = o.invite_id JOIN users u ON u.id = o.owner_id
         WHERE i.party_id = $1 AND i.expires_at > $2 AND i.uses_left > 0 ORDER BY o.created_at, o.id",
    )
    .bind(party)
    .bind(now())
    .fetch_all(state.db.pool())
    .await?;
    let mut found = Vec::new();
    for r in &rows {
        let row: (String, String, String, String, String, i64) = (
            r.try_get(0)?,
            r.try_get(1)?,
            r.try_get(2)?,
            r.try_get(3)?,
            r.try_get(4)?,
            r.try_get(5)?,
        );
        found.push(row);
    }
    let found: Vec<_> = found
        .into_iter()
        .filter(|row| valid_character_id(&row.4))
        .collect();
    let pairs: Vec<(String, String)> = found.iter().map(|r| (r.3.clone(), r.4.clone())).collect();
    let read = cards(state, &pairs).await?;
    Ok(found
        .into_iter()
        .zip(read)
        .map(|((id, invite, owner_name, _, character, created), card)| {
            let mut offer = serde_json::Map::new();
            offer.insert("id".into(), json!(id));
            offer.insert("inviteId".into(), json!(invite));
            offer.insert("ownerName".into(), json!(owner_name));
            offer.insert("characterId".into(), json!(character));
            card_json(&mut offer, &card);
            offer.insert("createdAt".into(), json!(created));
            Value::Object(offer)
        })
        .collect())
}

async fn party_detail(
    state: &AppState,
    user: &User,
    party: &Party,
    count: i64,
    role: Role,
) -> ApiResult<Value> {
    let mine = my_characters(state, &user.id).await?;
    let mut out = summary(
        party,
        count,
        role,
        mine.get(&party.id).map_or(&[][..], Vec::as_slice),
    );
    out["members"] = json!(members(state, user, party, role).await?);
    if role == Role::Dm {
        out["invites"] = json!(dm_invites(state, &party.id).await?);
        out["offers"] = json!(dm_offers(state, &party.id).await?);
    }
    Ok(out)
}

// The DM capability.

/// `GET /api/me/roles`.
pub async fn roles(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    Ok(Json(
        json!({ "dm": is_dm_capable(&state, &user.id).await? }),
    ))
}

/// `POST /api/me/roles/dm`: switch the DM capability on (self-service, D2).
pub async fn dm_on(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    sqlx::query(
        "INSERT INTO account_roles (user_id, role, granted_at, granted_by) VALUES ($1, $2, $3, NULL)
         ON CONFLICT DO NOTHING",
    )
    .bind(&user.id)
    .bind(DM)
    .bind(now())
    .execute(state.db.pool())
    .await?;
    tracing::info!("party: dm_on {}", user.id);
    Ok(Json(json!({ "dm": true })))
}

/// `DELETE /api/me/roles/dm`: switch it off, once the account runs no parties.
pub async fn dm_off(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    let running: i64 = sqlx::query("SELECT COUNT(*) FROM parties WHERE dm_id = $1")
        .bind(&user.id)
        .fetch_one(state.db.pool())
        .await?
        .try_get(0)?;
    if running > 0 {
        return Err(ApiError::Conflict(format!(
            "You still run {running} {}. Delete {} first.",
            if running == 1 { "party" } else { "parties" },
            if running == 1 { "it" } else { "them" },
        )));
    }
    sqlx::query("DELETE FROM account_roles WHERE user_id = $1 AND role = $2")
        .bind(&user.id)
        .bind(DM)
        .execute(state.db.pool())
        .await?;
    tracing::info!("party: dm_off {}", user.id);
    Ok(Json(json!({ "dm": false })))
}

// Parties.

/// `GET /api/parties`: every party the account runs or has a character in.
pub async fn list(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    let rows = sqlx::query(&format!(
        "SELECT {PARTY_COLUMNS} FROM parties p JOIN users u ON u.id = p.dm_id
         WHERE p.dm_id = $1 OR p.id IN (SELECT m.party_id FROM party_members m WHERE m.owner_id = $1)
         ORDER BY p.created_at DESC, p.id"
    ))
    .bind(&user.id)
    .fetch_all(state.db.pool())
    .await?;
    let mine = my_characters(&state, &user.id).await?;
    let mut parties = Vec::new();
    for row in &rows {
        let (party, count) = party_from(row)?;
        let role = if party.dm_id == user.id {
            Role::Dm
        } else {
            Role::Player
        };
        parties.push(summary(
            &party,
            count,
            role,
            mine.get(&party.id).map_or(&[][..], Vec::as_slice),
        ));
    }
    Ok(Json(json!({
        "dm": is_dm_capable(&state, &user.id).await?,
        "parties": parties,
    })))
}

/// A new party, or a party's changes.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartyForm {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    ruleset: Option<String>,
}

/// `POST /api/parties`: DM-capable accounts only.
pub async fn create(
    State(state): State<AppState>,
    user: User,
    Json(form): Json<PartyForm>,
) -> ApiResult<Json<Value>> {
    if !is_dm_capable(&state, &user.id).await? {
        return Err(ApiError::Refused(
            "dm_required",
            "Switch on Dungeon Master tools on your Account page to run parties.".into(),
        ));
    }
    let name = party_name(form.name.as_deref().unwrap_or_default())?;
    let ruleset = ruleset_arg(form.ruleset.as_deref())?.unwrap_or_else(|| RULESETS[0].to_string());
    let running: i64 = sqlx::query("SELECT COUNT(*) FROM parties WHERE dm_id = $1")
        .bind(&user.id)
        .fetch_one(state.db.pool())
        .await?
        .try_get(0)?;
    if running >= MAX_PARTIES {
        return Err(ApiError::TooMany);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let at = now();
    sqlx::query(
        "INSERT INTO parties (id, name, dm_id, ruleset, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&id)
    .bind(&name)
    .bind(&user.id)
    .bind(&ruleset)
    .bind(at)
    .bind(at)
    .execute(state.db.pool())
    .await?;
    tracing::info!("party: party_created {id} by {}", user.id);
    let (party, count) = run_by(&state, &user, &id).await?;
    Ok(Json(
        party_detail(&state, &user, &party, count, Role::Dm).await?,
    ))
}

/// `GET /api/parties/{party}`.
pub async fn detail(
    State(state): State<AppState>,
    user: User,
    Path(party): Path<String>,
) -> ApiResult<Json<Value>> {
    let (party, count, role) = viewed(&state, &user, &party).await?;
    Ok(Json(
        party_detail(&state, &user, &party, count, role).await?,
    ))
}

/// `GET /api/parties/{party}/roster`.
pub async fn roster(
    State(state): State<AppState>,
    user: User,
    Path(party): Path<String>,
) -> ApiResult<Json<Value>> {
    let (party, _, role) = viewed(&state, &user, &party).await?;
    Ok(Json(
        json!({ "members": members(&state, &user, &party, role).await? }),
    ))
}

/// `PATCH /api/parties/{party}`: the DM renames it or changes its ruleset.
pub async fn update(
    State(state): State<AppState>,
    user: User,
    Path(party): Path<String>,
    Json(form): Json<PartyForm>,
) -> ApiResult<Json<Value>> {
    run_by(&state, &user, &party).await?;
    let name = form.name.as_deref().map(party_name).transpose()?;
    let ruleset = ruleset_arg(form.ruleset.as_deref())?;
    sqlx::query(
        "UPDATE parties SET name = COALESCE($1, name), ruleset = COALESCE($2, ruleset), updated_at = $3
         WHERE id = $4 AND dm_id = $5",
    )
    .bind(name)
    .bind(ruleset)
    .bind(now())
    .bind(&party)
    .bind(&user.id)
    .execute(state.db.pool())
    .await?;
    let (party, count) = run_by(&state, &user, &party).await?;
    Ok(Json(
        party_detail(&state, &user, &party, count, Role::Dm).await?,
    ))
}

/// `DELETE /api/parties/{party}`: invites, offers and memberships go with it; no character file is touched.
pub async fn remove(
    State(state): State<AppState>,
    user: User,
    Path(party): Path<String>,
) -> ApiResult<Json<Value>> {
    id_arg(&party)?;
    let done = sqlx::query("DELETE FROM parties WHERE id = $1 AND dm_id = $2")
        .bind(&party)
        .bind(&user.id)
        .execute(state.db.pool())
        .await?;
    if done.rows_affected() == 0 {
        return Err(not_found());
    }
    tracing::info!("party: party_deleted {party} by {}", user.id);
    Ok(Json(json!({ "deleted": true })))
}

// Invites, the DM's side.

/// A new invite.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteForm {
    kind: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    uses: Option<i64>,
    #[serde(default)]
    days: Option<i64>,
}

/// A username as one could be: whether an account has it is never looked up here.
fn invitee(name: Option<&str>) -> ApiResult<String> {
    let name = name.unwrap_or_default().trim();
    let ok = (2..=40).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || "._-".contains(c));
    if ok {
        Ok(name.to_string())
    } else {
        Err(ApiError::Invalid(
            "Type the player's exact username.".into(),
        ))
    }
}

/// `POST /api/parties/{party}/invites`: by username (the same answer whether or not the account exists) or a join
/// code (shown once; only its hash is kept).
pub async fn invite(
    State(state): State<AppState>,
    user: User,
    client: ClientIp,
    Path(party): Path<String>,
    Json(form): Json<InviteForm>,
) -> ApiResult<Json<Value>> {
    run_by(&state, &user, &party).await?;
    let days = form.days.unwrap_or(MAX_DAYS);
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(ApiError::Invalid("Invites last 1 to 7 days.".into()));
    }
    let username = match form.kind.as_str() {
        "username" => Some(invitee(form.username.as_deref())?),
        "code" => None,
        _ => {
            return Err(ApiError::Invalid(
                "An invite is by username or by join code.".into(),
            ))
        }
    };
    let uses = form.uses.unwrap_or(1);
    if username.is_none() && !(1..=MAX_USES).contains(&uses) {
        return Err(ApiError::Invalid("A join code works 1 to 50 times.".into()));
    }
    if !open(&format!("invite:{}", user.id), INVITES_PER_DAY, DAY)
        || !take(
            &format!("invite-ip:{}", client.limit_key()),
            INVITES_PER_DAY_PER_IP,
            DAY,
        )
        || !take(&format!("invite:{}", user.id), INVITES_PER_DAY, DAY)
    {
        return Err(ApiError::TooMany);
    }
    let at = now();
    let expires = at + days * DAY;
    // Long-expired invites of this party go now; nothing reads them.
    sqlx::query("DELETE FROM party_invites WHERE party_id = $1 AND expires_at < $2")
        .bind(&party)
        .bind(at - 30 * DAY)
        .execute(state.db.pool())
        .await?;
    let id = uuid::Uuid::new_v4().to_string();
    if let Some(name) = username {
        // Sent again: the same invite, renewed.
        let renewed = sqlx::query(
            "UPDATE party_invites SET created_at = $1, expires_at = $2, uses_left = 1, declined_at = NULL,
                 created_by = $3
             WHERE party_id = $4 AND invited_username_lower = lower($5)",
        )
        .bind(at)
        .bind(expires)
        .bind(&user.id)
        .bind(&party)
        .bind(&name)
        .execute(state.db.pool())
        .await?;
        if renewed.rows_affected() == 0 {
            sqlx::query(
                "INSERT INTO party_invites (id, party_id, invited_username_lower, code_hash, created_by, created_at,
                     expires_at, uses_left)
                 VALUES ($1, $2, lower($3), NULL, $4, $5, $6, 1) ON CONFLICT DO NOTHING",
            )
            .bind(&id)
            .bind(&party)
            .bind(&name)
            .bind(&user.id)
            .bind(at)
            .bind(expires)
            .execute(state.db.pool())
            .await?;
        }
        let row = sqlx::query(
            "SELECT id, invited_username_lower, created_at, expires_at FROM party_invites
             WHERE party_id = $1 AND invited_username_lower = lower($2)",
        )
        .bind(&party)
        .bind(&name)
        .fetch_one(state.db.pool())
        .await?;
        let invite_id: String = row.try_get(0)?;
        tracing::info!("party: invite {invite_id} kind=username party={party}");
        return Ok(Json(json!({
            "sent": true,
            "invite": {
                "id": invite_id,
                "kind": "username",
                "username": row.try_get::<String, _>(1)?,
                "createdAt": row.try_get::<i64, _>(2)?,
                "expiresAt": row.try_get::<i64, _>(3)?,
            },
        })));
    }
    let code = new_code();
    sqlx::query(
        "INSERT INTO party_invites (id, party_id, invited_username_lower, code_hash, created_by, created_at,
             expires_at, uses_left)
         VALUES ($1, $2, NULL, $3, $4, $5, $6, $7)",
    )
    .bind(&id)
    .bind(&party)
    .bind(token_hash(&code))
    .bind(&user.id)
    .bind(at)
    .bind(expires)
    .bind(uses)
    .execute(state.db.pool())
    .await?;
    tracing::info!("party: invite {id} kind=code party={party}");
    Ok(Json(json!({
        "invite": { "id": id, "kind": "code", "usesLeft": uses, "createdAt": at, "expiresAt": expires },
        "code": grouped(&code),
        "link": state.config.public_url.as_ref().map(|base| format!("{base}/join#{code}")),
    })))
}

/// `DELETE /api/parties/{party}/invites/{invite}`.
pub async fn revoke(
    State(state): State<AppState>,
    user: User,
    Path((party, invite)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    run_by(&state, &user, &party).await?;
    id_arg(&invite)?;
    let done = sqlx::query("DELETE FROM party_invites WHERE id = $1 AND party_id = $2")
        .bind(&invite)
        .bind(&party)
        .execute(state.db.pool())
        .await?;
    if done.rows_affected() == 0 {
        return Err(not_found());
    }
    Ok(Json(json!({ "deleted": true })))
}

/// A character to add, offer or join with.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterForm {
    #[serde(default)]
    character_id: Option<String>,
}

/// Add a member row; `false` when the character is in a party already.
async fn insert_member<'c>(
    tx: &mut sqlx::Transaction<'c, sqlx::Any>,
    owner: &str,
    character: &str,
    party: &str,
) -> ApiResult<bool> {
    let done = sqlx::query(
        "INSERT INTO party_members (owner_id, character_id, party_id, joined_at) VALUES ($1, $2, $3, $4)
         ON CONFLICT DO NOTHING",
    )
    .bind(owner)
    .bind(character)
    .bind(party)
    .bind(now())
    .execute(&mut **tx)
    .await?;
    Ok(done.rows_affected() > 0)
}

fn in_a_party() -> ApiError {
    ApiError::Conflict("That character is in a party already. Withdraw it there first.".into())
}

/// The member card just added.
async fn member_json(
    state: &AppState,
    user: &User,
    party: &str,
    owner: &str,
    character: &str,
) -> ApiResult<Value> {
    let (party, _, role) = viewed(state, user, party).await?;
    members(state, user, &party, role)
        .await?
        .into_iter()
        .find(|m| m["characterId"] == character && (m["ownerId"] == owner || m["mine"] == true))
        .ok_or_else(not_found)
}

/// `POST /api/parties/{party}/members`: the DM adds their own character (D4).
pub async fn add_own(
    State(state): State<AppState>,
    user: User,
    Path(party): Path<String>,
    Json(form): Json<CharacterForm>,
) -> ApiResult<Json<Value>> {
    run_by(&state, &user, &party).await?;
    let character = own_character(&state, &user, form.character_id.as_deref()).await?;
    let mut tx = state.db.pool().begin().await?;
    if !insert_member(&mut tx, &user.id, &character, &party).await? {
        return Err(in_a_party());
    }
    delete_offers_of(&mut tx, &user.id, &character).await?;
    tx.commit().await?;
    tracing::info!("party: joined party={party} owner={} (dm)", user.id);
    Ok(Json(
        member_json(&state, &user, &party, &user.id, &character).await?,
    ))
}

async fn delete_offers_of<'c>(
    tx: &mut sqlx::Transaction<'c, sqlx::Any>,
    owner: &str,
    character: &str,
) -> ApiResult<()> {
    sqlx::query("DELETE FROM party_offers WHERE owner_id = $1 AND character_id = $2")
        .bind(owner)
        .bind(character)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// `DELETE /api/parties/{party}/members/{owner}/{character}`: the DM removes it, or its owner withdraws it.
pub async fn remove_member(
    State(state): State<AppState>,
    user: User,
    Path((party, owner, character)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    let (_, _, role) = viewed(&state, &user, &party).await?;
    id_arg(&owner)?;
    if (role != Role::Dm && owner != user.id) || !valid_character_id(&character) {
        return Err(not_found());
    }
    let done = sqlx::query(
        "DELETE FROM party_members WHERE party_id = $1 AND owner_id = $2 AND character_id = $3",
    )
    .bind(&party)
    .bind(&owner)
    .bind(&character)
    .execute(state.db.pool())
    .await?;
    if done.rows_affected() == 0 {
        return Err(not_found());
    }
    let kind = if owner == user.id { "left" } else { "removed" };
    tracing::info!("party: {kind} party={party} owner={owner} by {}", user.id);
    Ok(Json(json!({ "removed": true })))
}

/// The offer on this party's open invite: (invite, owner, character).
async fn party_offer(
    state: &AppState,
    party: &str,
    offer: &str,
) -> ApiResult<(String, String, String)> {
    id_arg(offer)?;
    let row = sqlx::query(
        "SELECT o.invite_id, o.owner_id, o.character_id FROM party_offers o JOIN party_invites i ON i.id = o.invite_id
         WHERE o.id = $1 AND i.party_id = $2",
    )
    .bind(offer)
    .bind(party)
    .fetch_optional(state.db.pool())
    .await?
    .ok_or_else(not_found)?;
    Ok((row.try_get(0)?, row.try_get(1)?, row.try_get(2)?))
}

/// Take one use of an open invite; false when it is used up, expired or gone.
async fn take_use<'c>(tx: &mut sqlx::Transaction<'c, sqlx::Any>, invite: &str) -> ApiResult<bool> {
    let done = sqlx::query(
        "UPDATE party_invites SET uses_left = uses_left - 1 WHERE id = $1 AND uses_left > 0 AND expires_at > $2",
    )
    .bind(invite)
    .bind(now())
    .execute(&mut **tx)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// `POST /api/parties/{party}/offers/{offer}/accept`: the DM takes an offered character into the party.
pub async fn accept_offer(
    State(state): State<AppState>,
    user: User,
    Path((party, offer)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    run_by(&state, &user, &party).await?;
    let (invite, owner, character) = party_offer(&state, &party, &offer).await?;
    let card = if valid_character_id(&character) {
        cards(&state, &[(owner.clone(), character.clone())]).await?
    } else {
        Vec::new()
    };
    if !card.first().is_some_and(|c| c.state == "ok") {
        return Err(not_found());
    }
    let mut tx = state.db.pool().begin().await?;
    if !take_use(&mut tx, &invite).await? {
        return Err(not_found());
    }
    if !insert_member(&mut tx, &owner, &character, &party).await? {
        return Err(in_a_party());
    }
    delete_offers_of(&mut tx, &owner, &character).await?;
    tx.commit().await?;
    tracing::info!("party: joined party={party} owner={owner} (offer {offer})");
    Ok(Json(
        member_json(&state, &user, &party, &owner, &character).await?,
    ))
}

/// `DELETE /api/parties/{party}/offers/{offer}`: the DM turns an offer down.
pub async fn decline_offer(
    State(state): State<AppState>,
    user: User,
    Path((party, offer)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    run_by(&state, &user, &party).await?;
    party_offer(&state, &party, &offer).await?;
    sqlx::query("DELETE FROM party_offers WHERE id = $1")
        .bind(&offer)
        .execute(state.db.pool())
        .await?;
    Ok(Json(json!({ "deleted": true })))
}

// Invites, the invitee's side.

/// Open invites the account may act on: sent to its username (and not declined), or join codes it entered.
/// `$1` now, `$2` the username, `$3` the account id.
const MINE: &str = "i.expires_at > $1 AND i.uses_left > 0 AND (
        (i.code_hash IS NULL AND i.invited_username_lower = lower($2) AND i.declined_at IS NULL)
        OR i.id IN (SELECT h.invite_id FROM party_invite_holds h WHERE h.user_id = $3))";

const MY_INVITE_COLUMNS: &str =
    "i.id, i.code_hash, p.name, u.username, p.ruleset, i.expires_at, i.party_id";

async fn my_offers(state: &AppState, user: &str) -> ApiResult<HashMap<String, Value>> {
    let rows = sqlx::query(
        "SELECT invite_id, id, character_id, created_at FROM party_offers WHERE owner_id = $1",
    )
    .bind(user)
    .fetch_all(state.db.pool())
    .await?;
    rows.iter()
        .map(|r| {
            Ok((
                r.try_get::<String, _>(0)?,
                json!({
                    "id": r.try_get::<String, _>(1)?,
                    "characterId": r.try_get::<String, _>(2)?,
                    "createdAt": r.try_get::<i64, _>(3)?,
                }),
            ))
        })
        .collect()
}

fn my_invite_json(row: &sqlx::any::AnyRow, offers: &HashMap<String, Value>) -> ApiResult<Value> {
    let id: String = row.try_get(0)?;
    let code: Option<String> = row.try_get(1)?;
    Ok(json!({
        "id": id,
        "kind": if code.is_some() { "code" } else { "username" },
        "party": {
            "name": row.try_get::<String, _>(2)?,
            "dmName": row.try_get::<String, _>(3)?,
            "ruleset": row.try_get::<String, _>(4)?,
        },
        "expiresAt": row.try_get::<i64, _>(5)?,
        "offer": offers.get(&id).cloned().unwrap_or(Value::Null),
    }))
}

/// An invite the account may act on, with its party id; else 404.
async fn my_invite(
    state: &AppState,
    user: &User,
    invite: &str,
) -> ApiResult<(sqlx::any::AnyRow, String)> {
    id_arg(invite)?;
    let row = sqlx::query(&format!(
        "SELECT {MY_INVITE_COLUMNS} FROM party_invites i JOIN parties p ON p.id = i.party_id
         JOIN users u ON u.id = p.dm_id WHERE {MINE} AND i.id = $4"
    ))
    .bind(now())
    .bind(&user.username)
    .bind(&user.id)
    .bind(invite)
    .fetch_optional(state.db.pool())
    .await?
    .ok_or_else(not_found)?;
    let party: String = row.try_get(6)?;
    Ok((row, party))
}

/// `GET /api/invites`.
pub async fn my_invites(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    let rows = sqlx::query(&format!(
        "SELECT {MY_INVITE_COLUMNS} FROM party_invites i JOIN parties p ON p.id = i.party_id
         JOIN users u ON u.id = p.dm_id WHERE {MINE} ORDER BY i.created_at DESC, i.id"
    ))
    .bind(now())
    .bind(&user.username)
    .bind(&user.id)
    .fetch_all(state.db.pool())
    .await?;
    let offers = my_offers(&state, &user.id).await?;
    let invites = rows
        .iter()
        .map(|r| my_invite_json(r, &offers))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(json!({ "invites": invites })))
}

/// Join `party` with one of the account's characters through `invite`, taking one use.
async fn join_with(
    state: &AppState,
    user: &User,
    invite: &str,
    party: &str,
    character: &str,
) -> ApiResult<Value> {
    let mut tx = state.db.pool().begin().await?;
    if !take_use(&mut tx, invite).await? {
        return Err(not_found());
    }
    if !insert_member(&mut tx, &user.id, character, party).await? {
        return Err(in_a_party());
    }
    delete_offers_of(&mut tx, &user.id, character).await?;
    sqlx::query("DELETE FROM party_invite_holds WHERE user_id = $1 AND invite_id = $2")
        .bind(&user.id)
        .bind(invite)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(
        "party: joined party={party} owner={} (invite {invite})",
        user.id
    );
    let (party, count, role) = viewed(state, user, party).await?;
    let mine = my_characters(state, &user.id).await?;
    Ok(json!({
        "party": summary(&party, count, role, mine.get(&party.id).map_or(&[][..], Vec::as_slice)),
    }))
}

/// `POST /api/invites/{invite}/accept`.
pub async fn accept(
    State(state): State<AppState>,
    user: User,
    Path(invite): Path<String>,
    Json(form): Json<CharacterForm>,
) -> ApiResult<Json<Value>> {
    let (_, party) = my_invite(&state, &user, &invite).await?;
    let character = own_character(&state, &user, form.character_id.as_deref()).await?;
    Ok(Json(
        join_with(&state, &user, &invite, &party, &character).await?,
    ))
}

/// `POST /api/invites/{invite}/offer`: offer a character; the DM decides.
pub async fn offer(
    State(state): State<AppState>,
    user: User,
    Path(invite): Path<String>,
    Json(form): Json<CharacterForm>,
) -> ApiResult<Json<Value>> {
    my_invite(&state, &user, &invite).await?;
    let character = own_character(&state, &user, form.character_id.as_deref()).await?;
    let member =
        sqlx::query("SELECT 1 FROM party_members WHERE owner_id = $1 AND character_id = $2")
            .bind(&user.id)
            .bind(&character)
            .fetch_optional(state.db.pool())
            .await?;
    if member.is_some() {
        return Err(in_a_party());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let at = now();
    let done = sqlx::query(
        "INSERT INTO party_offers (id, invite_id, owner_id, character_id, created_at) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT DO NOTHING",
    )
    .bind(&id)
    .bind(&invite)
    .bind(&user.id)
    .bind(&character)
    .bind(at)
    .execute(state.db.pool())
    .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::Conflict(
            "You already offered a character for this invite, or offered this character elsewhere."
                .into(),
        ));
    }
    tracing::info!("party: offer {id} invite={invite} owner={}", user.id);
    Ok(Json(json!({
        "offer": { "id": id, "characterId": character, "createdAt": at },
    })))
}

/// `DELETE /api/invites/{invite}/offer`: withdraw one's offer.
pub async fn withdraw_offer(
    State(state): State<AppState>,
    user: User,
    Path(invite): Path<String>,
) -> ApiResult<Json<Value>> {
    id_arg(&invite)?;
    let done = sqlx::query("DELETE FROM party_offers WHERE invite_id = $1 AND owner_id = $2")
        .bind(&invite)
        .bind(&user.id)
        .execute(state.db.pool())
        .await?;
    if done.rows_affected() == 0 {
        return Err(not_found());
    }
    Ok(Json(json!({ "deleted": true })))
}

/// `POST /api/invites/{invite}/decline`: it leaves the account's list. The DM's list doesn't change.
pub async fn decline(
    State(state): State<AppState>,
    user: User,
    Path(invite): Path<String>,
) -> ApiResult<Json<Value>> {
    let (row, _) = my_invite(&state, &user, &invite).await?;
    let code: Option<String> = row.try_get(1)?;
    let mut tx = state.db.pool().begin().await?;
    if code.is_some() {
        sqlx::query("DELETE FROM party_invite_holds WHERE user_id = $1 AND invite_id = $2")
            .bind(&user.id)
            .bind(&invite)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("UPDATE party_invites SET declined_at = $1 WHERE id = $2")
            .bind(now())
            .bind(&invite)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM party_offers WHERE invite_id = $1 AND owner_id = $2")
        .bind(&invite)
        .bind(&user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({ "declined": true })))
}

// Join codes.

/// A join code, and optionally a character to join with.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinForm {
    #[serde(default)]
    code: String,
    #[serde(default)]
    character_id: Option<String>,
}

/// The open invite a code belongs to: (id, party id).
async fn code_invite(state: &AppState, code: &str) -> ApiResult<Option<(String, String)>> {
    let Some(code) = normal_code(code) else {
        return Ok(None);
    };
    let row = sqlx::query(
        "SELECT id, party_id FROM party_invites WHERE code_hash = $1 AND expires_at > $2 AND uses_left > 0",
    )
    .bind(token_hash(&code))
    .bind(now())
    .fetch_optional(state.db.pool())
    .await?;
    Ok(row
        .map(|r| Ok::<_, sqlx::Error>((r.try_get(0)?, r.try_get(1)?)))
        .transpose()?)
}

async fn hold(state: &AppState, user: &str, invite: &str) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO party_invite_holds (user_id, invite_id, created_at) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(user)
    .bind(invite)
    .bind(now())
    .execute(state.db.pool())
    .await?;
    Ok(())
}

/// `POST /api/join`: keep a code on the account (it then shows in `GET /api/invites`), or join with a character.
pub async fn join(
    State(state): State<AppState>,
    user: User,
    Json(form): Json<JoinForm>,
) -> ApiResult<Json<Value>> {
    let key = format!("code:{}", user.id);
    if !open(&key, CODE_TRIES, TEN_MINUTES) {
        return Err(ApiError::TooMany);
    }
    let Some((invite, party)) = code_invite(&state, &form.code).await? else {
        take(&key, CODE_TRIES, TEN_MINUTES);
        return Err(not_found());
    };
    if form.character_id.is_some() {
        let character = own_character(&state, &user, form.character_id.as_deref()).await?;
        return Ok(Json(
            join_with(&state, &user, &invite, &party, &character).await?,
        ));
    }
    hold(&state, &user.id, &invite).await?;
    let (row, _) = my_invite(&state, &user, &invite).await?;
    let offers = my_offers(&state, &user.id).await?;
    Ok(Json(json!({ "invite": my_invite_json(&row, &offers)? })))
}

/// A code to look at.
#[derive(Deserialize)]
pub struct PreviewForm {
    #[serde(default)]
    code: String,
}

/// `POST /api/join/preview`: which party a code is for, without signing in (the join page, and the screen of an
/// account waiting for approval). Limited per client address.
pub async fn preview(
    State(state): State<AppState>,
    client: ClientIp,
    Json(form): Json<PreviewForm>,
) -> ApiResult<Json<Value>> {
    if !take(
        &format!("preview:{}", client.limit_key()),
        PREVIEWS,
        TEN_MINUTES,
    ) {
        return Err(ApiError::TooMany);
    }
    let Some((invite, _)) = code_invite(&state, &form.code).await? else {
        return Err(not_found());
    };
    let row = sqlx::query(
        "SELECT p.name, u.username, p.ruleset, i.expires_at FROM party_invites i JOIN parties p ON p.id = i.party_id
         JOIN users u ON u.id = p.dm_id WHERE i.id = $1",
    )
    .bind(&invite)
    .fetch_optional(state.db.pool())
    .await?
    .ok_or_else(not_found)?;
    Ok(Json(json!({
        "party": {
            "name": row.try_get::<String, _>(0)?,
            "dmName": row.try_get::<String, _>(1)?,
            "ruleset": row.try_get::<String, _>(2)?,
        },
        "expiresAt": row.try_get::<i64, _>(3)?,
    })))
}

/// Sign-up (`auth::register`): keep a join code typed there on the new account, even while it waits for approval
/// (D39). Never fails the sign-up: a wrong code is just not kept.
pub async fn hold_join_code(state: &AppState, user: &str, code: Option<&str>) {
    let Some(code) = code.filter(|c| !c.trim().is_empty()) else {
        return;
    };
    if !crate::config::features().parties {
        return;
    }
    match code_invite(state, code).await {
        Ok(Some((invite, _))) => {
            if let Err(e) = hold(state, user, &invite).await {
                tracing::warn!("party: keeping a join code for {user}: {e:?}");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("party: reading a join code for {user}: {e:?}"),
    }
}
