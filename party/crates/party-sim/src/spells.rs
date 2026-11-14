//! Spells in the simulation (0.3.3): the same definitions and the same `CastSpell` command as the combat tracker.
//!
//! * The server hands in the account's mechanics for the encounter's edition ([`SpellDefs`]: the DM's own
//!   definitions and links, then the curated SRD rows, keyed by spell element id) and the DM's chosen copy of each
//!   printed spell name. Without them (tests, a server that has none) the curated SRD row of the spell id is used,
//!   then a ruling.
//! * [`compile`] turns a definition into the caster's [`CastSpec`] exactly as the tracker's server does: the
//!   caster's first spellcasting block gives the DC, the attack bonus, the modifier and the cantrip tier.
//! * Tier `auto` and `partial` count as automated; `ruling` and uncurated spells are adjudicated (the cast still
//!   spends its slot and starts concentration through the engine); `utility` spells (no combat mechanics) are not
//!   simulated. Summons give info only: nothing spawns.
//! * The revision of every definition used goes into the run's fingerprints.

use std::collections::BTreeMap;

use party_combat::party_dice::Term;
use party_combat::spell::{
    cantrip_tier, CastSpec, CastTime, Delivery, FormulaVars, MechOrigin, Primitive, SpellFrequency,
    SpellMech, Targeting, Tier,
};
use party_combat::{Ability, AbilityKind, Combatant, Condition, Edition};
use serde::{Deserialize, Serialize};

use crate::sheet::Coverage;

/// One definition the server resolved for a spell key: the account's own (`def`), another spell's through a link,
/// or the curated SRD row, with where it came from and its revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpellDefn {
    pub mech: SpellMech,
    pub origin: MechOrigin,
}

/// The mechanics a run casts with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpellDefs {
    /// By spell element id (any copy of the one spell).
    pub defs: BTreeMap<String, SpellDefn>,
    /// The DM's chosen copy of a printed spell name (normalized), for stat block spells with no link yet.
    pub names: BTreeMap<String, String>,
}

/// The curated SRD row of `key`, when it is for `edition`.
fn curated(key: &str, edition: Edition) -> Option<SpellDefn> {
    let m = party_spells::data::curated(key)?;
    (m.edition == edition).then(|| SpellDefn {
        mech: m.clone(),
        origin: MechOrigin::Srd {
            key: m.key.clone(),
            rev: m.rev,
        },
    })
}

/// The cast spec of a spell ability for its caster, or `None` when the ability is not a spell.
#[must_use]
pub fn compile(
    defs: &SpellDefs,
    edition: Edition,
    caster: &Combatant,
    ability: &Ability,
) -> Option<CastSpec> {
    let link = ability.spell.as_ref()?;
    let key = link.key.clone().or_else(|| {
        defs.names
            .get(&party_spells::canonical::normalize(&ability.name))
            .cloned()
    });
    let casting = caster.casting.first();
    let modifier = match (casting, caster.stats.as_ref()) {
        (Some(c), Some(stats)) => stats.modifier(c.ability),
        _ => 0,
    };
    let tier = casting
        .and_then(|c| c.cantrip_tier.or_else(|| c.caster_level.map(cantrip_tier)))
        .unwrap_or(1);
    let found = key
        .as_deref()
        .and_then(|k| defs.defs.get(k).cloned().or_else(|| curated(k, edition)));
    let mut spec = if let Some(d) = found {
        let mut mech = d.mech;
        if let Some(k) = &key {
            mech.key.clone_from(k);
        }
        mech.edition = edition;
        CastSpec {
            v: party_combat::spell::MECH_VERSION,
            origin: d.origin,
            mech,
            dc: None,
            attack: None,
            modifier: 0,
            cantrip_tier: 1,
            baked_in: false,
        }
    } else if let Some(m) = ability.mech.as_deref() {
        m.clone()
    } else {
        CastSpec::ruling(
            key.as_deref().unwrap_or(&ability.key),
            &ability.name,
            edition,
            link.level,
            false,
        )
    };
    if let Some(c) = casting {
        spec.dc = c.dc.or(spec.dc);
        spec.attack = c.attack.or(spec.attack);
    }
    spec.modifier = modifier;
    spec.cantrip_tier = tier;
    spec.baked_in = link.baked_in;
    Some(spec)
}

/// Give every spell ability of `c` its compiled spec (its `mech`) and its kind from the casting time, and return the
/// fingerprints of the definitions used (`spell:<key>` → `srd:<rev>` / `def:<id>:<rev>` / `ruling`).
pub fn equip(
    defs: &SpellDefs,
    edition: Edition,
    c: &mut Combatant,
    prints: &mut BTreeMap<String, String>,
) {
    let snapshot = c.clone();
    for a in &mut c.abilities {
        if a.spell.is_none() {
            continue;
        }
        let Some(spec) = compile(defs, edition, &snapshot, a) else {
            continue;
        };
        a.kind = match spec.mech.cast {
            CastTime::Bonus => AbilityKind::BonusAction,
            CastTime::Reaction => AbilityKind::Reaction,
            CastTime::Action | CastTime::Minute => AbilityKind::Action,
        };
        let print = match &spec.origin {
            MechOrigin::Srd { rev, .. } => format!("srd:{rev}"),
            MechOrigin::Def { id, rev } => format!("def:{id}:{rev}"),
            MechOrigin::Statblock => "statblock".into(),
            MechOrigin::Ruling => "ruling".into(),
        };
        prints.insert(format!("spell:{}", spec.mech.key), print);
        if let Some(link) = a.spell.as_mut() {
            if link.key.is_none() && spec.origin != MechOrigin::Ruling {
                link.key = Some(spec.mech.key.clone());
            }
        }
        a.mech = Some(Box::new(spec));
    }
}

/// Every primitive of a definition.
pub fn prims(m: &SpellMech) -> impl Iterator<Item = &Primitive> {
    m.on_land.iter().chain(&m.on_resist).chain(&m.always)
}

/// The primitives that take an amount roll, in the engine's order.
#[must_use]
pub fn amount_count(m: &SpellMech) -> usize {
    prims(m)
        .filter(|p| {
            matches!(
                p,
                Primitive::Damage { .. } | Primitive::Heal { .. } | Primitive::TempHp { .. }
            )
        })
        .count()
}

/// It only gives a summon's options (nothing spawns, as in the tracker).
#[must_use]
pub fn summon_only(m: &SpellMech) -> bool {
    prims(m).any(|p| matches!(p, Primitive::Summon { .. }))
        && !prims(m).any(|p| {
            matches!(
                p,
                Primitive::Damage { .. } | Primitive::Condition { .. } | Primitive::Zone { .. }
            )
        })
}

/// A reaction that raises the caster's own AC (Shield): the runner casts it when it turns a hit into a miss.
#[must_use]
pub fn ac_reaction(m: &SpellMech) -> Option<i32> {
    if m.cast != CastTime::Reaction || !matches!(m.targeting, Targeting::SelfOnly) {
        return None;
    }
    prims(m).find_map(|p| match p {
        Primitive::AcBonus { n } => Some(*n),
        _ => None,
    })
}

/// How far the simulation runs a spell.
#[must_use]
pub fn coverage(spec: &CastSpec) -> Coverage {
    let m = &spec.mech;
    if summon_only(m) {
        return Coverage::NotSimulated;
    }
    if m.cast == CastTime::Reaction {
        return if ac_reaction(m).is_some() {
            Coverage::Automated
        } else {
            Coverage::NotSimulated
        };
    }
    match (m.tier, &spec.origin) {
        (_, MechOrigin::Ruling) => Coverage::Adjudicable,
        (Tier::Auto | Tier::Partial, _) => Coverage::Automated,
        // No combat mechanics (Light, Mage Hand): left out of cards.
        (Tier::Utility, _) => Coverage::NotSimulated,
        _ => Coverage::Adjudicable,
    }
}

/// The slot levels a spell can be cast with now: `[None]` for a cantrip or a slotless use, else every level from
/// its own up to 9 with a slot left (only the fixed level, when there is one).
#[must_use]
pub fn slots(c: &Combatant, ability: &Ability, spec: &CastSpec) -> Vec<Option<u8>> {
    let level = spec.mech.level;
    let link = ability.spell.as_ref();
    let slotless =
        link.is_some_and(|l| l.frequency != SpellFrequency::Slot) || ability.uses.is_some();
    if level == 0 || slotless {
        return vec![None];
    }
    if let Some(f) = link.and_then(|l| l.fixed_slot) {
        return vec![Some(f)];
    }
    c.spell_slots
        .iter()
        .filter(|(l, p)| **l >= level && p.remaining > 0)
        .map(|(l, _)| Some(*l))
        .collect()
}

/// The formula variables of a cast at `slot`.
#[must_use]
pub fn vars(spec: &CastSpec, slot: Option<u8>) -> FormulaVars {
    spec.vars(slot)
}

/// Twice the average of a formula's dice at `vars` (0 when it doesn't evaluate).
#[must_use]
pub fn avg2(f: &party_combat::spell::Formula, vars: &FormulaVars) -> i32 {
    match f.dice(vars) {
        Ok(e) => e
            .terms
            .iter()
            .map(|t| match *t {
                Term::Dice { count, sides, sign } => {
                    let v = i32::try_from(count * (sides + 1)).unwrap_or(0);
                    if sign == party_combat::party_dice::Sign::Minus {
                        -v
                    } else {
                        v
                    }
                }
                Term::Flat { value } => value * 2,
            })
            .sum(),
        Err(_) => 0,
    }
}

/// What a condition is worth to the side imposing it, twice over in HP (integer maths).
#[must_use]
pub fn condition_value2(c: Condition, target_hp: i32) -> i32 {
    match c {
        Condition::Paralyzed
        | Condition::Stunned
        | Condition::Unconscious
        | Condition::Petrified
        | Condition::Incapacitated => 2 * target_hp.clamp(4, 24),
        Condition::Restrained | Condition::Blinded | Condition::Frightened | Condition::Charmed => {
            10
        }
        Condition::Poisoned | Condition::Prone | Condition::Grappled => 6,
        _ => 2,
    }
}

/// The most targets of a spell at `slot` (rays for an attack spell), and whether the targets may repeat.
#[must_use]
pub fn max_targets(spec: &CastSpec, slot: Option<u8>) -> (u32, bool) {
    let v = spec.vars(slot);
    if let Delivery::Attack { rays, .. } = &spec.mech.delivery {
        return (
            u32::try_from(rays.int(&v).unwrap_or(1).max(1)).unwrap_or(1),
            true,
        );
    }
    match &spec.mech.targeting {
        Targeting::SelfOnly => (1, false),
        Targeting::Creatures { max, .. } => (
            u32::try_from(max.int(&v).unwrap_or(1).clamp(1, 8)).unwrap_or(1),
            false,
        ),
        Targeting::Area { shape, size_ft } => {
            let s = serde_json::to_value(shape)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            (crate::sheet::max_targets(&s, *size_ft), false)
        }
    }
}

/// A one-line summary of a spell at a slot (numbers from the definition, never its text).
#[must_use]
pub fn summary(spec: &CastSpec, slot: Option<u8>) -> String {
    let m = &spec.mech;
    let v = spec.vars(slot);
    let mut parts: Vec<String> = Vec::new();
    parts.push(match (m.level, slot) {
        (0, _) => "cantrip".into(),
        (l, Some(s)) if s > l => format!("level {l} at slot {s}"),
        (l, _) => format!("level {l}"),
    });
    match &m.delivery {
        Delivery::Save { ability, success } => parts.push(format!(
            "DC {} {} save{}",
            spec.dc.map_or_else(|| "?".into(), |d| d.to_string()),
            ability.as_str().to_uppercase(),
            if *success == party_combat::spell::SaveSuccess::Half {
                ", half on success"
            } else {
                ""
            }
        )),
        Delivery::Attack { rays, .. } => {
            let n = rays.int(&v).unwrap_or(1);
            parts.push(format!(
                "{} to hit{}",
                spec.attack.map_or_else(|| "?".into(), |a| format!("{a:+}")),
                if n > 1 {
                    format!(", {n} rays")
                } else {
                    String::new()
                }
            ));
        }
        Delivery::Auto => {}
    }
    for p in prims(m) {
        match p {
            Primitive::Damage {
                dice, damage_type, ..
            } => {
                if let Ok(n) = dice.notation(&v) {
                    let t = damage_type.map_or(String::new(), |t| format!(" {}", t.as_str()));
                    parts.push(format!("{n}{t}"));
                }
            }
            Primitive::Heal { dice, add_mod } => {
                if let Ok(n) = dice.notation(&v) {
                    let m = if *add_mod { spec.modifier } else { 0 };
                    parts.push(format!(
                        "heals {n}{}",
                        if m != 0 {
                            format!("{m:+}")
                        } else {
                            String::new()
                        }
                    ));
                }
            }
            Primitive::TempHp { dice, .. } => {
                if let Ok(n) = dice.notation(&v) {
                    parts.push(format!("{n} temp HP"));
                }
            }
            Primitive::Condition { condition, .. } => parts.push(format!("{condition}")),
            Primitive::AcBonus { n } => parts.push(format!("AC {n:+}")),
            Primitive::AcBase { base, add_dex } => {
                parts.push(format!("AC {base}{}", if *add_dex { " + DEX" } else { "" }));
            }
            Primitive::RollDice { term, .. } => parts.push(format!("{term} to rolls")),
            Primitive::Summon { .. } => parts.push("summon: options only, nothing spawns".into()),
            _ => {}
        }
    }
    match &m.targeting {
        Targeting::Area { shape, size_ft } => {
            let s = serde_json::to_value(shape)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            parts.push(format!("{size_ft}-ft {s}"));
        }
        Targeting::SelfOnly => parts.push("self".into()),
        Targeting::Creatures { .. } => {}
    }
    if m.concentration {
        parts.push("concentration".into());
    }
    parts.join(", ")
}
