//! A creature in an encounter: the copy of its stat block, and what players may see of it.

use borealis_content::{Element, MapKind, MonsterData, Ruleset as ContentRuleset};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::overrides::{Overrides, Printed};

/// What kind of element a creature was copied from (plan D17 b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A typed `Monster` element: its `MonsterData`.
    #[default]
    Monster,
    /// An Aurora `Companion` element (a familiar, beast companion, steed): only what it prints.
    Companion,
}

impl Kind {
    /// The stored word.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Monster => "monster",
            Self::Companion => "companion",
        }
    }
}

/// The copy of a creature an encounter keeps (plan 4.3): players' views never depend on anyone's installed content,
/// and a later change to the DM's content changes nothing until the DM refreshes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// Monster or companion.
    #[serde(default)]
    pub kind: Kind,
    /// The element's name ("Goblin Warrior").
    pub name: String,
    /// Its source book.
    pub source: String,
    /// `dnd2014`, `dnd2024` or `any`.
    pub ruleset: String,
    /// The counterpart in the other edition (`maps_to`), offered when editions don't match (D23).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maps_to: Option<String>,
    /// The stat block of a monster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<MonsterData>,
    /// The printed block of a companion (the bestiary's `CompanionDetail`, camelCase; missing fields are null).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub companion: Option<Value>,
}

/// Why an element can't go into an encounter.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    /// Only creatures can: typed Monster elements and Aurora Companion elements.
    #[error("Only monsters and companions can be added to an encounter.")]
    NotACreature,
    /// Its data doesn't fit the stat block model.
    #[error("This monster's stat block can't be read.")]
    Unreadable,
}

/// Stat block keys left out of every copy anyone is shown: what a creature is worth in XP (plan D16).
pub const HIDDEN_KEYS: [&str; 2] = ["xp", "xp_lair"];

fn counterpart(element: &Element, ruleset: ContentRuleset) -> Option<String> {
    let m = element.maps_to.as_ref()?;
    if m.kind == Some(MapKind::Equivalent) {
        return None;
    }
    match ruleset {
        ContentRuleset::Dnd2014 => m.dnd2024.clone(),
        _ => m.dnd2014.clone(),
    }
    .filter(|id| *id != element.id)
}

fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

impl Snapshot {
    /// Copy a typed Monster element.
    ///
    /// # Errors
    /// [`SnapshotError`] for any other element type, or a stat block that doesn't fit the model.
    pub fn of_monster(element: &Element) -> Result<Self, SnapshotError> {
        if element.ty != "Monster" {
            return Err(SnapshotError::NotACreature);
        }
        let data = MonsterData::from_element(element).map_err(|_| SnapshotError::Unreadable)?;
        let ruleset = element.ruleset.unwrap_or(data.ruleset);
        Ok(Self {
            kind: Kind::Monster,
            name: element.name.clone(),
            source: element.source.clone(),
            ruleset: ruleset.as_str().to_string(),
            maps_to: counterpart(element, ruleset),
            data: Some(data),
            companion: None,
        })
    }

    /// Copy an Aurora Companion element, given its printed block (the `CompanionDetail` shape).
    ///
    /// # Errors
    /// [`SnapshotError::NotACreature`] for any other element type.
    pub fn of_companion(element: &Element, block: Value) -> Result<Self, SnapshotError> {
        if element.ty != "Companion" {
            return Err(SnapshotError::NotACreature);
        }
        let ruleset = element.ruleset.unwrap_or(ContentRuleset::Any);
        Ok(Self {
            kind: Kind::Companion,
            name: element.name.clone(),
            source: element.source.clone(),
            ruleset: ruleset.as_str().to_string(),
            maps_to: counterpart(element, ruleset),
            data: None,
            companion: Some(block),
        })
    }

    /// The numbers its block prints.
    #[must_use]
    pub fn printed(&self) -> Printed {
        match (&self.data, &self.companion) {
            (Some(data), _) => Printed::of_monster(data),
            (None, Some(block)) => Printed::of_companion(block),
            (None, None) => Printed::default(),
        }
    }

    /// The block the encounter uses, overrides applied, without XP: a `MonsterData` (snake_case) for a monster, a
    /// `CompanionDetail` (camelCase) for a companion.
    #[must_use]
    pub fn block(&self, overrides: &Overrides) -> Value {
        let mut out = match (&self.data, &self.companion) {
            (Some(data), _) => serde_json::to_value(overrides.apply(data)).unwrap_or(Value::Null),
            (None, Some(block)) => overrides.apply_companion(block),
            (None, None) => Value::Null,
        };
        if let Some(map) = out.as_object_mut() {
            for key in HIDDEN_KEYS {
                map.remove(key);
            }
        }
        out
    }

    /// The hit points it starts with by default: the average it prints (overridden or not), at least 1.
    #[must_use]
    pub fn average_hp(&self, overrides: &Overrides) -> u32 {
        overrides
            .hp
            .as_ref()
            .map(|h| h.value.average)
            .or_else(|| self.printed().hp.map(|h| h.average))
            .unwrap_or(1)
            .max(1)
    }

    /// Its armor class, overridden or printed.
    #[must_use]
    pub fn ac(&self, overrides: &Overrides) -> Option<u32> {
        overrides
            .ac
            .as_ref()
            .map(|a| a.value.value)
            .or_else(|| self.printed().ac.map(|a| a.value))
    }

    /// Its challenge rating as printed (`1/4`), if any. Only a stat: nothing rates encounters with it (D16).
    #[must_use]
    pub fn challenge(&self) -> Option<String> {
        match (&self.data, &self.companion) {
            (Some(data), _) => data.cr.map(|c| c.as_text()),
            (None, Some(block)) => block["challenge"].as_str().map(str::to_string),
            (None, None) => None,
        }
    }

    /// The word players see by default instead of the creature's name: its type ("Beast", "Fey"), else "Creature".
    #[must_use]
    pub fn type_word(&self) -> String {
        let word = match (&self.data, &self.companion) {
            (Some(data), _) => data.creature_type.as_str().to_string(),
            (None, Some(block)) => block["type"]
                .as_str()
                .and_then(|t| t.split(['(', ',']).next())
                .map(|t| t.trim().to_lowercase())
                .filter(|t| !t.is_empty())
                .unwrap_or_default(),
            (None, None) => String::new(),
        };
        if word.is_empty() {
            "Creature".into()
        } else {
            capitalised(&word)
        }
    }

    /// Its size words (`small`, `medium`...).
    #[must_use]
    pub fn sizes(&self) -> Vec<String> {
        match (&self.data, &self.companion) {
            (Some(data), _) => data.size.iter().map(|s| s.as_str().to_string()).collect(),
            (None, Some(block)) => {
                let text = block["size"].as_str().unwrap_or("").to_lowercase();
                ["tiny", "small", "medium", "large", "huge", "gargantuan"]
                    .iter()
                    .filter(|s| {
                        text.split(|c: char| !c.is_ascii_alphabetic())
                            .any(|w| w == **s)
                    })
                    .map(|s| (*s).to_string())
                    .collect()
            }
            (None, None) => Vec::new(),
        }
    }

    /// A short, stable fingerprint of the copied block, to tell whether the source changed since. FNV-1a 64 over its
    /// JSON, as 16 hex digits.
    #[must_use]
    pub fn rev(&self) -> String {
        let text = match (&self.data, &self.companion) {
            (Some(data), _) => serde_json::to_string(data).unwrap_or_default(),
            (None, Some(block)) => block.to_string(),
            (None, None) => String::new(),
        };
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in text.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        format!("{hash:016x}")
    }
}

/// What a player sees of one revealed creature (plan D20 b: hidden creatures are not shown at all; revealed ones
/// with their whole stat block). `hp` is `(current, max)`; `owner` is the id of the creature it is controlled by,
/// given only when that one is revealed too. The DM's label, notes, the element id and XP are never part of this.
#[must_use]
pub fn player_view(
    id: &str,
    public_name: &str,
    snapshot: &Snapshot,
    overrides: &Overrides,
    hp: (u32, u32),
    conditions: &[String],
    owner: Option<&str>,
) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(id));
    out.insert("publicName".into(), json!(public_name));
    out.insert("kind".into(), json!(snapshot.kind.as_str()));
    out.insert("size".into(), json!(snapshot.sizes()));
    out.insert("type".into(), json!(snapshot.type_word()));
    out.insert("conditions".into(), json!(conditions));
    out.insert("hp".into(), json!({ "current": hp.0, "max": hp.1 }));
    out.insert("statBlock".into(), snapshot.block(overrides));
    if let Some(owner) = owner {
        out.insert("ownerId".into(), json!(owner));
    }
    Value::Object(out)
}
