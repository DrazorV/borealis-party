//! HP: damage typing tables, temp HP, the 0 HP rules, the death-save machine (D18) and the PC prompts (D7).

mod common;
use common::*;
use party_combat::damage::{concentration_dc, type_damage, DefenseKind};
use party_combat::party_dice::{D20Input, DiceInput};
use party_combat::*;

fn defenses(imm: &[DamageType], res: &[DamageType], vul: &[DamageType]) -> Defenses {
    Defenses {
        immunities: vec![DamageClause::plain(imm)],
        resistances: vec![DamageClause::plain(res)],
        vulnerabilities: vec![DamageClause::plain(vul)],
        condition_immunities: vec![],
    }
}

/// Immunity, then resistance (halve, round down), then vulnerability (double).
#[test]
fn damage_typing_table() {
    use DamageType::*;
    let d = defenses(&[Poison], &[Fire, Cold], &[Cold, Radiant]);
    type Row = (Option<DamageType>, i32, i32, bool, bool, bool);
    let table: &[Row] = &[
        // type, amount, result, immune, resisted, vulnerable
        (None, 9, 9, false, false, false),
        (Some(Slashing), 9, 9, false, false, false),
        (Some(Poison), 9, 0, true, false, false),
        (Some(Fire), 9, 4, false, true, false),
        (Some(Fire), 1, 0, false, true, false),
        (Some(Radiant), 9, 18, false, false, true),
        (Some(Cold), 9, 8, false, true, true), // halved to 4, then doubled
        (Some(Cold), 0, 0, false, true, true),
    ];
    for &(t, amount, want, imm, res, vul) in table {
        let r = type_damage(&d, t, amount, false, false);
        assert_eq!(
            (r.amount, r.immune, r.resisted, r.vulnerable),
            (want, imm, res, vul),
            "{t:?} {amount}"
        );
    }
}

#[test]
fn immunity_wins_over_vulnerability_and_resistances_count_once() {
    use DamageType::*;
    let mut d = defenses(&[Fire], &[Fire], &[Fire]);
    assert_eq!(type_damage(&d, Some(Fire), 10, false, false).amount, 0);
    d.immunities.clear();
    d.vulnerabilities.clear();
    d.resistances.push(DamageClause::plain(&[Fire]));
    assert_eq!(type_damage(&d, Some(Fire), 10, false, false).amount, 5);
}

#[test]
fn nonmagical_and_qualified_clauses() {
    let w = werewolf("w");
    let slash = Some(DamageType::Slashing);
    let r = type_damage(&w.defenses, slash, 12, false, false);
    assert!(r.immune);
    assert_eq!(r.clauses.len(), 1);
    assert_eq!(r.clauses[0].kind, DefenseKind::Immunity);
    assert_eq!(r.clauses[0].scope, ClauseScope::Nonmagical);
    assert!(
        r.clauses[0].text.contains("silvered"),
        "the DM sees the clause text"
    );
    let magic = type_damage(&w.defenses, slash, 12, true, false);
    assert!(!magic.immune);
    assert_eq!(magic.amount, 12);
    assert!(!magic.clauses[0].applied);

    let d = Defenses {
        resistances: vec![DamageClause::qualified(
            &[DamageType::Fire],
            "while in its lair",
        )],
        ..Defenses::default()
    };
    assert_eq!(
        type_damage(&d, Some(DamageType::Fire), 10, false, false).amount,
        10
    );
    assert_eq!(
        type_damage(&d, Some(DamageType::Fire), 10, false, true).amount,
        5
    );
}

#[test]
fn concentration_dc_table() {
    for (dmg, dc2014, dc2024) in [
        (0, 10, 10),
        (1, 10, 10),
        (21, 10, 10),
        (22, 11, 11),
        (45, 22, 22),
        (60, 30, 30),
        (61, 30, 30),
        (62, 31, 30),
        (200, 100, 30),
    ] {
        assert_eq!(concentration_dc(Edition::Srd51, dmg), dc2014, "2014 {dmg}");
        assert_eq!(concentration_dc(Edition::Srd521, dmg), dc2024, "2024 {dmg}");
    }
}

#[test]
fn damage_parts_are_typed_separately_and_rolled_with_the_server_die() {
    let mut combat = Combat::new(Edition::Srd51, 1);
    add(&mut combat, skeleton("s"));
    let cmd = Command::Damage {
        target: id("s"),
        parts: vec![
            DamagePart {
                amount: DiceInput::roll("1d6"),
                damage_type: Some(DamageType::Bludgeoning),
            },
            DamagePart::typed(5, Some(DamageType::Poison)),
        ],
        magical: false,
        apply_qualified: false,
        critical_hit: false,
    };
    let ev = scripted(&mut combat, cmd, &[3]);
    let DmEventKind::Damage { result, prompted } = &ev.dm[0].kind else {
        panic!("{:?}", ev.dm)
    };
    assert!(!prompted);
    assert_eq!(result.total, 6);
    assert_eq!(result.parts[0].roll.breakdown, "1d6 [3] = 3");
    assert!(result.parts[1].immune);
    assert_eq!(c(&combat, "s").hp.current, 7);
}

#[test]
fn temp_hp_absorbs_first_and_does_not_stack() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, owlbear("o"));
    dm(
        &mut combat,
        Command::GrantTempHp {
            id: id("o"),
            amount: 8,
        },
    );
    dm(
        &mut combat,
        Command::GrantTempHp {
            id: id("o"),
            amount: 5,
        },
    );
    assert_eq!(c(&combat, "o").hp.temp, 8);
    dm(&mut combat, hit("o", 10, None));
    assert_eq!(
        (c(&combat, "o").hp.temp, c(&combat, "o").hp.current),
        (0, 57)
    );
}

#[test]
fn heal_set_exact_and_the_dead() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, owlbear("o"));
    dm(&mut combat, hit("o", 30, None));
    dm(
        &mut combat,
        Command::Heal {
            target: id("o"),
            amount: typed(100),
        },
    );
    assert_eq!(c(&combat, "o").hp.current, 59);
    dm(
        &mut combat,
        Command::SetHp {
            id: id("o"),
            current: 12,
            max: None,
            temp: None,
        },
    );
    assert_eq!(c(&combat, "o").hp.current, 12);
    dm(
        &mut combat,
        Command::SetHp {
            id: id("o"),
            current: 0,
            max: None,
            temp: None,
        },
    );
    assert!(c(&combat, "o").life.is_dead());
    assert_eq!(
        dm_err(
            &mut combat,
            Command::Heal {
                target: id("o"),
                amount: typed(5)
            }
        ),
        CombatError::Dead
    );
    dm(
        &mut combat,
        Command::SetHp {
            id: id("o"),
            current: 20,
            max: Some(70),
            temp: None,
        },
    );
    assert!(c(&combat, "o").life.is_alive());
    assert_eq!(
        c(&combat, "o").hp,
        Hp {
            current: 20,
            max: 70,
            temp: 0
        }
    );
    assert!(matches!(
        dm_err(&mut combat, hit("o", -3, None)),
        CombatError::Invalid(_)
    ));
}

#[test]
fn a_monster_dies_at_zero() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, goblin("g"));
    let ev = dm(&mut combat, hit("g", 7, Some(DamageType::Slashing)));
    assert!(c(&combat, "g").life.is_dead());
    assert!(ev.dm.iter().any(|e| e.kind == DmEventKind::Died));
}

fn dying_pc() -> Combat {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, engine_pc("p"));
    dm(&mut combat, hit("p", 22, None));
    combat
}

fn life(combat: &Combat) -> Life {
    c(combat, "p").life
}

#[test]
fn dropping_to_zero_starts_dying_and_unconscious() {
    let combat = dying_pc();
    assert_eq!(
        life(&combat),
        Life::Dying {
            successes: 0,
            failures: 0
        }
    );
    assert!(derive::has_condition(
        &combat.state,
        &id("p"),
        Condition::Unconscious
    ));
}

#[test]
fn death_save_machine() {
    let save = |n: u32| Command::DeathSave {
        id: id("p"),
        roll: D20Input::typed_natural(n as i32, n),
    };
    // three successes: stable, still unconscious
    let mut combat = dying_pc();
    for n in [10, 15, 19] {
        dm(&mut combat, save(n));
    }
    assert_eq!(life(&combat), Life::Stable);
    assert!(derive::has_condition(
        &combat.state,
        &id("p"),
        Condition::Unconscious
    ));
    assert_eq!(dm_err(&mut combat, save(12)), CombatError::NotDying);

    // three failures: dead
    let mut combat = dying_pc();
    for n in [9, 2] {
        dm(&mut combat, save(n));
    }
    assert_eq!(
        life(&combat),
        Life::Dying {
            successes: 0,
            failures: 2
        }
    );
    dm(&mut combat, save(5));
    assert_eq!(life(&combat), Life::Dead);

    // a natural 1 is two failures
    let mut combat = dying_pc();
    dm(&mut combat, save(1));
    assert_eq!(
        life(&combat),
        Life::Dying {
            successes: 0,
            failures: 2
        }
    );

    // a natural 20: 1 HP and conscious
    let mut combat = dying_pc();
    dm(&mut combat, save(20));
    assert_eq!(life(&combat), Life::Alive);
    assert_eq!(c(&combat, "p").hp.current, 1);
    assert!(!derive::has_condition(
        &combat.state,
        &id("p"),
        Condition::Unconscious
    ));
}

#[test]
fn a_typed_total_without_its_natural_counts_20_and_1() {
    let mut combat = dying_pc();
    dm(
        &mut combat,
        Command::DeathSave {
            id: id("p"),
            roll: D20Input::typed(1),
        },
    );
    assert_eq!(
        life(&combat),
        Life::Dying {
            successes: 0,
            failures: 2
        }
    );
    dm(
        &mut combat,
        Command::DeathSave {
            id: id("p"),
            roll: D20Input::typed(20),
        },
    );
    assert_eq!(life(&combat), Life::Alive);
    // A rolled save uses the server die.
    let mut combat = dying_pc();
    scripted(
        &mut combat,
        Command::DeathSave {
            id: id("p"),
            roll: D20Input::roll(0, Default::default()),
        },
        &[11],
    );
    assert_eq!(
        life(&combat),
        Life::Dying {
            successes: 1,
            failures: 0
        }
    );
}

#[test]
fn damage_at_zero_hp_is_a_failure_and_a_crit_two() {
    let mut combat = dying_pc();
    dm(&mut combat, hit("p", 3, None));
    assert_eq!(
        life(&combat),
        Life::Dying {
            successes: 0,
            failures: 1
        }
    );
    let crit = Command::Damage {
        target: id("p"),
        parts: vec![DamagePart::typed(3, None)],
        magical: false,
        apply_qualified: false,
        critical_hit: true,
    };
    dm(&mut combat, crit);
    assert_eq!(life(&combat), Life::Dead);
}

#[test]
fn damage_to_a_stable_creature_starts_dying_again() {
    let mut combat = dying_pc();
    dm(&mut combat, Command::Stabilize { id: id("p") });
    assert_eq!(life(&combat), Life::Stable);
    dm(&mut combat, hit("p", 2, None));
    assert_eq!(
        life(&combat),
        Life::Dying {
            successes: 0,
            failures: 1
        }
    );
}

#[test]
fn massive_damage_kills_outright() {
    let mut combat = Combat::new(Edition::Srd51, 1);
    add(&mut combat, engine_pc("p")); // 22 max
    dm(&mut combat, hit("p", 44, None)); // 22 left over = max
    assert_eq!(life(&combat), Life::Dead);
    let mut combat = Combat::new(Edition::Srd51, 1);
    add(&mut combat, engine_pc("p"));
    dm(&mut combat, hit("p", 43, None));
    assert!(matches!(life(&combat), Life::Dying { .. }));
    dm(&mut combat, hit("p", 22, None)); // at 0 HP, damage of max HP or more
    assert_eq!(life(&combat), Life::Dead);
}

#[test]
fn healing_a_dying_creature_resets_its_saves() {
    let mut combat = dying_pc();
    dm(
        &mut combat,
        Command::DeathSave {
            id: id("p"),
            roll: D20Input::typed_natural(4, 4),
        },
    );
    dm(
        &mut combat,
        Command::Heal {
            target: id("p"),
            amount: typed(5),
        },
    );
    assert_eq!(
        (life(&combat), c(&combat, "p").hp.current),
        (Life::Alive, 5)
    );
    dm(&mut combat, hit("p", 5, None));
    assert_eq!(
        life(&combat),
        Life::Dying {
            successes: 0,
            failures: 0
        }
    );
}

#[test]
fn set_death_saves_corrects_the_counts() {
    let mut combat = dying_pc();
    dm(
        &mut combat,
        Command::SetDeathSaves {
            id: id("p"),
            successes: 2,
            failures: 1,
        },
    );
    assert_eq!(
        life(&combat),
        Life::Dying {
            successes: 2,
            failures: 1
        }
    );
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::SetDeathSaves {
                id: id("p"),
                successes: 3,
                failures: 0
            }
        ),
        CombatError::Invalid(_)
    ));
}

/// D7: damage on a player's PC is a prompt to its owner; the PC's HP doesn't change until the server syncs it.
#[test]
fn pc_damage_is_a_prompt_and_the_sync_applies_the_rules() {
    let mut combat = small_fight(Edition::Srd521);
    let ev = dm(&mut combat, hit("w", 7, Some(DamageType::Slashing)));
    assert_eq!(c(&combat, "w").hp.current, 22, "not written by the DM");
    let prompt = ev.players.iter().find_map(|p| match p {
        PlayerEvent::DamagePrompt {
            id: who,
            owner,
            amount,
            ..
        } => Some((who.clone(), owner.clone(), *amount)),
        _ => None,
    });
    assert_eq!(prompt, Some((id("w"), ALICE.to_string(), 7)));
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::SetHp {
                id: id("w"),
                current: 3,
                max: None,
                temp: None
            }
        ),
        CombatError::Invalid(_)
    ));

    // The player applies it; the server reports the sheet's HP back.
    dm(
        &mut combat,
        Command::SetConcentration {
            id: id("w"),
            name: "Bless".into(),
        },
    );
    let sync = Command::SyncPlayerHp {
        id: id("w"),
        current: 15,
        max: 22,
        temp: 0,
        damage: Some(DamageTaken {
            amount: 7,
            critical_hit: false,
        }),
    };
    let ev = combat.execute(&Actor::System, &sync).unwrap();
    assert_eq!(c(&combat, "w").hp.current, 15);
    assert_eq!(
        c(&combat, "w").pending_concentration,
        vec![ConcentrationCheck { dc: 10, damage: 7 }]
    );
    assert!(ev
        .players
        .iter()
        .any(|p| matches!(p, PlayerEvent::ConcentrationCheck { dc: 10, .. })));

    // Down to 0 by the sync: dying, concentration ends.
    let sync = Command::SyncPlayerHp {
        id: id("w"),
        current: 0,
        max: 22,
        temp: 0,
        damage: Some(DamageTaken {
            amount: 20,
            critical_hit: false,
        }),
    };
    combat.execute(&Actor::System, &sync).unwrap();
    assert_eq!(
        c(&combat, "w").life,
        Life::Dying {
            successes: 0,
            failures: 0
        }
    );
    assert_eq!(c(&combat, "w").concentration, None);
    // More damage at 0, synced with the crit flag: two failures.
    let sync = Command::SyncPlayerHp {
        id: id("w"),
        current: 0,
        max: 22,
        temp: 0,
        damage: Some(DamageTaken {
            amount: 4,
            critical_hit: true,
        }),
    };
    combat.execute(&Actor::System, &sync).unwrap();
    assert_eq!(
        c(&combat, "w").life,
        Life::Dying {
            successes: 0,
            failures: 2
        }
    );
    // Healing on the Play tab, synced without damage: back up.
    let sync = Command::SyncPlayerHp {
        id: id("w"),
        current: 6,
        max: 22,
        temp: 0,
        damage: None,
    };
    combat.execute(&Actor::System, &sync).unwrap();
    assert_eq!(c(&combat, "w").life, Life::Alive);
}

#[test]
fn pc_heal_and_a_natural_20_are_prompts() {
    let mut combat = small_fight(Edition::Srd521);
    let ev = dm(
        &mut combat,
        Command::Heal {
            target: id("w"),
            amount: typed(4),
        },
    );
    assert!(ev
        .players
        .iter()
        .any(|p| matches!(p, PlayerEvent::HealPrompt { amount: 4, .. })));
    let sync = Command::SyncPlayerHp {
        id: id("w"),
        current: 0,
        max: 22,
        temp: 0,
        damage: Some(DamageTaken {
            amount: 22,
            critical_hit: false,
        }),
    };
    combat.execute(&Actor::System, &sync).unwrap();
    let alice = Actor::Player {
        account: ALICE.into(),
    };
    let ev = combat
        .execute(
            &alice,
            &Command::DeathSave {
                id: id("w"),
                roll: D20Input::typed_natural(20, 20),
            },
        )
        .unwrap();
    assert!(ev
        .players
        .iter()
        .any(|p| matches!(p, PlayerEvent::HealPrompt { amount: 1, .. })));
    assert_eq!(c(&combat, "w").life, Life::Alive);
}

#[test]
fn sync_is_for_player_pcs_only() {
    let mut combat = small_fight(Edition::Srd521);
    let sync = Command::SyncPlayerHp {
        id: id("a"),
        current: 1,
        max: 7,
        temp: 0,
        damage: None,
    };
    assert!(matches!(
        combat.execute(&Actor::System, &sync),
        Err(CombatError::Invalid(_))
    ));
}
