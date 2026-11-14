//! The advantage table, as the combat engine has it since 0.3.2 (`party_combat::derive::{attack_mode, save_mod}`):
//! conditions, Dodge, range, Pack Tactics, spell effects (Bless dice, Faerie Fire advantage), exhaustion, automatic
//! failures and Magic Resistance. The simulation re-exports them so the tracker and the sim share one table.

use party_combat::party_dice::RollMode;
use party_combat::{derive, AbilityName, CombatState, CombatantId};
use serde::Serialize;

pub use party_combat::derive::{AttackContext as Context, AttackMode, DODGE};

/// The attack table (the engine's).
#[must_use]
pub fn attack_mode(
    state: &CombatState,
    attacker: &CombatantId,
    target: &CombatantId,
    cx: Context,
) -> AttackMode {
    derive::attack_mode(state, attacker, target, cx)
}

/// A save: automatic failure (STR and DEX while paralyzed, stunned, unconscious or petrified), or its mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SaveMode {
    pub auto_fail: bool,
    pub mode: RollMode,
}

/// The save table (the engine's `save_mod`). `magic_resistance` gives advantage against spells to a combatant whose
/// stats don't already say so.
#[must_use]
pub fn save_mode(
    state: &CombatState,
    target: &CombatantId,
    ability: AbilityName,
    spell: bool,
    magic_resistance: bool,
) -> SaveMode {
    let sm = derive::save_mod(state, target, ability, spell);
    let known = state
        .combatant(target)
        .and_then(|c| c.stats.as_ref())
        .is_some_and(|s| s.magic_resistance);
    let mode = if spell && magic_resistance && !known {
        match sm.mode {
            RollMode::Disadvantage => RollMode::Normal,
            _ => RollMode::Advantage,
        }
    } else {
        sm.mode
    };
    SaveMode {
        auto_fail: sm.auto_fail,
        mode,
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
    use party_combat::{apply, Actor, Combatant, Command, Condition, EffectSpec};

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
