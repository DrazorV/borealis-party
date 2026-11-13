//! Typed tool arguments. Every struct refuses unknown keys, so a model that invents an argument (`dmId`, `path`,
//! `owner`, ...) gets a clear error instead of having it silently ignored, and nothing a client sends is merged
//! into a request the server builds.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::ToolError;

/// `bestiary_search`: the fields of `POST /api/bestiary/search`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BestiarySearchArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cr_min: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cr_max: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sizes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environments: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ruleset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legendary: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

impl BestiarySearchArgs {
    /// No filter at all: a full listing, counted as a heavy call.
    #[must_use]
    pub fn unfiltered(&self) -> bool {
        self.text.as_deref().is_none_or(str::is_empty)
            && self.cr_min.is_none()
            && self.cr_max.is_none()
            && self.types.is_empty()
            && self.sizes.is_empty()
            && self.environments.is_empty()
            && self.sources.is_empty()
            && self.legendary.is_none()
    }
}

/// `monster_get`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdArgs {
    pub id: String,
}

/// `encounter_list`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartyArgs {
    pub party_id: String,
}

/// `encounter_get`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncounterArgs {
    pub encounter_id: String,
}

/// `encounter_create`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncounterCreateArgs {
    pub request_id: String,
    pub party_id: String,
    pub name: String,
    #[serde(default)]
    pub public_name: Option<String>,
    #[serde(default)]
    pub hp_mode: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// `encounter_update`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncounterUpdateArgs {
    pub request_id: String,
    pub encounter_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub public_name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub player_visible: Option<bool>,
    #[serde(default)]
    pub hp_mode: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// `encounter_add_creature`. `ownerCreatureId` makes it a pet, summon or mount of another creature of the
/// encounter; `hidden` keeps it from players (D20 b).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncounterAddCreatureArgs {
    pub request_id: String,
    pub encounter_id: String,
    pub element_id: String,
    #[serde(default)]
    pub count: Option<i64>,
    #[serde(default)]
    pub dm_label: Option<String>,
    #[serde(default)]
    pub public_name: Option<String>,
    #[serde(default)]
    pub hidden: Option<bool>,
    #[serde(default)]
    pub owner_creature_id: Option<String>,
    #[serde(default)]
    pub init_mode: Option<String>,
}

/// `encounter_remove_creature`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncounterRemoveCreatureArgs {
    pub request_id: String,
    pub encounter_id: String,
    pub creature_id: String,
}

/// `character_list`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NoArgs {}

/// `character_get`: one of the caller's own characters, or (with `partyId` and `ownerId`) a member character of
/// a party the caller runs, read-only.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CharacterGetArgs {
    pub character_id: String,
    #[serde(default)]
    pub party_id: Option<String>,
    #[serde(default)]
    pub owner_id: Option<String>,
}

/// One parsed tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolCall {
    BestiarySearch(BestiarySearchArgs),
    MonsterGet(IdArgs),
    EncounterList(PartyArgs),
    EncounterGet(EncounterArgs),
    EncounterCreate(EncounterCreateArgs),
    EncounterUpdate(EncounterUpdateArgs),
    EncounterAddCreature(EncounterAddCreatureArgs),
    EncounterRemoveCreature(EncounterRemoveCreatureArgs),
    CharacterList(NoArgs),
    CharacterGet(CharacterGetArgs),
    /// A tool another module serves (`homebrew_*`, `index_build`, `sim_*`), with its arguments as sent: that module
    /// checks them.
    External {
        tool: &'static str,
        args: Map<String, Value>,
    },
}

impl ToolCall {
    /// The write's request id, for idempotency.
    #[must_use]
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::EncounterCreate(a) => Some(&a.request_id),
            Self::EncounterUpdate(a) => Some(&a.request_id),
            Self::EncounterAddCreature(a) => Some(&a.request_id),
            Self::EncounterRemoveCreature(a) => Some(&a.request_id),
            Self::External { args, .. } => args.get("requestId").and_then(Value::as_str),
            _ => None,
        }
    }
}

fn typed<T: DeserializeOwned>(args: &Value) -> Result<T, ToolError> {
    serde_json::from_value(args.clone()).map_err(|e| ToolError::InvalidArgs(e.to_string()))
}

/// A request id: 8–64 of `[A-Za-z0-9_-]` (the combat API's rule, D44, with a minimum so ids aren't guessed).
#[must_use]
pub fn request_id_ok(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Parse `args` for `tool` (a name the registry knows: anything that isn't built in goes to its module).
///
/// # Errors
/// `InvalidArgs` for anything that doesn't fit the tool's arguments.
pub fn parse(tool: &'static str, args: &Value) -> Result<ToolCall, ToolError> {
    if !args.is_object() {
        return Err(ToolError::InvalidArgs("arguments must be an object".into()));
    }
    let call = match tool {
        "bestiary_search" => ToolCall::BestiarySearch(typed(args)?),
        "monster_get" => ToolCall::MonsterGet(typed(args)?),
        "encounter_list" => ToolCall::EncounterList(typed(args)?),
        "encounter_get" => ToolCall::EncounterGet(typed(args)?),
        "encounter_create" => ToolCall::EncounterCreate(typed(args)?),
        "encounter_update" => ToolCall::EncounterUpdate(typed(args)?),
        "encounter_add_creature" => ToolCall::EncounterAddCreature(typed(args)?),
        "encounter_remove_creature" => ToolCall::EncounterRemoveCreature(typed(args)?),
        "character_list" => ToolCall::CharacterList(typed(args)?),
        "character_get" => {
            let a: CharacterGetArgs = typed(args)?;
            if a.party_id.is_some() != a.owner_id.is_some() {
                return Err(ToolError::InvalidArgs(
                    "partyId and ownerId go together".into(),
                ));
            }
            ToolCall::CharacterGet(a)
        }
        t => ToolCall::External {
            tool: t,
            args: args.as_object().cloned().unwrap_or_default(),
        },
    };
    if let Some(id) = call.request_id() {
        if !request_id_ok(id) {
            return Err(ToolError::InvalidArgs(
                "requestId must be 8 to 64 letters, digits, - or _".into(),
            ));
        }
    }
    Ok(call)
}
