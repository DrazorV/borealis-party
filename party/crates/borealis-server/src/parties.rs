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
use crate::hub::{card_view_for, Audience, Card, Viewer, MINE_EVENT, PARTY_EVENT};
use crate::workspaces::Workspace;
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
            "/api/parties/{party}/members/{owner}/{character}/view/snapshot",
            post(view_snapshot),
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

// Roster cards.

fn character_file(state: &AppState, owner: &str, character: &str) -> PathBuf {
    state
        .workspaces
        .dir(owner)
        .join("characters")
        .join(format!("{character}.json"))
}

/// The saved file's state (`ok`, `unreadable` or `missing`) and, when it reads, a card of what the file alone says
/// (no numbers: those need the owner's content, [`refresh_card`]).
fn read_card(path: &std::path::Path) -> (Card, &'static str) {
    // A regular file only: no links out of the folder.
    match path.symlink_metadata() {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return (Card::default(), "unreadable"),
        Err(_) => return (Card::default(), "missing"),
    }
    match borealis_character::Character::load(path) {
        Ok(c) => (
            Card {
                name: Some(c.meta.name.clone()),
                level: Some(c.build.level()),
                ruleset: serde_json::to_value(c.ruleset()).ok(),
                ..Card::default()
            },
            "ok",
        ),
        Err(_) => (Card::default(), "unreadable"),
    }
}

/// Cards for `(owner, character)` pairs, off the async threads: the cached card when there is one and the file
/// still reads, else what the file says.
async fn cards(
    state: &AppState,
    pairs: &[(String, String)],
) -> ApiResult<Vec<(Card, &'static str)>> {
    let files: Vec<(String, String, PathBuf)> = pairs
        .iter()
        .map(|(owner, character)| {
            (
                owner.clone(),
                character.clone(),
                character_file(state, owner, character),
            )
        })
        .collect();
    let hub = std::sync::Arc::clone(&state.hub);
    tokio::task::spawn_blocking(move || {
        files
            .iter()
            .map(|(owner, character, file)| {
                let (basic, file_state) = read_card(file);
                if file_state != "ok" {
                    return (basic, file_state);
                }
                (hub.cards.get(owner, character).unwrap_or(basic), file_state)
            })
            .collect()
    })
    .await
    .map_err(internal)
}

/// The signed-in account's own saved character, or a 400 that says to pick one of theirs.
async fn own_character(state: &AppState, user: &User, id: Option<&str>) -> ApiResult<String> {
    let id = character_arg(id)?;
    let card = cards(state, &[(user.id.clone(), id.clone())]).await?;
    if card.first().is_some_and(|(_, s)| *s == "ok") {
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

/// The roster as `user` may see it: each card through [`card_view_for`]. A member whose owner has the workspace open
/// and no card yet gets one first.
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
    for (owner, _, character, _) in &found {
        if valid_character_id(character) && state.hub.cards.get(owner, character).is_none() {
            if let Some(workspace) = state.workspaces.loaded_now(owner) {
                refresh_card(state, owner, character, &workspace, &party.id).await;
            }
        }
    }
    let pairs: Vec<(String, String)> = found
        .iter()
        .filter(|(_, _, c, _)| valid_character_id(c))
        .map(|(o, _, c, _)| (o.clone(), c.clone()))
        .collect();
    let mut read = cards(state, &pairs).await?.into_iter();
    let mut out = Vec::new();
    for (owner, owner_name, character, joined) in found {
        let (card, file_state) = if valid_character_id(&character) {
            read.next().unwrap_or((Card::default(), "unreadable"))
        } else {
            (Card::default(), "unreadable")
        };
        let mine = owner == user.id;
        let viewer = if mine {
            Viewer::Owner
        } else if role == Role::Dm {
            Viewer::Dm {
                in_combat: in_combat(&party.id),
            }
        } else {
            Viewer::Member
        };
        let mut member = serde_json::Map::new();
        member.insert("characterId".into(), json!(character));
        member.insert("ownerName".into(), json!(owner_name));
        if role == Role::Dm || mine {
            member.insert("ownerId".into(), json!(owner));
        }
        if role == Role::Dm {
            member.insert("online".into(), json!(state.hub.online(&owner)));
        }
        member.insert("mine".into(), json!(mine));
        member.insert("dmCharacter".into(), json!(owner == party.dm_id));
        member.extend(card_view_for(viewer, &card, file_state));
        member.insert("build".into(), Value::Null);
        member.insert("joinedAt".into(), json!(joined));
        out.push(Value::Object(member));
    }
    Ok(out)
}

/// The sequence a view of `party` for `role` is as new as: read before the view, so an event that lands while the
/// view is read makes the client read it again rather than be missed.
fn seq_for(state: &AppState, party: &str, role: Role) -> u64 {
    state.hub.seq(
        party,
        if role == Role::Dm {
            Audience::Dm
        } else {
            Audience::Players
        },
    )
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
        .map(
            |((id, invite, owner_name, _, character, created), (card, file_state))| {
                let mut offer = serde_json::Map::new();
                offer.insert("id".into(), json!(id));
                offer.insert("inviteId".into(), json!(invite));
                offer.insert("ownerName".into(), json!(owner_name));
                offer.insert("characterId".into(), json!(character));
                offer.extend(card_view_for(
                    Viewer::Dm { in_combat: false },
                    &card,
                    file_state,
                ));
                offer.insert("createdAt".into(), json!(created));
                Value::Object(offer)
            },
        )
        .collect())
}

async fn party_detail(
    state: &AppState,
    user: &User,
    party: &Party,
    count: i64,
    role: Role,
) -> ApiResult<Value> {
    let pseq = seq_for(state, &party.id, role);
    let mine = my_characters(state, &user.id).await?;
    let mut out = summary(
        party,
        count,
        role,
        mine.get(&party.id).map_or(&[][..], Vec::as_slice),
    );
    out["members"] = json!(members(state, user, party, role).await?);
    out["pseq"] = json!(pseq);
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
    let pseq = seq_for(&state, &party.id, role);
    Ok(Json(
        json!({ "members": members(&state, &user, &party, role).await?, "pseq": pseq }),
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
    publish(&state, &party, "party", None, true).await;
    let (party, count) = run_by(&state, &user, &party).await?;
    Ok(Json(
        party_detail(&state, &user, &party, count, Role::Dm).await?,
    ))
}

/// `DELETE /api/parties/{party}`: invites, offers and memberships go with it; no character file is touched. The party
/// waits in its DM's Recently deleted for 30 days (offers and entered join codes are not kept).
pub async fn remove(
    State(state): State<AppState>,
    user: User,
    Path(party): Path<String>,
) -> ApiResult<Json<Value>> {
    id_arg(&party)?;
    let members = sqlx::query(
        "SELECT m.owner_id, m.character_id FROM party_members m JOIN parties p ON p.id = m.party_id
         WHERE m.party_id = $1 AND p.dm_id = $2",
    )
    .bind(&party)
    .bind(&user.id)
    .fetch_all(state.db.pool())
    .await?
    .iter()
    .map(|r| Ok((r.try_get(0)?, r.try_get(1)?)))
    .collect::<sqlx::Result<Vec<(String, String)>>>()?;
    // Kept in the DM's Recently deleted for 30 days (recently_deleted.rs); gone from every other query now.
    if !crate::recently_deleted::delete_party(&state.db, &user.id, &party).await? {
        return Err(not_found());
    }
    tracing::info!("party: party_deleted {party} by {}", user.id);
    // Its players hear only that their own list changed; nothing of this party reaches them again.
    state.hub.forget_party(&party);
    forget_cards(&state, &members).await;
    let mut owners: Vec<String> = members.into_iter().map(|(o, _)| o).collect();
    owners.push(user.id.clone());
    owners.sort();
    owners.dedup();
    for owner in &owners {
        tell_mine(&state, owner, "parties");
    }
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
        publish(&state, &party, "invites", None, false).await;
        // The invitee's own stream hears of it, later, so the DM's answer takes as long whether they exist or not.
        let later = state.clone();
        tokio::spawn(async move {
            let found = sqlx::query("SELECT id FROM users WHERE lower(username) = lower($1)")
                .bind(&name)
                .fetch_optional(later.db.pool())
                .await;
            if let Ok(Some(row)) = found {
                if let Ok(id) = row.try_get::<String, _>(0) {
                    tell_mine(&later, &id, "invites");
                }
            }
        });
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
    publish(&state, &party, "invites", None, false).await;
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
    publish(&state, &party, "invites", None, false).await;
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
    joined(&state, &user.id, &character, &party).await;
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
    left(&state, &owner, &character, &party).await;
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
    if !card.first().is_some_and(|(_, s)| *s == "ok") {
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
    joined(&state, &owner, &character, &party).await;
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
    let (_, owner, _) = party_offer(&state, &party, &offer).await?;
    sqlx::query("DELETE FROM party_offers WHERE id = $1")
        .bind(&offer)
        .execute(state.db.pool())
        .await?;
    publish(&state, &party, "offers", None, false).await;
    tell_mine(&state, &owner, "invites");
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
    joined(state, &user.id, character, party).await;
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
    let (_, party) = my_invite(&state, &user, &invite).await?;
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
    publish(&state, &party, "offers", None, false).await;
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
    let party = invite_party(&state, &invite).await?;
    let done = sqlx::query("DELETE FROM party_offers WHERE invite_id = $1 AND owner_id = $2")
        .bind(&invite)
        .bind(&user.id)
        .execute(state.db.pool())
        .await?;
    if done.rows_affected() == 0 {
        return Err(not_found());
    }
    if let Some(party) = party {
        publish(&state, &party, "offers", None, false).await;
    }
    Ok(Json(json!({ "deleted": true })))
}

/// The party an invite is for, if it still exists.
async fn invite_party(state: &AppState, invite: &str) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query("SELECT party_id FROM party_invites WHERE id = $1")
            .bind(invite)
            .fetch_optional(state.db.pool())
            .await?
            .map(|r| r.try_get(0))
            .transpose()?,
    )
}

/// `POST /api/invites/{invite}/decline`: it leaves the account's list. The DM's list doesn't change.
pub async fn decline(
    State(state): State<AppState>,
    user: User,
    Path(invite): Path<String>,
) -> ApiResult<Json<Value>> {
    let (row, party) = my_invite(&state, &user, &invite).await?;
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
    let withdrew = sqlx::query("DELETE FROM party_offers WHERE invite_id = $1 AND owner_id = $2")
        .bind(&invite)
        .bind(&user.id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    // A decline itself is never told to the DM; an offer that goes with it leaves their list, as a withdrawn one does.
    if withdrew > 0 {
        publish(&state, &party, "offers", None, false).await;
    }
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

// Live updates (`hub.rs`): who hears of a change is read from the database when it is sent.

/// The party's DM and the owners of its member characters other than the DM, now; none when it is gone.
async fn audience(state: &AppState, party: &str) -> ApiResult<Option<(String, Vec<String>)>> {
    let Some(row) = sqlx::query("SELECT dm_id FROM parties WHERE id = $1")
        .bind(party)
        .fetch_optional(state.db.pool())
        .await?
    else {
        return Ok(None);
    };
    let dm: String = row.try_get(0)?;
    let rows = sqlx::query(
        "SELECT DISTINCT owner_id FROM party_members WHERE party_id = $1 AND owner_id <> $2 ORDER BY owner_id",
    )
    .bind(party)
    .bind(&dm)
    .fetch_all(state.db.pool())
    .await?;
    let owners = rows
        .iter()
        .map(|r| r.try_get(0))
        .collect::<sqlx::Result<Vec<String>>>()?;
    Ok(Some((dm, owners)))
}

/// A party's view changed: tell its DM, and its players too when `players` (they see the change). `kind`: `party`
/// (its name or ruleset), `roster` (who is in it), `card` (a member's card; `id` is the character), or for the DM
/// only, `invites` and `offers`.
pub(crate) async fn publish(
    state: &AppState,
    party: &str,
    kind: &str,
    id: Option<&str>,
    players: bool,
) {
    publish_to(state, party, kind, id, true, players).await;
}

/// [`publish`] to the DM only when `dm` (0.3.3: a change the DM doesn't see moves nothing the DM can count).
pub(crate) async fn publish_to(
    state: &AppState,
    party: &str,
    kind: &str,
    id: Option<&str>,
    dm_too: bool,
    players: bool,
) {
    if !crate::config::features().parties {
        return;
    }
    match audience(state, party).await {
        Ok(Some((dm, owners))) => {
            let (dm_seq, players_seq) = state.hub.bump_for(party, dm_too, players);
            if let Some(dm_seq) = dm_seq {
                state.hub.deliver(
                    &dm,
                    PARTY_EVENT,
                    &json!({ "party": party, "pseq": dm_seq, "kind": kind, "id": id }),
                );
            }
            if let Some(seq) = players_seq {
                let payload = json!({ "party": party, "pseq": seq, "kind": kind, "id": id });
                for owner in owners {
                    state.hub.deliver(&owner, PARTY_EVENT, &payload);
                }
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("party: telling party {party} of a change: {e:?}"),
    }
}

/// The account's own party list (`parties`) or invites (`invites`) changed.
fn tell_mine(state: &AppState, user: &str, kind: &str) {
    if crate::config::features().parties {
        state
            .hub
            .deliver(user, MINE_EVENT, &json!({ "kind": kind }));
    }
}

/// The party a character is in, if any.
async fn party_of(state: &AppState, owner: &str, character: &str) -> ApiResult<Option<String>> {
    Ok(
        sqlx::query("SELECT party_id FROM party_members WHERE owner_id = $1 AND character_id = $2")
            .bind(owner)
            .bind(character)
            .fetch_optional(state.db.pool())
            .await?
            .map(|r| r.try_get(0))
            .transpose()?,
    )
}

/// Upstream's snapshot of a character as its owner has it now: the open session's, or else the saved file read with
/// the owner's content. Opens no session. None when there is no such character.
pub(crate) fn snapshot_of(
    app: &tauri::AppHandle,
    id: &str,
) -> ApiResult<Option<borealis_lib::dto::CharacterSnapshot>> {
    use tauri::Manager;
    let backend = app.state::<borealis_lib::state::AppState>();
    if let Ok(snapshot) = backend.get_snapshot(id) {
        return Ok(Some(snapshot));
    }
    let path = crate::sync::characters_dir(&backend)?.join(format!("{id}.json"));
    if !path.symlink_metadata().is_ok_and(|m| m.is_file()) {
        return Ok(None);
    }
    let mut character = borealis_character::Character::load(&path).map_err(internal)?;
    let hub = backend.hub().map_err(internal)?;
    let settings = backend.settings().map_err(internal)?;
    let _ = borealis_lib::renamed_ids::migrate(&mut character.build, &hub.store);
    Ok(Some(borealis_lib::snapshot_map::build_snapshot(
        &borealis_lib::snapshot_map::MapInput {
            id,
            character: &character,
            dirty: false,
            display_path: String::new(),
            hub: &hub,
            settings: &settings,
            rolls: &[],
            warnings: &[],
        },
    )))
}

/// Whether a party is in a combat: the other members then see each other's health band (D6, the user's choice:
/// during combat only). True while the party has a live combat (`combat.rs`).
pub(crate) fn in_combat(party: &str) -> bool {
    crate::combat::party_in_combat(party)
}

/// Whether `dm` may get a deleted party back: DM tools on, and room for one more party.
pub(crate) async fn may_restore(state: &AppState, dm: &str) -> ApiResult<()> {
    if !is_dm_capable(state, dm).await? {
        return Err(ApiError::Refused(
            "dm_required",
            "Switch on Dungeon Master tools on your Account page to restore a party.".into(),
        ));
    }
    let running: i64 = sqlx::query("SELECT COUNT(*) FROM parties WHERE dm_id = $1")
        .bind(dm)
        .fetch_one(state.db.pool())
        .await?
        .try_get(0)?;
    if running >= MAX_PARTIES {
        return Err(ApiError::TooMany);
    }
    Ok(())
}

/// A party is back (Recently deleted): its DM's and members' lists change; cards are read again when it is opened.
pub(crate) async fn restored(state: &AppState, dm: &str, party: &str) {
    let mut owners: Vec<String> =
        sqlx::query("SELECT owner_id FROM party_members WHERE party_id = $1")
            .bind(party)
            .fetch_all(state.db.pool())
            .await
            .map(|rows| rows.iter().filter_map(|r| r.try_get(0).ok()).collect())
            .unwrap_or_default();
    owners.push(dm.to_string());
    owners.sort();
    owners.dedup();
    crate::combat::forget_live(party);
    for owner in &owners {
        tell_mine(state, owner, "parties");
    }
    tracing::info!("party: party_restored {party} by {dm}");
}

/// Read a member character's card again from its owner's workspace, keep it, and tell the party when it changed.
/// Never fails: a card that can't be read stays as it was.
///
/// Membership is read again under the owner's refresh lock, which [`forget_cards`] takes too: a character that left
/// its party while this was under way gets no card back and its old party hears nothing. The snapshot is still
/// answered (the DM's view checked membership itself).
pub(crate) async fn refresh_card(
    state: &AppState,
    owner: &str,
    character: &str,
    workspace: &Workspace,
    party: &str,
) -> Option<borealis_lib::dto::CharacterSnapshot> {
    let _one = state.hub.cards.refreshing(owner).await;
    let member = matches!(party_of(state, owner, character).await, Ok(Some(p)) if p == party);
    let combat = in_combat(party);
    let hub = std::sync::Arc::clone(&state.hub);
    let (owner_id, id) = (owner.to_string(), character.to_string());
    let read = workspace
        .blocking(move |app| {
            let Some(snapshot) = snapshot_of(app, &id)? else {
                return Ok(None);
            };
            let changed = if member {
                hub.cards
                    .put(&owner_id, &id, Card::from_snapshot(&snapshot), combat)
            } else {
                crate::hub::Changed::default()
            };
            Ok(Some((snapshot, changed)))
        })
        .await;
    match read {
        Ok(Some((snapshot, changed))) => {
            // 0.3.3: the DM hears of a card only when the DM's view of it changed (never for HP alone), the other
            // members when theirs did (the band in combat).
            if changed.dm || changed.players {
                publish_to(
                    state,
                    party,
                    "card",
                    Some(character),
                    changed.dm,
                    changed.players,
                )
                .await;
            }
            if changed.stored {
                crate::combat::card_changed(state, party, owner, character).await;
            }
            Some(snapshot)
        }
        Ok(None) => None,
        Err(e) => {
            tracing::warn!("party: reading a card of {owner}: {e:?}");
            None
        }
    }
}

/// Cards of characters that left their party (their rows are gone already). Each owner's refresh lock is taken first,
/// so a refresh under way finishes before its card is forgotten, and one after it sees the membership gone.
async fn forget_cards(state: &AppState, gone: &[(String, String)]) {
    for (owner, character) in gone {
        let _one = state.hub.cards.refreshing(owner).await;
        let hub = std::sync::Arc::clone(&state.hub);
        let (o, c) = (owner.clone(), character.clone());
        let _ = tokio::task::spawn_blocking(move || hub.cards.forget(&o, &c)).await;
    }
}

/// A character joined: the party hears, the owner's list changes, and its card is read when its owner has the
/// workspace open.
async fn joined(state: &AppState, owner: &str, character: &str, party: &str) {
    publish(state, party, "roster", Some(character), true).await;
    tell_mine(state, owner, "parties");
    if let Some(workspace) = state.workspaces.loaded_now(owner) {
        refresh_card(state, owner, character, &workspace, party).await;
    }
}

/// A character that left its party: its card goes, the party hears, the owner's list changes.
async fn left(state: &AppState, owner: &str, character: &str, party: &str) {
    forget_cards(state, &[(owner.to_string(), character.to_string())]).await;
    publish(state, party, "roster", Some(character), true).await;
    tell_mine(state, owner, "parties");
}

/// Upstream commands that change what a card shows, besides those that write character files (`sync.rs`): they
/// change only the open session. A test checks each is a command the server offers.
pub const PLAY_COMMANDS: [&str; 10] = [
    "take_damage",
    "regain_hit_points",
    "set_temp_hit_points",
    "spend_hit_die",
    "restore_hit_die",
    "short_rest",
    "long_rest",
    "set_concentration",
    "spend_spell_slot",
    "set_exhaustion",
];

/// Commands that name a character but change nothing about it: no card is read after them.
const READS: [&str; 13] = [
    "get_snapshot",
    "get_slot_options",
    "list_character_options",
    "explain_stat",
    "render_sheet",
    "export_sheet",
    "export_character",
    "get_element",
    "list_added_pick_options",
    "preview_source_restrictions",
    "preview_convert_to_2024",
    "get_compatibility_report",
    "party_sync_get",
];

/// Upstream commands that reload the account's content (or recheck its characters against it): any card's class
/// line and numbers may change, so all of the owner's member cards are read again. A test checks each is offered.
/// The jobs (`job_start`) refresh nothing when they end; a card catches up on the owner's next command.
pub const CONTENT_COMMANDS: [&str; 14] = [
    "bundle_accept",
    "recheck_missing_content",
    "import_pack",
    "install_import_preview",
    "set_pack_enabled",
    "remove_pack",
    "set_source_edition",
    "remove_index",
    "update_indexes",
    "clear_downloaded_content",
    "remove_5etools_url",
    "update_5etools_url",
    "update_all_content",
    "import_5etools",
];

/// What a command's success means for the cards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardTarget {
    /// Nothing.
    None,
    /// This character may have changed.
    Character(String),
    /// This character was deleted: it leaves its party.
    Deleted(String),
    /// Any of the account's characters may have changed.
    All,
}

/// Which cards a command may change, from its name and arguments (taken before it runs). Every command that names
/// one character and isn't a read counts, the [`PLAY_COMMANDS`] among them.
pub fn card_target(command: &str, args: &serde_json::Map<String, Value>) -> CardTarget {
    if CONTENT_COMMANDS.contains(&command) {
        return CardTarget::All;
    }
    if READS.contains(&command) && !PLAY_COMMANDS.contains(&command) {
        return CardTarget::None;
    }
    let key = if command == "party_sync_push" {
        "id"
    } else {
        "characterId"
    };
    let Some(id) = args
        .get(key)
        .and_then(Value::as_str)
        .filter(|id| valid_character_id(id))
    else {
        return CardTarget::None;
    };
    if command == "delete_character" {
        CardTarget::Deleted(id.to_string())
    } else {
        CardTarget::Character(id.to_string())
    }
}

/// The account's content changed outside an RPC command (a homebrew save or delete, share links added): its party
/// cards are read again from its open workspace, so the DM's roster shows the new classes and numbers (0.3.3).
pub(crate) async fn content_changed(state: &AppState, owner: &str) {
    if let Some(workspace) = state.workspaces.loaded_now(owner) {
        after_command(state, owner, &workspace, CardTarget::All).await;
    }
}

/// After a command of `owner`'s worked (`rpc.rs`): refresh the cards it may have changed, and take a deleted
/// character out of its party. Never fails the command.
pub async fn after_command(
    state: &AppState,
    owner: &str,
    workspace: &Workspace,
    target: CardTarget,
) {
    if !crate::config::features().parties || target == CardTarget::None {
        return;
    }
    let result: ApiResult<()> = async {
        match target {
            CardTarget::None => {}
            CardTarget::Character(id) => {
                if let Some(party) = party_of(state, owner, &id).await? {
                    refresh_card(state, owner, &id, workspace, &party).await;
                }
            }
            CardTarget::Deleted(id) => {
                let Some(party) = party_of(state, owner, &id).await? else {
                    return Ok(());
                };
                sqlx::query("DELETE FROM party_members WHERE owner_id = $1 AND character_id = $2")
                    .bind(owner)
                    .bind(&id)
                    .execute(state.db.pool())
                    .await?;
                sqlx::query("DELETE FROM party_offers WHERE owner_id = $1 AND character_id = $2")
                    .bind(owner)
                    .bind(&id)
                    .execute(state.db.pool())
                    .await?;
                tracing::info!("party: left party={party} owner={owner} (deleted)");
                left(state, owner, &id, &party).await;
            }
            CardTarget::All => {
                let rows = sqlx::query(
                    "SELECT character_id, party_id FROM party_members WHERE owner_id = $1",
                )
                .bind(owner)
                .fetch_all(state.db.pool())
                .await?;
                for row in rows {
                    let (id, party): (String, String) = (row.try_get(0)?, row.try_get(1)?);
                    if valid_character_id(&id) {
                        refresh_card(state, owner, &id, workspace, &party).await;
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    if let Err(e) = result {
        tracing::warn!("party: after a command of {owner}: {e:?}");
    }
}

// The DM's read-only view of a member character.

/// Views one DM may have under way at once, and workspaces of offline players a view may open in ten minutes.
const VIEWS_AT_ONCE: usize = 2;
const OPENS: usize = 4;

static VIEWING: LazyLock<Mutex<HashMap<String, usize>>> = LazyLock::new(Mutex::default);

/// One view under way; counted down when dropped.
struct Viewing(String);

impl Viewing {
    fn start(dm: &str) -> Option<Self> {
        let mut map = VIEWING.lock().ok()?;
        let n = map.entry(dm.to_string()).or_insert(0);
        if *n >= VIEWS_AT_ONCE {
            return None;
        }
        *n += 1;
        Some(Self(dm.to_string()))
    }
}

impl Drop for Viewing {
    fn drop(&mut self) {
        if let Ok(mut map) = VIEWING.lock() {
            if let Some(n) = map.get_mut(&self.0) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    map.remove(&self.0);
                }
            }
        }
    }
}

/// What a view leaves out of upstream's snapshot: where the owner keeps things, and their library's own marks.
const VIEW_HIDES: [&str; 5] = ["displayPath", "dirty", "vaultNote", "group", "favorite"];

/// The keys of a sheet's `play` the DM's read-only view keeps (0.3.3, an allowlist: "DM should never know players
/// HP and resources"): speeds, exhaustion, Heroic Inspiration, defenses, senses, advantages, save DCs, masteries, the
/// concentration spells it knows, and whether it concentrates now. Never hit points, hit dice, slots, resources,
/// trackers, toggles, free casts, Wild Shape or the player's notes.
pub(crate) const DM_SHEET_PLAY: &[&str] = &[
    "speeds",
    "exhaustion",
    "inspiration",
    "defenses",
    "senses",
    "advantages",
    "initiativeAdvantage",
    "saveDcs",
    "masteries",
    "concentrationSpells",
    "compatibility",
    "concentration",
];

/// A sheet snapshot as the DM may read it (0.3.3): `play` cut to [`DM_SHEET_PLAY`] (concentration as yes/no), no
/// maximum HP, no spell slot tables, and companions without their hit points.
pub(crate) fn dm_sheet(snapshot: &mut Value) {
    let Some(map) = snapshot.as_object_mut() else {
        return;
    };
    if let Some(Value::Object(play)) = map.get_mut("play") {
        let concentrating = play.get("concentration").is_some_and(|c| !c.is_null());
        play.retain(|k, _| DM_SHEET_PLAY.contains(&k.as_str()));
        play.insert("concentration".into(), json!(concentrating));
    }
    if let Some(Value::Object(stats)) = map.get_mut("stats") {
        stats.remove("hpMax");
    }
    if let Some(Value::Object(casting)) = map.get_mut("spellcasting") {
        casting.remove("slots");
        casting.remove("multiclassSlots");
        if let Some(Value::Array(classes)) = casting.get_mut("classes") {
            for c in classes.iter_mut().filter_map(Value::as_object_mut) {
                c.remove("pact");
            }
        }
    }
    let no_hp = |c: &mut serde_json::Map<String, Value>| {
        c.remove("hp");
        c.remove("hpNote");
    };
    if let Some(Value::Object(c)) = map.get_mut("companion") {
        no_hp(c);
    }
    if let Some(Value::Array(list)) = map.get_mut("companions") {
        list.iter_mut()
            .filter_map(Value::as_object_mut)
            .for_each(no_hp);
    }
}

/// `POST /api/parties/{party}/members/{owner}/{character}/view/snapshot`: the DM reads a member character, as its
/// owner has it now, without opening a session or writing anything. Owner and character come from the member row;
/// nothing else of the request is used. Since 0.3.3, while the party is in a combat it has none of the sheet's hit
/// points or resources ([`dm_sheet`]).
pub async fn view_snapshot(
    State(state): State<AppState>,
    user: User,
    Path((party, owner, character)): Path<(String, String, String)>,
) -> ApiResult<Json<Value>> {
    run_by(&state, &user, &party).await?;
    id_arg(&owner)?;
    if !valid_character_id(&character) {
        return Err(not_found());
    }
    let row = sqlx::query(
        "SELECT m.owner_id, m.character_id, u.username FROM party_members m JOIN users u ON u.id = m.owner_id
         WHERE m.party_id = $1 AND m.owner_id = $2 AND m.character_id = $3",
    )
    .bind(&party)
    .bind(&owner)
    .bind(&character)
    .fetch_optional(state.db.pool())
    .await?
    .ok_or_else(not_found)?;
    let (owner, character, owner_name): (String, String, String) =
        (row.try_get(0)?, row.try_get(1)?, row.try_get(2)?);
    let _viewing = Viewing::start(&user.id).ok_or(ApiError::TooMany)?;
    let workspace = match state.workspaces.loaded_now(&owner) {
        Some(workspace) => workspace,
        None => {
            if !take(&format!("view-open:{}", user.id), OPENS, TEN_MINUTES) {
                return Err(ApiError::TooMany);
            }
            state.workspaces.get(&owner).await?
        }
    };
    let pseq = seq_for(&state, &party, Role::Dm);
    let snapshot = refresh_card(&state, &owner, &character, &workspace, &party)
        .await
        .ok_or_else(not_found)?;
    let mut snapshot = serde_json::to_value(&snapshot).map_err(internal)?;
    if let Some(map) = snapshot.as_object_mut() {
        for key in VIEW_HIDES {
            map.remove(key);
        }
    }
    // 0.3.3: during a combat the DM reads no HP or resources of the sheet (the build, yes).
    let fighting = in_combat(&party);
    if fighting {
        dm_sheet(&mut snapshot);
    }
    crate::rpc::scrub_paths(&mut snapshot);
    let (card, file_state) = cards(&state, &[(owner, character)])
        .await?
        .pop()
        .unwrap_or((Card::default(), "unreadable"));
    let mut card = card_view_for(
        Viewer::Dm {
            in_combat: fighting,
        },
        &card,
        file_state,
    );
    card.insert("ownerName".into(), json!(owner_name));
    Ok(Json(json!({
        "snapshot": snapshot,
        "card": card,
        "pseq": pseq,
    })))
}

// An account being deleted (`admin.rs`): its parties go with it, and so do its characters in other parties.

/// What a deleted account's parties need to hear, read before the delete.
pub struct Departing {
    /// Parties it ran: each with the members that go with it.
    ran: Vec<(String, Vec<(String, String)>)>,
    /// Other parties it had characters in.
    played: Vec<String>,
}

/// Read who must hear of an account's delete, before it happens.
pub async fn before_account_delete(state: &AppState, user: &str) -> ApiResult<Departing> {
    let mut ran = Vec::new();
    for row in sqlx::query("SELECT id FROM parties WHERE dm_id = $1")
        .bind(user)
        .fetch_all(state.db.pool())
        .await?
    {
        let party: String = row.try_get(0)?;
        let members =
            sqlx::query("SELECT owner_id, character_id FROM party_members WHERE party_id = $1")
                .bind(&party)
                .fetch_all(state.db.pool())
                .await?
                .iter()
                .map(|r| Ok((r.try_get(0)?, r.try_get(1)?)))
                .collect::<sqlx::Result<Vec<(String, String)>>>()?;
        ran.push((party, members));
    }
    let played = sqlx::query(
        "SELECT DISTINCT m.party_id FROM party_members m JOIN parties p ON p.id = m.party_id
         WHERE m.owner_id = $1 AND p.dm_id <> $1",
    )
    .bind(user)
    .fetch_all(state.db.pool())
    .await?
    .iter()
    .map(|r| r.try_get(0))
    .collect::<sqlx::Result<Vec<String>>>()?;
    Ok(Departing { ran, played })
}

/// After the delete: the parties it ran are gone for their players, the others have lost its characters.
pub async fn after_account_delete(state: &AppState, user: &str, departing: Departing) {
    for (party, members) in departing.ran {
        state.hub.forget_party(&party);
        forget_cards(state, &members).await;
        let mut owners: Vec<String> = members.into_iter().map(|(o, _)| o).collect();
        owners.sort();
        owners.dedup();
        for owner in owners.iter().filter(|o| *o != user) {
            tell_mine(state, owner, "parties");
        }
    }
    for party in departing.played {
        publish(state, &party, "roster", None, true).await;
    }
    let _one = state.hub.cards.refreshing(user).await;
    let hub = std::sync::Arc::clone(&state.hub);
    let owner = user.to_string();
    let _ = tokio::task::spawn_blocking(move || hub.cards.forget_owner(&owner)).await;
}
