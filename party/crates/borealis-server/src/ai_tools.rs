//! The assistant's tools: a stand-in for part B's shared `party-tools` registry (`party/docs/party-tools.md` on
//! `feat/mcp`), until it lands. The names, scopes and argument names here are B's, so the swap touches this file
//! alone:
//! - [`all`]: every tool, each with its name, a description that never quotes anyone's content, a JSON Schema for
//!   its arguments, the scope it needs and what it changes ([`Effect`]);
//! - [`call`]: run one as the signed-in account. At merge this becomes `Registry::call(&ServerTools, &Caller { via:
//!   Via::Assistant, .. }, name, args)`. Until then the encounter and bestiary tools call the same route handlers as
//!   the app, so the same permission and projection checks apply; the character, simulation, homebrew and index tools
//!   are B's, E's and D's and answer "not available yet".
//!
//! Scopes (the user's D25, D26, D47): `bestiary:read`, `encounters:read`, `encounters:write`, `encounters:simulate`,
//! `characters:read`, `homebrew:read`, `homebrew:write`. There is no combat tool (D25: the assistant never touches a
//! live combat) and no publishing (D47: an index is built and downloaded, never hosted). No result ever carries XP or
//! a difficulty (the user's D16): [`strip_d16`] takes them out of everything a tool returns.

use axum::body::Bytes;
use axum::extract::{Path, State};
use serde_json::{json, Value};

use crate::auth::User;
use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// What a tool changes, which decides whether the person confirms it first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Reads, or works on a copy (a simulation, an index built for download): writes nothing.
    Read,
    /// Writes a draft only the account sees (an encounter draft, a homebrew draft): the person confirms each one,
    /// unless they turned on auto-apply for drafts.
    Draft,
}

impl Effect {
    /// As sent to the panel.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Draft => "draft",
        }
    }
}

/// What the assistant is helping with; each purpose offers only its tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Encounter building and simulation.
    Encounter,
    /// Homebrew building.
    Homebrew,
    /// Custom index creation (a download, D47).
    Index,
}

impl Purpose {
    /// From the panel's `purpose`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "encounter" => Some(Self::Encounter),
            "homebrew" => Some(Self::Homebrew),
            "index" => Some(Self::Index),
            _ => None,
        }
    }
}

/// One tool.
pub struct Tool {
    /// Name, as the model calls it.
    pub name: &'static str,
    /// What it does.
    pub description: &'static str,
    /// The scope it needs.
    pub scope: &'static str,
    /// What it changes.
    pub effect: Effect,
    /// The purposes that offer it.
    pub purposes: &'static [Purpose],
    /// JSON Schema of its arguments.
    pub schema: fn() -> Value,
}

const ENC: &[Purpose] = &[Purpose::Encounter];
const BREW: &[Purpose] = &[Purpose::Homebrew, Purpose::Index];
const ANY: &[Purpose] = &[Purpose::Encounter, Purpose::Homebrew, Purpose::Index];

fn object(properties: &Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

static TOOLS: &[Tool] = &[
    Tool {
        name: "character_list",
        description: "This account's characters, and the parties it plays in or runs as DM (id, name, ruleset, role).",
        scope: "characters:read",
        effect: Effect::Read,
        purposes: ENC,
        schema: || object(&json!({}), &[]),
    },
    Tool {
        name: "bestiary_search",
        description: "Search this account's bestiary. Filters: text (part of the name), crMin and crMax (as printed, \
                      e.g. \"1/2\", \"5\"), types, sizes, environments (e.g. [\"forest\"]), ruleset (dnd2014 or \
                      dnd2024), legendary, limit (at most 20). CR is a printed stat, not a difficulty.",
        scope: "bestiary:read",
        effect: Effect::Read,
        purposes: ANY,
        schema: || {
            object(
                &json!({
                    "text": { "type": "string" },
                    "crMin": { "type": "string" },
                    "crMax": { "type": "string" },
                    "types": { "type": "array", "items": { "type": "string" } },
                    "sizes": { "type": "array", "items": { "type": "string" } },
                    "environments": { "type": "array", "items": { "type": "string" } },
                    "ruleset": { "type": "string", "enum": ["dnd2014", "dnd2024"] },
                    "legendary": { "type": "boolean" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 20 },
                }),
                &[],
            )
        },
    },
    Tool {
        name: "monster_get",
        description: "One creature's full stat block from this account's bestiary, by the id bestiary_search gave.",
        scope: "bestiary:read",
        effect: Effect::Read,
        purposes: ANY,
        schema: || object(&json!({ "id": { "type": "string" } }), &["id"]),
    },
    Tool {
        name: "encounter_list",
        description: "The encounters of one party this account runs: id, name and status.",
        scope: "encounters:read",
        effect: Effect::Read,
        purposes: ENC,
        schema: || object(&json!({ "partyId": { "type": "string" } }), &["partyId"]),
    },
    Tool {
        name: "encounter_get",
        description: "One encounter with its creatures, effects and rules.",
        scope: "encounters:read",
        effect: Effect::Read,
        purposes: ENC,
        schema: || object(&json!({ "encounterId": { "type": "string" } }), &["encounterId"]),
    },
    Tool {
        name: "encounter_create",
        description: "Create a new draft encounter in a party this account runs as DM. Players don't see drafts.",
        scope: "encounters:write",
        effect: Effect::Draft,
        purposes: ENC,
        schema: || {
            object(
                &json!({
                    "partyId": { "type": "string" },
                    "name": { "type": "string" },
                    "notes": { "type": "string", "description": "Notes only the DM sees: the setup, tactics." },
                }),
                &["partyId", "name"],
            )
        },
    },
    Tool {
        name: "encounter_add_creature",
        description: "Add 1 to 20 copies of one creature (an id from bestiary_search) to a draft encounter.",
        scope: "encounters:write",
        effect: Effect::Draft,
        purposes: ENC,
        schema: || {
            object(
                &json!({
                    "encounterId": { "type": "string" },
                    "elementId": { "type": "string" },
                    "count": { "type": "integer", "minimum": 1, "maximum": 20 },
                    "hidden": { "type": "boolean" },
                }),
                &["encounterId", "elementId"],
            )
        },
    },
    Tool {
        name: "sim_start",
        description: "Start a seeded simulation of an encounter on a copy: it writes nothing to the encounter or any \
                      combat. Answers the simulation's id, its roster and what it can't simulate.",
        scope: "encounters:simulate",
        effect: Effect::Read,
        purposes: ENC,
        schema: || {
            object(
                &json!({ "encounter_id": { "type": "string" }, "seed": { "type": "integer", "minimum": 0 } }),
                &["encounter_id"],
            )
        },
    },
    Tool {
        name: "sim_auto",
        description: "Let the basic policy play a started simulation until the turn, the round or the end.",
        scope: "encounters:simulate",
        effect: Effect::Read,
        purposes: ENC,
        schema: || {
            object(
                &json!({
                    "sim_id": { "type": "string" },
                    "step": { "type": "integer" },
                    "until": { "type": "string", "enum": ["turn", "round", "end"] },
                }),
                &["sim_id", "step", "until"],
            )
        },
    },
    Tool {
        name: "sim_report",
        description: "A simulation's report: the outcome, key moments and facts per character and creature. It ends \
                      the simulation. Describe the outcome; never rate difficulty.",
        scope: "encounters:simulate",
        effect: Effect::Read,
        purposes: ENC,
        schema: || object(&json!({ "sim_id": { "type": "string" } }), &["sim_id"]),
    },
    Tool {
        name: "sim_batch",
        description: "Run an encounter many times with the basic policy: counts out of N, never odds or a rating.",
        scope: "encounters:simulate",
        effect: Effect::Read,
        purposes: ENC,
        schema: || {
            object(
                &json!({
                    "encounter_id": { "type": "string" },
                    "runs": { "type": "integer", "minimum": 1, "maximum": 100 },
                    "seed": { "type": "integer", "minimum": 0 },
                }),
                &["encounter_id"],
            )
        },
    },
    Tool {
        name: "homebrew_validate",
        description: "Check a homebrew element (Aurora XML) with the rules engine and list its problems.",
        scope: "homebrew:read",
        effect: Effect::Read,
        purposes: BREW,
        schema: || object(&json!({ "xml": { "type": "string" } }), &["xml"]),
    },
    Tool {
        name: "homebrew_create",
        description: "Add a homebrew element (Aurora XML) to this account's own homebrew pack.",
        scope: "homebrew:write",
        effect: Effect::Draft,
        purposes: BREW,
        schema: || object(&json!({ "xml": { "type": "string" } }), &["xml"]),
    },
    Tool {
        name: "homebrew_update",
        description: "Change a homebrew element of this account's own pack, keeping its id.",
        scope: "homebrew:write",
        effect: Effect::Draft,
        purposes: BREW,
        schema: || object(&json!({ "id": { "type": "string" }, "xml": { "type": "string" } }), &["id", "xml"]),
    },
    Tool {
        name: "index_build",
        description: "Build a custom .index and its element files from this account's own homebrew packs, as data: \
                      the app offers them to the person as a zip. Nothing is published or hosted.",
        scope: "homebrew:read",
        effect: Effect::Read,
        purposes: &[Purpose::Index],
        schema: || {
            object(
                &json!({
                    "packId": { "type": "string", "description": "One of this account's own homebrew packs." },
                    "baseUrl": { "type": "string", "description": "Where the person will host the files (their own git repository)." },
                }),
                &["packId", "baseUrl"],
            )
        },
    },
];

/// Every tool.
#[must_use]
pub fn all() -> &'static [Tool] {
    TOOLS
}

/// The tool called `name`.
#[must_use]
pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

/// Keys that would carry XP or a difficulty rating (the user's D16), removed from every result.
const D16_KEYS: [&str; 8] = [
    "xp",
    "xp_lair",
    "xpLair",
    "difficulty",
    "xpBudget",
    "xp_budget",
    "adjustedXp",
    "evaluation",
];

/// `value` without XP or difficulty anywhere in it.
pub fn strip_d16(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|k, _| !D16_KEYS.contains(&k.as_str()));
            map.values_mut().for_each(strip_d16);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_d16),
        _ => {}
    }
}

fn arg(args: &Value, name: &str) -> ApiResult<String> {
    args[name]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ApiError::Invalid(format!("The tool needs {name}.")))
}

fn parties_on() -> ApiResult<()> {
    if crate::config::features().parties {
        Ok(())
    } else {
        Err(ApiError::NotFound("The server does not offer that.".into()))
    }
}

fn not_yet(what: &str) -> ApiResult<Value> {
    Err(ApiError::Refused(
        "not_available",
        format!("{what} isn't available on this server yet."),
    ))
}

/// Run tool `name` as `user`. The caller has checked the scope and the confirmation; this does what the app would.
///
/// # Errors
/// What the route behind the tool answers, or a tool that doesn't exist.
pub async fn call(state: &AppState, user: &User, name: &str, args: &Value) -> ApiResult<Value> {
    let st = || State(state.clone());
    let me = || user.clone();
    let body = |v: Value| Bytes::from(v.to_string());
    let mut out = match name {
        "character_list" => {
            parties_on()?;
            let all = crate::parties::list(st(), me()).await?.0;
            json!({ "parties": all["parties"].as_array().map(|ps| ps.iter().map(|p| json!({
                "id": p["id"], "name": p["name"], "ruleset": p["ruleset"], "role": p["role"], "memberCount": p["memberCount"],
            })).collect::<Vec<_>>()).unwrap_or_default() })
        }
        "bestiary_search" => {
            let mut query = serde_json::Map::new();
            for key in [
                "text",
                "crMin",
                "crMax",
                "types",
                "sizes",
                "environments",
                "ruleset",
                "legendary",
            ] {
                if !args[key].is_null() {
                    query.insert(key.into(), args[key].clone());
                }
            }
            let limit = args["limit"].as_u64().unwrap_or(10).clamp(1, 20);
            query.insert("limit".into(), json!(limit));
            let query: crate::bestiary::SearchQuery = serde_json::from_value(Value::Object(query))
                .map_err(|e| ApiError::Invalid(format!("The search doesn't fit: {e}")))?;
            serde_json::to_value(crate::bestiary::search(state, &user.id, &query).await?)
                .map_err(|e| ApiError::Internal(e.to_string()))?
        }
        "monster_get" => {
            let id = arg(args, "id")?;
            let found = crate::bestiary::monster(state, &user.id, &id)
                .await?
                .ok_or_else(|| ApiError::NotFound("No such creature.".into()))?;
            serde_json::to_value(found).map_err(|e| ApiError::Internal(e.to_string()))?
        }
        "encounter_list" => {
            parties_on()?;
            crate::encounters::list(st(), me(), Path(arg(args, "partyId")?))
                .await?
                .0
        }
        "encounter_get" => {
            parties_on()?;
            crate::encounters::read(st(), me(), Path(arg(args, "encounterId")?))
                .await?
                .0
        }
        "encounter_create" => {
            parties_on()?;
            let party = arg(args, "partyId")?;
            let mut form = json!({ "name": arg(args, "name")? });
            if let Some(notes) = args["notes"].as_str() {
                form["dmNotes"] = json!(notes);
            }
            crate::encounters::create(st(), me(), Path(party), body(form))
                .await?
                .0
        }
        "encounter_add_creature" => {
            parties_on()?;
            let encounter = arg(args, "encounterId")?;
            let mut form = json!({ "elementId": arg(args, "elementId")? });
            for key in ["count", "hidden"] {
                if !args[key].is_null() {
                    form[key] = args[key].clone();
                }
            }
            crate::encounters::add_creatures(st(), me(), Path(encounter), body(form))
                .await?
                .0
        }
        "sim_start" | "sim_auto" | "sim_report" | "sim_batch" => not_yet("Simulation")?,
        "homebrew_validate" | "homebrew_create" | "homebrew_update" => {
            not_yet("Homebrew authoring")?
        }
        "index_build" => not_yet("Building a custom index")?,
        _ => return Err(ApiError::NotFound("There is no such tool.".into())),
    };
    strip_d16(&mut out);
    Ok(out)
}
