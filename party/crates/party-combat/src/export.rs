//! Versioned JSON export and import of a combat (D45). Import always makes a new combat.

use crate::combat::{Combat, LogEntry};
use crate::error::CombatError;
use crate::model::CombatState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// The `format` field of an export.
pub const EXPORT_FORMAT: &str = "borealis-party-combat";

/// The current export schema version. Bump it with every change to the exported shape, and add a step to
/// `migrate`.
pub const SCHEMA_VERSION: u32 = 1;

/// The largest export accepted.
pub const MAX_IMPORT_BYTES: usize = 8 * 1024 * 1024;

/// The exported file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportFile {
    /// Always [`EXPORT_FORMAT`].
    pub format: String,
    /// [`SCHEMA_VERSION`] when written.
    pub schema_version: u32,
    /// The state.
    pub state: CombatState,
    /// The DM-only log.
    #[serde(default)]
    pub log: Vec<LogEntry>,
}

/// The DM's export: the state and the log, not the undo journal or the die. DM only: it contains hidden creatures.
#[must_use]
pub fn export_json(combat: &Combat) -> String {
    let file = ExportFile {
        format: EXPORT_FORMAT.into(),
        schema_version: SCHEMA_VERSION,
        state: combat.state.clone(),
        log: combat.log.clone(),
    };
    serde_json::to_string_pretty(&file).expect("combat state always serializes")
}

/// Brings an older export up to [`SCHEMA_VERSION`]. Version 1 is the first, so there is nothing to do yet.
fn migrate(value: Value, from: u32) -> Result<Value, CombatError> {
    match from {
        SCHEMA_VERSION => Ok(value),
        v => Err(CombatError::Import(format!("unknown schema version {v}"))),
    }
}

/// Reads an export into a NEW combat (D45): a fresh die from `seed`, an empty undo journal, `seq` 0. The state is
/// checked for consistency before it is accepted.
pub fn import_json(text: &str, seed: u32) -> Result<Combat, CombatError> {
    if text.len() > MAX_IMPORT_BYTES {
        return Err(CombatError::Import("the file is too large".into()));
    }
    let value: Value =
        serde_json::from_str(text).map_err(|e| CombatError::Import(format!("not JSON ({e})")))?;
    if value.get("format").and_then(Value::as_str) != Some(EXPORT_FORMAT) {
        return Err(CombatError::Import(
            "not a Borealis Connect combat export".into(),
        ));
    }
    let version = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| CombatError::Import("no schema version".into()))?;
    if version > SCHEMA_VERSION {
        return Err(CombatError::Import(format!(
            "made by a newer version (schema {version}; this one reads up to {SCHEMA_VERSION})"
        )));
    }
    if version == 0 {
        return Err(CombatError::Import("unknown schema version 0".into()));
    }
    let value = migrate(value, version)?;
    let file: ExportFile =
        serde_json::from_value(value).map_err(|e| CombatError::Import(e.to_string()))?;
    validate(&file.state)?;
    let mut combat = Combat::from_state(file.state, seed);
    combat.log = file.log;
    Ok(combat)
}

/// Every reference in the state points at something that exists.
fn validate(s: &CombatState) -> Result<(), CombatError> {
    let bad = |m: String| Err(CombatError::Import(m));
    let mut ids = BTreeSet::new();
    for c in &s.combatants {
        if !ids.insert(&c.id) {
            return bad(format!("combatant {} appears twice", c.id));
        }
        if c.hp.max < 0
            || c.hp.current < 0
            || c.hp.current > c.hp.max
            || c.hp.temp < 0
            || c.exhaustion > 6
        {
            return bad(format!(
                "combatant {} has impossible HP or exhaustion",
                c.id
            ));
        }
    }
    let known = |id: &crate::model::CombatantId| ids.contains(id);
    for c in &s.combatants {
        if c.owner_id.as_ref().is_some_and(|o| !known(o)) {
            return bad(format!("combatant {} has an unknown owner", c.id));
        }
        if let crate::model::Controller::Player { pc } = &c.controller {
            if !s
                .combatant(pc)
                .is_some_and(crate::model::Combatant::is_character)
            {
                return bad(format!(
                    "combatant {} is controlled by an unknown player character",
                    c.id
                ));
            }
        }
        if c.linked_to.as_ref().is_some_and(|l| !known(l)) {
            return bad(format!(
                "combatant {} is linked to an unknown combatant",
                c.id
            ));
        }
    }
    if s.current.as_ref().is_some_and(|c| !known(c))
        || s.last_visible.as_ref().is_some_and(|m| !known(&m.id))
    {
        return bad("the turn points at an unknown combatant".into());
    }
    let mut effect_ids = BTreeSet::new();
    for e in &s.effects {
        if !effect_ids.insert(e.id) || e.id.0 >= s.next_effect {
            return bad(format!("effect {} has a bad id", e.id.0));
        }
        let refs = std::iter::once(&e.spec.target)
            .chain(e.spec.source.iter())
            .chain(e.spec.ends_on_defeat_of.iter());
        if refs.into_iter().any(|r| !known(r)) {
            return bad(format!("effect {} refers to an unknown combatant", e.id.0));
        }
    }
    for (k, p) in &s.legendary_pools {
        if p.remaining > p.max || p.members.iter().any(|m| !known(m)) {
            return bad(format!("legendary pool {k} is inconsistent"));
        }
    }
    for r in &s.rules {
        if r.linked.as_ref().is_some_and(|l| !known(l)) {
            return bad(format!("rule {} is linked to an unknown combatant", r.id));
        }
    }
    Ok(())
}
