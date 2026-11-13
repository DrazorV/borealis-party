//! The advantage table: what the engine's conditions mean for an attack roll or a save (SRD 5.1 and 5.2.1 agree on
//! these). Shared, so the combat tracker can reuse it later.

use party_combat::party_dice::RollMode;
use party_combat::{derive, AbilityName, CombatState, CombatantId, Condition};
use serde::Serialize;

/// The name of the Dodge effect the simulation adds.
pub const DODGE: &str = "Dodge";

/// How an attack is rolled, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttackMode {
    pub mode: RollMode,
    /// A hit is a critical hit (a paralyzed or unconscious target hit in melee).
    pub auto_crit: bool,
    pub reasons: Vec<&'static str>,
}

/// What a combatant has that matters to the table.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context {
    pub ranged: bool,
    /// A conscious enemy is next to the attacker (ranged attacks).
    pub engaged: bool,
    /// Beyond normal range.
    pub long_range: bool,
    /// An ally of the attacker is next to the target (Pack Tactics).
    pub ally_adjacent: bool,
    pub pack_tactics: bool,
    /// The target is next to the attacker (melee within 5 feet).
    pub adjacent: bool,
}

fn dodging(state: &CombatState, id: &CombatantId) -> bool {
    state
        .effects
        .iter()
        .any(|e| &e.spec.target == id && e.spec.name == DODGE)
        && !derive::incapacitated(state, id)
}

/// The attack table.
#[must_use]
pub fn attack_mode(
    state: &CombatState,
    attacker: &CombatantId,
    target: &CombatantId,
    cx: Context,
) -> AttackMode {
    let a = derive::conditions(state, attacker);
    let t = derive::conditions(state, target);
    let mut adv = Vec::new();
    let mut dis = Vec::new();
    for c in &a {
        match c {
            Condition::Blinded => dis.push("attacker blinded"),
            Condition::Poisoned => dis.push("attacker poisoned"),
            Condition::Prone => dis.push("attacker prone"),
            Condition::Restrained => dis.push("attacker restrained"),
            Condition::Frightened => dis.push("attacker frightened"),
            Condition::Invisible => adv.push("attacker unseen"),
            _ => {}
        }
    }
    if cx.ranged && cx.engaged {
        dis.push("ranged attack while engaged");
    }
    if cx.ranged && cx.long_range {
        dis.push("long range");
    }
    for c in &t {
        match c {
            Condition::Blinded => adv.push("target blinded"),
            Condition::Restrained => adv.push("target restrained"),
            Condition::Stunned => adv.push("target stunned"),
            Condition::Paralyzed => adv.push("target paralyzed"),
            Condition::Unconscious => adv.push("target unconscious"),
            Condition::Petrified => adv.push("target petrified"),
            Condition::Invisible => dis.push("target unseen"),
            Condition::Prone if cx.ranged => dis.push("target prone, ranged"),
            Condition::Prone => adv.push("target prone, melee"),
            _ => {}
        }
    }
    if dodging(state, target) {
        dis.push("target dodging");
    }
    if cx.pack_tactics && cx.ally_adjacent {
        adv.push("pack tactics");
    }
    let auto_crit = !cx.ranged
        && cx.adjacent
        && t.iter()
            .any(|c| matches!(c, Condition::Paralyzed | Condition::Unconscious));
    let mut reasons = adv.clone();
    reasons.extend(dis.iter().copied());
    AttackMode {
        mode: RollMode::combine(!adv.is_empty(), !dis.is_empty()),
        auto_crit,
        reasons,
    }
}

/// A save: automatic failure (STR and DEX while paralyzed, stunned, unconscious or petrified), or its mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SaveMode {
    pub auto_fail: bool,
    pub mode: RollMode,
}

/// The save table. `magic_resistance` applies against spells.
#[must_use]
pub fn save_mode(
    state: &CombatState,
    target: &CombatantId,
    ability: AbilityName,
    spell: bool,
    magic_resistance: bool,
) -> SaveMode {
    let t = derive::conditions(state, target);
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
    let adv =
        (ability == AbilityName::Dex && dodging(state, target)) || (spell && magic_resistance);
    let dis = ability == AbilityName::Dex && t.contains(&Condition::Restrained);
    SaveMode {
        auto_fail,
        mode: RollMode::combine(adv, dis),
    }
}

/// The chance of a d20 roll + `bonus` reaching `target`, in 1/400 (integer maths).
#[must_use]
pub fn chance_400(bonus: i32, target: i32, mode: RollMode, nat20_wins: bool) -> i32 {
    let need = (target - bonus).clamp(1, 21);
    let mut faces = 21 - need; // faces out of 20 that succeed
    if nat20_wins {
        faces = faces.clamp(1, 19);
    } else {
        faces = faces.clamp(0, 20);
    }
    let p = faces; // out of 20
    match mode {
        RollMode::Normal => p * 20,
        RollMode::Advantage => 400 - (20 - p) * (20 - p),
        RollMode::Disadvantage => p * p,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use party_combat::party_dice::SeededDie;
    use party_combat::{apply, Actor, Combatant, Command, EffectSpec};

    fn state() -> CombatState {
        let mut s = CombatState::new(party_combat::Edition::Srd521);
        s.combatants.push(Combatant::creature("a", "A", 10, 12, 0));
        s.combatants.push(Combatant::creature("b", "B", 10, 12, 0));
        s
    }

    fn with(s: &CombatState, target: &str, c: Condition) -> CombatState {
        let mut e = EffectSpec::new(c.as_str(), target);
        e.conditions = vec![c];
        apply(
            s,
            &Actor::Dm,
            &Command::AddCondition {
                effect: Box::new(e),
            },
            &mut SeededDie::new(1),
        )
        .unwrap()
        .state
    }

    fn id(s: &str) -> CombatantId {
        CombatantId::new(s)
    }

    #[test]
    fn attacker_conditions_give_disadvantage() {
        for c in [
            Condition::Blinded,
            Condition::Poisoned,
            Condition::Prone,
            Condition::Restrained,
            Condition::Frightened,
        ] {
            let s = with(&state(), "a", c);
            assert_eq!(
                attack_mode(&s, &id("a"), &id("b"), Context::default()).mode,
                RollMode::Disadvantage,
                "{c}"
            );
        }
    }

    #[test]
    fn target_conditions_give_advantage_and_crits() {
        for c in [
            Condition::Blinded,
            Condition::Restrained,
            Condition::Stunned,
            Condition::Paralyzed,
            Condition::Unconscious,
            Condition::Petrified,
        ] {
            let s = with(&state(), "b", c);
            assert_eq!(
                attack_mode(&s, &id("a"), &id("b"), Context::default()).mode,
                RollMode::Advantage,
                "{c}"
            );
        }
        let s = with(&state(), "b", Condition::Paralyzed);
        let melee = Context {
            adjacent: true,
            ..Context::default()
        };
        assert!(attack_mode(&s, &id("a"), &id("b"), melee).auto_crit);
        let ranged = Context {
            ranged: true,
            ..Context::default()
        };
        assert!(!attack_mode(&s, &id("a"), &id("b"), ranged).auto_crit);
    }

    #[test]
    fn prone_depends_on_range_and_both_cancel() {
        let s = with(&state(), "b", Condition::Prone);
        assert_eq!(
            attack_mode(&s, &id("a"), &id("b"), Context::default()).mode,
            RollMode::Advantage
        );
        let ranged = Context {
            ranged: true,
            ..Context::default()
        };
        assert_eq!(
            attack_mode(&s, &id("a"), &id("b"), ranged).mode,
            RollMode::Disadvantage
        );
        let s = with(&s, "a", Condition::Poisoned);
        assert_eq!(
            attack_mode(&s, &id("a"), &id("b"), Context::default()).mode,
            RollMode::Normal
        );
    }

    #[test]
    fn ranged_engaged_long_and_pack_tactics() {
        let s = state();
        let cx = Context {
            ranged: true,
            engaged: true,
            ..Context::default()
        };
        assert_eq!(
            attack_mode(&s, &id("a"), &id("b"), cx).mode,
            RollMode::Disadvantage
        );
        let cx = Context {
            ranged: true,
            long_range: true,
            ..Context::default()
        };
        assert_eq!(
            attack_mode(&s, &id("a"), &id("b"), cx).mode,
            RollMode::Disadvantage
        );
        let cx = Context {
            pack_tactics: true,
            ally_adjacent: true,
            ..Context::default()
        };
        assert_eq!(
            attack_mode(&s, &id("a"), &id("b"), cx).mode,
            RollMode::Advantage
        );
    }

    #[test]
    fn saves_auto_fail_and_dodge() {
        let s = with(&state(), "b", Condition::Stunned);
        assert!(save_mode(&s, &id("b"), AbilityName::Dex, false, false).auto_fail);
        assert!(!save_mode(&s, &id("b"), AbilityName::Wis, false, false).auto_fail);
        let mut e = EffectSpec::new(DODGE, "b");
        e.duration = party_combat::Duration::TargetTurns {
            remaining: 1,
            at: party_combat::Boundary::Start,
        };
        let s = apply(
            &state(),
            &Actor::Dm,
            &Command::AddEffect {
                effect: Box::new(e),
            },
            &mut SeededDie::new(1),
        )
        .unwrap()
        .state;
        assert_eq!(
            save_mode(&s, &id("b"), AbilityName::Dex, false, false).mode,
            RollMode::Advantage
        );
        assert_eq!(
            attack_mode(&s, &id("a"), &id("b"), Context::default()).mode,
            RollMode::Disadvantage
        );
        assert_eq!(
            save_mode(&state(), &id("b"), AbilityName::Wis, true, true).mode,
            RollMode::Advantage
        );
    }

    #[test]
    fn chances() {
        assert_eq!(chance_400(5, 15, RollMode::Normal, true), 11 * 20);
        assert_eq!(chance_400(5, 30, RollMode::Normal, true), 20);
        assert_eq!(chance_400(5, 15, RollMode::Advantage, true), 400 - 81);
    }
}
