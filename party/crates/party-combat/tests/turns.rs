//! Initiative and turn flow: order, ties, linked initiative (C3), skipping the defeated (C4), mid-fight roster
//! changes (C7) and surprise.

mod common;
use common::*;
use party_combat::party_dice::{D20Input, RollMode};
use party_combat::*;

#[test]
fn order_is_initiative_then_bonus_then_tiebreak() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, goblin("g1")); // bonus 2
    add(&mut combat, owlbear("o")); // bonus 1
    add(&mut combat, goblin("g2")); // bonus 2, added later
    add(&mut combat, wizard_pc("w")); // bonus 3
    for k in ["g1", "o", "g2", "w"] {
        init(&mut combat, k, 14);
    }
    assert_eq!(order(&combat), ["w", "g1", "g2", "o"]);
    dm(
        &mut combat,
        Command::SwapTie {
            a: id("g1"),
            b: id("g2"),
        },
    );
    assert_eq!(order(&combat), ["w", "g2", "g1", "o"]);
    // Only true ties swap.
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::SwapTie {
                a: id("w"),
                b: id("o")
            }
        ),
        CombatError::Invalid(_)
    ));
}

#[test]
fn a_pc_without_initiative_is_not_in_the_fight() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, goblin("g"));
    add(&mut combat, wizard_pc("w"));
    add(&mut combat, fighter_pc("f"));
    init(&mut combat, "g", 12);
    init(&mut combat, "f", 9);
    assert_eq!(order(&combat), ["g", "f"]);
    init(&mut combat, "w", 20);
    dm(
        &mut combat,
        Command::SetAbsent {
            id: id("w"),
            absent: true,
        },
    );
    assert_eq!(order(&combat), ["g", "f"]);
    dm(
        &mut combat,
        Command::SetAbsent {
            id: id("w"),
            absent: false,
        },
    );
    assert_eq!(order(&combat), ["w", "g", "f"]);
}

#[test]
fn roll_initiative_rolls_creatures_only_and_is_seeded() {
    let run = || {
        let mut combat = Combat::new(Edition::Srd51, 99);
        add(&mut combat, goblin("g1"));
        add(&mut combat, goblin("g2"));
        add(&mut combat, wizard_pc("w"));
        dm(
            &mut combat,
            Command::RollInitiative {
                ids: vec![],
                only_missing: false,
            },
        );
        combat
    };
    let (a, b) = (run(), run());
    assert_eq!(a.state, b.state);
    assert!(c(&a, "g1").initiative.is_some() && c(&a, "g2").initiative.is_some());
    assert_eq!(c(&a, "w").initiative, None, "players enter their own");
}

#[test]
fn rolled_initiative_uses_the_modifier_and_advantage() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    let mut g = goblin("g");
    g.initiative_advantage = true;
    add(&mut combat, g);
    scripted(
        &mut combat,
        Command::RollInitiative {
            ids: vec![id("g")],
            only_missing: false,
        },
        &[4, 17],
    );
    assert_eq!(c(&combat, "g").initiative, Some(19));
    // only_missing keeps it
    scripted(
        &mut combat,
        Command::RollInitiative {
            ids: vec![],
            only_missing: true,
        },
        &[],
    );
    assert_eq!(c(&combat, "g").initiative, Some(19));
}

#[test]
fn surprise_2024_is_disadvantage_on_initiative() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    let mut g = goblin("g");
    g.surprised = true;
    add(&mut combat, g);
    scripted(
        &mut combat,
        Command::RollInitiative {
            ids: vec![],
            only_missing: false,
        },
        &[18, 3],
    );
    assert_eq!(c(&combat, "g").initiative, Some(5));
}

#[test]
fn surprise_2014_costs_the_first_turn() {
    let mut combat = Combat::new(Edition::Srd51, 1);
    let mut g = goblin("g");
    g.surprised = true;
    add(&mut combat, g);
    add(&mut combat, wizard_pc("w"));
    init(&mut combat, "g", 15);
    init(&mut combat, "w", 10);
    let ev = dm(&mut combat, Command::Start);
    assert!(ev.dm.iter().any(|e| e.kind == DmEventKind::Surprised));
    let e = c(&combat, "g").economy;
    assert!(e.action && e.bonus_action && e.reaction);
    dm(&mut combat, Command::NextTurn);
    dm(&mut combat, Command::NextTurn); // round 2
    assert_eq!(combat.state.round, 2);
    assert!(!c(&combat, "g").economy.action);
    assert!(!c(&combat, "g").surprised);
}

#[test]
fn typed_pc_initiative_with_its_natural_die() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, wizard_pc("w"));
    dm(
        &mut combat,
        Command::SetInitiative {
            id: id("w"),
            roll: D20Input::typed_natural(23, 20),
        },
    );
    assert_eq!(c(&combat, "w").initiative, Some(23));
    // A rolled one for a PC uses the given modifier and mode.
    scripted(
        &mut combat,
        Command::SetInitiative {
            id: id("w"),
            roll: D20Input::app(3, RollMode::Disadvantage),
        },
        &[12, 6],
    );
    assert_eq!(c(&combat, "w").initiative, Some(9));
}

#[test]
fn rounds_advance_and_turns_cycle() {
    let mut combat = small_fight(Edition::Srd521);
    assert_eq!((current(&combat), combat.state.round), ("a".into(), 1));
    dm(&mut combat, Command::NextTurn);
    assert_eq!(current(&combat), "o");
    dm(&mut combat, Command::NextTurn);
    assert_eq!(current(&combat), "w");
    dm(&mut combat, Command::NextTurn);
    assert_eq!((current(&combat), combat.state.round), ("a".into(), 2));
    dm(&mut combat, Command::PreviousTurn);
    assert_eq!((current(&combat), combat.state.round), ("w".into(), 1));
}

#[test]
fn previous_turn_stops_at_the_first_turn() {
    let mut combat = small_fight(Edition::Srd521);
    assert!(matches!(
        dm_err(&mut combat, Command::PreviousTurn),
        CombatError::Invalid(_)
    ));
}

#[test]
fn defeated_creatures_are_skipped() {
    let mut combat = small_fight(Edition::Srd521);
    dm(&mut combat, hit("o", 100, None));
    assert!(c(&combat, "o").life.is_dead());
    dm(&mut combat, Command::NextTurn);
    assert_eq!(current(&combat), "w");
    // The DM still sees it in the order, as defeated.
    assert_eq!(order(&combat), ["a", "o", "w"]);
}

#[test]
fn a_defeated_active_creature_never_restarts_the_round() {
    let mut combat = small_fight(Edition::Srd521);
    dm(&mut combat, Command::NextTurn); // owlbear's turn
    dm(&mut combat, hit("o", 100, None)); // it dies on its own turn
    dm(&mut combat, Command::NextTurn);
    assert_eq!((current(&combat), combat.state.round), ("w".into(), 1));
    dm(&mut combat, Command::NextTurn);
    assert_eq!((current(&combat), combat.state.round), ("a".into(), 2));
}

#[test]
fn a_removed_active_creature_keeps_its_place() {
    let mut combat = small_fight(Edition::Srd521);
    dm(&mut combat, Command::NextTurn); // owlbear
    dm(&mut combat, Command::RemoveCombatant { id: id("o") });
    assert_eq!(order(&combat), ["a", "w"]);
    dm(&mut combat, Command::NextTurn);
    assert_eq!((current(&combat), combat.state.round), ("w".into(), 1));
    assert!(matches!(
        dm_err(&mut combat, hit("o", 1, None)),
        CombatError::UnknownCombatant(_)
    ));
}

#[test]
fn creatures_join_mid_fight_and_are_renamed() {
    let mut combat = small_fight(Edition::Srd521);
    let mut die = party_combat::party_dice::SequenceDie::new(vec![19]);
    combat
        .execute_with(
            &Actor::Dm,
            &Command::AddCombatant {
                combatant: Box::new(goblin("b")),
                roll_initiative: true,
            },
            &mut die,
        )
        .unwrap();
    assert_eq!(c(&combat, "b").initiative, Some(21));
    assert_eq!(order(&combat), ["b", "a", "o", "w"]);
    // It acts next round: the round goes on from the goblin's place.
    dm(&mut combat, Command::NextTurn);
    assert_eq!(current(&combat), "o");
    dm(
        &mut combat,
        Command::Rename {
            id: id("b"),
            label: "Goblin boss's guard".into(),
        },
    );
    assert_eq!(c(&combat, "b").label, "Goblin boss's guard");
    assert_eq!(c(&combat, "b").public_name, "Goblin");
    let dup = Command::AddCombatant {
        combatant: Box::new(goblin("b")),
        roll_initiative: false,
    };
    assert!(matches!(
        dm_err(&mut combat, dup),
        CombatError::DuplicateCombatant(_)
    ));
}

#[test]
fn start_needs_someone_with_initiative_and_runs_once() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, goblin("g"));
    assert_eq!(
        dm_err(&mut combat, Command::Start),
        CombatError::NoInitiative
    );
    assert_eq!(
        dm_err(&mut combat, Command::NextTurn),
        CombatError::NotActive
    );
    init(&mut combat, "g", 3);
    dm(&mut combat, Command::Start);
    assert_eq!(
        dm_err(&mut combat, Command::Start),
        CombatError::AlreadyStarted
    );
    dm(&mut combat, Command::End);
    assert_eq!(dm_err(&mut combat, Command::NextTurn), CombatError::Ended);
}

/// C3: the mount acts right after its rider, at the rider's score, and on its own once the rider is down.
#[test]
fn linked_initiative_follows_the_rider_until_it_falls() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, knight("k"));
    add(&mut combat, warhorse("h"));
    add(&mut combat, wizard_pc("w"));
    add(&mut combat, goblin("g"));
    init(&mut combat, "k", 8);
    init(&mut combat, "h", 20);
    init(&mut combat, "w", 12);
    init(&mut combat, "g", 5);
    dm(
        &mut combat,
        Command::Link {
            id: id("h"),
            to: Some(id("k")),
        },
    );
    assert_eq!(order(&combat), ["w", "k", "h", "g"]);
    assert_eq!(
        derive::effective_initiative(&combat.state, c(&combat, "h")),
        Some(8)
    );
    // No loops.
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::Link {
                id: id("k"),
                to: Some(id("h"))
            }
        ),
        CombatError::Invalid(_)
    ));

    dm(&mut combat, Command::Start);
    dm(&mut combat, Command::NextTurn);
    assert_eq!(current(&combat), "k");
    let ev = dm(&mut combat, hit("k", 100, None));
    assert!(ev
        .dm
        .iter()
        .any(|e| matches!(&e.kind, DmEventKind::Unlinked { from } if from == &id("k"))));
    assert_eq!(c(&combat, "h").linked_to, None);
    // The horse is back on its own 20: it already "passed" this round, so the goblin is next.
    assert_eq!(order(&combat), ["h", "w", "k", "g"]);
    dm(&mut combat, Command::NextTurn);
    assert_eq!(current(&combat), "g");
    dm(&mut combat, Command::NextTurn);
    assert_eq!((current(&combat), combat.state.round), ("h".into(), 2));
}

#[test]
fn a_linked_follower_without_its_own_initiative_follows_its_leader() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, knight("k"));
    add(&mut combat, warhorse("h"));
    add(&mut combat, goblin("g"));
    init(&mut combat, "k", 15);
    init(&mut combat, "g", 10);
    dm(
        &mut combat,
        Command::Link {
            id: id("h"),
            to: Some(id("k")),
        },
    );
    // It acts on the knight's score while linked...
    assert_eq!(order(&combat), ["k", "h", "g"]);
    // ...and, with no score of its own, leaves the order when the link ends, until the DM rolls one.
    dm(
        &mut combat,
        Command::Link {
            id: id("h"),
            to: None,
        },
    );
    assert_eq!(order(&combat), ["k", "g"]);
    init(&mut combat, "h", 2);
    assert_eq!(order(&combat), ["k", "g", "h"]);
}

#[test]
fn two_mounts_follow_one_leader_in_the_order_added() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, knight("k"));
    add(&mut combat, warhorse("h1"));
    add(&mut combat, warhorse("h2"));
    for (k, v) in [("k", 9), ("h1", 3), ("h2", 18)] {
        init(&mut combat, k, v);
    }
    dm(
        &mut combat,
        Command::Link {
            id: id("h2"),
            to: Some(id("k")),
        },
    );
    dm(
        &mut combat,
        Command::Link {
            id: id("h1"),
            to: Some(id("k")),
        },
    );
    assert_eq!(order(&combat), ["k", "h1", "h2"]);
}
