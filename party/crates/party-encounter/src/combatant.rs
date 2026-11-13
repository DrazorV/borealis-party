//! Combatants from encounter copies: the fighter a creature's copy and overrides give, and the combat engine's
//! creature built from it (abilities with recharges, uses and legendary costs, C14–C17). Shared by the live combat
//! server and the encounter simulation (`party-sim`), so both build the same creature from the same copy.

use std::collections::BTreeSet;

use borealis_content::MonsterData;
use party_combat::{
    Ability, AbilityKind, Combatant, CombatantId, DamageClause, Defenses, LegendaryPool, Pool,
    Recharge, Reset, Resource, Speeds,
};
use serde_json::{json, Value};

use crate::creature::Snapshot;
use crate::overrides::Overrides;

/// A creature as its encounter copy has it: the block it fights with (overrides applied, no XP), its typed
/// `MonsterData` when it is a monster, its name (the DM's), and its HP and AC.
#[derive(Debug, Clone)]
pub struct Fighter {
    pub block: Value,
    pub data: Option<MonsterData>,
    pub name: String,
    pub ac: Option<i32>,
    pub average: i32,
    pub formula: Option<String>,
}

/// The fighter of an encounter creature's copy (`statblock_json`) with its overrides.
pub fn fighter_of(statblock: &str, overrides: &str) -> Result<Fighter, serde_json::Error> {
    let overrides: Overrides = serde_json::from_str(overrides).unwrap_or_default();
    let snap: Snapshot = serde_json::from_str(statblock)?;
    Ok(fighter_from_snapshot(&snap, &overrides))
}

/// The fighter of a stat block copy with its overrides.
pub fn fighter_from_snapshot(snap: &Snapshot, overrides: &Overrides) -> Fighter {
    let formula = overrides
        .hp
        .as_ref()
        .and_then(|h| h.value.formula.clone())
        .or_else(|| snap.printed().hp.and_then(|h| h.formula));
    Fighter {
        block: snap.block(overrides),
        data: snap.data.as_ref().map(|d| overrides.apply(d)),
        name: snap.name.clone(),
        ac: snap.ac(overrides).and_then(|a| i32::try_from(a).ok()),
        average: i32::try_from(snap.average_hp(overrides))
            .unwrap_or(1)
            .max(1),
        formula,
    }
}

/// A creature combatant for a fighter: its block's numbers and abilities, the AC it prints (none for a thin
/// companion block), and `hp`.
pub fn creature_of(id: &str, label: &str, public: &str, f: &Fighter, hp: i32) -> Combatant {
    let mut c = build_creature(id, label, public, f.data.as_ref(), hp);
    if f.data.is_none() {
        c.ac = f.ac;
        c.initiative_bonus = f
            .block
            .get("initiative")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok())
            .unwrap_or(0);
    }
    c
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

/// A creature combatant from its stat block: HP, AC, initiative, speeds, defenses, and its abilities with their
/// recharges, uses and legendary costs (C14–C17). The label is the DM's; players see `public`.
pub fn build_creature(
    id: &str,
    label: &str,
    public: &str,
    data: Option<&MonsterData>,
    hp: i32,
) -> Combatant {
    let Some(d) = data else {
        let mut c = Combatant::creature(id, label, hp, 10, 0);
        c.public_name = public.to_string();
        return c;
    };
    let ac =
        d.ac.first()
            .map_or(10, |a| i32::try_from(a.value).unwrap_or(10));
    let init = d.initiative.as_ref().map_or_else(
        || {
            d.abilities
                .modifier(borealis_content::monster_types::Ability::Dex)
        },
        |i| i.modifier,
    );
    let mut c = Combatant::creature(id, label, hp, ac, init);
    c.public_name = public.to_string();
    c.speed = Speeds {
        walk: d.speed.walk.unwrap_or(0),
        fly: d.speed.fly,
        swim: d.speed.swim,
        climb: d.speed.climb,
        burrow: d.speed.burrow,
        hover: d.speed.hover,
    };
    let clauses = |list: &[borealis_content::monster_types::DamageClause]| -> Vec<DamageClause> {
        list.iter()
            .filter_map(|cl| {
                serde_json::to_value(cl)
                    .ok()
                    .and_then(|v| serde_json::from_value(v).ok())
            })
            .collect()
    };
    c.defenses = Defenses {
        immunities: clauses(&d.damage_immunities),
        resistances: clauses(&d.damage_resistances),
        vulnerabilities: clauses(&d.damage_vulnerabilities),
        condition_immunities: d
            .condition_immunities
            .iter()
            .filter_map(|s| serde_json::from_value(json!(s.trim().to_lowercase())).ok())
            .collect(),
    };
    let mut keys = BTreeSet::new();
    let mut add =
        |c: &mut Combatant, f: &borealis_content::monster_types::Feature, kind: AbilityKind| {
            let mut key = slug(&f.name);
            if key.is_empty() {
                key = "ability".into();
            }
            let base = key.clone();
            let mut n = 2;
            while !keys.insert(key.clone()) {
                key = format!("{base}_{n}");
                n += 1;
            }
            let mut ability = Ability {
                key: key.clone(),
                name: f.name.clone(),
                kind,
                text: f.text.clone(),
                legendary_cost: None,
                recharge: None,
                uses: None,
                group_limit: None,
                requires_effect: None,
            };
            if kind == AbilityKind::Legendary {
                ability.legendary_cost = Some(u32::from(f.cost.unwrap_or(1)));
            }
            match &f.usage {
                Some(borealis_content::monster_types::Usage::Recharge { min, .. }) => {
                    ability.recharge = Some(Recharge {
                        min: *min,
                        ready: true,
                    });
                }
                Some(
                    borealis_content::monster_types::Usage::PerDay(n)
                    | borealis_content::monster_types::Usage::PerDayInLair { normal: n, .. },
                ) => {
                    let pool = Pool::full(u32::from(*n));
                    if f.name.to_lowercase().starts_with("legendary resistance") {
                        c.legendary_resistance = Some(pool);
                    } else {
                        let resource = format!("uses_{key}");
                        c.resources.insert(
                            resource.clone(),
                            Resource {
                                label: f.name.clone(),
                                pool,
                                reset: Reset::Never,
                            },
                        );
                        ability.uses = Some(resource);
                    }
                }
                _ => {}
            }
            c.abilities.push(ability);
        };
    for f in &d.traits {
        add(&mut c, f, AbilityKind::Passive);
    }
    for f in &d.actions {
        add(&mut c, f, AbilityKind::Action);
    }
    for f in &d.bonus_actions {
        add(&mut c, f, AbilityKind::BonusAction);
    }
    for f in &d.reactions {
        add(&mut c, f, AbilityKind::Reaction);
    }
    if let Some(l) = &d.legendary {
        for f in &l.actions {
            add(&mut c, f, AbilityKind::Legendary);
        }
    }
    c
}

/// A creature's legendary pool, when its block has legendary actions (C16, one pool per creature to start with).
pub fn legendary_pool(id: &str, d: Option<&MonsterData>) -> Option<LegendaryPool> {
    let l = d?.legendary.as_ref()?;
    if l.actions.is_empty() {
        return None;
    }
    let max = u32::from(l.per_round.unwrap_or(3));
    Some(LegendaryPool {
        max,
        remaining: max,
        members: vec![CombatantId::new(id)],
    })
}
