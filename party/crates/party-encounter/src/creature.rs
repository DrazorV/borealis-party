//! A creature in an encounter: the snapshot of its stat block, what it is worth, and what players may see of it.

use borealis_content::{Element, MapKind, MonsterData, Ruleset as ContentRuleset};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{Cr, MonsterGroup};

/// The copy of a Monster element an encounter keeps (plan 4.3): players' views never depend on anyone's installed
/// content, and a later change to the DM's content changes nothing until the DM refreshes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// The element's name ("Goblin Warrior").
    pub name: String,
    /// Its source book.
    pub source: String,
    /// The counterpart in the other edition (`maps_to`), offered when editions don't match (D23).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maps_to: Option<String>,
    /// The stat block.
    pub data: MonsterData,
}

/// Why an element can't go into an encounter.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    /// Only typed Monster elements can (D17): Aurora companions and other types can't.
    #[error("Only monsters can be added to an encounter.")]
    NotAMonster,
    /// Its data doesn't fit the stat block model.
    #[error("This monster's stat block can't be read.")]
    Unreadable,
}

impl Snapshot {
    /// Copy a Monster element.
    ///
    /// # Errors
    /// [`SnapshotError`] for any other element type, or a stat block that doesn't fit the model.
    pub fn from_element(element: &Element) -> Result<Self, SnapshotError> {
        if element.ty != "Monster" {
            return Err(SnapshotError::NotAMonster);
        }
        let data = MonsterData::from_element(element).map_err(|_| SnapshotError::Unreadable)?;
        let ruleset = element.ruleset.unwrap_or(data.ruleset);
        let maps_to = element.maps_to.as_ref().and_then(|m| {
            if m.kind == Some(MapKind::Equivalent) {
                return None;
            }
            match ruleset {
                ContentRuleset::Dnd2014 => m.dnd2024.clone(),
                _ => m.dnd2014.clone(),
            }
            .filter(|id| *id != element.id)
        });
        Ok(Self {
            name: element.name.clone(),
            source: element.source.clone(),
            maps_to,
            data,
        })
    }

    /// The edition of the block (`dnd2014` or `dnd2024`; `any` for neither).
    #[must_use]
    pub fn ruleset(&self) -> &'static str {
        self.data.ruleset.as_str()
    }
}

/// A short, stable fingerprint of a stat block, to tell whether the source changed since the snapshot (the plan's
/// `pack_rev`). FNV-1a 64 over its JSON, as 16 hex digits.
#[must_use]
pub fn block_rev(data: &MonsterData) -> String {
    let text = serde_json::to_string(data).unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The creature's challenge rating, when it has one.
#[must_use]
pub fn challenge(data: &MonsterData) -> Option<Cr> {
    data.cr?.as_text().parse().ok()
}

/// One creature as an encounter group (the stored XP, HB5), or `None` for a creature with no challenge rating: it
/// adds nothing to the XP and is reported as unrated.
#[must_use]
pub fn group(data: &MonsterData) -> Option<MonsterGroup> {
    let cr = challenge(data)?;
    Some(MonsterGroup::with_stored_xp(cr, data.xp.map(u64::from), 1))
}

/// The word players see by default instead of the creature's name: its type ("Humanoid", "Beast").
#[must_use]
pub fn type_word(data: &MonsterData) -> String {
    let word = data.creature_type.as_str();
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// How much of a revealed creature players see (D20). Hidden creatures are not shown at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RevealLevel {
    /// The public name, size, type and conditions.
    #[default]
    Name,
    /// That, and how hurt it looks (Unhurt / Bloodied / Down).
    Band,
    /// That, and the stat block (no XP), exact hit points.
    Full,
}

impl RevealLevel {
    /// The stored word.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Band => "band",
            Self::Full => "full",
        }
    }

    /// From the stored word.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "name" => Some(Self::Name),
            "band" => Some(Self::Band),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

/// The health band players see at `band` and above: Down at 0, Bloodied at half or less, Unhurt otherwise.
#[must_use]
pub fn health_band(current: u32, max: u32) -> &'static str {
    if current == 0 {
        "down"
    } else if u64::from(current) * 2 <= u64::from(max) {
        "bloodied"
    } else {
        "unhurt"
    }
}

/// Stat block keys players never get, even at [`RevealLevel::Full`]: what a creature is worth.
pub const PLAYER_HIDDEN_KEYS: [&str; 2] = ["xp", "xp_lair"];

/// What a player sees of one revealed creature (the reveal field matrix, plan 4.3). `data` is the block the encounter
/// uses (overrides applied); `hp` is `(current, max)`. The creature's id is a random UUID; the DM's label, notes, the
/// element id, XP and difficulty are never part of this.
#[must_use]
pub fn player_view(
    id: &str,
    public_name: &str,
    level: RevealLevel,
    data: &MonsterData,
    hp: (u32, u32),
    conditions: &[String],
) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(id));
    out.insert("publicName".into(), json!(public_name));
    out.insert("revealLevel".into(), json!(level.as_str()));
    out.insert(
        "size".into(),
        json!(data.size.iter().map(|s| s.as_str()).collect::<Vec<_>>()),
    );
    out.insert("type".into(), json!(type_word(data)));
    out.insert("conditions".into(), json!(conditions));
    if level != RevealLevel::Name {
        out.insert("band".into(), json!(health_band(hp.0, hp.1)));
    }
    if level == RevealLevel::Full {
        let mut block = serde_json::to_value(data).unwrap_or(Value::Null);
        if let Some(map) = block.as_object_mut() {
            for key in PLAYER_HIDDEN_KEYS {
                map.remove(key);
            }
        }
        out.insert("statBlock".into(), block);
        out.insert("hp".into(), json!({ "current": hp.0, "max": hp.1 }));
    }
    Value::Object(out)
}
