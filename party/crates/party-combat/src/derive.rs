//! Values derived from the state: the turn order, conditions, effective AC and speed, and whether an ability can
//! be used now. Read-only.

use crate::error::Unavailable;
use crate::model::{
    Ability, AbilityKind, CombatState, Combatant, CombatantId, Condition, Edition, InitiativeMode,
    Modifier, RuleTarget, Speeds,
};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;

/// The leader is up and in the order, so the follower acts right after it (C3).
#[must_use]
pub fn link_active(state: &CombatState, c: &Combatant) -> bool {
    c.linked_to
        .as_ref()
        .and_then(|l| state.combatant(l))
        .is_some_and(|l| {
            !l.removed && !l.absent && l.life.is_alive() && effective_initiative(state, l).is_some()
        })
}

/// Whom a combatant acts right after, if anyone:
/// - its C3 link (a mount after its rider) while that link is active;
/// - otherwise its owner, for a controlled creature in `after_owner` mode whose owner is in the fight. A defeated
///   or dismissed owner still leads: the DM decides what happens next (`owner_defeated`).
#[must_use]
pub fn leader<'a>(state: &'a CombatState, c: &Combatant) -> Option<&'a Combatant> {
    leader_at(state, c, 0)
}

fn leader_at<'a>(state: &'a CombatState, c: &Combatant, depth: usize) -> Option<&'a Combatant> {
    if depth > state.combatants.len() {
        return None;
    }
    if let Some(l) = c.linked_to.as_ref().and_then(|l| state.combatant(l)) {
        if !l.removed
            && !l.absent
            && l.life.is_alive()
            && initiative_at(state, l, depth + 1).is_some()
        {
            return Some(l);
        }
    }
    if c.initiative_mode == InitiativeMode::AfterOwner {
        if let Some(o) = c.owner_id.as_ref().and_then(|o| state.combatant(o)) {
            if !o.absent && initiative_at(state, o, depth + 1).is_some() {
                return Some(o);
            }
        }
    }
    None
}

/// The initiative a combatant acts on: its leader's while it follows one, else its own.
#[must_use]
pub fn effective_initiative(state: &CombatState, c: &Combatant) -> Option<i32> {
    initiative_at(state, c, 0)
}

fn initiative_at(state: &CombatState, c: &Combatant, depth: usize) -> Option<i32> {
    match leader_at(state, c, depth) {
        Some(l) => initiative_at(state, l, depth + 1),
        None => c.initiative,
    }
}

/// Everyone with an initiative, removed and absent ones included, in turn order. The turn flow steps through this
/// list, so a removed or defeated active combatant keeps its place and never restarts the round (C4). Followers
/// come right after their leader, in the order they were added (their `tiebreak`), nested to any depth.
pub(crate) fn anchor_order(state: &CombatState) -> Vec<CombatantId> {
    let mut roots: Vec<&Combatant> = state
        .combatants
        .iter()
        .filter(|c| c.initiative.is_some() && leader(state, c).is_none())
        .collect();
    roots.sort_by_key(|c| {
        (
            Reverse(c.initiative),
            Reverse(c.initiative_bonus),
            c.tiebreak,
        )
    });
    let mut out = Vec::new();
    for root in roots {
        push_with_followers(state, root, &mut out, 0);
    }
    out
}

fn push_with_followers(
    state: &CombatState,
    c: &Combatant,
    out: &mut Vec<CombatantId>,
    depth: usize,
) {
    if depth > state.combatants.len() || out.contains(&c.id) {
        return;
    }
    out.push(c.id.clone());
    let mut followers: Vec<&Combatant> = state
        .combatants
        .iter()
        .filter(|f| leader(state, f).is_some_and(|l| l.id == c.id))
        .collect();
    followers.sort_by_key(|f| f.tiebreak);
    for f in followers {
        push_with_followers(state, f, out, depth + 1);
    }
}

/// Takes turns: in the order, not absent, not removed, not dead.
pub(crate) fn takes_turn(c: &Combatant) -> bool {
    !c.removed && !c.absent && !c.life.is_dead()
}

/// The turn order the DM sees: everyone with an initiative who isn't removed or absent (dead ones included, shown
/// as defeated).
#[must_use]
pub fn turn_order(state: &CombatState) -> Vec<CombatantId> {
    anchor_order(state)
        .into_iter()
        .filter(|id| state.combatant(id).is_some_and(|c| !c.removed && !c.absent))
        .collect()
}

/// The conditions on a combatant, from its effects, sorted, each once.
#[must_use]
pub fn conditions(state: &CombatState, id: &CombatantId) -> Vec<Condition> {
    let mut out: Vec<Condition> = state
        .effects
        .iter()
        .filter(|e| &e.spec.target == id)
        .flat_map(|e| e.spec.conditions.iter().copied())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Has `condition`.
#[must_use]
pub fn has_condition(state: &CombatState, id: &CombatantId, condition: Condition) -> bool {
    conditions(state, id).contains(&condition)
}

/// Incapacitated, or a condition that includes it.
#[must_use]
pub fn incapacitated(state: &CombatState, id: &CombatantId) -> bool {
    conditions(state, id).iter().any(|c| c.incapacitates())
}

/// AC: the base, effect modifiers and the best cover (C13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcBreakdown {
    /// The stat block's AC (`None` when unknown).
    pub base: Option<i32>,
    /// The sum of effect modifiers.
    pub effects: i32,
    /// The best cover's bonus.
    pub cover: i32,
    /// The total (`None` when the base is unknown).
    pub total: Option<i32>,
}

/// The effective AC of a combatant.
#[must_use]
pub fn effective_ac(state: &CombatState, c: &Combatant) -> AcBreakdown {
    let mods = || {
        state
            .effects
            .iter()
            .filter(|e| e.spec.target == c.id)
            .flat_map(|e| e.spec.modifiers.iter())
    };
    let effects: i32 = mods()
        .map(|m| if let Modifier::Ac(n) = m { *n } else { 0 })
        .sum();
    let cover = mods()
        .filter_map(|m| {
            if let Modifier::Cover(c) = m {
                Some(*c)
            } else {
                None
            }
        })
        .max();
    let cover = cover.map_or(0, |c| c.ac_bonus());
    AcBreakdown {
        base: c.ac,
        effects,
        cover,
        total: c.ac.map(|ac| ac + effects + cover),
    }
}

/// The effective speeds (C13): additions, then multiplications and halving, then zeroing by an effect, a condition
/// or exhaustion. 2014 exhaustion halves speed at level 2 and zeroes it at 5; 2024 exhaustion takes 5 ft per level.
#[must_use]
pub fn effective_speed(state: &CombatState, c: &Combatant) -> Speeds {
    let mods: Vec<Modifier> = state
        .effects
        .iter()
        .filter(|e| e.spec.target == c.id)
        .flat_map(|e| e.spec.modifiers.iter().copied())
        .collect();
    let zero = mods.contains(&Modifier::SpeedZero)
        || conditions(state, &c.id).iter().any(|k| k.stops_movement())
        || (state.edition == Edition::Srd51 && c.exhaustion >= 5);
    let add: i32 = mods
        .iter()
        .map(|m| if let Modifier::SpeedAdd(n) = m { *n } else { 0 })
        .sum();
    let adjust = |base: u32| -> u32 {
        if zero {
            return 0;
        }
        let mut v = i64::from(base) + i64::from(add);
        for m in &mods {
            match m {
                Modifier::SpeedMultiply(k) => v *= i64::from(*k),
                Modifier::SpeedHalve => v /= 2,
                _ => {}
            }
        }
        match state.edition {
            Edition::Srd51 if c.exhaustion >= 2 => v /= 2,
            Edition::Srd521 => v -= 5 * i64::from(c.exhaustion),
            Edition::Srd51 => {}
        }
        v.clamp(0, i64::from(u32::MAX)) as u32
    };
    let s = c.speed;
    Speeds {
        walk: adjust(s.walk),
        fly: s.fly.map(adjust),
        swim: s.swim.map(adjust),
        climb: s.climb.map(adjust),
        burrow: s.burrow.map(adjust),
        hover: s.hover,
    }
}

/// The legendary pool a combatant is in.
#[must_use]
pub fn pool_of<'a>(
    state: &'a CombatState,
    id: &CombatantId,
) -> Option<(&'a String, &'a crate::model::LegendaryPool)> {
    state
        .legendary_pools
        .iter()
        .find(|(_, p)| p.members.contains(id))
}

/// The member whose turn start refills a pool: the first that isn't defeated.
#[must_use]
pub fn pool_reset_owner<'a>(
    state: &'a CombatState,
    pool: &'a crate::model::LegendaryPool,
) -> Option<&'a CombatantId> {
    pool.members
        .iter()
        .find(|m| state.combatant(m).is_some_and(|c| !c.is_defeated()))
}

/// The name of an encounter rule that switches this ability off now, if any (D42).
#[must_use]
pub fn rule_blocking(state: &CombatState, c: &Combatant, ability: &str) -> Option<String> {
    for rule in &state.rules {
        for flag in rule.flags.iter().filter(|f| f.ability == ability) {
            let hits = match &flag.target {
                RuleTarget::Combatant(id) => id == &c.id,
                RuleTarget::Group(g) => c.group.as_deref() == Some(g.as_str()),
            };
            if hits && rule.active != flag.while_active {
                return Some(rule.name.clone());
            }
        }
    }
    None
}

/// Whether `ability` can be used now, with `slot` spent (C14). `Err` says why not.
pub fn ability_status(
    state: &CombatState,
    c: &Combatant,
    ability: &Ability,
    slot: Option<u8>,
) -> Result<(), Unavailable> {
    if c.is_defeated() {
        return Err(Unavailable::Defeated);
    }
    if ability.kind == AbilityKind::Passive {
        return Err(Unavailable::Passive);
    }
    if !c.life.is_alive() {
        return Err(Unavailable::Down);
    }
    if ability.kind != AbilityKind::Free && incapacitated(state, &c.id) {
        return Err(Unavailable::Incapacitated);
    }
    if let Some(rule) = rule_blocking(state, c, &ability.key) {
        return Err(Unavailable::Rule(rule));
    }
    if let Some(name) = &ability.requires_effect {
        let on = state
            .effects
            .iter()
            .any(|e| e.spec.target == c.id && &e.spec.name == name)
            || c.concentration.as_ref().is_some_and(|k| &k.name == name);
        if !on {
            return Err(Unavailable::Requires(name.clone()));
        }
    }
    if ability.recharge.is_some_and(|r| !r.ready) {
        return Err(Unavailable::NeedsRecharge);
    }
    if let Some(key) = &ability.uses {
        match c.resources.get(key) {
            Some(r) if r.pool.remaining > 0 => {}
            Some(r) => return Err(Unavailable::NoUses(r.label.clone())),
            None => return Err(Unavailable::NoUses(key.clone())),
        }
    }
    if let Some(level) = slot {
        if c.spell_slots.get(&level).is_none_or(|p| p.remaining == 0) {
            return Err(Unavailable::NoSlot(level));
        }
    }
    if let Some(tag) = &ability.group_limit {
        if state
            .group_limits
            .get(tag)
            .is_some_and(|g| g.used >= g.per_round)
        {
            return Err(Unavailable::GroupLimit(tag.clone()));
        }
    }
    match ability.kind {
        AbilityKind::Action if c.economy.action => return Err(Unavailable::ActionSpent),
        AbilityKind::BonusAction if c.economy.bonus_action => {
            return Err(Unavailable::BonusActionSpent)
        }
        AbilityKind::Reaction if c.economy.reaction => return Err(Unavailable::ReactionSpent),
        AbilityKind::Legendary => {
            let (_, pool) = pool_of(state, &c.id).ok_or(Unavailable::NoLegendaryPool)?;
            if state.current.as_ref() == Some(&c.id) {
                return Err(Unavailable::OwnTurn);
            }
            if pool.remaining < ability.legendary_cost.unwrap_or(1) {
                return Err(Unavailable::NotEnoughLegendary);
            }
        }
        _ => {}
    }
    Ok(())
}
