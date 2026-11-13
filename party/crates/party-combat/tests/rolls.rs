//! D21 as decided: the DM and the players roll in the app (the server's die) or type a table roll, for every roll.

mod common;
use common::*;
use party_combat::party_dice::{D20Input, DiceInput, RollMode, RollSource, SequenceDie};
use party_combat::*;

fn alice() -> Actor {
    Actor::Player {
        account: ALICE.into(),
    }
}

fn rolled(ev: &Events) -> Vec<(&str, i32, RollSource)> {
    ev.players
        .iter()
        .filter_map(|p| match p {
            PlayerEvent::RollResult {
                label,
                total,
                source,
                ..
            } => Some((label.as_str(), *total, *source)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_players_app_roll_uses_the_server_die_and_is_logged_as_app() {
    let mut combat = small_fight(Edition::Srd521);
    let mut die = SequenceDie::new(vec![13]);
    let cmd = Command::SetInitiative {
        id: id("w"),
        roll: D20Input::app(3, RollMode::Normal),
    };
    let ev = combat.execute_with(&alice(), &cmd, &mut die).unwrap();
    assert_eq!(c(&combat, "w").initiative, Some(16));
    assert_eq!(rolled(&ev), [("Initiative", 16, RollSource::App)]);
    let DmEventKind::Initiative { roll } = &combat.log.last().unwrap().event.kind else {
        panic!()
    };
    assert_eq!(roll.source, RollSource::App);
    // Typed from the table: logged as typed.
    let ev = combat
        .execute(
            &alice(),
            &Command::SetInitiative {
                id: id("w"),
                roll: D20Input::typed(9),
            },
        )
        .unwrap();
    assert_eq!(rolled(&ev), [("Initiative", 9, RollSource::Typed)]);
}

#[test]
fn generic_rolls_go_to_the_rollers_player_only() {
    let mut combat = small_fight(Edition::Srd521);
    let save = |who: &str, roll| Command::Roll {
        id: id(who),
        label: "Dexterity save".into(),
        roll,
    };
    let ev = scripted(
        &mut combat,
        save("w", RollInput::D20(D20Input::app(2, RollMode::Advantage))),
        &[4, 15],
    );
    assert_eq!(rolled(&ev), [("Dexterity save", 17, RollSource::App)]);
    let DmEventKind::Rolled {
        label,
        result: RollOutcome::D20(r),
    } = &ev.dm[0].kind
    else {
        panic!("{:?}", ev.dm)
    };
    assert_eq!((label.as_str(), r.total), ("Dexterity save", 17));
    // A creature's roll is the DM's: no player event.
    let ev = dm(&mut combat, save("a", RollInput::Dice(DiceInput::typed(7))));
    assert!(ev.players.is_empty());
    // Dice expressions too.
    let ev = scripted(
        &mut combat,
        save("w", RollInput::Dice(DiceInput::app("2d4+1"))),
        &[2, 3],
    );
    assert_eq!(rolled(&ev), [("Dexterity save", 6, RollSource::App)]);
}

#[test]
fn attacks_against_effective_ac() {
    let mut combat = small_fight(Edition::Srd521);
    dm(&mut combat, Command::Reveal { id: id("a") });
    let attack = |roll| Command::Attack {
        attacker: id("w"),
        target: id("a"),
        label: "Fire Bolt".into(),
        roll,
    };
    let hit_of = |ev: &Events| match &ev.dm[0].kind {
        DmEventKind::Attack {
            hit,
            critical,
            target_ac,
            ..
        } => (*hit, *critical, *target_ac),
        k => panic!("{k:?}"),
    };
    let ev = combat
        .execute(&alice(), &attack(D20Input::typed(15)))
        .unwrap();
    assert_eq!(hit_of(&ev), (Some(true), false, Some(15)), "meets AC 15");
    assert!(ev.players.iter().any(|p| matches!(
        p,
        PlayerEvent::RollResult {
            hit: Some(true),
            ..
        }
    )));
    let ev = combat
        .execute(&alice(), &attack(D20Input::typed(14)))
        .unwrap();
    assert_eq!(hit_of(&ev).0, Some(false));
    let ev = combat
        .execute(&alice(), &attack(D20Input::typed_natural(3, 1)))
        .unwrap();
    assert_eq!(hit_of(&ev).0, Some(false), "a natural 1 misses");
    let ev = combat
        .execute(&alice(), &attack(D20Input::typed_natural(9, 20)))
        .unwrap();
    assert_eq!(
        hit_of(&ev),
        (Some(true), true, Some(15)),
        "a natural 20 hits and crits"
    );
    // Effects count: Shield of Faith makes it 17.
    let mut spec = effect("Shield of Faith", "a");
    spec.modifiers = vec![Modifier::Ac(2)];
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(spec),
        },
    );
    let ev = scripted(
        &mut combat,
        attack(D20Input::app(5, RollMode::Normal)),
        &[11],
    );
    assert_eq!(hit_of(&ev), (Some(false), false, Some(17)));
}

#[test]
fn hp_from_a_formula_in_the_app_or_typed() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, owlbear("o"));
    scripted(
        &mut combat,
        Command::RollHp {
            id: id("o"),
            roll: DiceInput::app("7d10+21"),
        },
        &[5, 5, 5, 5, 5, 5, 5],
    );
    assert_eq!(
        c(&combat, "o").hp,
        Hp {
            current: 56,
            max: 56,
            temp: 0
        }
    );
    dm(
        &mut combat,
        Command::RollHp {
            id: id("o"),
            roll: DiceInput::typed(0),
        },
    );
    assert_eq!(c(&combat, "o").hp.max, 1, "at least 1");
    add(&mut combat, wizard_pc("w"));
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::RollHp {
                id: id("w"),
                roll: DiceInput::typed(9)
            }
        ),
        CombatError::Invalid(_)
    ));
}

#[test]
fn creature_recharges_are_the_dms_choice_by_default() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    let mut d = adult_red_dragon("d");
    d.abilities[1].recharge = Some(Recharge {
        min: 5,
        ready: false,
    });
    add(&mut combat, d);
    init(&mut combat, "d", 15);
    let ev = scripted(&mut combat, Command::Start, &[]);
    assert!(
        ev.dm
            .iter()
            .any(|e| matches!(&e.kind, DmEventKind::RechargeDue { .. })),
        "no automatic roll"
    );
    let cmd = Command::RollRecharge {
        id: id("d"),
        ability: "fire-breath".into(),
        roll: DiceInput::app("1d6"),
    };
    let ev = scripted(&mut combat, cmd, &[6]);
    let DmEventKind::Recharge { roll, ready, .. } = &ev.dm[0].kind else {
        panic!()
    };
    assert!(*ready);
    assert_eq!(roll.source, RollSource::App);
}

#[test]
fn death_saves_concentration_and_effect_saves_report_to_their_player() {
    let mut combat = small_fight(Edition::Srd521);
    combat
        .execute(
            &alice(),
            &Command::SetConcentration {
                id: id("w"),
                name: "Bless".into(),
            },
        )
        .unwrap();
    let sync = Command::SyncPlayerHp {
        id: id("w"),
        current: 10,
        max: 22,
        temp: 0,
        damage: Some(DamageTaken {
            amount: 12,
            critical_hit: false,
        }),
    };
    combat.execute(&Actor::System, &sync).unwrap();
    let mut die = SequenceDie::new(vec![8]);
    let cmd = Command::ResolveConcentration {
        id: id("w"),
        roll: D20Input::app(2, RollMode::Normal),
    };
    let ev = combat.execute_with(&alice(), &cmd, &mut die).unwrap();
    assert_eq!(rolled(&ev), [("Concentration save", 10, RollSource::App)]);
    let mut spec = effect("Hold Person", "w");
    spec.conditions = vec![Condition::Paralyzed];
    spec.repeat_save = Some(RepeatSave {
        ability: AbilityName::Wis,
        dc: 13,
        at: Boundary::End,
    });
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(spec),
        },
    );
    let e = combat.state.effects.last().unwrap().id;
    let ev = combat
        .execute(
            &alice(),
            &Command::ResolveEffectSave {
                effect: e,
                roll: D20Input::typed(14),
            },
        )
        .unwrap();
    assert_eq!(rolled(&ev), [("wis save", 14, RollSource::Typed)]);
}

#[test]
fn input_shapes_are_app_and_typed() {
    let v = serde_json::to_value(D20Input::app(3, RollMode::Advantage)).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"kind": "app", "modifier": 3, "mode": "advantage"})
    );
    let v = serde_json::to_value(DiceInput::app("1d6")).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"kind": "app", "notation": "1d6", "critical": false})
    );
    let v = serde_json::to_value(RollInput::D20(D20Input::typed(12))).unwrap();
    assert_eq!(
        v,
        serde_json::json!({"d20": {"kind": "typed", "total": 12}})
    );
    assert_eq!(serde_json::to_value(RollSource::App).unwrap(), "app");
}
