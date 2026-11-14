//! Per-encounter changes to a creature's copied stat block (plan D41, option A; companion feature C23).
//!
//! An encounter keeps a copy of each creature's block as it was when the DM added it (see [`crate::creature`]), so a
//! later change to the DM's content never changes a planned encounter by itself. The DM can change a few things for
//! this encounter only: hit points, armor class, saving throws, extra traits, and a private note. A wholly new
//! creature is out of scope (D41 B and C are "not now").
//!
//! Every changed number keeps the source value it replaced (`base`). "Refresh stat block" ([`refresh`]) compares that
//! with the source as it is now: when both the DM and the source changed a field, the DM chooses which one wins.
//! The same changes apply to a typed monster block and to an Aurora companion's printed block; [`Printed`] is what
//! either prints.

use std::collections::BTreeMap;

use borealis_content::monster::{Ability, ArmorClass, Feature, HitPoints, MonsterData};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

/// Longest added trait name, in characters.
pub const MAX_TRAIT_NAME: usize = 80;
/// Longest trait text and note.
pub const MAX_TEXT: usize = 4_000;
/// Most added traits on one creature.
pub const MAX_TRAITS: usize = 12;
/// Highest hit points an override may set.
pub const MAX_HP: u32 = 9_999;
/// Highest armor class.
pub const MAX_AC: u32 = 40;
/// Saving throw bonuses an override may set, inclusive.
pub const SAVE_RANGE: std::ops::RangeInclusive<i32> = -10..=30;

/// One overridden value and the source value it replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Changed<T> {
    /// What this encounter uses.
    pub value: T,
    /// The source's value when the override was made (`None`: the source printed none).
    pub base: Option<T>,
}

/// Hit points as an override sets them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HpValue {
    /// The average (the number the encounter uses unless it rolls).
    pub average: u32,
    /// A hit dice formula (`4d10+8`) for "roll from formula" (D22); none keeps the average.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
}

/// Armor class as an override sets it: one value, with what gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcValue {
    /// The armor class.
    pub value: u32,
    /// "natural armor", "shield"...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// A trait the DM adds to the block for this encounter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddedTrait {
    /// Its name.
    pub name: String,
    /// Its text.
    pub text: String,
}

/// The numbers a block prints that overrides can change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Printed {
    /// Hit points (with hit dice when printed).
    pub hp: Option<HpValue>,
    /// The first (main) armor class.
    pub ac: Option<AcValue>,
    /// Each ability's saving throw bonus: the proficient one, else the ability modifier; `None` when the block
    /// prints neither (a companion without that score).
    pub saves: BTreeMap<Ability, Option<i32>>,
}

/// The ability keys a companion block uses.
const COMPANION_ABILITIES: [(Ability, &str); 6] = [
    (Ability::Str, "str"),
    (Ability::Dex, "dex"),
    (Ability::Con, "con"),
    (Ability::Int, "int"),
    (Ability::Wis, "wis"),
    (Ability::Cha, "cha"),
];

fn modifier(score: i64) -> i32 {
    i32::try_from((score - 10).div_euclid(2)).unwrap_or(0)
}

impl Printed {
    /// What a typed monster block prints.
    #[must_use]
    pub fn of_monster(data: &MonsterData) -> Self {
        Self {
            hp: data.hp.as_ref().map(|h| HpValue {
                average: h.average,
                formula: h.formula.clone(),
            }),
            ac: data.ac.first().map(|a| AcValue {
                value: a.value,
                source: a.source.clone(),
            }),
            saves: Ability::ALL
                .iter()
                .map(|a| (*a, Some(data.save_bonus(*a))))
                .collect(),
        }
    }

    /// What an Aurora companion's block prints (the bestiary's `CompanionDetail` shape: `hp`, `hpFormula`, `ac`,
    /// `abilities`, `saves`). Missing numbers stay `None`; nothing is filled in.
    #[must_use]
    pub fn of_companion(block: &Value) -> Self {
        let num = |v: &Value| v.as_u64().and_then(|n| u32::try_from(n).ok());
        Self {
            hp: num(&block["hp"]).map(|average| HpValue {
                average,
                formula: block["hpFormula"].as_str().map(str::to_string),
            }),
            ac: num(&block["ac"]).map(|value| AcValue {
                value,
                source: None,
            }),
            saves: COMPANION_ABILITIES
                .iter()
                .map(|(a, key)| {
                    let printed = block["saves"][key]
                        .as_i64()
                        .and_then(|n| i32::try_from(n).ok());
                    let from_score = block["abilities"][key].as_i64().map(modifier);
                    (*a, printed.or(from_score))
                })
                .collect(),
        }
    }

    fn save(&self, ability: Ability) -> Option<i32> {
        self.saves.get(&ability).copied().flatten()
    }
}

/// Everything an encounter changes on one creature's block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Overrides {
    /// Hit points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hp: Option<Changed<HpValue>>,
    /// Armor class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ac: Option<Changed<AcValue>>,
    /// Saving throw bonuses, by ability (`str`, `dex`, ...).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub saves: BTreeMap<Ability, Changed<i32>>,
    /// Traits added after the block's own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<AddedTrait>,
    /// The DM's note on this creature (where the change came from, a correction...). Never shown to players.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Spells the DM adds for this encounter, picked from installed spell elements, with slots and casting numbers
    /// (0.3.2, plan 4.5b item 3). A 0.3.1 server refuses overrides that have them (`deny_unknown_fields`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spells: Option<crate::spellbook::SpellsOverride>,
}

/// What the DM asks to change, as the builder sends it. Each field: absent = leave as is, `null` = drop the override,
/// a value = override with it. The source values (`base`) always come from the copy, never from the client.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OverridesForm {
    /// Hit points.
    #[serde(default, deserialize_with = "some")]
    pub hp: Option<Option<HpValue>>,
    /// Armor class.
    #[serde(default, deserialize_with = "some")]
    pub ac: Option<Option<AcValue>>,
    /// Saving throws: each ability present is set (`null` drops it); abilities not named stay as they are.
    #[serde(default)]
    pub saves: Option<BTreeMap<Ability, Option<i32>>>,
    /// All added traits (replaces the list).
    #[serde(default)]
    pub traits: Option<Vec<AddedTrait>>,
    /// The note (`null` or "" drops it).
    #[serde(default, deserialize_with = "some")]
    pub notes: Option<Option<String>>,
    /// The added spells (replaces them; `null` drops them).
    #[serde(default, deserialize_with = "some")]
    pub spells: Option<Option<crate::spellbook::SpellsOverride>>,
}

/// `null` as `Some(None)`, so a field can be dropped as well as left out.
fn some<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// Why an override was refused. The texts are for the DM.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OverrideError {
    /// A number outside what a stat block can say.
    #[error("{0}")]
    OutOfRange(&'static str),
    /// Added spells that don't check out.
    #[error("{0}")]
    BadSpells(&'static str),
    /// Text too long, empty or with control characters.
    #[error("{0}")]
    BadText(&'static str),
    /// A hit dice formula that isn't `NdM`, `NdM+K` or `NdM-K`.
    #[error("Hit dice look like 4d10+8.")]
    BadFormula,
}

fn clean_text(text: &str, max: usize, what: &'static str) -> Result<String, OverrideError> {
    let t = text.trim();
    let ok =
        t.chars().count() <= max && t.chars().all(|c| !c.is_control() || c == '\n' || c == '\t');
    if ok {
        Ok(t.to_string())
    } else {
        Err(OverrideError::BadText(what))
    }
}

/// True for `NdM`, `NdM+K` and `NdM-K` (no spaces), with small numbers.
#[must_use]
pub fn valid_formula(formula: &str) -> bool {
    let Some((count, rest)) = formula.split_once('d') else {
        return false;
    };
    let (die, bonus) = match rest.find(['+', '-']) {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    let small = |s: &str, max: u32| {
        !s.is_empty()
            && s.bytes().all(|b| b.is_ascii_digit())
            && s.parse::<u32>().is_ok_and(|n| n <= max)
    };
    small(count, 99)
        && count != "0"
        && small(die, 100)
        && die != "0"
        && bonus.is_none_or(|b| small(b, 9_999))
}

fn hp_value(v: HpValue) -> Result<HpValue, OverrideError> {
    if !(1..=MAX_HP).contains(&v.average) {
        return Err(OverrideError::OutOfRange("Hit points are 1 to 9999."));
    }
    let formula = match v.formula.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(f) => {
            let f: String = f.chars().filter(|c| !c.is_whitespace()).collect();
            if !valid_formula(&f) {
                return Err(OverrideError::BadFormula);
            }
            Some(f)
        }
    };
    Ok(HpValue {
        average: v.average,
        formula,
    })
}

fn ac_value(v: AcValue) -> Result<AcValue, OverrideError> {
    if !(1..=MAX_AC).contains(&v.value) {
        return Err(OverrideError::OutOfRange("Armor class is 1 to 40."));
    }
    let source = match v.source {
        Some(s) => Some(clean_text(
            &s,
            MAX_TRAIT_NAME,
            "What gives the armor class is up to 80 characters.",
        )?)
        .filter(|s| !s.is_empty()),
        None => None,
    };
    Ok(AcValue {
        value: v.value,
        source,
    })
}

impl Overrides {
    /// True when nothing is changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// True when the block the encounter uses differs from the copy (a note alone doesn't change it).
    #[must_use]
    pub fn changes_block(&self) -> bool {
        self.hp.is_some()
            || self.ac.is_some()
            || !self.saves.is_empty()
            || !self.traits.is_empty()
            || self.spells.is_some()
    }

    /// Apply what the DM asked to `self`, taking the source values from `printed` (the copy's).
    ///
    /// # Errors
    /// [`OverrideError`] for a value out of range or text that is too long; nothing changes then.
    pub fn edit(&self, form: OverridesForm, printed: &Printed) -> Result<Self, OverrideError> {
        let mut out = self.clone();
        if let Some(hp) = form.hp {
            out.hp = match hp {
                None => None,
                Some(v) => Some(Changed {
                    value: hp_value(v)?,
                    base: printed.hp.clone(),
                }),
            };
        }
        if let Some(ac) = form.ac {
            out.ac = match ac {
                None => None,
                Some(v) => Some(Changed {
                    value: ac_value(v)?,
                    base: printed.ac.clone(),
                }),
            };
        }
        if let Some(saves) = form.saves {
            for (ability, value) in saves {
                match value {
                    None => {
                        out.saves.remove(&ability);
                    }
                    Some(v) if SAVE_RANGE.contains(&v) => {
                        out.saves.insert(
                            ability,
                            Changed {
                                value: v,
                                base: printed.save(ability),
                            },
                        );
                    }
                    Some(_) => {
                        return Err(OverrideError::OutOfRange(
                            "Saving throw bonuses are -10 to +30.",
                        ))
                    }
                }
            }
        }
        if let Some(traits) = form.traits {
            if traits.len() > MAX_TRAITS {
                return Err(OverrideError::OutOfRange(
                    "A creature can have 12 added traits.",
                ));
            }
            out.traits = traits
                .into_iter()
                .map(|t| {
                    let name = clean_text(
                        &t.name,
                        MAX_TRAIT_NAME,
                        "Trait names are 1 to 80 characters.",
                    )?;
                    let text =
                        clean_text(&t.text, MAX_TEXT, "Trait text is up to 4000 characters.")?;
                    if name.is_empty() {
                        return Err(OverrideError::BadText(
                            "Trait names are 1 to 80 characters.",
                        ));
                    }
                    Ok(AddedTrait { name, text })
                })
                .collect::<Result<_, _>>()?;
        }
        if let Some(notes) = form.notes {
            out.notes = match notes {
                None => None,
                Some(n) => Some(clean_text(
                    &n,
                    MAX_TEXT,
                    "Notes are up to 4000 characters.",
                )?)
                .filter(|n| !n.is_empty()),
            };
        }
        if let Some(spells) = form.spells {
            out.spells = match spells {
                None => None,
                Some(s) => {
                    s.check().map_err(OverrideError::BadSpells)?;
                    (!s.is_empty()).then_some(s)
                }
            };
        }
        Ok(out)
    }

    /// The monster block this encounter uses: the copy with the overrides applied. The note is not part of it.
    #[must_use]
    pub fn apply(&self, source: &MonsterData) -> MonsterData {
        let mut out = source.clone();
        if let Some(hp) = &self.hp {
            out.hp = Some(HitPoints {
                average: hp.value.average,
                formula: hp.value.formula.clone(),
            });
            out.hp_note = None;
        }
        if let Some(ac) = &self.ac {
            out.ac = vec![ArmorClass {
                value: ac.value.value,
                source: ac.value.source.clone(),
                condition: None,
            }];
            out.ac_note = None;
        }
        for (ability, save) in &self.saves {
            out.saves.insert(*ability, save.value);
        }
        out.traits.extend(self.traits.iter().map(|t| Feature {
            name: t.name.clone(),
            text: t.text.clone(),
            usage: None,
            cost: None,
            condition: None,
            attack: None,
            save: None,
            spellcasting: None,
        }));
        out
    }

    /// The companion block this encounter uses (the `CompanionDetail` shape), with the overrides applied.
    #[must_use]
    pub fn apply_companion(&self, source: &Value) -> Value {
        let mut out = source.clone();
        let Some(map) = out.as_object_mut() else {
            return out;
        };
        if let Some(hp) = &self.hp {
            map.insert("hp".into(), json!(hp.value.average));
            map.insert("hpFormula".into(), json!(hp.value.formula));
            map.insert(
                "hpText".into(),
                json!(match &hp.value.formula {
                    Some(f) => format!("{} ({f})", hp.value.average),
                    None => hp.value.average.to_string(),
                }),
            );
        }
        if let Some(ac) = &self.ac {
            map.insert("ac".into(), json!(ac.value.value));
            map.insert(
                "acText".into(),
                json!(match &ac.value.source {
                    Some(s) => format!("{} ({s})", ac.value.value),
                    None => ac.value.value.to_string(),
                }),
            );
        }
        if !self.saves.is_empty() {
            let saves = map.entry("saves").or_insert_with(|| json!({}));
            if let Some(saves) = saves.as_object_mut() {
                for (ability, save) in &self.saves {
                    saves.insert(ability.as_str().to_string(), json!(save.value));
                }
            }
        }
        if !self.traits.is_empty() {
            let traits = map.entry("traits").or_insert_with(|| json!([]));
            if let Some(list) = traits.as_array_mut() {
                list.extend(
                    self.traits
                        .iter()
                        .map(|t| json!({ "name": t.name, "text": t.text })),
                );
            }
        }
        out
    }
}

/// One field both the DM and the source changed since the override was made.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    /// `hp`, `ac` or `saves.<ability>`.
    pub field: String,
    /// The source's value when the DM overrode it.
    pub was: Value,
    /// The source's value now.
    pub source: Value,
    /// The DM's value.
    pub value: Value,
}

/// How the DM settles a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Choice {
    /// Keep the DM's value (its `base` becomes the new source value).
    Keep,
    /// Take the source's new value (the override goes).
    Source,
}

/// What a refresh would do, or did.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Refresh {
    /// Fields both sides changed.
    pub conflicts: Vec<Conflict>,
    /// The overrides after the refresh (only when every conflict had a choice).
    #[serde(skip)]
    pub overrides: Option<Overrides>,
}

fn to_json<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// Compare an encounter's overrides with what the source prints now (`fresh`). Fields the DM overrode and the source
/// changed since are conflicts: each needs a `choices` entry (`hp`, `ac`, `saves.dex`...), or nothing is decided and
/// `overrides` is `None`. Overrides of fields the source didn't change stay as they are.
#[must_use]
pub fn refresh(
    current: &Overrides,
    fresh: &Printed,
    choices: &BTreeMap<String, Choice>,
) -> Refresh {
    let mut conflicts = Vec::new();
    let mut out = current.clone();
    let mut undecided = false;
    let mut settle = |field: &str, conflict: Conflict| -> Option<Choice> {
        let choice = choices.get(field).copied();
        if choice.is_none() {
            undecided = true;
        }
        conflicts.push(conflict);
        choice
    };
    if let Some(hp) = &current.hp {
        if fresh.hp != hp.base {
            let c = Conflict {
                field: "hp".into(),
                was: to_json(&hp.base),
                source: to_json(&fresh.hp),
                value: to_json(&hp.value),
            };
            match settle("hp", c) {
                Some(Choice::Source) => out.hp = None,
                Some(Choice::Keep) | None => {
                    out.hp = Some(Changed {
                        value: hp.value.clone(),
                        base: fresh.hp.clone(),
                    });
                }
            }
        }
    }
    if let Some(ac) = &current.ac {
        if fresh.ac != ac.base {
            let c = Conflict {
                field: "ac".into(),
                was: to_json(&ac.base),
                source: to_json(&fresh.ac),
                value: to_json(&ac.value),
            };
            match settle("ac", c) {
                Some(Choice::Source) => out.ac = None,
                Some(Choice::Keep) | None => {
                    out.ac = Some(Changed {
                        value: ac.value.clone(),
                        base: fresh.ac.clone(),
                    });
                }
            }
        }
    }
    for (ability, save) in &current.saves {
        let now = fresh.save(*ability);
        if now != save.base {
            let field = format!("saves.{}", ability.as_str());
            let c = Conflict {
                field: field.clone(),
                was: to_json(&save.base),
                source: to_json(&now),
                value: to_json(&save.value),
            };
            match settle(&field, c) {
                Some(Choice::Source) => {
                    out.saves.remove(ability);
                }
                Some(Choice::Keep) | None => {
                    out.saves.insert(
                        *ability,
                        Changed {
                            value: save.value,
                            base: now,
                        },
                    );
                }
            }
        }
    }
    Refresh {
        overrides: (!undecided).then_some(out),
        conflicts,
    }
}
