//! "Suggested" mechanics drafted at runtime from the user's OWN installed 5etools spell data (nothing is shipped).
//!
//! The draft uses only the spell's structured tags: `savingThrow`, `damageInflict`, `conditionInflict`,
//! `affectsCreatureType`, `spellAttack`, `scalingLevelDice`, `miscTags` (MAC, SMN, HL, THP), `areaTags`, the casting
//! time, range and duration, and the inline `{@damage}` / `{@dice}` / `{@scaledamage}` / `{@scaledice}` dice. It never
//! reads numbers or rules from prose: buff numbers (Mage Armor's 13 + DEX), half damage on a save, target counts and
//! area sizes are left to the DM, listed as [`Check`]s that open the matching builder card (MAC opens the AC card).
//! The DM confirms the draft once; until then it is shown as "Suggested" and never cast silently.

use party_combat::spell::{
    AreaShape, CastTime, Delivery, Formula, Primitive, SaveSuccess, SpellDuration, SpellMech,
    SummonInfo, TargetFilter, Targeting, Tier,
};
use party_combat::{AbilityName, Condition, DamageType, Edition};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Something the DM must confirm or fill in before the draft is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// Whether a successful save halves the damage or negates it (the draft says it negates).
    HalfOnSave,
    /// How many creatures it targets, and how that grows with the slot (the draft says one).
    Targets,
    /// The area's size (the draft says 0 ft.).
    AreaSize,
    /// It changes AC (MAC): the AC card.
    Ac,
    /// It heals (HL): the dice and whether the modifier is added.
    Heal,
    /// It grants temporary hit points (THP).
    TempHp,
    /// It summons (SMN): the summon card (types, CR, counts).
    Summon,
    /// Damage of several types: which, or the caster's choice.
    DamageType,
    /// A condition it inflicts: when it ends, and any repeat save.
    Condition,
    /// The damage dice or their upcasting weren't tagged.
    Dice,
    /// It needs a definition the tags can't give (a buff, a utility effect).
    Effect,
}

/// A drafted definition and what the DM must check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    /// The suggestion (tier `partial` when it does something, else `ruling`).
    pub mech: SpellMech,
    /// What to confirm, sorted.
    pub checks: Vec<Check>,
}

/// Every `{@tag body}` of a 5etools string, as (tag, body).
fn tags(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("{@") {
        let after = &rest[i + 2..];
        let Some(end) = after.find('}') else { break };
        let inner = &after[..end];
        let (tag, body) = inner.split_once(' ').unwrap_or((inner, ""));
        out.push((tag.to_string(), body.to_string()));
        rest = &after[end + 1..];
    }
    out
}

/// All strings of a 5etools entries tree.
fn strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        Value::Object(m) => m.values().for_each(|x| strings(x, out)),
        _ => {}
    }
}

/// `NdM` with small numbers (no modifier): (count, sides).
fn plain_dice(text: &str) -> Option<(u32, u32)> {
    let (n, m) = text.trim().split_once('d')?;
    let n: u32 = n.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    ((1..=40).contains(&n) && [4, 6, 8, 10, 12, 20].contains(&m)).then_some((n, m))
}

/// `NdM` with an optional `+K`/`-K` checked by the formula parser.
fn dice_formula(text: &str) -> Option<String> {
    let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let head = t.split(['+', '-']).next()?;
    plain_dice(head)?;
    Formula::parse(&t).ok().map(|_| t)
}

fn ability(word: &str) -> Option<AbilityName> {
    Some(match word.to_lowercase().as_str() {
        "strength" | "str" => AbilityName::Str,
        "dexterity" | "dex" => AbilityName::Dex,
        "constitution" | "con" => AbilityName::Con,
        "intelligence" | "int" => AbilityName::Int,
        "wisdom" | "wis" => AbilityName::Wis,
        "charisma" | "cha" => AbilityName::Cha,
        _ => return None,
    })
}

fn from_word<T: for<'de> Deserialize<'de>>(word: &str) -> Option<T> {
    serde_json::from_value(Value::String(word.trim().to_lowercase())).ok()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The cast time of `time[0]`.
fn cast_time(v: &Value) -> CastTime {
    match v[0]["unit"].as_str().unwrap_or("action") {
        "bonus" => CastTime::Bonus,
        "reaction" => CastTime::Reaction,
        "minute" | "hour" => CastTime::Minute,
        _ => CastTime::Action,
    }
}

/// Duration and concentration of `duration[0]`.
fn duration(v: &Value) -> (SpellDuration, bool) {
    let d = &v[0];
    let conc = d["concentration"].as_bool().unwrap_or(false);
    let amount = d["duration"]["amount"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(1);
    let out = match d["type"].as_str().unwrap_or("instant") {
        "timed" => match d["duration"]["type"].as_str().unwrap_or("") {
            "round" => SpellDuration::SourceTurns {
                n: amount,
                at: party_combat::Boundary::Start,
            },
            "minute" => SpellDuration::Minutes { n: amount },
            _ => SpellDuration::Long,
        },
        "permanent" => SpellDuration::UntilDispelled,
        _ => SpellDuration::Instant,
    };
    (out, conc)
}

/// The area a spell's `range` and `areaTags` give.
fn area(spell: &Value) -> Option<AreaShape> {
    let range = spell["range"]["type"].as_str().unwrap_or("");
    let from_range = match range {
        "radius" | "sphere" => Some(AreaShape::Sphere),
        "cone" => Some(AreaShape::Cone),
        "line" => Some(AreaShape::Line),
        "cube" => Some(AreaShape::Cube),
        "cylinder" => Some(AreaShape::Cylinder),
        "emanation" => Some(AreaShape::Emanation),
        _ => None,
    };
    from_range.or_else(|| {
        strs(&spell["areaTags"])
            .iter()
            .find_map(|t| match t.as_str() {
                "S" => Some(AreaShape::Sphere),
                "N" => Some(AreaShape::Cone),
                "L" => Some(AreaShape::Line),
                "C" | "Q" => Some(AreaShape::Cube),
                "Y" => Some(AreaShape::Cylinder),
                _ => None,
            })
    })
}

/// The size in feet a self-centered `range` gives ("radius 10 feet"); areas at a point print theirs only in prose.
fn area_size(spell: &Value) -> Option<u32> {
    let r = &spell["range"];
    if r["type"].as_str() == Some("point") {
        return None;
    }
    (r["distance"]["type"].as_str() == Some("feet"))
        .then(|| r["distance"]["amount"].as_u64())
        .flatten()
        .and_then(|n| u32::try_from(n).ok())
}

/// The upcast formula of `base` from a `{@scaledamage base|levels|step}` body.
fn upcast(base: &str, scaled: Option<&str>) -> Option<String> {
    let parts: Vec<&str> = scaled?.split('|').collect();
    let step = plain_dice(parts.get(2)?)?;
    let (_, sides) = plain_dice(base.split(['+', '-']).next()?)?;
    if step.1 != sides && !base.contains(['+', '-']) {
        return Some(format!("{base}+({}*slot_above)d{}", step.0, step.1));
    }
    Some(if step.0 == 1 {
        format!("{base}+(slot_above)d{}", step.1)
    } else {
        format!("{base}+({}*slot_above)d{}", step.0, step.1)
    })
}

/// A cantrip's `scalingLevelDice` as a cantrip-tier formula (`(cantrip_tier)d10`), when it scales linearly.
fn cantrip_formula(v: &Value) -> Option<String> {
    let scaling = if v.is_array() {
        &v[0]["scaling"]
    } else {
        &v["scaling"]
    };
    let at = |lvl: &str| scaling[lvl].as_str().and_then(plain_dice);
    let (n1, s1) = at("1")?;
    let tiers = [at("5")?, at("11")?, at("17")?];
    let linear = tiers
        .iter()
        .enumerate()
        .all(|(i, (n, s))| *s == s1 && *n == n1 * (u32::try_from(i).unwrap_or(0) + 2));
    linear.then(|| {
        if n1 == 1 {
            format!("(cantrip_tier)d{s1}")
        } else {
            format!("({n1}*cantrip_tier)d{s1}")
        }
    })
}

/// Draft a definition from one 5etools spell object. `key` is the installed element's id.
#[must_use]
pub fn from_5etools(key: &str, edition: Edition, spell: &Value) -> Draft {
    let mut checks = Vec::new();
    let level = spell["level"]
        .as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .unwrap_or(0)
        .min(9);
    let (dur, concentration) = duration(&spell["duration"]);
    let misc = strs(&spell["miscTags"]);
    let has = |t: &str| misc.iter().any(|m| m == t);

    let mut text = Vec::new();
    strings(&spell["entries"], &mut text);
    let mut higher = Vec::new();
    strings(&spell["entriesHigherLevel"], &mut higher);
    let all_tags: Vec<(String, String)> = text.iter().flat_map(|t| tags(t)).collect();
    let higher_tags: Vec<(String, String)> = higher.iter().flat_map(|t| tags(t)).collect();
    let first = |names: &[&str]| {
        all_tags
            .iter()
            .find(|(t, _)| names.contains(&t.as_str()))
            .map(|(_, b)| b.split('|').next().unwrap_or("").to_string())
    };
    let scaled = |names: &[&str]| {
        higher_tags
            .iter()
            .chain(all_tags.iter())
            .find(|(t, _)| names.contains(&t.as_str()))
            .map(|(_, b)| b.clone())
    };

    // Targeting.
    let types = strs(&spell["affectsCreatureType"]);
    let filter = if types.is_empty() {
        TargetFilter::Any
    } else {
        TargetFilter::Type { types }
    };
    let targeting = if let Some(shape) = area(spell) {
        let size = area_size(spell);
        if size.is_none() {
            checks.push(Check::AreaSize);
        }
        Targeting::Area {
            shape,
            size_ft: size.unwrap_or(0),
        }
    } else if spell["range"]["distance"]["type"].as_str() == Some("self") {
        Targeting::SelfOnly
    } else {
        checks.push(Check::Targets);
        Targeting::Creatures {
            max: Formula::constant(1),
            filter,
            include_self: false,
        }
    };

    // Delivery.
    let saves = strs(&spell["savingThrow"]);
    let attacks = strs(&spell["spellAttack"]);
    let delivery = if let Some(a) = attacks.first() {
        Delivery::Attack {
            attack: if a == "M" {
                party_combat::spell::AttackKind::Melee
            } else {
                party_combat::spell::AttackKind::Ranged
            },
            rays: Formula::constant(1),
        }
    } else if let Some(ab) = saves.first().and_then(|s| ability(s)) {
        Delivery::Save {
            ability: ab,
            success: SaveSuccess::None,
        }
    } else {
        Delivery::Auto
    };

    let mut on_land = Vec::new();
    let mut always = Vec::new();

    // Damage.
    let damage_types: Vec<DamageType> = strs(&spell["damageInflict"])
        .iter()
        .filter_map(|t| from_word(t))
        .collect();
    if !damage_types.is_empty() {
        let dice = if level == 0 {
            spell
                .get("scalingLevelDice")
                .and_then(cantrip_formula)
                .or_else(|| first(&["damage"]).and_then(|d| dice_formula(&d)))
        } else {
            first(&["damage"])
                .and_then(|d| dice_formula(&d))
                .map(|base| upcast(&base, scaled(&["scaledamage"]).as_deref()).unwrap_or(base))
        };
        if level > 0 && scaled(&["scaledamage"]).is_none() {
            checks.push(Check::Dice);
        }
        match dice.and_then(|d| Formula::parse(&d).ok()) {
            Some(dice) => {
                let (damage_type, choice) = if damage_types.len() == 1 {
                    (Some(damage_types[0]), Vec::new())
                } else {
                    checks.push(Check::DamageType);
                    (None, damage_types.clone())
                };
                on_land.push(Primitive::Damage {
                    dice,
                    damage_type,
                    choice,
                });
                if matches!(delivery, Delivery::Save { .. }) {
                    checks.push(Check::HalfOnSave);
                }
            }
            None => checks.push(Check::Dice),
        }
    }

    // Conditions.
    for c in strs(&spell["conditionInflict"]) {
        if let Some(condition) = from_word::<Condition>(&c) {
            on_land.push(Primitive::Condition {
                condition,
                repeat_save: None,
                ends_on_damage: false,
                ends_on_attack: false,
            });
            checks.push(Check::Condition);
        }
    }

    // Healing and temporary hit points: the tagged dice only; whether the modifier adds is the DM's.
    let heal_dice = || {
        first(&["dice", "damage"])
            .and_then(|d| dice_formula(&d))
            .map(|base| {
                upcast(&base, scaled(&["scaledice", "scaledamage"]).as_deref()).unwrap_or(base)
            })
            .and_then(|d| Formula::parse(&d).ok())
    };
    if has("HL") {
        checks.push(Check::Heal);
        if let Some(dice) = heal_dice() {
            on_land.push(Primitive::Heal {
                dice,
                add_mod: false,
            });
        }
    }
    if has("THP") {
        checks.push(Check::TempHp);
        if let Some(dice) = heal_dice() {
            on_land.push(Primitive::TempHp {
                dice,
                add_mod: false,
            });
        }
    }
    if has("MAC") {
        checks.push(Check::Ac);
    }
    if has("SMN") {
        checks.push(Check::Summon);
        let creature_types: Vec<String> = Vec::new();
        always.push(Primitive::Summon {
            info: SummonInfo {
                creature_types,
                ..SummonInfo::default()
            },
        });
    }
    if on_land.is_empty() && always.is_empty() {
        checks.push(Check::Effect);
    }
    checks.sort();
    checks.dedup();

    let tier = if on_land.is_empty() && always.is_empty() {
        Tier::Ruling
    } else {
        Tier::Partial
    };
    let mech = SpellMech {
        v: party_combat::spell::MECH_VERSION,
        key: key.to_string(),
        edition,
        name: spell["name"].as_str().unwrap_or("").to_string(),
        rev: 0,
        level,
        concentration,
        cast: cast_time(&spell["time"]),
        targeting,
        delivery,
        on_land,
        on_resist: Vec::new(),
        always,
        duration: dur,
        tier,
        notes: Vec::new(),
    };
    Draft { mech, checks }
}
