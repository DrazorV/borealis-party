//! The assistant's tools: part B's shared `party-tools` registry (`party/docs/party-tools.md`), as the assistant panel
//! (`ai_assistant.rs`) uses it.
//!
//! - [`all`] / [`find`]: the registry's tools for the assistant's scopes, each with the purposes that offer it and what
//!   it changes ([`Effect`]: a tool that writes is a draft the person confirms).
//! - [`call`]: one call through [`crate::mcp::registry`] with [`ServerTools`] as the signed-in account, `via:
//!   Assistant`, no token. The registry checks the scope, the arguments, the per-account rate, the heavy-call limit and
//!   a write's `requestId` (a repeated one answers the first result again), and records the call in the account's
//!   activity. The handlers are the API's own, so the permission and projection checks are the app's.
//!
//! Scopes (the user's D25, D26, D47): see [`crate::ai_assistant::SCOPES`]. There is no combat tool and nothing is
//! published. No result carries XP or a difficulty (the user's D16): [`strip_d16`] takes them out of every result.

use std::collections::BTreeSet;

use party_tools::{Caller, Scope, ToolError, ToolSpec, Via};
use serde_json::Value;

use crate::auth::User;
use crate::tools_backend::ServerTools;
use crate::AppState;

/// What a tool changes, which decides whether the person confirms it first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Reads, or works on a copy (a simulation, an index built for download): writes nothing.
    Read,
    /// Writes a draft only the account sees (an encounter draft, homebrew): the person confirms each one, unless
    /// they turned on auto-apply for drafts.
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

const ENC: &[Purpose] = &[Purpose::Encounter];
const BREW: &[Purpose] = &[Purpose::Homebrew];
const BREW_INDEX: &[Purpose] = &[Purpose::Homebrew, Purpose::Index];
const INDEX: &[Purpose] = &[Purpose::Index];
const READ_ALL: &[Purpose] = &[Purpose::Encounter, Purpose::Homebrew];
const NONE: &[Purpose] = &[];

/// The purposes that offer the tool `name`.
fn purposes_of(name: &str) -> &'static [Purpose] {
    match name {
        "bestiary_search" | "monster_get" => READ_ALL,
        "index_build" => INDEX,
        "homebrew_list" | "homebrew_get" | "homebrew_validate" | "homebrew_types" => BREW_INDEX,
        n if n.starts_with("homebrew_") => BREW,
        n if n.starts_with("encounter_")
            || n.starts_with("character_")
            || n.starts_with("sim_") =>
        {
            ENC
        }
        _ => NONE,
    }
}

/// One tool as the assistant offers it.
pub struct Tool {
    /// Name, as the model calls it.
    pub name: &'static str,
    /// What it does: the registry's fixed text, never anyone's content.
    pub description: &'static str,
    /// The scope it needs.
    pub scope: &'static str,
    /// What it changes.
    pub effect: Effect,
    /// The purposes that offer it.
    pub purposes: &'static [Purpose],
    /// JSON Schema of its arguments.
    pub schema: Value,
}

impl Tool {
    fn of(spec: &ToolSpec) -> Self {
        Self {
            name: spec.name,
            description: spec.description,
            scope: spec.scope.as_str(),
            effect: if spec.write {
                Effect::Draft
            } else {
                Effect::Read
            },
            purposes: purposes_of(spec.name),
            schema: spec.input_schema.clone(),
        }
    }
}

/// The assistant's scopes as the registry takes them.
fn scopes() -> BTreeSet<Scope> {
    crate::ai_assistant::SCOPES
        .iter()
        .filter_map(|s| Scope::parse(s))
        .collect()
}

fn caller(user: &User) -> Caller {
    Caller {
        account: user.id.clone(),
        token: None,
        scopes: scopes(),
        via: Via::Assistant,
    }
}

/// Every tool the assistant's scopes allow and some purpose offers.
#[must_use]
pub fn all() -> Vec<Tool> {
    let scopes = scopes();
    crate::mcp::registry()
        .all()
        .into_iter()
        .filter(|s| scopes.contains(&s.scope) && !purposes_of(s.name).is_empty())
        .map(Tool::of)
        .collect()
}

/// The tool called `name`, if the assistant may use it.
#[must_use]
pub fn find(name: &str) -> Option<Tool> {
    all().into_iter().find(|t| t.name == name)
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

/// Run tool `name` as `user` through the shared registry. The caller has checked the person's confirmation.
///
/// # Errors
/// The registry's [`ToolError`], which the model gets in words.
pub async fn call(
    state: &AppState,
    user: &User,
    name: &str,
    args: Value,
) -> Result<Value, ToolError> {
    let backend = ServerTools::new(state.clone(), user.clone());
    let out = crate::mcp::registry()
        .call(&backend, &caller(user), name, args)
        .await?;
    let mut value = out.value;
    strip_d16(&mut value);
    Ok(value)
}
