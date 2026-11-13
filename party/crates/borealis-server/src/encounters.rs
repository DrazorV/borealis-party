//! Encounters: a party's DM builds them from their own installed creatures; players see a player-visible one as a
//! projection. The API is `party/docs/encounters-api.md`; the stat block copies are `party-encounter`.
//!
//! * **Authorization** is read from the database on every request: the DM of the encounter's party
//!   (`parties.dm_id`) may do everything; a player of the party (a `party_members` row) may only read an encounter
//!   that is player-visible (`player_visible` and `ready` or `live`); everyone else, and every id a caller may not see,
//!   gets one fixed 404. Request bodies are read only after the DM check, so a malformed body changes nothing.
//! * **One serializer for players:** [`view_for`]. Hidden creatures are not in it at all; revealed ones come with
//!   their whole stat block (D20 b); the DM's names, notes, element ids and stat block copies' sources never do.
//! * **Events:** every write tells the DM. Players hear of it only when their projection changed (hashed before and
//!   after the write), so editing a draft or a hidden creature moves nothing a player can count or time.
//! * **No difficulty:** there is no XP budget or rating (the user's D16); an encounter uses its party's edition (D23 b)
//!   and only warns about creatures and characters of the other one.
//! * **Controlled creatures** (pets, summons, mounts): a creature can belong to another one of the encounter, acting
//!   right after it or on its own initiative. Each has its own hidden switch: revealing an owner reveals none of its
//!   creatures.
//!
//! Like `parties.rs`, nothing here goes to the admin page's activity log (D40); the server log gets ids only.

use std::collections::{BTreeMap, BTreeSet};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::Json;
use borealis_content::Element;
use party_encounter::creature::{player_view, Snapshot};
use party_encounter::overrides::{refresh as refresh_block, Choice, Overrides, OverridesForm};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::auth::User;
use crate::db::now;
use crate::error::{ApiError, ApiResult};
use crate::hub::PARTY_EVENT;
use crate::AppState;

/// Encounters one party may have, creatures one encounter, and its effects and rules.
const MAX_ENCOUNTERS: i64 = 50;
const MAX_CREATURES: i64 = 60;
const MAX_ADD: i64 = 20;
const MAX_EFFECTS: i64 = 40;
const MAX_RULES: i64 = 20;
const MAX_FLAGS: usize = 20;
const RULESETS: [&str; 2] = ["dnd2024", "dnd2014"];
/// The event kind on `party://party`.
const KIND: &str = "encounters";

/// The one answer for an encounter (or anything in it) the caller may not see, or that doesn't exist.
fn not_found() -> ApiError {
    ApiError::NotFound("There is no such encounter here.".into())
}

/// The parties' own answer for a party the caller isn't in.
fn no_party() -> ApiError {
    ApiError::NotFound("There is no such party, invite or character here.".into())
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

fn invalid(message: &str) -> ApiError {
    ApiError::Invalid(message.into())
}

/// The encounter routes, while parties are on in the server config (they are a party feature).
pub fn routes() -> axum::Router<AppState> {
    routes_if(crate::config::features().parties)
}

/// The encounter routes when `on`; none otherwise.
pub(crate) fn routes_if(on: bool) -> axum::Router<AppState> {
    use axum::routing::{get, patch, post};
    if !on {
        return axum::Router::new();
    }
    axum::Router::new()
        .route("/api/parties/{party}/encounters", get(list).post(create))
        .route(
            "/api/encounters/{encounter}",
            get(read).patch(update).delete(remove),
        )
        .route("/api/encounters/{encounter}/creatures", post(add_creatures))
        .route(
            "/api/encounters/{encounter}/creatures/{creature}",
            patch(update_creature).delete(remove_creature),
        )
        .route(
            "/api/encounters/{encounter}/creatures/{creature}/refresh",
            post(refresh_creature),
        )
        .route(
            "/api/encounters/{encounter}/creatures/{creature}/swap",
            post(swap_creature),
        )
        .route("/api/encounters/{encounter}/effects", post(add_effect))
        .route(
            "/api/encounters/{encounter}/effects/{effect}",
            patch(update_effect).delete(remove_effect),
        )
        .route("/api/encounters/{encounter}/rules", post(add_rule))
        .route(
            "/api/encounters/{encounter}/rules/{rule}",
            patch(update_rule).delete(remove_rule),
        )
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, private"),
        ))
}

// Checks of what clients send.

/// An id from a path: the server's ids are UUIDs; anything else is the same 404 as an id nobody has.
fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A request body, read only once the caller is known to be the DM.
fn body<T: DeserializeOwned + Default>(bytes: &Bytes) -> ApiResult<T> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(bytes).map_err(|e| {
        ApiError::Invalid(format!(
            "The request doesn't fit: {}",
            e.to_string().split(" at line").next().unwrap_or_default()
        ))
    })
}

/// Trimmed text of `min..=max` characters, no control characters (new lines only where `lines`).
fn text(value: &str, min: usize, max: usize, lines: bool, message: &str) -> ApiResult<String> {
    let t = value.trim();
    let n = t.chars().count();
    let ok = (min..=max).contains(&n)
        && t.chars()
            .all(|c| !c.is_control() || (lines && (c == '\n' || c == '\t')));
    if ok {
        Ok(t.to_string())
    } else {
        Err(invalid(message))
    }
}

fn name_arg(v: &str) -> ApiResult<String> {
    text(v, 1, 80, false, "Encounter names are 1 to 80 characters.")
}
fn public_arg(v: &str) -> ApiResult<String> {
    text(v, 1, 60, false, "Names players see are 1 to 60 characters.")
}
fn label_arg(v: &str) -> ApiResult<String> {
    text(v, 1, 80, false, "Labels are 1 to 80 characters.")
}
fn notes_arg(v: &str) -> ApiResult<String> {
    text(v, 0, 4_000, true, "Notes are up to 4000 characters.")
}
fn hp_mode_arg(v: &str) -> ApiResult<String> {
    match v {
        "average" | "rolled" => Ok(v.to_string()),
        _ => Err(invalid("Hit points are average or rolled.")),
    }
}
fn init_mode_arg(v: &str) -> ApiResult<String> {
    match v {
        "after_owner" | "own" => Ok(v.to_string()),
        _ => Err(invalid(
            "A controlled creature acts after its owner, or on its own initiative.",
        )),
    }
}
fn ranged(
    v: Option<i64>,
    range: std::ops::RangeInclusive<i64>,
    message: &str,
) -> ApiResult<Option<i64>> {
    match v {
        Some(n) if !range.contains(&n) => Err(invalid(message)),
        other => Ok(other),
    }
}

/// `null` as `Some(None)`: a field that can be cleared as well as left out.
fn some<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

fn flag(on: bool) -> i64 {
    i64::from(on)
}

// Rows.

/// How the caller sees an encounter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    /// The party's DM.
    Dm,
    /// A player of the party.
    Player,
}

#[derive(Debug, Clone)]
struct Encounter {
    id: String,
    party_id: String,
    name: String,
    public_name: String,
    status: String,
    player_visible: bool,
    hp_mode: String,
    dm_notes: String,
    created_at: i64,
    updated_at: i64,
    /// The party's edition, which the encounter uses (D23 b).
    ruleset: String,
    dm_id: String,
}

impl Encounter {
    /// Players see it exactly now.
    fn visible(&self) -> bool {
        self.player_visible && matches!(self.status.as_str(), "ready" | "live")
    }
}

const ENCOUNTER_COLUMNS: &str =
    "e.id, e.party_id, e.name, e.public_name, e.status, e.player_visible, \
     e.hp_mode, e.dm_notes, e.created_at, e.updated_at, p.ruleset, p.dm_id";

fn encounter_from(row: &sqlx::any::AnyRow) -> sqlx::Result<Encounter> {
    Ok(Encounter {
        id: row.try_get(0)?,
        party_id: row.try_get(1)?,
        name: row.try_get(2)?,
        public_name: row.try_get(3)?,
        status: row.try_get(4)?,
        player_visible: row.try_get::<i64, _>(5)? != 0,
        hp_mode: row.try_get(6)?,
        dm_notes: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        ruleset: row.try_get(10)?,
        dm_id: row.try_get(11)?,
    })
}

async fn encounter_row(state: &AppState, id: &str) -> ApiResult<Option<Encounter>> {
    let row = sqlx::query(&format!(
        "SELECT {ENCOUNTER_COLUMNS} FROM encounters e JOIN parties p ON p.id = e.party_id WHERE e.id = $1"
    ))
    .bind(id)
    .fetch_optional(state.db.pool())
    .await?;
    Ok(row.as_ref().map(encounter_from).transpose()?)
}

async fn is_member(state: &AppState, user: &str, party: &str) -> ApiResult<bool> {
    Ok(
        sqlx::query("SELECT 1 FROM party_members WHERE party_id = $1 AND owner_id = $2")
            .bind(party)
            .bind(user)
            .fetch_optional(state.db.pool())
            .await?
            .is_some(),
    )
}

/// The encounter as the caller may see it: the DM always, a player of its party only while it is player-visible.
async fn access(state: &AppState, user: &User, id: &str) -> ApiResult<(Encounter, Viewer)> {
    if !id_ok(id) {
        return Err(not_found());
    }
    let enc = encounter_row(state, id).await?.ok_or_else(not_found)?;
    if enc.dm_id == user.id {
        return Ok((enc, Viewer::Dm));
    }
    if enc.visible() && is_member(state, &user.id, &enc.party_id).await? {
        return Ok((enc, Viewer::Player));
    }
    Err(not_found())
}

/// The encounter when the caller is its party's DM; the same 404 otherwise, players included.
async fn run_by(state: &AppState, user: &User, id: &str) -> ApiResult<Encounter> {
    match access(state, user, id).await? {
        (enc, Viewer::Dm) => Ok(enc),
        _ => Err(not_found()),
    }
}

/// How the caller sees a party they run or play in.
async fn party_access(state: &AppState, user: &User, party: &str) -> ApiResult<Viewer> {
    if !id_ok(party) {
        return Err(no_party());
    }
    let row = sqlx::query("SELECT dm_id FROM parties WHERE id = $1")
        .bind(party)
        .fetch_optional(state.db.pool())
        .await?
        .ok_or_else(no_party)?;
    let dm: String = row.try_get(0)?;
    if dm == user.id {
        Ok(Viewer::Dm)
    } else if is_member(state, &user.id, party).await? {
        Ok(Viewer::Player)
    } else {
        Err(no_party())
    }
}

#[derive(Debug, Clone)]
struct Creature {
    id: String,
    sort: i64,
    dm_label: String,
    public_name: String,
    element_id: String,
    pack_id: String,
    pack_rev: String,
    snapshot: Snapshot,
    overrides: Overrides,
    hp_mode: String,
    hp_max: Option<i64>,
    hidden: bool,
    revealed_at: Option<i64>,
    owner: Option<String>,
    init_mode: String,
}

impl Creature {
    /// Its hit points at the start: a number the DM typed, else the block's average (rolled ones are rolled when the
    /// combat starts, D22).
    fn start_hp(&self) -> u32 {
        match (self.hp_mode.as_str(), self.hp_max) {
            ("fixed", Some(n)) => u32::try_from(n).unwrap_or(1),
            _ => self.snapshot.average_hp(&self.overrides),
        }
    }
}

async fn creatures(state: &AppState, encounter: &str) -> ApiResult<Vec<Creature>> {
    let rows = sqlx::query(
        "SELECT id, sort, dm_label, public_name, element_id, pack_id, pack_rev, statblock_json, overrides_json,
                hp_mode, hp_max, visibility, revealed_at, owner_creature_id, init_mode
         FROM encounter_creatures WHERE encounter_id = $1 ORDER BY sort, id",
    )
    .bind(encounter)
    .fetch_all(state.db.pool())
    .await?;
    let mut out = Vec::new();
    for row in &rows {
        let block: String = row.try_get(7)?;
        let overrides: String = row.try_get(8)?;
        let id: String = row.try_get(0)?;
        // A copy that doesn't read any more (a later model change) is left out rather than failing the encounter.
        let Ok(snapshot) = serde_json::from_str::<Snapshot>(&block) else {
            tracing::warn!("encounters: creature {id} has a stat block that doesn't read");
            continue;
        };
        out.push(Creature {
            id,
            sort: row.try_get(1)?,
            dm_label: row.try_get(2)?,
            public_name: row.try_get(3)?,
            element_id: row.try_get(4)?,
            pack_id: row.try_get(5)?,
            pack_rev: row.try_get(6)?,
            snapshot,
            overrides: serde_json::from_str(&overrides).unwrap_or_default(),
            hp_mode: row.try_get(9)?,
            hp_max: row.try_get(10)?,
            hidden: row.try_get::<String, _>(11)? != "revealed",
            revealed_at: row.try_get(12)?,
            owner: row.try_get(13)?,
            init_mode: row.try_get(14)?,
        });
    }
    Ok(out)
}

async fn effects(state: &AppState, encounter: &str) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT id, creature_id, sort, name, detail, source_creature_id, duration_turns, charges, concentration,
                slot_level, modifiers_json
         FROM encounter_effects WHERE encounter_id = $1 ORDER BY sort, id",
    )
    .bind(encounter)
    .fetch_all(state.db.pool())
    .await?;
    rows.iter()
        .map(|r| {
            let modifiers: String = r.try_get(10)?;
            Ok(json!({
                "id": r.try_get::<String, _>(0)?,
                "creatureId": r.try_get::<String, _>(1)?,
                "sort": r.try_get::<i64, _>(2)?,
                "name": r.try_get::<String, _>(3)?,
                "detail": r.try_get::<String, _>(4)?,
                "sourceCreatureId": r.try_get::<Option<String>, _>(5)?,
                "durationTurns": r.try_get::<Option<i64>, _>(6)?,
                "charges": r.try_get::<Option<i64>, _>(7)?,
                "concentration": r.try_get::<i64, _>(8)? != 0,
                "slotLevel": r.try_get::<Option<i64>, _>(9)?,
                "modifiers": serde_json::from_str::<Value>(&modifiers).unwrap_or_else(|_| json!({})),
            }))
        })
        .collect::<sqlx::Result<_>>()
        .map_err(Into::into)
}

async fn rules(state: &AppState, encounter: &str) -> ApiResult<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT id, sort, name, text, creature_id, ends_when, active, flags_json
         FROM encounter_rules WHERE encounter_id = $1 ORDER BY sort, id",
    )
    .bind(encounter)
    .fetch_all(state.db.pool())
    .await?;
    rows.iter()
        .map(|r| {
            let flags: String = r.try_get(7)?;
            Ok(json!({
                "id": r.try_get::<String, _>(0)?,
                "sort": r.try_get::<i64, _>(1)?,
                "name": r.try_get::<String, _>(2)?,
                "text": r.try_get::<String, _>(3)?,
                "creatureId": r.try_get::<Option<String>, _>(4)?,
                "endsWhen": r.try_get::<String, _>(5)?,
                "active": r.try_get::<i64, _>(6)? != 0,
                "flags": serde_json::from_str::<Value>(&flags).unwrap_or_else(|_| json!([])),
            }))
        })
        .collect::<sqlx::Result<_>>()
        .map_err(Into::into)
}

// Edition warnings (D23 b).

/// The editions of the party's readable member characters.
async fn member_rulesets(state: &AppState, party: &str) -> ApiResult<Vec<String>> {
    let rows = sqlx::query("SELECT owner_id, character_id FROM party_members WHERE party_id = $1")
        .bind(party)
        .fetch_all(state.db.pool())
        .await?;
    let mut pairs = Vec::new();
    for row in &rows {
        let owner: String = row.try_get(0)?;
        let character: String = row.try_get(1)?;
        if crate::parties::valid_character_id(&character) {
            let file = state
                .workspaces
                .dir(&owner)
                .join("characters")
                .join(format!("{character}.json"));
            pairs.push((owner, character, file));
        }
    }
    let hub = std::sync::Arc::clone(&state.hub);
    tokio::task::spawn_blocking(move || {
        pairs
            .into_iter()
            .filter_map(|(owner, character, file)| {
                if let Some(r) = hub
                    .cards
                    .get(&owner, &character)
                    .and_then(|card| card.ruleset)
                    .and_then(|r| r.as_str().map(str::to_string))
                {
                    return Some(r);
                }
                if !file.symlink_metadata().is_ok_and(|m| m.is_file()) {
                    return None;
                }
                let c = borealis_character::Character::load(&file).ok()?;
                serde_json::to_value(c.ruleset())
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
            })
            .collect()
    })
    .await
    .map_err(internal)
}

/// What the DM should know: creatures and characters of the other edition. No difficulty (D16).
fn warnings(ruleset: &str, list: &[Creature], member_rulesets: &[String]) -> Vec<Value> {
    let mut out = Vec::new();
    let other: Vec<&Creature> = list
        .iter()
        .filter(|c| {
            RULESETS.contains(&c.snapshot.ruleset.as_str()) && c.snapshot.ruleset != ruleset
        })
        .collect();
    if !other.is_empty() {
        out.push(json!({
            "kind": "edition",
            "creatureIds": other.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
            "creatures": other.iter().map(|c| json!({
                "creatureId": c.id, "ruleset": c.snapshot.ruleset, "mapsTo": c.snapshot.maps_to,
            })).collect::<Vec<_>>(),
            "text": "Some stat blocks are from the other edition.",
        }));
    }
    let others = member_rulesets
        .iter()
        .filter(|r| RULESETS.contains(&r.as_str()) && r.as_str() != ruleset)
        .count();
    if others > 0 {
        out.push(json!({
            "kind": "characters_edition", "count": others,
            "text": format!("{others} of the party's characters use the other edition's rules."),
        }));
    }
    out
}

// Views.

fn creature_dm(c: &Creature) -> Value {
    let printed = c.snapshot.printed();
    json!({
        "id": c.id,
        "sort": c.sort,
        "kind": c.snapshot.kind.as_str(),
        "dmLabel": c.dm_label,
        "publicName": c.public_name,
        "hidden": c.hidden,
        "revealedAt": c.revealed_at,
        "ownerId": c.owner,
        "initMode": c.init_mode,
        "elementId": c.element_id,
        "packId": c.pack_id,
        "packRev": c.pack_rev,
        "name": c.snapshot.name,
        "source": c.snapshot.source,
        "ruleset": c.snapshot.ruleset,
        "mapsTo": c.snapshot.maps_to,
        "cr": c.snapshot.challenge(),
        "type": c.snapshot.type_word(),
        "size": c.snapshot.sizes(),
        "hp": c.overrides.hp.as_ref().map(|h| json!(h.value)).or_else(|| printed.hp.as_ref().map(|h| json!(h))),
        "hpMode": c.hp_mode,
        "hpMax": c.hp_max,
        "ac": c.snapshot.ac(&c.overrides),
        "printed": printed,
        "overrides": c.overrides,
        "modified": c.overrides.changes_block(),
        "statBlock": c.snapshot.block(&c.overrides),
    })
}

/// The only serializer of an encounter. For a player: the public name, status and the revealed creatures with their
/// stat blocks, nothing else (callers check [`Encounter::visible`] first). For the DM: everything.
fn view_for(
    viewer: Viewer,
    enc: &Encounter,
    list: &[Creature],
    dm: Option<(Vec<Value>, Vec<Value>, Vec<Value>)>,
) -> Value {
    if viewer == Viewer::Player {
        let revealed: BTreeSet<&str> = list
            .iter()
            .filter(|c| !c.hidden)
            .map(|c| c.id.as_str())
            .collect();
        let shown: Vec<Value> = list
            .iter()
            .filter(|c| !c.hidden)
            .map(|c| {
                let hp = c.start_hp();
                // An owner players can't see is never named.
                let owner = c.owner.as_deref().filter(|o| revealed.contains(o));
                player_view(
                    &c.id,
                    &c.public_name,
                    &c.snapshot,
                    &c.overrides,
                    (hp, hp),
                    &[],
                    owner,
                )
            })
            .collect();
        return json!({
            "id": enc.id,
            "partyId": enc.party_id,
            "publicName": enc.public_name,
            "status": enc.status,
            "creatures": shown,
        });
    }
    let (effects, rules, warnings) = dm.unwrap_or_default();
    json!({
        "id": enc.id,
        "partyId": enc.party_id,
        "name": enc.name,
        "publicName": enc.public_name,
        "ruleset": enc.ruleset,
        "status": enc.status,
        "playerVisible": enc.player_visible,
        "visibleToPlayers": enc.visible(),
        "hpMode": enc.hp_mode,
        "dmNotes": enc.dm_notes,
        "createdAt": enc.created_at,
        "updatedAt": enc.updated_at,
        "creatures": list.iter().map(creature_dm).collect::<Vec<_>>(),
        "effects": effects,
        "rules": rules,
        "warnings": warnings,
    })
}

async fn dm_view(state: &AppState, id: &str) -> ApiResult<Value> {
    let enc = encounter_row(state, id).await?.ok_or_else(not_found)?;
    let list = creatures(state, id).await?;
    let members = member_rulesets(state, &enc.party_id).await?;
    let extra = (
        effects(state, id).await?,
        rules(state, id).await?,
        warnings(&enc.ruleset, &list, &members),
    );
    Ok(view_for(Viewer::Dm, &enc, &list, Some(extra)))
}

/// What players see of an encounter now: its projection, or nothing while it isn't player-visible (or is gone).
async fn players_see(state: &AppState, id: &str) -> ApiResult<Option<Value>> {
    let Some(enc) = encounter_row(state, id).await? else {
        return Ok(None);
    };
    if !enc.visible() {
        return Ok(None);
    }
    let list = creatures(state, id).await?;
    Ok(Some(view_for(Viewer::Player, &enc, &list, None)))
}

// Events.

/// Tell the party's DM that an encounter changed, and its players too when `players` (their projection changed).
async fn publish(state: &AppState, party: &str, encounter: &str, players: bool) {
    if !crate::config::features().parties {
        return;
    }
    let dm = match sqlx::query("SELECT dm_id FROM parties WHERE id = $1")
        .bind(party)
        .fetch_optional(state.db.pool())
        .await
    {
        Ok(Some(row)) => row.try_get::<String, _>(0).ok(),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!("encounters: telling party {party} of a change: {e:?}");
            None
        }
    };
    let Some(dm) = dm else { return };
    let owners: Vec<String> = if players {
        match sqlx::query(
            "SELECT DISTINCT owner_id FROM party_members WHERE party_id = $1 AND owner_id <> $2 ORDER BY owner_id",
        )
        .bind(party)
        .bind(&dm)
        .fetch_all(state.db.pool())
        .await
        {
            Ok(rows) => rows.iter().filter_map(|r| r.try_get(0).ok()).collect(),
            Err(e) => {
                tracing::warn!("encounters: reading party {party}'s players: {e:?}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let (dm_seq, players_seq) = state.hub.bump(party, players);
    state.hub.deliver(
        &dm,
        PARTY_EVENT,
        &json!({ "party": party, "pseq": dm_seq, "kind": KIND, "id": encounter }),
    );
    if let Some(seq) = players_seq {
        let payload = json!({ "party": party, "pseq": seq, "kind": KIND, "id": encounter });
        for owner in owners {
            state.hub.deliver(&owner, PARTY_EVENT, &payload);
        }
    }
}

/// After a DM write: stamp the encounter, tell whoever must hear, and answer the DM view.
async fn finish(
    state: &AppState,
    enc: &Encounter,
    before: Option<Value>,
) -> ApiResult<Json<Value>> {
    sqlx::query("UPDATE encounters SET updated_at = $1 WHERE id = $2")
        .bind(now())
        .bind(&enc.id)
        .execute(state.db.pool())
        .await?;
    let after = players_see(state, &enc.id).await?;
    publish(state, &enc.party_id, &enc.id, before != after).await;
    Ok(Json(dm_view(state, &enc.id).await?))
}

async fn count(state: &AppState, sql: &str, id: &str) -> ApiResult<i64> {
    Ok(sqlx::query(sql)
        .bind(id)
        .fetch_one(state.db.pool())
        .await?
        .try_get(0)?)
}

async fn next_sort(state: &AppState, table: &str, encounter: &str) -> ApiResult<i64> {
    count(
        state,
        &format!("SELECT COALESCE(MAX(sort), -1) + 1 FROM {table} WHERE encounter_id = $1"),
        encounter,
    )
    .await
}

// Encounters.

/// `GET /api/parties/{party}/encounters`.
pub async fn list(
    State(state): State<AppState>,
    user: User,
    Path(party): Path<String>,
) -> ApiResult<Json<Value>> {
    let viewer = party_access(&state, &user, &party).await?;
    let rows = sqlx::query(&format!(
        "SELECT {ENCOUNTER_COLUMNS} FROM encounters e JOIN parties p ON p.id = e.party_id
         WHERE e.party_id = $1 ORDER BY e.created_at DESC, e.id"
    ))
    .bind(&party)
    .fetch_all(state.db.pool())
    .await?;
    let all = rows
        .iter()
        .map(encounter_from)
        .collect::<sqlx::Result<Vec<_>>>()?;
    if viewer == Viewer::Player {
        let mut shown: Vec<&Encounter> = all.iter().filter(|e| e.visible()).collect();
        // Live first, then ready; newest first within each (the order of `all`).
        shown.sort_by_key(|e| e.status != "live");
        let list: Vec<Value> = shown
            .iter()
            .map(|e| json!({ "id": e.id, "publicName": e.public_name, "status": e.status }))
            .collect();
        return Ok(Json(json!({ "role": "player", "encounters": list })));
    }
    let mut out = Vec::new();
    for e in &all {
        let list = creatures(&state, &e.id).await?;
        out.push(json!({
            "id": e.id,
            "name": e.name,
            "publicName": e.public_name,
            "status": e.status,
            "playerVisible": e.player_visible,
            "visibleToPlayers": e.visible(),
            "creatureCount": list.len(),
            "hiddenCount": list.iter().filter(|c| c.hidden).count(),
            "updatedAt": e.updated_at,
        }));
    }
    Ok(Json(json!({ "role": "dm", "encounters": out })))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EncounterForm {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    public_name: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    player_visible: Option<bool>,
    #[serde(default)]
    hp_mode: Option<String>,
    #[serde(default)]
    dm_notes: Option<String>,
}

/// `POST /api/parties/{party}/encounters`: the party's DM only.
pub async fn create(
    State(state): State<AppState>,
    user: User,
    Path(party): Path<String>,
    bytes: Bytes,
) -> ApiResult<Json<Value>> {
    if party_access(&state, &user, &party).await? != Viewer::Dm {
        return Err(no_party());
    }
    let form: EncounterForm = body(&bytes)?;
    if form.status.is_some() || form.player_visible.is_some() {
        return Err(invalid("A new encounter starts as a draft."));
    }
    let name = name_arg(form.name.as_deref().unwrap_or_default())?;
    let public_name = match form.public_name.as_deref() {
        Some(p) => public_arg(p)?,
        None => public_arg(&name.chars().take(60).collect::<String>())?,
    };
    let hp_mode = hp_mode_arg(form.hp_mode.as_deref().unwrap_or("average"))?;
    let notes = notes_arg(form.dm_notes.as_deref().unwrap_or_default())?;
    if count(
        &state,
        "SELECT COUNT(*) FROM encounters WHERE party_id = $1",
        &party,
    )
    .await?
        >= MAX_ENCOUNTERS
    {
        return Err(ApiError::TooMany);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let at = now();
    sqlx::query(
        "INSERT INTO encounters (id, party_id, name, public_name, status, player_visible, hp_mode, dm_notes,
                                 created_at, updated_at)
         VALUES ($1, $2, $3, $4, 'draft', 0, $5, $6, $7, $8)",
    )
    .bind(&id)
    .bind(&party)
    .bind(&name)
    .bind(&public_name)
    .bind(&hp_mode)
    .bind(&notes)
    .bind(at)
    .bind(at)
    .execute(state.db.pool())
    .await?;
    tracing::info!("encounters: created {id} in party {party}");
    let enc = run_by(&state, &user, &id).await?;
    finish(&state, &enc, None).await
}

/// `GET /api/encounters/{encounter}`: the DM view, or a player's projection.
pub async fn read(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    match access(&state, &user, &id).await? {
        (_, Viewer::Dm) => Ok(Json(dm_view(&state, &id).await?)),
        (enc, Viewer::Player) => {
            let list = creatures(&state, &id).await?;
            Ok(Json(view_for(Viewer::Player, &enc, &list, None)))
        }
    }
}

/// `PATCH /api/encounters/{encounter}`.
pub async fn update(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    bytes: Bytes,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    let form: EncounterForm = body(&bytes)?;
    let before = players_see(&state, &id).await?;
    let name = form
        .name
        .as_deref()
        .map(name_arg)
        .transpose()?
        .unwrap_or_else(|| enc.name.clone());
    let public_name = form
        .public_name
        .as_deref()
        .map(public_arg)
        .transpose()?
        .unwrap_or_else(|| enc.public_name.clone());
    let status = match form.status.as_deref() {
        None => enc.status.clone(),
        Some(s @ ("draft" | "ready" | "ended")) => s.to_string(),
        Some("live") => {
            return Err(invalid("An encounter goes live when its combat starts."));
        }
        Some(_) => return Err(invalid("The status is draft, ready or ended.")),
    };
    let mut visible = form.player_visible.unwrap_or(enc.player_visible);
    if status == "draft" {
        visible = false;
    }
    let hp_mode = form
        .hp_mode
        .as_deref()
        .map(hp_mode_arg)
        .transpose()?
        .unwrap_or_else(|| enc.hp_mode.clone());
    let notes = form
        .dm_notes
        .as_deref()
        .map(notes_arg)
        .transpose()?
        .unwrap_or_else(|| enc.dm_notes.clone());
    sqlx::query(
        "UPDATE encounters SET name = $1, public_name = $2, status = $3, player_visible = $4, hp_mode = $5,
                               dm_notes = $6 WHERE id = $7",
    )
    .bind(&name)
    .bind(&public_name)
    .bind(&status)
    .bind(flag(visible))
    .bind(&hp_mode)
    .bind(&notes)
    .bind(&id)
    .execute(state.db.pool())
    .await?;
    finish(&state, &enc, before).await
}

/// `DELETE /api/encounters/{encounter}`.
pub async fn remove(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    let before = players_see(&state, &id).await?;
    sqlx::query("DELETE FROM encounters WHERE id = $1")
        .bind(&id)
        .execute(state.db.pool())
        .await?;
    tracing::info!("encounters: deleted {id}");
    publish(&state, &enc.party_id, &id, before.is_some()).await;
    Ok(Json(json!({ "deleted": true })))
}

// Creatures.

/// The setter `name` of an element, trimmed; `None` when empty.
fn setter<'a>(el: &'a Element, name: &str) -> Option<&'a str> {
    el.setter_value(name)
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// The number a setter starts with (`11 (natural armor)` -> 11).
fn leading_u32(text: &str) -> Option<u32> {
    let digits: String = text
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// The hit dice in a companion's `hp` setter (`10 (3d4+3)` -> `3d4+3`).
fn hp_formula(text: &str) -> Option<String> {
    let inner = text.split_once('(')?.1.split_once(')')?.0;
    let formula: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
    (formula.contains('d') && formula.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .then_some(formula)
}

const ABILITY_SETTERS: [(&str, &str); 6] = [
    ("str", "strength"),
    ("dex", "dexterity"),
    ("con", "constitution"),
    ("int", "intelligence"),
    ("wis", "wisdom"),
    ("cha", "charisma"),
];

/// An Aurora Companion's printed block, in the bestiary's `CompanionDetail` shape (`party/docs/bestiary-api.md`):
/// only what its setters and linked Companion Trait/Action/Reaction elements print; anything missing is null. Read
/// with upstream's companion reader, as the bestiary is. Once `bestiary.rs` is merged, `bestiary::monster` gives the
/// same block and this can go.
fn companion_block(el: &Element, store: &borealis_content::ContentStore) -> Value {
    let block = borealis_lib::companion_block::element_block(el, &|id| store.element(id).cloned());
    let text = |name: &str| setter(el, name).map(str::to_string);
    let named = |list: Vec<borealis_lib::content_dto::CompanionFeature>| -> Vec<Value> {
        list.into_iter()
            .map(|f| json!({ "name": f.name, "text": f.text }))
            .collect()
    };
    let mut speeds = serde_json::Map::new();
    if let Some(walk) = setter(el, "speed").and_then(leading_u32) {
        speeds.insert("walk".into(), json!(walk));
    }
    for (mode, feet) in ["fly", "climb", "swim", "burrow"].iter().zip(block.speeds) {
        if feet > 0 {
            speeds.insert((*mode).into(), json!(feet));
        }
    }
    let printed = setter(el, "proficiency").is_some()
        || setter(el, "challenge")
            .and_then(borealis_content::Cr::parse)
            .is_some();
    json!({
        "ac": setter(el, "ac").and_then(leading_u32),
        "acText": text("ac"),
        "hp": setter(el, "hp").and_then(leading_u32),
        "hpText": text("hp"),
        "hpFormula": setter(el, "hp").and_then(hp_formula),
        "speed": text("speed"),
        "speeds": speeds,
        "abilities": ABILITY_SETTERS.iter()
            .map(|(key, name)| ((*key).to_string(), json!(setter(el, name).and_then(leading_u32))))
            .collect::<serde_json::Map<_, _>>(),
        "saves": ABILITY_SETTERS.iter().zip(block.lines.save_bonuses)
            .filter_map(|((key, _), bonus)| bonus.map(|b| ((*key).to_string(), json!(b))))
            .collect::<serde_json::Map<_, _>>(),
        "proficiency": printed.then_some(block.proficiency),
        "skills": text("skills"),
        "senses": text("senses"),
        "languages": text("languages"),
        "challenge": text("challenge"),
        "size": text("size"),
        "type": text("type"),
        "alignment": text("alignment"),
        "defenses": named(block.lines.defenses),
        "traits": named(block.traits),
        "actions": named(block.actions),
        "reactions": named(block.reactions),
    })
}

/// A typed Monster or an Aurora Companion element of the DM's own content, copied: its snapshot and the pack file's
/// name. `None` when the DM has no such element.
pub(crate) async fn copy_creature(
    state: &AppState,
    dm: &str,
    element: &str,
) -> ApiResult<Option<(Snapshot, String)>> {
    if element.is_empty() || element.len() > 200 || element.chars().any(char::is_control) {
        return Err(invalid("That creature isn't in your content."));
    }
    let workspace = state.workspaces.get(dm).await?;
    let element = element.to_string();
    workspace
        .blocking(move |app| {
            use tauri::Manager;
            let backend = app.state::<borealis_lib::state::AppState>();
            let hub = backend.hub().map_err(internal)?;
            let Some(stored) = hub.store.get(&element) else {
                return Ok(None);
            };
            let el = &stored.element;
            let snapshot = if el.ty == "Companion" {
                Snapshot::of_companion(el, companion_block(el, &hub.store))
            } else {
                Snapshot::of_monster(el)
            }
            .map_err(|e| invalid(&e.to_string()))?;
            // The pack's file name only: never a path on the server.
            let pack = std::path::Path::new(&*stored.file)
                .file_stem()
                .map(|s| s.to_string_lossy().chars().take(80).collect::<String>())
                .unwrap_or_default();
            Ok(Some((snapshot, pack)))
        })
        .await
}

/// An owner for a controlled creature: a creature of this encounter that isn't itself controlled.
fn owner_arg(list: &[Creature], owner: &str, itself: Option<&str>) -> ApiResult<String> {
    let found = list.iter().find(|c| c.id == owner);
    match found {
        Some(c) if Some(c.id.as_str()) != itself && c.owner.is_none() => Ok(c.id.clone()),
        Some(_) => Err(invalid(
            "A controlled creature belongs to a creature that isn't controlled itself.",
        )),
        None => Err(invalid("Choose a creature of this encounter.")),
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AddForm {
    #[serde(default)]
    element_id: Option<String>,
    #[serde(default)]
    count: Option<i64>,
    #[serde(default)]
    dm_label: Option<String>,
    #[serde(default)]
    public_name: Option<String>,
    #[serde(default)]
    hidden: Option<bool>,
    #[serde(default)]
    owner_id: Option<String>,
    #[serde(default)]
    init_mode: Option<String>,
}

/// `POST /api/encounters/{encounter}/creatures`: copy a creature from the DM's content into the encounter, on its
/// own or controlled by another one (`ownerId`).
pub async fn add_creatures(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    bytes: Bytes,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    let form: AddForm = body(&bytes)?;
    let n = form.count.unwrap_or(1);
    if !(1..=MAX_ADD).contains(&n) {
        return Err(invalid("Add 1 to 20 at a time."));
    }
    let label = form.dm_label.as_deref().map(label_arg).transpose()?;
    let public = form.public_name.as_deref().map(public_arg).transpose()?;
    let list = creatures(&state, &id).await?;
    let owner = form
        .owner_id
        .as_deref()
        .map(|o| owner_arg(&list, o, None))
        .transpose()?;
    let init_mode = init_mode_arg(form.init_mode.as_deref().unwrap_or("after_owner"))?;
    if i64::try_from(list.len()).unwrap_or(i64::MAX) + n > MAX_CREATURES {
        return Err(invalid("An encounter can have 60 creatures."));
    }
    let element = form.element_id.unwrap_or_default();
    let (snapshot, pack) = copy_creature(&state, &user.id, &element)
        .await?
        .ok_or_else(|| invalid("That creature isn't in your content."))?;
    let before = players_see(&state, &id).await?;
    let same = i64::try_from(list.iter().filter(|c| c.element_id == element).count()).unwrap_or(0);
    let base_label = label.unwrap_or_else(|| snapshot.name.chars().take(70).collect());
    // No numbers on the name players see: "Fey 2" would tell them a "Fey 1" is hiding.
    let public = public.unwrap_or_else(|| snapshot.type_word());
    let rev = snapshot.rev();
    let block = serde_json::to_string(&snapshot).map_err(internal)?;
    let hidden = form.hidden.unwrap_or(true);
    // A controlled creature goes right after its owner and the owner's others.
    let mut sort = match &owner {
        Some(o) => {
            list.iter()
                .filter(|c| c.id == *o || c.owner.as_deref() == Some(o))
                .map(|c| c.sort)
                .max()
                .unwrap_or(0)
                + 1
        }
        None => next_sort(&state, "encounter_creatures", &id).await?,
    };
    let at = now();
    let mut tx = state.db.pool().begin().await?;
    if owner.is_some() {
        sqlx::query("UPDATE encounter_creatures SET sort = sort + $1 WHERE encounter_id = $2 AND sort >= $3")
            .bind(n)
            .bind(&id)
            .bind(sort)
            .execute(&mut *tx)
            .await?;
    }
    for i in 0..n {
        let label = if same == 0 && n == 1 {
            base_label.clone()
        } else {
            format!("{base_label} {}", same + i + 1)
        };
        sqlx::query(
            "INSERT INTO encounter_creatures (id, encounter_id, sort, kind, dm_label, public_name, element_id, pack_id,
                 pack_rev, statblock_json, overrides_json, hp_mode, visibility, revealed_at, owner_creature_id,
                 init_mode, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, '{}', 'encounter', $11, $12, $13, $14, $15, $16)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&id)
        .bind(sort)
        .bind(snapshot.kind.as_str())
        .bind(&label)
        .bind(&public)
        .bind(&element)
        .bind(&pack)
        .bind(&rev)
        .bind(&block)
        .bind(if hidden { "hidden" } else { "revealed" })
        .bind(if hidden { None } else { Some(at) })
        .bind(&owner)
        .bind(&init_mode)
        .bind(at)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        sort += 1;
    }
    tx.commit().await?;
    finish(&state, &enc, before).await
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreatureForm {
    #[serde(default)]
    dm_label: Option<String>,
    #[serde(default)]
    public_name: Option<String>,
    #[serde(default)]
    hidden: Option<bool>,
    #[serde(default)]
    hp_mode: Option<String>,
    #[serde(default, deserialize_with = "some")]
    hp_max: Option<Option<i64>>,
    #[serde(default)]
    sort: Option<i64>,
    #[serde(default, deserialize_with = "some")]
    owner_id: Option<Option<String>>,
    #[serde(default)]
    init_mode: Option<String>,
    #[serde(default)]
    overrides: Option<OverridesForm>,
}

/// One creature of an encounter the caller runs; the encounter 404 otherwise.
fn creature_in(list: &[Creature], creature: &str) -> ApiResult<Creature> {
    if !id_ok(creature) {
        return Err(not_found());
    }
    list.iter()
        .find(|c| c.id == creature)
        .cloned()
        .ok_or_else(not_found)
}

/// `PATCH /api/encounters/{encounter}/creatures/{creature}`.
pub async fn update_creature(
    State(state): State<AppState>,
    user: User,
    Path((id, cid)): Path<(String, String)>,
    bytes: Bytes,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    let list = creatures(&state, &id).await?;
    let c = creature_in(&list, &cid)?;
    let form: CreatureForm = body(&bytes)?;
    let before = players_see(&state, &id).await?;
    let label = form
        .dm_label
        .as_deref()
        .map(label_arg)
        .transpose()?
        .unwrap_or_else(|| c.dm_label.clone());
    let public = form
        .public_name
        .as_deref()
        .map(public_arg)
        .transpose()?
        .unwrap_or_else(|| c.public_name.clone());
    let hidden = form.hidden.unwrap_or(c.hidden);
    let hp_mode = match form.hp_mode.as_deref() {
        None => c.hp_mode.clone(),
        Some(m @ ("encounter" | "fixed")) => m.to_string(),
        Some(_) => return Err(invalid("Hit points follow the encounter, or are fixed.")),
    };
    let hp_max = match form.hp_max {
        None => c.hp_max,
        Some(v) => ranged(v, 1..=9_999, "Hit points are 1 to 9999.")?,
    };
    if hp_mode == "fixed" && hp_max.is_none() {
        return Err(invalid("Type the hit points this creature starts with."));
    }
    let owner = match form.owner_id {
        None => c.owner.clone(),
        Some(None) => None,
        Some(Some(o)) => {
            if list
                .iter()
                .any(|x| x.owner.as_deref() == Some(c.id.as_str()))
            {
                return Err(invalid(
                    "A creature with controlled creatures of its own can't be controlled.",
                ));
            }
            Some(owner_arg(&list, &o, Some(&c.id))?)
        }
    };
    let init_mode = form
        .init_mode
        .as_deref()
        .map(init_mode_arg)
        .transpose()?
        .unwrap_or_else(|| c.init_mode.clone());
    let overrides = match form.overrides {
        None => c.overrides.clone(),
        Some(f) => c
            .overrides
            .edit(f, &c.snapshot.printed())
            .map_err(|e| invalid(&e.to_string()))?,
    };
    let revealed_at = match (c.hidden, hidden) {
        (true, false) => Some(now()),
        (_, true) => None,
        _ => c.revealed_at,
    };
    let mut tx = state.db.pool().begin().await?;
    sqlx::query(
        "UPDATE encounter_creatures SET dm_label = $1, public_name = $2, visibility = $3, hp_mode = $4, hp_max = $5,
             overrides_json = $6, revealed_at = $7, owner_creature_id = $8, init_mode = $9, updated_at = $10
         WHERE id = $11",
    )
    .bind(&label)
    .bind(&public)
    .bind(if hidden { "hidden" } else { "revealed" })
    .bind(&hp_mode)
    .bind(hp_max)
    .bind(serde_json::to_string(&overrides).map_err(internal)?)
    .bind(revealed_at)
    .bind(&owner)
    .bind(&init_mode)
    .bind(now())
    .bind(&cid)
    .execute(&mut *tx)
    .await?;
    if let Some(position) = form.sort {
        // Move it to `position` among the others and number them all again.
        let mut ids: Vec<String> = list
            .iter()
            .map(|x| x.id.clone())
            .filter(|x| *x != cid)
            .collect();
        let at = usize::try_from(position.max(0))
            .unwrap_or(usize::MAX)
            .min(ids.len());
        ids.insert(at, cid.clone());
        for (i, x) in ids.iter().enumerate() {
            sqlx::query("UPDATE encounter_creatures SET sort = $1 WHERE id = $2")
                .bind(i64::try_from(i).unwrap_or(i64::MAX))
                .bind(x)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    finish(&state, &enc, before).await
}

/// `DELETE /api/encounters/{encounter}/creatures/{creature}`: the creature, and the creatures it controls.
pub async fn remove_creature(
    State(state): State<AppState>,
    user: User,
    Path((id, cid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    let list = creatures(&state, &id).await?;
    creature_in(&list, &cid)?;
    let gone: BTreeSet<String> = list
        .iter()
        .filter(|c| c.id == cid || c.owner.as_deref() == Some(cid.as_str()))
        .map(|c| c.id.clone())
        .collect();
    let before = players_see(&state, &id).await?;
    let mut tx = state.db.pool().begin().await?;
    // Rule flags name creatures in JSON: drop the ones for the creatures that go.
    let rows = sqlx::query("SELECT id, flags_json FROM encounter_rules WHERE encounter_id = $1")
        .bind(&id)
        .fetch_all(&mut *tx)
        .await?;
    for row in &rows {
        let rule: String = row.try_get(0)?;
        let flags: String = row.try_get(1)?;
        let mut flags: Vec<Value> = serde_json::from_str(&flags).unwrap_or_default();
        let n = flags.len();
        flags.retain(|f| !f["creatureId"].as_str().is_some_and(|c| gone.contains(c)));
        if flags.len() != n {
            sqlx::query("UPDATE encounter_rules SET flags_json = $1 WHERE id = $2")
                .bind(Value::Array(flags).to_string())
                .bind(&rule)
                .execute(&mut *tx)
                .await?;
        }
    }
    for gone_id in &gone {
        sqlx::query("DELETE FROM encounter_creatures WHERE id = $1 AND encounter_id = $2")
            .bind(gone_id)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    finish(&state, &enc, before).await
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RefreshForm {
    #[serde(default)]
    choices: BTreeMap<String, Choice>,
}

/// `POST /api/encounters/{encounter}/creatures/{creature}/refresh`: copy the block in again, the DM settling any
/// field both the DM and the source changed (D41).
pub async fn refresh_creature(
    State(state): State<AppState>,
    user: User,
    Path((id, cid)): Path<(String, String)>,
    bytes: Bytes,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    let list = creatures(&state, &id).await?;
    let c = creature_in(&list, &cid)?;
    let form: RefreshForm = body(&bytes)?;
    let (fresh, pack) = copy_creature(&state, &user.id, &c.element_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            ApiError::Conflict("That creature isn't in your content any more.".into())
        })?;
    let changed = fresh.rev() != c.snapshot.rev();
    let outcome = refresh_block(&c.overrides, &fresh.printed(), &form.choices);
    let conflicts = serde_json::to_value(&outcome.conflicts).map_err(internal)?;
    let Some(overrides) = outcome.overrides else {
        return Ok(Json(json!({
            "changed": changed, "conflicts": conflicts, "applied": false,
            "encounter": dm_view(&state, &id).await?,
        })));
    };
    let before = players_see(&state, &id).await?;
    sqlx::query(
        "UPDATE encounter_creatures SET statblock_json = $1, pack_id = $2, pack_rev = $3, overrides_json = $4,
             updated_at = $5 WHERE id = $6",
    )
    .bind(serde_json::to_string(&fresh).map_err(internal)?)
    .bind(&pack)
    .bind(fresh.rev())
    .bind(serde_json::to_string(&overrides).map_err(internal)?)
    .bind(now())
    .bind(&cid)
    .execute(state.db.pool())
    .await?;
    let Json(view) = finish(&state, &enc, before).await?;
    Ok(Json(json!({
        "changed": changed, "conflicts": conflicts, "applied": true, "encounter": view,
    })))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SwapForm {
    #[serde(default)]
    element_id: Option<String>,
}

/// `POST /api/encounters/{encounter}/creatures/{creature}/swap`: the matching-edition block in place of the copy
/// (D23). Its overrides go: they were numbers of the other block.
pub async fn swap_creature(
    State(state): State<AppState>,
    user: User,
    Path((id, cid)): Path<(String, String)>,
    bytes: Bytes,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    let list = creatures(&state, &id).await?;
    let c = creature_in(&list, &cid)?;
    let form: SwapForm = body(&bytes)?;
    let element = form
        .element_id
        .or_else(|| c.snapshot.maps_to.clone())
        .ok_or_else(|| invalid("This creature names no other edition's block."))?;
    let (snapshot, pack) = copy_creature(&state, &user.id, &element)
        .await?
        .ok_or_else(|| invalid("That creature isn't in your content."))?;
    let before = players_see(&state, &id).await?;
    sqlx::query(
        "UPDATE encounter_creatures SET element_id = $1, pack_id = $2, pack_rev = $3, statblock_json = $4, kind = $5,
             overrides_json = '{}', updated_at = $6 WHERE id = $7",
    )
    .bind(&element)
    .bind(&pack)
    .bind(snapshot.rev())
    .bind(serde_json::to_string(&snapshot).map_err(internal)?)
    .bind(snapshot.kind.as_str())
    .bind(now())
    .bind(&cid)
    .execute(state.db.pool())
    .await?;
    finish(&state, &enc, before).await
}

// Starting effects (C19).

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Modifiers {
    #[serde(default)]
    ac: Option<i64>,
    #[serde(default)]
    speed_add: Option<i64>,
    #[serde(default)]
    speed_mult: Option<i64>,
    #[serde(default)]
    speed_zero: Option<bool>,
}

impl Modifiers {
    fn checked(self) -> ApiResult<Value> {
        ranged(self.ac, -10..=10, "An armor class change is -10 to +10.")?;
        ranged(
            self.speed_add,
            -120..=120,
            "A speed change is -120 to +120 feet.",
        )?;
        ranged(self.speed_mult, 0..=4, "A speed multiplier is 0 to 4.")?;
        let mut out = serde_json::Map::new();
        if let Some(v) = self.ac {
            out.insert("ac".into(), json!(v));
        }
        if let Some(v) = self.speed_add {
            out.insert("speedAdd".into(), json!(v));
        }
        if let Some(v) = self.speed_mult {
            out.insert("speedMult".into(), json!(v));
        }
        if let Some(v) = self.speed_zero {
            out.insert("speedZero".into(), json!(v));
        }
        Ok(Value::Object(out))
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EffectForm {
    #[serde(default)]
    creature_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default, deserialize_with = "some")]
    source_creature_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "some")]
    duration_turns: Option<Option<i64>>,
    #[serde(default, deserialize_with = "some")]
    charges: Option<Option<i64>>,
    #[serde(default)]
    concentration: Option<bool>,
    #[serde(default, deserialize_with = "some")]
    slot_level: Option<Option<i64>>,
    #[serde(default)]
    modifiers: Option<Modifiers>,
}

/// The ids of an encounter's creatures.
async fn creature_ids(state: &AppState, encounter: &str) -> ApiResult<Vec<String>> {
    let rows = sqlx::query("SELECT id FROM encounter_creatures WHERE encounter_id = $1")
        .bind(encounter)
        .fetch_all(state.db.pool())
        .await?;
    Ok(rows
        .iter()
        .map(|r| r.try_get(0))
        .collect::<sqlx::Result<_>>()?)
}

fn one_of(ids: &[String], id: &str) -> ApiResult<String> {
    if ids.iter().any(|x| x == id) {
        Ok(id.to_string())
    } else {
        Err(invalid("Choose a creature of this encounter."))
    }
}

/// An effect's fields, checked, from the form over what it had (`None` for a new one).
struct EffectFields {
    creature: String,
    name: String,
    detail: String,
    source: Option<String>,
    duration: Option<i64>,
    charges: Option<i64>,
    concentration: bool,
    slot: Option<i64>,
    modifiers: String,
}

fn effect_fields(form: EffectForm, ids: &[String], had: Option<&Value>) -> ApiResult<EffectFields> {
    let old = |key: &str| had.map(|h| h[key].clone()).unwrap_or(Value::Null);
    let creature = match (form.creature_id, had) {
        (Some(c), None) => one_of(ids, &c)?,
        (None, Some(h)) => h["creatureId"].as_str().unwrap_or_default().to_string(),
        (Some(_), Some(_)) => {
            return Err(invalid(
                "An effect stays on its creature: delete it and add a new one.",
            ))
        }
        (None, None) => return Err(invalid("Choose the creature the effect is on.")),
    };
    let name = match form.name {
        Some(n) => text(&n, 1, 80, false, "Effect names are 1 to 80 characters.")?,
        None => old("name")
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| invalid("Effect names are 1 to 80 characters."))?,
    };
    let detail = match form.detail {
        Some(d) => text(
            &d,
            0,
            2_000,
            true,
            "Effect details are up to 2000 characters.",
        )?,
        None => old("detail").as_str().unwrap_or_default().to_string(),
    };
    let source = match form.source_creature_id {
        None => old("sourceCreatureId").as_str().map(str::to_string),
        Some(None) => None,
        Some(Some(s)) => Some(one_of(ids, &s)?),
    };
    let number = |v: Option<Option<i64>>, key: &str, range, message| -> ApiResult<Option<i64>> {
        match v {
            None => Ok(old(key).as_i64()),
            Some(n) => ranged(n, range, message),
        }
    };
    let duration = number(
        form.duration_turns,
        "durationTurns",
        1..=1_000,
        "A duration is 1 to 1000 turns.",
    )?;
    let charges = number(form.charges, "charges", 1..=1_000, "Charges are 1 to 1000.")?;
    let slot = number(
        form.slot_level,
        "slotLevel",
        1..=9,
        "A spell slot is level 1 to 9.",
    )?;
    let concentration = form
        .concentration
        .unwrap_or_else(|| old("concentration").as_bool().unwrap_or(false));
    let modifiers = match form.modifiers {
        Some(m) => m.checked()?,
        None => match old("modifiers") {
            Value::Null => json!({}),
            v => v,
        },
    };
    Ok(EffectFields {
        creature,
        name,
        detail,
        source,
        duration,
        charges,
        concentration,
        slot,
        modifiers: modifiers.to_string(),
    })
}

/// `POST /api/encounters/{encounter}/effects`.
pub async fn add_effect(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    bytes: Bytes,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    let form: EffectForm = body(&bytes)?;
    let ids = creature_ids(&state, &id).await?;
    let f = effect_fields(form, &ids, None)?;
    if count(
        &state,
        "SELECT COUNT(*) FROM encounter_effects WHERE encounter_id = $1",
        &id,
    )
    .await?
        >= MAX_EFFECTS
    {
        return Err(invalid("An encounter can have 40 starting effects."));
    }
    let before = players_see(&state, &id).await?;
    let sort = next_sort(&state, "encounter_effects", &id).await?;
    sqlx::query(
        "INSERT INTO encounter_effects (id, encounter_id, creature_id, sort, name, detail, source_creature_id,
             duration_turns, charges, concentration, slot_level, modifiers_json, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&id)
    .bind(&f.creature)
    .bind(sort)
    .bind(&f.name)
    .bind(&f.detail)
    .bind(&f.source)
    .bind(f.duration)
    .bind(f.charges)
    .bind(flag(f.concentration))
    .bind(f.slot)
    .bind(&f.modifiers)
    .bind(now())
    .execute(state.db.pool())
    .await?;
    finish(&state, &enc, before).await
}

async fn effect_of(state: &AppState, encounter: &str, effect: &str) -> ApiResult<Value> {
    if !id_ok(effect) {
        return Err(not_found());
    }
    effects(state, encounter)
        .await?
        .into_iter()
        .find(|e| e["id"] == effect)
        .ok_or_else(not_found)
}

/// `PATCH /api/encounters/{encounter}/effects/{effect}`.
pub async fn update_effect(
    State(state): State<AppState>,
    user: User,
    Path((id, eid)): Path<(String, String)>,
    bytes: Bytes,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    let had = effect_of(&state, &id, &eid).await?;
    let form: EffectForm = body(&bytes)?;
    let ids = creature_ids(&state, &id).await?;
    let f = effect_fields(form, &ids, Some(&had))?;
    let before = players_see(&state, &id).await?;
    sqlx::query(
        "UPDATE encounter_effects SET name = $1, detail = $2, source_creature_id = $3, duration_turns = $4,
             charges = $5, concentration = $6, slot_level = $7, modifiers_json = $8 WHERE id = $9",
    )
    .bind(&f.name)
    .bind(&f.detail)
    .bind(&f.source)
    .bind(f.duration)
    .bind(f.charges)
    .bind(flag(f.concentration))
    .bind(f.slot)
    .bind(&f.modifiers)
    .bind(&eid)
    .execute(state.db.pool())
    .await?;
    finish(&state, &enc, before).await
}

/// `DELETE /api/encounters/{encounter}/effects/{effect}`.
pub async fn remove_effect(
    State(state): State<AppState>,
    user: User,
    Path((id, eid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    effect_of(&state, &id, &eid).await?;
    let before = players_see(&state, &id).await?;
    sqlx::query("DELETE FROM encounter_effects WHERE id = $1 AND encounter_id = $2")
        .bind(&eid)
        .bind(&id)
        .execute(state.db.pool())
        .await?;
    finish(&state, &enc, before).await
}

// Encounter rules (C20, D42).

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FlagForm {
    creature_id: String,
    ability: String,
    while_active: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuleForm {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default, deserialize_with = "some")]
    creature_id: Option<Option<String>>,
    #[serde(default)]
    ends_when: Option<String>,
    #[serde(default)]
    active: Option<bool>,
    #[serde(default)]
    flags: Option<Vec<FlagForm>>,
}

struct RuleFields {
    name: String,
    text: String,
    creature: Option<String>,
    ends_when: String,
    active: bool,
    flags: String,
}

fn rule_fields(form: RuleForm, ids: &[String], had: Option<&Value>) -> ApiResult<RuleFields> {
    let old = |key: &str| had.map(|h| h[key].clone()).unwrap_or(Value::Null);
    let name = match form.name {
        Some(n) => text(&n, 1, 80, false, "Rule names are 1 to 80 characters.")?,
        None => old("name")
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| invalid("Rule names are 1 to 80 characters."))?,
    };
    let body = match form.text {
        Some(t) => text(
            &t,
            0,
            2_000,
            true,
            "A rule's text is up to 2000 characters.",
        )?,
        None => old("text").as_str().unwrap_or_default().to_string(),
    };
    let creature = match form.creature_id {
        None => old("creatureId").as_str().map(str::to_string),
        Some(None) => None,
        Some(Some(c)) => Some(one_of(ids, &c)?),
    };
    let ends_when = match form.ends_when.as_deref() {
        None => old("endsWhen")
            .as_str()
            .unwrap_or("creature_down")
            .to_string(),
        Some(w @ ("creature_down" | "manual")) => w.to_string(),
        Some(_) => {
            return Err(invalid(
                "A rule ends when its creature is down, or by hand.",
            ))
        }
    };
    if ends_when == "creature_down"
        && creature.is_none()
        && (form.ends_when.is_some() || had.is_none())
    {
        return Err(invalid(
            "Choose the creature whose fall ends the rule, or end it by hand.",
        ));
    }
    let active = form
        .active
        .unwrap_or_else(|| old("active").as_bool().unwrap_or(true));
    let flags = match form.flags {
        None => match old("flags") {
            Value::Null => json!([]),
            v => v,
        },
        Some(list) => {
            if list.len() > MAX_FLAGS {
                return Err(invalid("A rule can switch 20 abilities."));
            }
            let mut out = Vec::new();
            for f in list {
                let creature = one_of(ids, &f.creature_id)?;
                let ability = text(
                    &f.ability,
                    1,
                    80,
                    false,
                    "Ability names are 1 to 80 characters.",
                )?;
                if !matches!(f.while_active.as_str(), "on" | "off") {
                    return Err(invalid("While the rule is active an ability is on or off."));
                }
                out.push(json!({ "creatureId": creature, "ability": ability, "whileActive": f.while_active }));
            }
            Value::Array(out)
        }
    };
    Ok(RuleFields {
        name,
        text: body,
        creature,
        ends_when,
        active,
        flags: flags.to_string(),
    })
}

/// `POST /api/encounters/{encounter}/rules`.
pub async fn add_rule(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    bytes: Bytes,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    let form: RuleForm = body(&bytes)?;
    let ids = creature_ids(&state, &id).await?;
    let f = rule_fields(form, &ids, None)?;
    if count(
        &state,
        "SELECT COUNT(*) FROM encounter_rules WHERE encounter_id = $1",
        &id,
    )
    .await?
        >= MAX_RULES
    {
        return Err(invalid("An encounter can have 20 rules."));
    }
    let before = players_see(&state, &id).await?;
    let sort = next_sort(&state, "encounter_rules", &id).await?;
    sqlx::query(
        "INSERT INTO encounter_rules (id, encounter_id, sort, name, text, creature_id, ends_when, active, flags_json,
             created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&id)
    .bind(sort)
    .bind(&f.name)
    .bind(&f.text)
    .bind(&f.creature)
    .bind(&f.ends_when)
    .bind(flag(f.active))
    .bind(&f.flags)
    .bind(now())
    .execute(state.db.pool())
    .await?;
    finish(&state, &enc, before).await
}

async fn rule_of(state: &AppState, encounter: &str, rule: &str) -> ApiResult<Value> {
    if !id_ok(rule) {
        return Err(not_found());
    }
    rules(state, encounter)
        .await?
        .into_iter()
        .find(|r| r["id"] == rule)
        .ok_or_else(not_found)
}

/// `PATCH /api/encounters/{encounter}/rules/{rule}`.
pub async fn update_rule(
    State(state): State<AppState>,
    user: User,
    Path((id, rid)): Path<(String, String)>,
    bytes: Bytes,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    let had = rule_of(&state, &id, &rid).await?;
    let form: RuleForm = body(&bytes)?;
    let ids = creature_ids(&state, &id).await?;
    let f = rule_fields(form, &ids, Some(&had))?;
    let before = players_see(&state, &id).await?;
    sqlx::query(
        "UPDATE encounter_rules SET name = $1, text = $2, creature_id = $3, ends_when = $4, active = $5,
             flags_json = $6 WHERE id = $7",
    )
    .bind(&f.name)
    .bind(&f.text)
    .bind(&f.creature)
    .bind(&f.ends_when)
    .bind(flag(f.active))
    .bind(&f.flags)
    .bind(&rid)
    .execute(state.db.pool())
    .await?;
    finish(&state, &enc, before).await
}

/// `DELETE /api/encounters/{encounter}/rules/{rule}`.
pub async fn remove_rule(
    State(state): State<AppState>,
    user: User,
    Path((id, rid)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let enc = run_by(&state, &user, &id).await?;
    rule_of(&state, &id, &rid).await?;
    let before = players_see(&state, &id).await?;
    sqlx::query("DELETE FROM encounter_rules WHERE id = $1 AND encounter_id = $2")
        .bind(&rid)
        .bind(&id)
        .execute(state.db.pool())
        .await?;
    finish(&state, &enc, before).await
}
