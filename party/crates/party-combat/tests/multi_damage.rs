//! D13 (0.3.5): one damage roll on several targets from the DM's plain keypad, each with its own save, half on a
//! success; a PC gets a damage prompt as usual (D7).

mod common;
use common::*;
use party_combat::party_dice::DiceInput;
use party_combat::*;

fn fireball(targets: Vec<MultiTarget>, amount: DiceInput) -> Command {
    Command::MultiDamage {
        targets,
        amount,
        damage_type: Some(DamageType::Fire),
        save: Some(MultiSave {
            ability: AbilityName::Dex,
            dc: 15,
            half: true,
        }),
        magical: true,
        apply_qualified: false,
    }
}

fn t(who: &str, save: RowRoll) -> MultiTarget {
    MultiTarget {
        id: id(who),
        save: Some(save),
    }
}

fn typed_save(total: i32) -> RowRoll {
    RowRoll::Typed {
        total,
        natural: None,
    }
}

#[test]
fn one_roll_full_on_a_fail_half_on_a_save_and_the_pc_is_asked() {
    let mut combat = small_fight(Edition::Srd521);
    let ev = dm(
        &mut combat,
        fireball(
            vec![
                t("a", typed_save(9)),
                t("o", typed_save(17)),
                t("w", typed_save(12)),
            ],
            DiceInput::typed(25),
        ),
    );
    assert_eq!(c(&combat, "a").hp.current, 0, "the goblin failed: 25");
    assert_eq!(
        c(&combat, "o").hp.current,
        59 - 12,
        "the owlbear saved: half of 25, rounded down"
    );
    assert_eq!(
        c(&combat, "w").hp.current,
        22,
        "the PC's HP is the player's"
    );
    let asked: Vec<_> = ev
        .players
        .iter()
        .filter_map(|p| match p {
            PlayerEvent::DamagePrompt {
                id: who, amount, ..
            } => Some((who.clone(), *amount)),
            _ => None,
        })
        .collect();
    assert_eq!(
        asked,
        [(id("w"), 25)],
        "a failed save: the full amount, as a prompt"
    );
}

#[test]
fn the_app_rolls_creature_saves_once_each_and_the_amount_once() {
    let mut combat = small_fight(Edition::Srd521);
    // 4d6 = 4+4+4+4 = 16 rolled once; then goblin d20 = 20 (saves), owlbear d20 = 2 (fails).
    scripted(
        &mut combat,
        fireball(
            vec![t("a", RowRoll::App), t("o", RowRoll::App)],
            DiceInput::app("4d6"),
        ),
        &[4, 4, 4, 4, 20, 2],
    );
    assert_eq!(
        c(&combat, "a").hp.current,
        0,
        "saved: half of 16 still drops it"
    );
    assert_eq!(c(&combat, "o").hp.current, 59 - 16);
}

#[test]
fn no_half_none_on_a_save_and_no_save_hits_everyone() {
    let mut combat = small_fight(Edition::Srd521);
    let mut cmd = fireball(
        vec![t(
            "o",
            RowRoll::Outcome {
                outcome: RowOutcome::Saved,
            },
        )],
        DiceInput::typed(10),
    );
    if let Command::MultiDamage { save: Some(s), .. } = &mut cmd {
        s.half = false;
    }
    dm(&mut combat, cmd);
    assert_eq!(c(&combat, "o").hp.current, 59);
    dm(
        &mut combat,
        Command::MultiDamage {
            targets: vec![
                MultiTarget {
                    id: id("a"),
                    save: None,
                },
                MultiTarget {
                    id: id("o"),
                    save: None,
                },
            ],
            amount: DiceInput::typed(5),
            damage_type: None,
            save: None,
            magical: false,
            apply_qualified: false,
        },
    );
    assert_eq!(
        (c(&combat, "a").hp.current, c(&combat, "o").hp.current),
        (2, 54)
    );
}

#[test]
fn refusals_leave_everything_as_it_was() {
    let mut combat = small_fight(Edition::Srd521);
    // The player's save is the player's: the app doesn't roll it.
    assert!(matches!(
        dm_err(
            &mut combat,
            fireball(vec![t("w", RowRoll::App)], DiceInput::typed(5))
        ),
        CombatError::Invalid(_)
    ));
    // A save is needed for each target when there is one.
    assert!(matches!(
        dm_err(
            &mut combat,
            fireball(
                vec![MultiTarget {
                    id: id("a"),
                    save: None
                }],
                DiceInput::typed(5)
            )
        ),
        CombatError::Invalid(_)
    ));
    assert!(matches!(
        dm_err(&mut combat, fireball(vec![], DiceInput::typed(5))),
        CombatError::Invalid(_)
    ));
    assert_eq!(c(&combat, "a").hp.current, 7);
    // Players can't send it.
    let alice = Actor::Player {
        account: ALICE.into(),
    };
    assert!(combat
        .execute(
            &alice,
            &fireball(vec![t("w", typed_save(3))], DiceInput::typed(5))
        )
        .is_err());
}

#[test]
fn resistance_applies_after_the_half_and_one_undo_takes_it_all_back() {
    let mut combat = small_fight(Edition::Srd521);
    add(&mut combat, skeleton("s"));
    init(&mut combat, "s", 5);
    let before = combat.state.clone();
    let mut cmd = fireball(
        vec![t("s", typed_save(3)), t("o", typed_save(3))],
        DiceInput::typed(6),
    );
    if let Command::MultiDamage { damage_type, .. } = &mut cmd {
        *damage_type = Some(DamageType::Bludgeoning);
    }
    dm(&mut combat, cmd);
    assert_eq!(
        c(&combat, "s").hp.current,
        13 - 12,
        "vulnerable: double the 6"
    );
    combat.undo(&Actor::Dm, false).unwrap();
    assert_eq!(
        c(&combat, "s").hp.current,
        before.combatant(&id("s")).unwrap().hp.current
    );
    assert_eq!(c(&combat, "o").hp.current, 59);
}
