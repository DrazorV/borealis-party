//! Encounter simulation on the server (0.3.1, D26): the `sim_*` tools over `party-sim`.
//!
//! * **Read only (D25).** Everything here SELECTs: the encounter's creature copies, effects and rules, the party's
//!   DM and members, and the members' sheets (read with the same `snapshot_of` as the DM's sheet view, with no
//!   roster card written). Nothing touches the combat tables, the live-combat registry or the hub; there is no
//!   migration, no activity row and no event. No tool takes or returns a combat id.
//! * **DM only.** Every call re-reads `parties.dm_id` of the encounter's party; anyone else gets the generic "not
//!   found", the same as an unknown id.
//! * **Kept across a restart (0.3.5).** A session lives in [`STORE`] for 30 idle minutes, at most 2 per account and
//!   600 steps each. After every call it is also saved whole (setup, engine state with its dice, transcript) as one
//!   JSON file in `<data>/sims/`, and [`install`] loads the unexpired ones at start-up, so a server update doesn't end
//!   a running simulation and it continues exactly as it would have. Files, not database rows: the database stays
//!   untouched by simulations (D25), and nothing needs a migration. A file the new version can't read is dropped.
//!   Sim ids are random 122-bit values bound to the account.
//! * **Off the async threads.** The fight itself runs in `spawn_blocking` under the combat module's per-account job
//!   semaphore.
//! * **No model calls (0.3.3).** The server never calls an AI service: an MCP client plays with its own model
//!   through `sim_act` (the Agent oracle), and everything else is the Basic heuristic or a transcript.
//! * **Spells (0.3.3).** Casts use the tracker's own definitions: the DM's mechanics for the party's edition (their
//!   definitions and links, then the curated SRD rows) and their chosen copy of each printed spell name, read once
//!   per setup; their revisions are in the run's fingerprints, so a replay after an edit reports the divergence.
//!
//! The tool table ([`tool_defs`]) and the dispatcher ([`call`]) know nothing of the tool registry; the adapter in
//! `simulate_tools.rs` plugs them into `party-tools`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use party_sim::runner::{Entry, Pending};
use party_sim::setup::{AgentControl, Band, Control, PcHp, Start};
use party_sim::spells::{SpellDefn, SpellDefs};
use party_sim::{By, CreatureInput, Decision, PcInput, Sim, SimOpts, SimSetup, Until};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::Row;

use crate::auth::User;
use crate::db::now;
use crate::AppState;

/// Idle minutes before a session is dropped.
const IDLE_SECONDS: i64 = 30 * 60;
/// Sessions an account may hold.
const PER_ACCOUNT: usize = 2;
/// Steps a session may take.
const MAX_STEPS: u32 = 600;
/// Wall clock of one batch.
const BATCH_BUDGET: Duration = Duration::from_secs(3);
/// The fixed play note in every sim tool description (never user content).
const PLAY_NOTE: &str = "Each side plays to win with only what it can see. Never make a side softer or harder to make the fight interesting. Describe the outcome; never rate difficulty.";

/// A refusal, mapped to the registry's errors by the adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimError {
    /// Unknown, or not the caller's: one answer for both.
    NotFound,
    Invalid(String),
    Busy(String),
    Internal,
}

type SimResult<T> = Result<T, SimError>;

fn internal<E: std::fmt::Display>(e: E) -> SimError {
    tracing::warn!("simulate: {e}");
    SimError::Internal
}

impl From<sqlx::Error> for SimError {
    fn from(e: sqlx::Error) -> Self {
        internal(e)
    }
}

// ----- Tool table -----

/// One tool, as its module defines it (the adapter turns it into a `party_tools::ToolSpec`).
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub title: &'static str,
    pub description: String,
    pub heavy: bool,
    pub schema: Value,
}

fn id_prop(what: &str) -> Value {
    json!({ "type": "string", "minLength": 1, "maxLength": 64, "description": what })
}

fn opts_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "pcHp": { "type": "string", "enum": ["full", "current"], "description": "PCs start at full HP (default) or at the HP their sheets show now." },
            "maxRounds": { "type": "integer", "minimum": 1, "maximum": 20, "description": "Round cap (default 10)." },
            "start": {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "partyBand": { "type": "string", "enum": ["front", "near", "far", "distant"] },
                    "foesBand": { "type": "string", "enum": ["front", "near", "far", "distant"] },
                    "surprise": { "type": "array", "items": { "type": "string", "maxLength": 64 }, "maxItems": 40, "description": "Combatant ids or encounter creature ids that start surprised." }
                }
            },
            "control": {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "agent": { "description": "Who you play: \"foes\" (default), \"party\", \"all\", \"none\", or a list of combatant ids. Basic plays the rest.",
                        "anyOf": [ { "type": "string", "enum": ["foes", "party", "all", "none"] }, { "type": "array", "items": { "type": "string", "maxLength": 64 }, "maxItems": 40 } ] },
                    "rest": { "type": "string", "enum": ["basic"] }
                }
            },
            "fog": { "type": "boolean", "description": "Cards show enemies' health as unhurt/bloodied/down (default true)." },
            "lair": { "type": "boolean", "description": "Lair actions (not simulated in 0.3.1)." },
            "profile": { "type": "string", "enum": ["small", "frontier"], "description": "small: text-only options are skipped instead of adjudicated." }
        }
    })
}

fn decision_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["o"],
        "properties": {
            "o": { "type": "string", "maxLength": 24, "description": "An option id from the card, or \"repeat\" (your last option again) or \"basic\" (let the heuristic play this turn)." },
            "t": { "type": "array", "items": { "type": "string", "maxLength": 24 }, "maxItems": 8, "description": "Target ids from that option's targets." },
            "mv": { "type": "string", "maxLength": 32, "description": "Move first: engage:<id>, near, far, back or hold." },
            "adj": { "type": "object", "description": "For a text-only option: {as, to_hit_ref, dc_ref, ability, dice_refs, type, half, condition, turns, repeat, targets, concentration}; refs are indexes into its numbers[]." },
            "why": { "type": "string", "maxLength": 160 },
            "memo": { "type": "string", "maxLength": 200, "description": "A note to yourself; it comes back on this creature's cards only." }
        }
    })
}

/// The sim tools (scope `encounters:simulate`), built once.
#[must_use]
pub fn tool_defs() -> &'static [ToolDef] {
    static DEFS: LazyLock<Vec<ToolDef>> = LazyLock::new(defs);
    &DEFS
}

fn defs() -> Vec<ToolDef> {
    let enc = id_prop("An encounter id (from encounter_list). Combat ids are not accepted.");
    let sim = id_prop("A simulation id from sim_start.");
    let step = json!({ "type": "integer", "minimum": 0, "maximum": 100_000, "description": "The card's step: a stale step returns the current card and changes nothing." });
    vec![
        ToolDef {
            name: "sim_start",
            title: "Start a simulation",
            description: format!("DM only. Start a seeded dry-run fight on a COPY of one of your encounters with your party (never a live combat; nothing is saved). The engine rolls every number; you answer decision cards for the combatants you control (default: the foes) by picking ids. Returns the roster, what is automated, adjudicable or not simulated, the assumptions, and the first card. {PLAY_NOTE}"),
            heavy: true,
            schema: json!({ "type": "object", "additionalProperties": false, "required": ["encounterId"], "properties": {
                "encounterId": enc, "seed": { "type": "integer", "minimum": 0, "maximum": 4_294_967_295u64 }, "opts": opts_schema() } }),
        },
        ToolDef {
            name: "sim_card",
            title: "Read the decision card",
            description: "The current decision card of a simulation (idempotent). card:null means the fight is over: call sim_report.".into(),
            heavy: false,
            schema: json!({ "type": "object", "additionalProperties": false, "required": ["simId"], "properties": { "simId": sim } }),
        },
        ToolDef {
            name: "sim_act",
            title: "Decide in a simulation",
            description: format!("Answer the current card: d = {{o, t?, mv?, adj?, why?, memo?}} or a list of up to 3 for the same turn. Play the creature as its INT and type suggest; use its memo to keep intent. An invalid answer returns the reason (ids only) and the same card. {PLAY_NOTE}"),
            heavy: false,
            schema: json!({ "type": "object", "additionalProperties": false, "required": ["simId", "step", "d"], "properties": {
                "simId": sim, "step": step,
                "d": { "anyOf": [ decision_schema(), { "type": "array", "items": decision_schema(), "minItems": 1, "maxItems": 3 } ] } } }),
        },
        ToolDef {
            name: "sim_auto",
            title: "Let the simulation play",
            description: "Let the Basic heuristic play (until the end of this turn, this round, or the fight), for everyone or only `who`. The server never calls an AI model: to play with your own, answer the cards with sim_act.".into(),
            heavy: true,
            schema: json!({ "type": "object", "additionalProperties": false, "required": ["simId", "step", "until"], "properties": {
                "simId": sim, "step": step,
                "until": { "type": "string", "enum": ["turn", "round", "end"] },
                "who": { "type": "array", "items": { "type": "string", "maxLength": 24 }, "maxItems": 40, "description": "Hand only these combatants to Basic, for the rest of the run." },
                "oracle": { "type": "string", "enum": ["basic"] } } }),
        },
        ToolDef {
            name: "sim_state",
            title: "Read a simulation's board",
            description: "The whole board for the DM (not fogged): HP, positions, conditions, concentration, slots, resources and the recent log.".into(),
            heavy: false,
            schema: json!({ "type": "object", "additionalProperties": false, "required": ["simId"], "properties": { "simId": sim } }),
        },
        ToolDef {
            name: "sim_report",
            title: "Report a simulation",
            description: "The report of a simulation (outcome, rounds, timeline, per-PC and per-creature facts, adjudications, steering, coverage, assumptions, decisions, transcript); it ends the session. Describe what happened; never rate difficulty.".into(),
            heavy: false,
            schema: json!({ "type": "object", "additionalProperties": false, "required": ["simId"], "properties": { "simId": sim } }),
        },
        ToolDef {
            name: "sim_batch",
            title: "Run simulations",
            description: "DM only. N seeded runs of an encounter with the Basic heuristic only (no AI), within a 3 s budget: plain counts out of N (party standing, creatures standing, round cap), rounds, per-PC and per-creature counts, and up to 3 notable facts. Counts describe these runs; they are not odds or a rating.".into(),
            heavy: true,
            schema: json!({ "type": "object", "additionalProperties": false, "required": ["encounterId"], "properties": {
                "encounterId": enc, "runs": { "type": "integer", "minimum": 1, "maximum": 100 },
                "seed": { "type": "integer", "minimum": 0, "maximum": 4_294_967_295u64 }, "opts": opts_schema() } }),
        },
        ToolDef {
            name: "sim_replay",
            title: "Replay a simulation",
            description: "Replay a report's transcript on a fresh copy of the encounter with the same seed and opts: identical, or the first divergence (step or changed stat blocks).".into(),
            heavy: true,
            schema: json!({ "type": "object", "additionalProperties": false, "required": ["encounterId", "seed", "transcript"], "properties": {
                "encounterId": enc, "seed": { "type": "integer", "minimum": 0, "maximum": 4_294_967_295u64 }, "opts": opts_schema(),
                "transcript": { "type": "array", "maxItems": 2000, "items": { "type": "object" } },
                "fingerprints": { "type": "object" } } }),
        },
    ]
}

// ----- Arguments -----

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StartBands {
    #[serde(alias = "party_band")]
    party_band: Option<Band>,
    #[serde(alias = "foes_band")]
    foes_band: Option<Band>,
    #[serde(default)]
    surprise: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ControlArg {
    agent: Option<AgentControl>,
    rest: Option<String>,
}

/// How much a client's model is asked to do: `small` skips text-only options instead of adjudicating them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Profile {
    Small,
    Frontier,
}

/// `opts` of sim_start, sim_batch and sim_replay.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OptsArg {
    #[serde(alias = "pc_hp", skip_serializing_if = "Option::is_none")]
    pc_hp: Option<PcHp>,
    #[serde(alias = "max_rounds", skip_serializing_if = "Option::is_none")]
    max_rounds: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    control: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fog: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lair: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile: Option<Profile>,
}

impl OptsArg {
    fn to_opts(&self) -> SimResult<SimOpts> {
        let mut o = SimOpts {
            max_steps: MAX_STEPS,
            ..SimOpts::default()
        };
        if let Some(h) = self.pc_hp {
            o.pc_hp = h;
        }
        if let Some(r) = self.max_rounds {
            if !(1..=20).contains(&r) {
                return Err(SimError::Invalid("maxRounds is 1 to 20".into()));
            }
            o.max_rounds = r;
        }
        if let Some(s) = &self.start {
            let s: StartBands = serde_json::from_value(s.clone())
                .map_err(|e| SimError::Invalid(format!("start: {e}")))?;
            o.start = Start {
                party_band: s.party_band.unwrap_or(Band::Near),
                foes_band: s.foes_band.unwrap_or(Band::Near),
                surprise: s.surprise.into_iter().take(40).collect(),
            };
        }
        if let Some(c) = &self.control {
            let c: ControlArg = serde_json::from_value(c.clone())
                .map_err(|e| SimError::Invalid(format!("control: {e}")))?;
            if c.rest.as_deref().is_some_and(|r| r != "basic") {
                return Err(SimError::Invalid("control.rest is \"basic\"".into()));
            }
            if let Some(a) = c.agent {
                if let AgentControl::Side(s) = &a {
                    if !matches!(s.as_str(), "foes" | "party" | "all" | "none") {
                        return Err(SimError::Invalid(
                            "control.agent is foes, party, all, none or a list of ids".into(),
                        ));
                    }
                }
                o.control = Control { agent: a };
            }
        }
        if let Some(f) = self.fog {
            o.fog = f;
        }
        if let Some(l) = self.lair {
            o.lair = l;
        }
        o.adjudicate = self.profile != Some(Profile::Small);
        Ok(o)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StartArgs {
    #[serde(alias = "encounter_id")]
    encounter_id: String,
    seed: Option<u32>,
    #[serde(default)]
    opts: OptsArg,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SimIdArgs {
    #[serde(alias = "sim_id")]
    sim_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrMore {
    One(Box<Decision>),
    More(Vec<Decision>),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ActArgs {
    #[serde(alias = "sim_id")]
    sim_id: String,
    step: u32,
    d: OneOrMore,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AutoArgs {
    #[serde(alias = "sim_id")]
    sim_id: String,
    step: u32,
    until: Until,
    #[serde(default)]
    who: Vec<String>,
    oracle: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct BatchArgs {
    #[serde(alias = "encounter_id")]
    encounter_id: String,
    runs: Option<u32>,
    seed: Option<u32>,
    #[serde(default)]
    opts: OptsArg,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ReplayArgs {
    #[serde(alias = "encounter_id")]
    encounter_id: String,
    seed: u32,
    #[serde(default)]
    opts: OptsArg,
    transcript: Vec<Entry>,
    #[serde(default)]
    fingerprints: Option<std::collections::BTreeMap<String, String>>,
}

fn parse<T: serde::de::DeserializeOwned>(args: Map<String, Value>) -> SimResult<T> {
    serde_json::from_value(Value::Object(args))
        .map_err(|e| SimError::Invalid(format!("arguments: {e}")))
}

// ----- Sessions -----

#[derive(Serialize, Deserialize)]
struct Session {
    account: String,
    party: String,
    encounter: String,
    setup: SimSetup,
    sim: Sim,
    last_used: i64,
}

type Shared = Arc<Mutex<Session>>;

/// The open sessions, by sim id.
static STORE: LazyLock<Mutex<HashMap<String, Shared>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn store() -> std::sync::MutexGuard<'static, HashMap<String, Shared>> {
    STORE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock(s: &Shared) -> std::sync::MutexGuard<'_, Session> {
    s.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Drop idle sessions (and their files).
fn sweep() {
    let cut = now() - IDLE_SECONDS;
    let mut gone = Vec::new();
    store().retain(|id, s| {
        let keep = lock(s).last_used > cut;
        if !keep {
            gone.push(id.clone());
        }
        keep
    });
    for id in gone {
        forget(&id);
    }
}

// ----- Saved sessions (0.3.5) -----

/// The version of the saved file. A file of another version (or that no longer parses) is dropped at start-up.
const SAVED_FORMAT: u32 = 1;

/// Where sessions are saved: `<data>/sims`, set by [`install`]. `None` (tests that don't ask): memory only.
static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

fn dir() -> Option<PathBuf> {
    DIR.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[derive(Serialize)]
struct SavedRef<'a> {
    format: u32,
    session: &'a Session,
}

#[derive(Deserialize)]
struct Saved {
    format: u32,
    session: Session,
}

/// A sim id as [`start`] makes them (32 hex digits): the only file names read or written.
fn file_id_ok(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Save a session after a call, replacing its file whole (written aside, then renamed). A failure is logged and the
/// session goes on in memory: only a restart would lose it.
fn save(id: &str, s: &Session) {
    let Some(dir) = dir() else { return };
    if !file_id_ok(id) {
        return;
    }
    let body = match serde_json::to_vec(&SavedRef {
        format: SAVED_FORMAT,
        session: s,
    }) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("simulate: a session could not be saved: {e}");
            return;
        }
    };
    let tmp = dir.join(format!("{id}.json.tmp"));
    let file = dir.join(format!("{id}.json"));
    if let Err(e) = std::fs::write(&tmp, body).and_then(|()| std::fs::rename(&tmp, &file)) {
        tracing::warn!("simulate: a session could not be saved: {e}");
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Remove a session's file (ended, expired, or unreadable).
fn forget(id: &str) {
    let Some(dir) = dir() else { return };
    if file_id_ok(id) {
        let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
    }
}

/// Read the saved sessions in `dir` into the store: the unexpired ones the current version can read. The others'
/// files are removed. Returns how many came back.
fn restore(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let cut = now() - IDLE_SECONDS;
    let mut back = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".json.tmp") {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let Some(id) = name.strip_suffix(".json").filter(|id| file_id_ok(id)) else {
            continue;
        };
        let saved = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Saved>(&b).ok())
            .filter(|s| s.format == SAVED_FORMAT);
        match saved {
            Some(s) if s.session.last_used > cut => {
                store().insert(id.to_string(), Arc::new(Mutex::new(s.session)));
                back += 1;
            }
            Some(_) => {
                let _ = std::fs::remove_file(&path);
            }
            None => {
                tracing::warn!("simulate: a saved session this version can't read was dropped");
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    back
}

/// Keep sessions in `dir` (`<data>/sims`) from now on, and load the ones a previous run left there. Called once at
/// start-up, before the server listens.
pub fn install(dir: PathBuf) {
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(
            "simulate: {} can't be created, so simulations end with a restart: {e}",
            dir.display()
        );
        return;
    }
    let back = restore(&dir);
    if back > 0 {
        tracing::info!("simulate: {back} open simulation(s) restored");
    }
    *DIR.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(dir);
}

fn session_of(user: &User, sim_id: &str) -> SimResult<Shared> {
    sweep();
    let s = store().get(sim_id).cloned().ok_or(SimError::NotFound)?;
    if lock(&s).account != user.id {
        return Err(SimError::NotFound);
    }
    Ok(s)
}

/// The caller still runs the session's party (DM status is re-read on every call).
async fn still_dm(state: &AppState, user: &User, s: &Shared) -> SimResult<()> {
    let party = lock(s).party.clone();
    let dm: Option<String> = sqlx::query("SELECT dm_id FROM parties WHERE id = $1")
        .bind(&party)
        .fetch_optional(state.db.pool())
        .await?
        .map(|r| r.try_get(0))
        .transpose()?;
    if dm.as_deref() != Some(user.id.as_str()) {
        return Err(SimError::NotFound);
    }
    Ok(())
}

// ----- Reading the encounter (SELECTs only) -----

fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The copy of an encounter the caller runs, as a simulation setup.
async fn load(
    state: &AppState,
    user: &User,
    encounter: &str,
    opts: SimOpts,
    seed: u32,
) -> SimResult<(SimSetup, String)> {
    if !id_ok(encounter) {
        return Err(SimError::NotFound);
    }
    let Some(row) = sqlx::query(
        "SELECT e.party_id, p.dm_id, p.ruleset FROM encounters e JOIN parties p ON p.id = e.party_id WHERE e.id = $1",
    )
    .bind(encounter)
    .fetch_optional(state.db.pool())
    .await?
    else {
        return Err(SimError::NotFound);
    };
    let party: String = row.try_get(0)?;
    let dm: String = row.try_get(1)?;
    let ruleset: String = row.try_get(2)?;
    if dm != user.id {
        return Err(SimError::NotFound);
    }
    let edition = if ruleset == "dnd2014" {
        party_combat::Edition::Srd51
    } else {
        party_combat::Edition::Srd521
    };
    let mut creatures = Vec::new();
    for row in sqlx::query(
        "SELECT id, dm_label, statblock_json, overrides_json, element_id, automation_json FROM encounter_creatures
         WHERE encounter_id = $1 ORDER BY sort, id",
    )
    .bind(encounter)
    .fetch_all(state.db.pool())
    .await?
    {
        let id: String = row.try_get(0)?;
        let label: String = row.try_get(1)?;
        let block: String = row.try_get(2)?;
        let overrides: String = row.try_get(3)?;
        let element: String = row.try_get(4).unwrap_or_default();
        let automation: Option<String> = row.try_get(5).ok().flatten();
        let Ok(snapshot) = serde_json::from_str(&block) else {
            tracing::warn!("simulate: an unreadable stat block copy in {encounter}");
            continue;
        };
        creatures.push(CreatureInput {
            id,
            element_id: element,
            snapshot,
            overrides: serde_json::from_str(&overrides).unwrap_or_default(),
            label,
            automation,
        });
    }
    if creatures.is_empty() {
        return Err(SimError::Invalid(
            "The encounter has no creatures to simulate.".into(),
        ));
    }
    let mut start_effects = Vec::new();
    if let Ok(rows) = sqlx::query(
        "SELECT creature_id, name, detail, source_creature_id, duration_turns, concentration FROM encounter_effects
         WHERE encounter_id = $1 ORDER BY sort, id",
    )
    .bind(encounter)
    .fetch_all(state.db.pool())
    .await
    {
        for row in rows {
            let Ok(target) = row.try_get::<String, _>(0) else { continue };
            let mut spec = party_combat::EffectSpec::new(row.try_get::<String, _>(1).unwrap_or_default(), target);
            spec.detail = row.try_get(2).unwrap_or_default();
            spec.source = row.try_get::<Option<String>, _>(3).ok().flatten().map(party_combat::CombatantId::new);
            if let Some(turns) = row.try_get::<Option<i64>, _>(4).ok().flatten() {
                spec.duration = party_combat::Duration::SourceTurns {
                    remaining: u32::try_from(turns).unwrap_or(1),
                    at: party_combat::Boundary::End,
                };
            }
            spec.concentration = row.try_get::<i64, _>(5).unwrap_or(0) != 0;
            start_effects.push(spec);
        }
    }
    let mut rules = Vec::new();
    if let Ok(rows) = sqlx::query(
        "SELECT id, name, text, creature_id, ends_when, active, flags_json FROM encounter_rules
         WHERE encounter_id = $1 ORDER BY sort, id",
    )
    .bind(encounter)
    .fetch_all(state.db.pool())
    .await
    {
        for row in rows {
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
                            Some(party_combat::RuleFlag {
                                target: party_combat::RuleTarget::Combatant(
                                    party_combat::CombatantId::new(f.get("creatureId")?.as_str()?),
                                ),
                                ability: f.get("ability")?.as_str()?.to_string(),
                                while_active: f.get("whileActive").and_then(Value::as_str)
                                    != Some("off"),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let ends: String = row.try_get(4).unwrap_or_default();
            rules.push(party_combat::EncounterRule {
                id: row.try_get(0).unwrap_or_default(),
                name: row.try_get(1).unwrap_or_default(),
                text_active: row.try_get(2).unwrap_or_default(),
                text_ended: String::new(),
                linked: if ends == "manual" {
                    None
                } else {
                    row.try_get::<Option<String>, _>(3)
                        .ok()
                        .flatten()
                        .map(party_combat::CombatantId::new)
                },
                active: row.try_get::<i64, _>(5).unwrap_or(1) != 0,
                flags,
            });
        }
    }
    let pcs = party_pcs(state, &party).await?;
    let spells = spell_defs(state, user, edition, &creatures, &pcs).await;
    Ok((
        SimSetup {
            edition,
            creatures,
            pcs,
            start_effects,
            rules,
            opts,
            seed,
            spells,
        },
        party,
    ))
}

/// The DM's spell mechanics for the spells this setup can cast: every spell of the creatures' spellbooks (the
/// printed block, the kept 5etools spellcasting, the DM's added spells) and of the PCs' sheets, resolved like a
/// combat start (`spells::Library`: the account's definition or link, then the curated SRD row; unlinked stat block
/// names go to the DM's chosen copy). A spell with none of those uses the suggested draft from the DM's own installed
/// data (the 5etools drafter, as the spell detail shows it), marked as an unreviewed draft: the dry run casts it where
/// combat would ask for a ruling, and nothing is saved. When the account's content can't be read now, the curated SRD rows still
/// apply (party-sim's own fallback) and the run says nothing different.
async fn spell_defs(
    state: &AppState,
    user: &User,
    edition: party_combat::Edition,
    creatures: &[CreatureInput],
    pcs: &[PcInput],
) -> SpellDefs {
    let lib = match crate::spells::library(state, &user.id).await {
        Ok(lib) => lib,
        Err(e) => {
            tracing::warn!("simulate: spells unreadable, using the SRD rows: {e:?}");
            return SpellDefs::default();
        }
    };
    let ruleset = crate::spells::ruleset_of(edition);
    let mut out = SpellDefs::default();
    let mut keys: Vec<String> = Vec::new();
    for cr in creatures {
        let f = party_encounter::combatant::fighter_from_snapshot(&cr.snapshot, &cr.overrides);
        let mut c = party_encounter::combatant::creature_of("x", &cr.label, &cr.label, &f, 1);
        party_encounter::spellbook::equip_start(
            &mut c,
            f.data.as_ref(),
            cr.automation.as_deref(),
            &cr.overrides,
        );
        for a in &c.abilities {
            let Some(link) = &a.spell else { continue };
            let key = match &link.key {
                Some(k) => Some(k.clone()),
                None => {
                    let found = lib.resolve(&a.name, ruleset);
                    if let Some(k) = &found {
                        out.names
                            .insert(party_spells::canonical::normalize(&a.name), k.clone());
                    }
                    found
                }
            };
            keys.extend(key);
        }
    }
    for pc in pcs {
        for cl in &pc.spellcasting.classes {
            keys.extend(
                cl.spells
                    .iter()
                    .map(|s| s.id.clone())
                    .filter(|k| !k.is_empty()),
            );
        }
    }
    keys.sort();
    keys.dedup();
    for key in keys {
        let Some(r) = lib.mechanics(ruleset, &key) else {
            if let Some(d) = drafted(state, user, &key).await {
                out.defs.insert(key, d);
            }
            continue;
        };
        let Ok(mech) = serde_json::from_value::<party_combat::spell::SpellMech>(r.def) else {
            tracing::warn!(
                "simulate: a saved spell effect no longer fits; the spell is ruled instead"
            );
            continue;
        };
        let rev = u32::try_from(r.rev).unwrap_or(u32::MAX);
        let origin = if r.origin == "srd" {
            party_combat::spell::MechOrigin::Srd {
                key: r.from_key,
                rev,
            }
        } else {
            party_combat::spell::MechOrigin::Def {
                id: r.from_key,
                rev,
            }
        };
        out.defs.insert(
            key,
            SpellDefn {
                mech,
                origin,
                drafted: false,
            },
        );
    }
    out
}

/// The suggested draft of a spell with no definition, from the DM's own installed data (`spells::detail`).
async fn drafted(state: &AppState, user: &User, key: &str) -> Option<SpellDefn> {
    let d = crate::spells::detail(state, &user.id, key).await.ok()?;
    let mech: party_combat::spell::SpellMech =
        serde_json::from_value(d.get("suggested")?.get("mech")?.clone()).ok()?;
    Some(SpellDefn {
        mech,
        origin: party_combat::spell::MechOrigin::Def {
            id: key.to_string(),
            rev: 0,
        },
        drafted: true,
    })
}

/// The party's characters: each member's sheet read like the DM's sheet view (no roster card written), or its
/// roster card when the sheet can't be read now.
async fn party_pcs(state: &AppState, party: &str) -> SimResult<Vec<PcInput>> {
    let rows = sqlx::query(
        "SELECT owner_id, character_id FROM party_members WHERE party_id = $1 ORDER BY joined_at, character_id",
    )
    .bind(party)
    .fetch_all(state.db.pool())
    .await?;
    let mut out = Vec::new();
    for row in rows {
        let owner: String = row.try_get(0)?;
        let character: String = row.try_get(1)?;
        let sheet = match state.workspaces.loaded_now(&owner) {
            Some(w) => {
                let id = character.clone();
                w.blocking(move |app| crate::parties::snapshot_of(app, &id))
                    .await
                    .ok()
                    .flatten()
                    .and_then(|s| serde_json::to_value(&s).ok())
            }
            None => None,
        };
        let pc = match sheet.and_then(|v| serde_json::from_value::<PcInput>(v).ok()) {
            Some(mut pc) if pc.play.hit_points.max > 0 => {
                pc.id = character.clone();
                pc.owner = owner.clone();
                pc
            }
            _ => {
                let hub = Arc::clone(&state.hub);
                let (o, c) = (owner.clone(), character.clone());
                let card = tokio::task::spawn_blocking(move || hub.cards.get(&o, &c))
                    .await
                    .map_err(internal)?
                    .unwrap_or_default();
                let stats = card.stats.clone().unwrap_or_default();
                let hp = party_combat::Hp {
                    current: i32::try_from(stats.hp).unwrap_or(1),
                    max: i32::try_from(stats.hp_max).unwrap_or(1).max(1),
                    temp: i32::try_from(stats.temp_hp).unwrap_or(0),
                };
                PcInput::from_card(
                    &character,
                    &owner,
                    card.name.as_deref().unwrap_or("Character"),
                    hp,
                    stats.armor_class,
                    stats.initiative,
                    stats.speed,
                )
            }
        };
        out.push(pc);
    }
    Ok(out)
}

// ----- Views -----

fn roster(sim: &Sim) -> Vec<Value> {
    let alive = sim.alive_ids();
    sim.state
        .combatants
        .iter()
        .map(|c| {
            json!({
                "id": c.id, "name": sim.name(&c.id), "side": sim.side(&c.id), "hp": c.hp.max, "ac": c.ac,
                "initiative": c.initiative, "band": sim.space.band(&c.id, &alive),
                "controlledBy": if sim.info.get(&c.id).is_some_and(|i| i.agent) { "agent" } else { "basic" },
            })
        })
        .collect()
}

fn board(s: &Session) -> Value {
    let sim = &s.sim;
    let combatants: Vec<Value> = sim
        .state
        .combatants
        .iter()
        .map(|c| {
            json!({
                "id": c.id, "name": sim.name(&c.id), "side": sim.side(&c.id),
                "hp": c.hp.current, "maxHp": c.hp.max, "tempHp": c.hp.temp, "life": c.life,
                "position": sim.space.at(&c.id), "inFight": sim.space.present(&c.id),
                "conditions": party_combat::derive::conditions(&sim.state, &c.id),
                "concentration": c.concentration.as_ref().map(|k| k.name.clone()),
                "slots": c.spell_slots, "resources": c.resources.values().map(|r| json!({"name": r.label, "left": r.pool.remaining})).collect::<Vec<_>>(),
                "legendaryResistance": c.legendary_resistance.map(|p| p.remaining),
            })
        })
        .collect();
    let start = sim.log.len().saturating_sub(30);
    json!({
        "step": sim.step, "round": sim.state.round, "current": sim.state.current, "ended": sim.outcome,
        "combatants": combatants,
        "log": sim.log[start..].iter().map(|l| l.dm.clone()).collect::<Vec<_>>(),
    })
}

/// The open card (advancing Basic turns first), or null when it ended.
fn next_card(sim: &mut Sim) -> Value {
    match sim.advance() {
        Pending::Decide { card, .. } => card,
        Pending::Ended(_) => Value::Null,
    }
}

// ----- Calls -----

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> SimResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(internal)
}

/// Run one sim tool for `user`. Arguments are the registry's (already scope-checked; `requestId` removed).
///
/// # Errors
/// [`SimError`]: not found for anything not the caller's, invalid arguments, busy.
pub async fn call(
    state: &AppState,
    user: &User,
    tool: &str,
    args: Map<String, Value>,
) -> SimResult<Value> {
    if args.contains_key("combatId") || args.contains_key("combat_id") {
        return Err(SimError::Invalid(
            "Simulations take an encounterId; they never touch combats.".into(),
        ));
    }
    match tool {
        "sim_start" => start(state, user, parse(args)?).await,
        "sim_card" => {
            let a: SimIdArgs = parse(args)?;
            let s = session_of(user, &a.sim_id)?;
            still_dm(state, user, &s).await?;
            blocking(move || {
                let mut g = lock(&s);
                g.last_used = now();
                let card = next_card(&mut g.sim);
                save(&a.sim_id, &g);
                json!({ "card": card })
            })
            .await
        }
        "sim_act" => act(state, user, parse(args)?).await,
        "sim_auto" => auto(state, user, parse(args)?).await,
        "sim_state" => {
            let a: SimIdArgs = parse(args)?;
            let s = session_of(user, &a.sim_id)?;
            still_dm(state, user, &s).await?;
            blocking(move || {
                let mut g = lock(&s);
                g.last_used = now();
                save(&a.sim_id, &g);
                board(&g)
            })
            .await
        }
        "sim_report" => {
            let a: SimIdArgs = parse(args)?;
            let s = session_of(user, &a.sim_id)?;
            still_dm(state, user, &s).await?;
            store().remove(&a.sim_id);
            forget(&a.sim_id);
            let g = lock(&s);
            let mut v = serde_json::to_value(party_sim::report(&g.sim)).map_err(internal)?;
            v["replay"] = json!({ "encounterId": g.encounter, "seed": g.setup.seed });
            v["ended"] = json!(g.sim.outcome.is_some());
            Ok(v)
        }
        "sim_batch" => batch(state, user, parse(args)?).await,
        "sim_replay" => replay(state, user, parse(args)?).await,
        _ => Err(SimError::NotFound),
    }
}

async fn start(state: &AppState, user: &User, a: StartArgs) -> SimResult<Value> {
    sweep();
    let opts = a.opts.to_opts()?;
    let seed = a.seed.unwrap_or_else(rand::random);
    let (setup, party) = load(state, user, &a.encounter_id, opts, seed).await?;
    let mine = store()
        .values()
        .filter(|s| lock(s).account == user.id)
        .count();
    if mine >= PER_ACCOUNT {
        return Err(SimError::Busy(format!(
            "{PER_ACCOUNT} simulations are open; finish one with sim_report first (idle ones close after 30 minutes)."
        )));
    }
    let _job = crate::combat::job_slot(&user.id)
        .await
        .ok_or_else(|| SimError::Busy("Too many heavy calls at once.".into()))?;
    let encounter = a.encounter_id.clone();
    let account = user.id.clone();
    let (sim_id, out) = blocking(move || -> SimResult<(String, Value)> {
        let mut sim = Sim::new(&setup).map_err(SimError::Invalid)?;
        let card = next_card(&mut sim);
        let out = json!({
            "seed": setup.seed,
            "roster": roster(&sim),
            "coverage": sim.coverage,
            "assumptions": party_sim::report::assumptions(&sim),
            "card": card,
        });
        let id = uuid::Uuid::new_v4().simple().to_string();
        let session = Session {
            account,
            party,
            encounter,
            setup,
            sim,
            last_used: now(),
        };
        save(&id, &session);
        store().insert(id.clone(), Arc::new(Mutex::new(session)));
        Ok((id, out))
    })
    .await??;
    let mut out = out;
    out["simId"] = json!(sim_id);
    Ok(out)
}

async fn act(state: &AppState, user: &User, a: ActArgs) -> SimResult<Value> {
    let s = session_of(user, &a.sim_id)?;
    still_dm(state, user, &s).await?;
    let ds = match a.d {
        OneOrMore::One(d) => vec![*d],
        OneOrMore::More(v) => v,
    };
    if ds.is_empty() || ds.len() > 3 {
        return Err(SimError::Invalid(
            "d is one decision or a list of 1 to 3".into(),
        ));
    }
    let id = a.sim_id.clone();
    let step = a.step;
    blocking(move || {
        let mut g = lock(&s);
        let out = act_on(&mut g, step, ds);
        save(&id, &g);
        out
    })
    .await?
}

/// [`act`] on the locked session.
fn act_on(g: &mut Session, step: u32, ds: Vec<Decision>) -> SimResult<Value> {
    g.last_used = now();
    let card = next_card(&mut g.sim);
    if card.is_null() {
        return Ok(
            json!({ "step": g.sim.step, "lines": [], "card": null, "ended": g.sim.outcome }),
        );
    }
    if step != g.sim.step {
        return Ok(json!({ "stale": true, "step": g.sim.step, "lines": [], "card": card }));
    }
    let actor = g.sim.decider();
    let mut lines = Vec::new();
    for (i, d) in ds.into_iter().enumerate() {
        if i > 0 && (g.sim.decider() != actor || matches!(g.sim.advance(), Pending::Ended(_))) {
            break; // a bundle is one combatant's turn
        }
        match g.sim.answer(d, By::Agent) {
            Ok(more) => lines.extend(more),
            Err(refusal) => {
                let card = next_card(&mut g.sim);
                return Ok(
                    json!({ "step": g.sim.step, "error": refusal.0, "lines": lines, "card": card }),
                );
            }
        }
    }
    let card = next_card(&mut g.sim);
    Ok(json!({ "step": g.sim.step, "lines": lines, "card": card, "ended": g.sim.outcome }))
}

async fn auto(state: &AppState, user: &User, a: AutoArgs) -> SimResult<Value> {
    let s = session_of(user, &a.sim_id)?;
    still_dm(state, user, &s).await?;
    if a.oracle.as_deref().is_some_and(|o| o != "basic") {
        return Err(SimError::Invalid(
            "oracle is basic: the server never calls an AI model; play with your own through sim_act".into(),
        ));
    }
    let _job = crate::combat::job_slot(&user.id)
        .await
        .ok_or_else(|| SimError::Busy("Too many heavy calls at once.".into()))?;
    blocking(move || {
        let mut g = lock(&s);
        g.last_used = now();
        let _ = next_card(&mut g.sim);
        if a.step != g.sim.step {
            let card = next_card(&mut g.sim);
            save(&a.sim_id, &g);
            return Ok(json!({ "stale": true, "step": g.sim.step, "card": card }));
        }
        let before = g.sim.log.len();
        if !a.who.is_empty() {
            for w in &a.who {
                let id = party_combat::CombatantId::new(w.clone());
                if let Some(i) = g.sim.info.get_mut(&id) {
                    i.agent = false;
                }
            }
            let _ = g.sim.advance();
        } else {
            let _ = g.sim.auto(a.until);
        }
        let fog = g.sim.opts.fog;
        let lines: Vec<String> = g.sim.log[before..]
            .iter()
            .map(|l| if fog { l.fog.clone() } else { l.dm.clone() })
            .collect();
        let card = next_card(&mut g.sim);
        save(&a.sim_id, &g);
        Ok(json!({ "step": g.sim.step, "lines": lines, "card": card, "ended": g.sim.outcome }))
    })
    .await?
}

async fn batch(state: &AppState, user: &User, a: BatchArgs) -> SimResult<Value> {
    let runs = a.runs.unwrap_or(30);
    if !(1..=100).contains(&runs) {
        return Err(SimError::Invalid("runs is 1 to 100".into()));
    }
    let mut opts = a.opts.to_opts()?;
    opts.control.agent = AgentControl::Side("none".into());
    let seed = a.seed.unwrap_or_else(rand::random);
    let (setup, _) = load(state, user, &a.encounter_id, opts, seed).await?;
    let _job = crate::combat::job_slot(&user.id)
        .await
        .ok_or_else(|| SimError::Busy("Too many heavy calls at once.".into()))?;
    blocking(move || {
        let t = Instant::now();
        let r = party_sim::batch(&setup, runs, seed, &mut || t.elapsed() < BATCH_BUDGET)
            .map_err(SimError::Invalid)?;
        serde_json::to_value(r).map_err(internal)
    })
    .await?
}

async fn replay(state: &AppState, user: &User, a: ReplayArgs) -> SimResult<Value> {
    let opts = a.opts.to_opts()?;
    let (setup, _) = load(state, user, &a.encounter_id, opts, a.seed).await?;
    let _job = crate::combat::job_slot(&user.id)
        .await
        .ok_or_else(|| SimError::Busy("Too many heavy calls at once.".into()))?;
    blocking(move || {
        let sim = Sim::new(&setup).map_err(SimError::Invalid)?;
        if let Some(fp) = &a.fingerprints {
            let changed: Vec<&String> = fp
                .iter()
                .filter(|(k, v)| sim.fingerprints.get(*k) != Some(*v))
                .map(|(k, _)| k)
                .collect();
            if !changed.is_empty() {
                return Ok(
                    json!({ "identical": false, "divergence": { "fingerprints": changed } }),
                );
            }
        }
        match party_sim::replay(&setup, &a.transcript) {
            Ok(sim) => Ok(json!({ "identical": true, "report": party_sim::report(&sim) })),
            Err(e) => Ok(json!({ "identical": false, "divergence": { "step": e } })),
        }
    })
    .await?
}

/// Forget every open session in memory (their files stay, as after a crash).
#[cfg(test)]
pub(crate) fn clear_sessions() {
    store().clear();
}

/// A server restart: memory is gone, and start-up loads `dir` again (as [`install`]).
#[cfg(test)]
pub(crate) fn restart(dir: &Path) {
    store().clear();
    install(dir.to_path_buf());
}

/// Back to memory only (the other tests' default).
#[cfg(test)]
pub(crate) fn stop_saving() {
    *DIR.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}
