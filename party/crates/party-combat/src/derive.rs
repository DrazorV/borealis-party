//! Values derived from the state: the turn order, conditions, effective AC and speed, and whether an ability can
//! be used now. Read-only.

use crate::error::Unavailable;
use crate::model::{
    Ability, AbilityKind, AbilityName, CombatState, Combatant, CombatantId, Condition, Cover,
    DamageClause, Defenses, Edition, Effect, InitiativeMode, Modifier, RuleTarget, Speeds,
    SpellTurn,
};
use crate::spell::{AdvDis, CastSpec, CastTime, DefenseKind, DiceTerm, ModeOn, RollOn, SpeedMode};
use party_dice::RollMode;
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

/// One line of an AC breakdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcPart {
    /// "Mage Armor 13", "DEX", "Shield", "half cover".
    pub label: String,
    /// Its value.
    pub value: i32,
}

/// AC (C13, 0.3.2): `max(printed, best base effect) + Σ AC bonuses`, then at least the best minimum, then cover.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcBreakdown {
    /// The stat block's AC (`None` when unknown).
    pub base: Option<i32>,
    /// The sum of AC bonus modifiers.
    pub effects: i32,
    /// The best cover's bonus.
    pub cover: i32,
    /// The total (`None` when no base is known).
    pub total: Option<i32>,
    /// The base effect that beat the printed AC (Mage Armor), with its value (DEX included).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_effect: Option<AcPart>,
    /// A minimum that raised the AC (Barkskin).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<i32>,
    /// The lines that add up to the total.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<AcPart>,
    /// "15 = Mage Armor 13 + DEX 2", or "15" when nothing changes it.
    #[serde(default)]
    pub text: String,
}

fn targeted<'a>(state: &'a CombatState, id: &'a CombatantId) -> impl Iterator<Item = &'a Effect> {
    state.effects.iter().filter(move |e| &e.spec.target == id)
}

/// The effective AC of a combatant, with its breakdown.
#[must_use]
pub fn effective_ac(state: &CombatState, c: &Combatant) -> AcBreakdown {
    let dex = c.stats.as_ref().map_or(0, |s| s.modifier(AbilityName::Dex));
    let mut best: Option<(i32, i32, &str)> = None; // (total, base, effect name)
    let mut bonus = Vec::new();
    let mut min: Option<(i32, &str)> = None;
    let mut cover: Option<Cover> = None;
    for e in targeted(state, &c.id) {
        for m in &e.spec.modifiers {
            match *m {
                Modifier::AcBase { base, add_dex } => {
                    let v = base + if add_dex { dex } else { 0 };
                    if best.is_none_or(|(b, _, _)| v > b) {
                        best = Some((v, base, &e.spec.name));
                    }
                }
                Modifier::Ac(n) => bonus.push(AcPart {
                    label: e.spec.name.clone(),
                    value: n,
                }),
                Modifier::AcMin(n) if min.is_none_or(|(m, _)| n > m) => {
                    min = Some((n, &e.spec.name));
                }
                Modifier::Cover(k) => cover = cover.max(Some(k)),
                _ => {}
            }
        }
    }
    let mut parts = Vec::new();
    let mut base_effect = None;
    let start = match (c.ac, best) {
        (printed, Some((v, base, name))) if printed.is_none_or(|p| v > p) => {
            parts.push(AcPart {
                label: name.to_string(),
                value: base,
            });
            if v != base {
                parts.push(AcPart {
                    label: "DEX".into(),
                    value: v - base,
                });
            }
            base_effect = Some(AcPart {
                label: name.to_string(),
                value: v,
            });
            Some(v)
        }
        (Some(p), _) => {
            parts.push(AcPart {
                label: "AC".into(),
                value: p,
            });
            Some(p)
        }
        (None, _) => None,
    };
    let effects: i32 = bonus.iter().map(|p| p.value).sum();
    parts.extend(bonus);
    let mut total = start.map(|s| s + effects);
    let mut min_used = None;
    if let (Some(t), Some((m, name))) = (total, min) {
        if m > t {
            total = Some(m);
            min_used = Some(m);
            parts = vec![AcPart {
                label: format!("{name} minimum"),
                value: m,
            }];
        }
    }
    let cover_bonus = cover.map_or(0, Cover::ac_bonus);
    if let Some(k) = cover {
        parts.push(AcPart {
            label: match k {
                Cover::Half => "half cover".into(),
                Cover::ThreeQuarters => "three-quarters cover".into(),
            },
            value: cover_bonus,
        });
    }
    let total = total.map(|t| t + cover_bonus);
    let text = match total {
        None => "AC unknown".to_string(),
        Some(t) if parts.len() <= 1 && base_effect.is_none() && min_used.is_none() => t.to_string(),
        Some(t) => {
            let mut s = format!("{t} =");
            for (i, p) in parts.iter().enumerate() {
                if i == 0 {
                    s.push_str(&format!(" {} {}", p.label, p.value));
                } else {
                    s.push_str(&format!(
                        " {} {} {}",
                        if p.value < 0 { "-" } else { "+" },
                        p.label,
                        p.value.unsigned_abs()
                    ));
                }
            }
            s
        }
    };
    AcBreakdown {
        base: c.ac,
        effects,
        cover: cover_bonus,
        total,
        base_effect,
        min: min_used,
        parts,
        text,
    }
}

/// The SRD armor names a printed AC source may mention ("leather armor", "chain mail"). A creature wearing one of
/// these can't benefit from Mage Armor; the DM may apply it anyway.
pub const SRD_ARMOR: &[&str] = &[
    "padded",
    "leather",
    "studded leather",
    "hide",
    "chain shirt",
    "scale mail",
    "breastplate",
    "half plate",
    "ring mail",
    "chain mail",
    "splint",
    "plate",
];

/// The armor the combatant wears, read from its printed AC source ("natural armor" is none).
#[must_use]
pub fn wears_armor(c: &Combatant) -> Option<String> {
    let src = c.stats.as_ref()?.armor.as_ref()?.to_lowercase();
    if src.contains("natural") || src.contains("mage armor") {
        return None;
    }
    SRD_ARMOR
        .iter()
        .find(|a| src.contains(*a))
        .map(|a| (*a).to_string())
}

/// The combatant's defenses with the effects' resistances, immunities and vulnerabilities added (as plain
/// clauses).
#[must_use]
pub fn effective_defenses(state: &CombatState, c: &Combatant) -> Defenses {
    let mut d = c.defenses.clone();
    for e in targeted(state, &c.id) {
        for m in &e.spec.modifiers {
            if let Modifier::Defense {
                defense,
                damage_type,
            } = *m
            {
                let list = match defense {
                    DefenseKind::Resist => &mut d.resistances,
                    DefenseKind::Immune => &mut d.immunities,
                    DefenseKind::Vuln => &mut d.vulnerabilities,
                };
                list.push(DamageClause::plain(&[damage_type]));
            }
        }
    }
    d
}

// ----- d20 tests: the advantage tables (moved from party-sim's rules.rs) and bonuses -----

/// The name of the Dodge effect (the standard action and the simulation add it).
pub const DODGE: &str = "Dodge";

fn dodging(state: &CombatState, id: &CombatantId) -> bool {
    targeted(state, id).any(|e| e.spec.name == DODGE) && !incapacitated(state, id)
}

/// How an attack is rolled, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttackMode {
    /// Normal, advantage or disadvantage.
    pub mode: RollMode,
    /// A hit is a critical hit (a paralyzed or unconscious target hit in melee).
    pub auto_crit: bool,
    /// Why ("target restrained", "Bless").
    pub reasons: Vec<String>,
    /// Dice added to the roll (Bless).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dice: Vec<DiceTerm>,
}

/// What the table needs to know about an attack's circumstances.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttackContext {
    /// A ranged attack.
    pub ranged: bool,
    /// A conscious enemy is next to the attacker (ranged attacks).
    pub engaged: bool,
    /// Beyond normal range.
    pub long_range: bool,
    /// An ally of the attacker is next to the target (Pack Tactics).
    pub ally_adjacent: bool,
    /// The attacker has Pack Tactics.
    pub pack_tactics: bool,
    /// The target is next to the attacker (melee within 5 feet).
    pub adjacent: bool,
}

fn mode_mods<'a>(
    state: &'a CombatState,
    id: &'a CombatantId,
    on: ModeOn,
    ability: Option<AbilityName>,
) -> impl Iterator<Item = (AdvDis, &'a str)> + 'a {
    targeted(state, id).flat_map(move |e| {
        e.spec.modifiers.iter().filter_map(move |m| match *m {
            Modifier::RollMode {
                on: o,
                ability: a,
                mode,
            } if o == on && (a.is_none() || a == ability) => Some((mode, e.spec.name.as_str())),
            _ => None,
        })
    })
}

fn dice_mods(
    state: &CombatState,
    id: &CombatantId,
    on: RollOn,
    ability: Option<AbilityName>,
) -> Vec<(DiceTerm, String)> {
    targeted(state, id)
        .flat_map(|e| {
            e.spec.modifiers.iter().filter_map(move |m| match *m {
                Modifier::RollDice {
                    on: o,
                    ability: a,
                    term,
                } if o == on && (a.is_none() || a == ability) => Some((term, e.spec.name.clone())),
                _ => None,
            })
        })
        .collect()
}

/// The attack table: conditions, Dodge, range, Pack Tactics, roll-mode effects on both sides, exhaustion, and the
/// attacker's roll dice (Bless). SRD 5.1 and 5.2.1 agree on the conditions.
#[must_use]
pub fn attack_mode(
    state: &CombatState,
    attacker: &CombatantId,
    target: &CombatantId,
    cx: AttackContext,
) -> AttackMode {
    let a = conditions(state, attacker);
    let t = conditions(state, target);
    let mut adv: Vec<String> = Vec::new();
    let mut dis: Vec<String> = Vec::new();
    for c in &a {
        match c {
            Condition::Blinded => dis.push("attacker blinded".into()),
            Condition::Poisoned => dis.push("attacker poisoned".into()),
            Condition::Prone => dis.push("attacker prone".into()),
            Condition::Restrained => dis.push("attacker restrained".into()),
            Condition::Frightened => dis.push("attacker frightened".into()),
            Condition::Invisible => adv.push("attacker unseen".into()),
            _ => {}
        }
    }
    if cx.ranged && cx.engaged {
        dis.push("ranged attack while engaged".into());
    }
    if cx.ranged && cx.long_range {
        dis.push("long range".into());
    }
    for c in &t {
        match c {
            Condition::Blinded => adv.push("target blinded".into()),
            Condition::Restrained => adv.push("target restrained".into()),
            Condition::Stunned => adv.push("target stunned".into()),
            Condition::Paralyzed => adv.push("target paralyzed".into()),
            Condition::Unconscious => adv.push("target unconscious".into()),
            Condition::Petrified => adv.push("target petrified".into()),
            Condition::Invisible => dis.push("target unseen".into()),
            Condition::Prone if cx.ranged => dis.push("target prone, ranged".into()),
            Condition::Prone => adv.push("target prone, melee".into()),
            _ => {}
        }
    }
    if dodging(state, target) {
        dis.push("target dodging".into());
    }
    if cx.pack_tactics && cx.ally_adjacent {
        adv.push("pack tactics".into());
    }
    for (mode, name) in mode_mods(state, attacker, ModeOn::AttacksBy, None) {
        let list = if mode == AdvDis::Advantage {
            &mut adv
        } else {
            &mut dis
        };
        list.push(name.to_string());
    }
    for (mode, name) in mode_mods(state, target, ModeOn::AttacksAgainst, None) {
        if name == DODGE {
            continue;
        }
        let list = if mode == AdvDis::Advantage {
            &mut adv
        } else {
            &mut dis
        };
        list.push(format!("target {name}"));
    }
    if state.edition == Edition::Srd51
        && state.combatant(attacker).is_some_and(|c| c.exhaustion >= 3)
    {
        dis.push("exhaustion".into());
    }
    let auto_crit = !cx.ranged
        && cx.adjacent
        && t.iter()
            .any(|c| matches!(c, Condition::Paralyzed | Condition::Unconscious));
    let dice_mods = dice_mods(state, attacker, RollOn::Attacks, None);
    let mut reasons = adv.clone();
    reasons.extend(dis.iter().cloned());
    reasons.extend(dice_mods.iter().map(|(t, n)| format!("{n} {t}")));
    AttackMode {
        mode: RollMode::combine(!adv.is_empty(), !dis.is_empty()),
        auto_crit,
        reasons,
        dice: dice_mods.into_iter().map(|(t, _)| t).collect(),
    }
}

/// A saving throw: its bonus, extra dice, mode, automatic failure, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveMod {
    /// The flat bonus: the save from the stat block (`stats`), cover for DEX, 2024 exhaustion.
    pub bonus: i32,
    /// Dice added (Bless +1d4, Bane −1d4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dice: Vec<DiceTerm>,
    /// Normal, advantage or disadvantage.
    pub mode: RollMode,
    /// It fails without a roll (STR and DEX while paralyzed, stunned, unconscious or petrified).
    pub auto_fail: bool,
    /// Why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    /// The save bonus is known (`stats` present); otherwise `bonus` has only the situational parts.
    pub known: bool,
}

impl SaveMod {
    /// The d20 input for an app roll (`dice` are rolled separately).
    #[must_use]
    pub fn d20(&self) -> party_dice::D20Input {
        party_dice::D20Input::app(self.bonus, self.mode)
    }
}

/// The saving throw of `id` with `ability`; `spell` for a save against a spell (Magic Resistance).
#[must_use]
pub fn save_mod(
    state: &CombatState,
    id: &CombatantId,
    ability: AbilityName,
    spell: bool,
) -> SaveMod {
    let c = state.combatant(id);
    let t = conditions(state, id);
    let mut reasons = Vec::new();
    let physical = matches!(ability, AbilityName::Str | AbilityName::Dex);
    let auto_fail = physical
        && t.iter().any(|c| {
            matches!(
                c,
                Condition::Paralyzed
                    | Condition::Stunned
                    | Condition::Unconscious
                    | Condition::Petrified
            )
        });
    if auto_fail {
        reasons.push(format!("automatic failure ({})", ability));
    }
    let stats = c.and_then(|c| c.stats.as_ref());
    let mut bonus = stats.map_or(0, |s| s.save(ability));
    let mut adv = Vec::new();
    let mut dis = Vec::new();
    if ability == AbilityName::Dex && dodging(state, id) {
        adv.push("dodging".to_string());
    }
    if spell && stats.is_some_and(|s| s.magic_resistance) {
        adv.push("Magic Resistance".to_string());
    }
    if ability == AbilityName::Dex && t.contains(&Condition::Restrained) {
        dis.push("restrained".to_string());
    }
    for (mode, name) in mode_mods(state, id, ModeOn::Saves, Some(ability)) {
        if name == DODGE {
            continue;
        }
        let list = if mode == AdvDis::Advantage {
            &mut adv
        } else {
            &mut dis
        };
        list.push(name.to_string());
    }
    if let Some(c) = c {
        match state.edition {
            Edition::Srd51 if c.exhaustion >= 3 => dis.push("exhaustion".to_string()),
            Edition::Srd521 if c.exhaustion > 0 => {
                let pen = 2 * i32::from(c.exhaustion);
                bonus -= pen;
                reasons.push(format!("exhaustion -{pen}"));
            }
            _ => {}
        }
        if ability == AbilityName::Dex {
            let cover = targeted(state, id)
                .flat_map(|e| e.spec.modifiers.iter())
                .filter_map(|m| {
                    if let Modifier::Cover(k) = m {
                        Some(*k)
                    } else {
                        None
                    }
                })
                .max();
            if let Some(k) = cover {
                bonus += k.ac_bonus();
                reasons.push(format!("cover +{}", k.ac_bonus()));
            }
        }
    }
    let dice = dice_mods(state, id, RollOn::Saves, Some(ability));
    reasons.extend(adv.iter().cloned());
    reasons.extend(dis.iter().cloned());
    reasons.extend(dice.iter().map(|(t, n)| format!("{n} {t}")));
    SaveMod {
        bonus,
        dice: dice.into_iter().map(|(t, _)| t).collect(),
        mode: RollMode::combine(!adv.is_empty(), !dis.is_empty()),
        auto_fail,
        reasons,
        known: stats.is_some(),
    }
}

/// Whether the caster may cast `spec` now at `slot` (0.3.2). `slotless` is an at-will or per-day use of a
/// levelled spell; `precast` skips the economy and edition rules (cast before the fight). The edition rules:
/// - 2014: after a bonus action spell, only a cantrip with a casting time of one action this turn; and no bonus
///   action spell after a levelled action spell;
/// - 2024: one spell slot per turn (reactions on other turns included).
///
/// `Err` says why not; the DM's `force` overrides everything but a missing or too-low slot.
pub fn cast_status(
    state: &CombatState,
    c: &Combatant,
    spec: &CastSpec,
    slot: Option<u8>,
    slotless: bool,
    precast: bool,
) -> Result<(), Unavailable> {
    slot_status(c, spec, slot, slotless)?;
    if precast {
        return Ok(());
    }
    rules_status(state, c, spec, slot)
}

/// The slot part of [`cast_status`] (never overridden).
pub fn slot_status(
    c: &Combatant,
    spec: &CastSpec,
    slot: Option<u8>,
    slotless: bool,
) -> Result<(), Unavailable> {
    let level = spec.mech.level;
    match slot {
        Some(l) if l < level || level == 0 => return Err(Unavailable::SlotTooLow(level)),
        Some(l) if c.spell_slots.get(&l).is_none_or(|p| p.remaining == 0) => {
            return Err(Unavailable::NoSlot(l))
        }
        None if level > 0 && !slotless => return Err(Unavailable::NeedsSlot(level)),
        _ => {}
    }
    Ok(())
}

/// The economy and edition part of [`cast_status`] (the DM's `force` overrides it).
pub fn rules_status(
    state: &CombatState,
    c: &Combatant,
    spec: &CastSpec,
    slot: Option<u8>,
) -> Result<(), Unavailable> {
    if c.is_defeated() {
        return Err(Unavailable::Defeated);
    }
    if !c.life.is_alive() {
        return Err(Unavailable::Down);
    }
    if incapacitated(state, &c.id) {
        return Err(Unavailable::Incapacitated);
    }
    let cast = spec.mech.cast;
    match cast {
        CastTime::Action | CastTime::Minute if c.economy.action => {
            return Err(Unavailable::ActionSpent)
        }
        CastTime::Bonus if c.economy.bonus_action => return Err(Unavailable::BonusActionSpent),
        CastTime::Reaction if c.economy.reaction => return Err(Unavailable::ReactionSpent),
        _ => {}
    }
    let Some(turn) = this_turn(state, c) else {
        return Ok(());
    };
    match state.edition {
        Edition::Srd51 => {
            let cantrip_action = spec.mech.level == 0 && cast == CastTime::Action;
            if turn.bonus_spell && cast != CastTime::Reaction && !cantrip_action {
                return Err(Unavailable::BonusSpellRule);
            }
            if cast == CastTime::Bonus && turn.leveled_action_spell {
                return Err(Unavailable::BonusSpellRule);
            }
        }
        Edition::Srd521 => {
            if slot.is_some() && turn.slot_spent {
                return Err(Unavailable::OneSlotPerTurn);
            }
        }
    }
    Ok(())
}

/// The caster's spell record for the current turn, if it cast this turn.
#[must_use]
pub fn this_turn<'a>(state: &CombatState, c: &'a Combatant) -> Option<&'a SpellTurn> {
    let cur = state.current.as_ref()?;
    c.spell_turn
        .as_ref()
        .filter(|t| t.round == state.round && &t.turn == cur)
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
    let mut s = c.speed;
    for m in &mods {
        if let Modifier::SpeedSetMode { mode, ft } = *m {
            let slot = match mode {
                SpeedMode::Walk => {
                    s.walk = s.walk.max(ft);
                    continue;
                }
                SpeedMode::Fly => &mut s.fly,
                SpeedMode::Swim => &mut s.swim,
                SpeedMode::Climb => &mut s.climb,
                SpeedMode::Burrow => &mut s.burrow,
            };
            *slot = Some(slot.unwrap_or(0).max(ft));
        }
    }
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
