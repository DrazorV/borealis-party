//! Per-encounter changes to a snapshotted stat block (plan D41, option A; companion feature C23).
//!
//! An encounter keeps a copy of each creature's `MonsterData` as it was when the DM added it (the *snapshot*), so a
//! later change to the DM's content never changes a planned encounter by itself. The DM can then change a few things
//! for this encounter only: hit points, armor class, saving throws, extra traits, and a private note. Nothing else of
//! the block is edited here; a wholly new creature is out of scope (D41 B and C are "not now").
//!
//! Every changed number keeps the source value it replaced (`base`). "Refresh stat block" ([`refresh`]) compares that
//! with the source as it is now: when both the DM and the source changed a field, the DM chooses which one wins.

use std::collections::BTreeMap;

use borealis_content::monster::{Ability, ArmorClass, Feature, HitPoints, MonsterData};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Longest added trait name, trait text and note, in characters.
pub const MAX_TRAIT_NAME: usize = 80;
/// Longest trait text and note.
pub const MAX_TEXT: usize = 4_000;
/// Most added traits on one creature.
pub const MAX_TRAITS: usize = 12;
/// Highest hit points and armor class an override may set.
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
    /// The source's value when the override was made (`None`: the source had none).
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

/// Everything an encounter changes on one creature's stat block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Overrides {
    /// Hit points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hp: Option<Changed<HpValue>>,
    /// Armor class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ac: Option<Changed<AcValue>>,
    /// Saving throw bonuses, by ability (`str`, `dex`, ...). `base` is the printed bonus (the proficient one, else the
    /// ability modifier).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub saves: BTreeMap<Ability, Changed<i32>>,
    /// Traits added after the block's own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<AddedTrait>,
    /// The DM's note on this creature (where the change came from, a correction...). Never shown to players.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// What the DM asks to change, as the builder sends it. Each field: absent = leave as is, `null` = drop the override,
/// a value = override with it. The source values (`base`) are always taken from the snapshot, never from the client.
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

/// The source's hit points as an override records them.
fn source_hp(data: &MonsterData) -> Option<HpValue> {
    data.hp.as_ref().map(|h| HpValue {
        average: h.average,
        formula: h.formula.clone(),
    })
}

/// The source's armor class as an override records it: the first (main) one.
fn source_ac(data: &MonsterData) -> Option<AcValue> {
    data.ac.first().map(|a| AcValue {
        value: a.value,
        source: a.source.clone(),
    })
}

impl Overrides {
    /// True when nothing is changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// True when a number that rates the creature changed (HP, AC or a save): its difficulty still uses its CR, with a
    /// "modified" warning (D41).
    #[must_use]
    pub fn changes_numbers(&self) -> bool {
        self.hp.is_some() || self.ac.is_some() || !self.saves.is_empty() || !self.traits.is_empty()
    }

    /// Apply what the DM asked to `self`, taking the source values from `source` (the snapshot).
    ///
    /// # Errors
    /// [`OverrideError`] for a value out of range or text that is too long; nothing changes then.
    pub fn edit(&self, form: OverridesForm, source: &MonsterData) -> Result<Self, OverrideError> {
        let mut out = self.clone();
        if let Some(hp) = form.hp {
            out.hp = match hp {
                None => None,
                Some(v) => Some(Changed {
                    value: hp_value(v)?,
                    base: source_hp(source),
                }),
            };
        }
        if let Some(ac) = form.ac {
            out.ac = match ac {
                None => None,
                Some(v) => Some(Changed {
                    value: ac_value(v)?,
                    base: source_ac(source),
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
                                base: Some(source.save_bonus(ability)),
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
        Ok(out)
    }

    /// The stat block this encounter uses: the snapshot with the overrides applied. The note is not part of it.
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
}

/// One field both the DM and the source changed since the override was made.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    /// `hp`, `ac` or `saves.<ability>`.
    pub field: String,
    /// The source's value when the DM overrode it.
    pub was: serde_json::Value,
    /// The source's value now.
    pub source: serde_json::Value,
    /// The DM's value.
    pub value: serde_json::Value,
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
    /// True when the source's block differs from the snapshot.
    pub changed: bool,
    /// Fields both sides changed.
    pub conflicts: Vec<Conflict>,
    /// The overrides after the refresh (only when every conflict had a choice).
    #[serde(skip)]
    pub overrides: Option<Overrides>,
}

fn json<T: Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

/// Compare an encounter's overrides with the source's block as it is now (`fresh`; `snapshot` is the encounter's copy). Fields the source changed and the
/// DM overrode are conflicts: each needs a `choices` entry (`hp`, `ac`, `saves.dex`...), or nothing is decided and
/// `overrides` is `None`. Overrides of fields the source didn't change stay as they are, with `base` updated.
#[must_use]
pub fn refresh(
    current: &Overrides,
    snapshot: &MonsterData,
    fresh: &MonsterData,
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
        let now = source_hp(fresh);
        if now != hp.base {
            let c = Conflict {
                field: "hp".into(),
                was: json(&hp.base),
                source: json(&now),
                value: json(&hp.value),
            };
            match settle("hp", c) {
                Some(Choice::Source) => out.hp = None,
                Some(Choice::Keep) | None => {
                    out.hp = Some(Changed {
                        value: hp.value.clone(),
                        base: now,
                    });
                }
            }
        }
    }
    if let Some(ac) = &current.ac {
        let now = source_ac(fresh);
        if now != ac.base {
            let c = Conflict {
                field: "ac".into(),
                was: json(&ac.base),
                source: json(&now),
                value: json(&ac.value),
            };
            match settle("ac", c) {
                Some(Choice::Source) => out.ac = None,
                Some(Choice::Keep) | None => {
                    out.ac = Some(Changed {
                        value: ac.value.clone(),
                        base: now,
                    });
                }
            }
        }
    }
    for (ability, save) in &current.saves {
        let now = Some(fresh.save_bonus(*ability));
        if now != save.base {
            let field = format!("saves.{}", ability.as_str());
            let c = Conflict {
                field: field.clone(),
                was: json(&save.base),
                source: json(&now),
                value: json(&save.value),
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
        changed: snapshot != fresh,
        overrides: (!undecided).then_some(out),
        conflicts,
    }
}
