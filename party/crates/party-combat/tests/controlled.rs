//! Controlled creatures: summons, pets and companions of PCs and villains (4.4 "Controlled creatures").

mod common;
use common::*;
use party_combat::party_dice::{D20Input, DiceInput};
use party_combat::*;

fn alice() -> Actor {
    Actor::Player {
        account: ALICE.into(),
    }
}

fn bob() -> Actor {
    Actor::Player {
        account: BOB.into(),
    }
}

fn summon(owner: &str, c: Combatant) -> Command {
    Command::Summon {
        owner: id(owner),
        combatant: Box::new(c),
        initiative: None,
        visible: None,
    }
}

fn wolf(key: &str) -> Combatant {
    let mut c = Combatant::creature(key, "Wolf", 11, 13, 2);
    c.speed = Speeds {
        walk: 40,
        ..Speeds::default()
    };
    c
}

/// Wizard w (15) with an owl familiar and the owl's own summon; a hidden mage m (12) with two hidden wolves; a
/// goblin g (8); the fighter f (5) with a wolf.
fn fight() -> Combat {
    let mut combat = Combat::new(Edition::Srd521, 9);
    add(&mut combat, wizard_pc("w"));
    add(&mut combat, mage("m"));
    add(&mut combat, goblin("g"));
    add(&mut combat, fighter_pc("f"));
    for (k, v) in [("w", 15), ("m", 12), ("g", 8), ("f", 5)] {
        init(&mut combat, k, v);
    }
    combat
        .execute(
            &alice(),
            &summon("w", Combatant::creature("owl", "Owl", 1, 11, 3)),
        )
        .unwrap();
    combat
        .execute(
            &alice(),
            &summon("owl", Combatant::creature("mote", "Mote", 1, 10, 0)),
        )
        .unwrap();
    dm(&mut combat, summon("m", wolf("p1")));
    dm(&mut combat, summon("m", wolf("p2")));
    combat.execute(&bob(), &summon("f", wolf("fw"))).unwrap();
    combat
}

#[test]
fn controlled_creatures_act_right_after_their_owner_nested() {
    let mut combat = fight();
    assert_eq!(
        order(&combat),
        ["w", "owl", "mote", "m", "p1", "p2", "g", "f", "fw"]
    );
    assert_eq!(
        c(&combat, "owl").controller,
        Controller::Player { pc: id("w") }
    );
    assert_eq!(
        c(&combat, "mote").controller,
        Controller::Player { pc: id("w") },
        "nested: still the wizard's player"
    );
    assert_eq!(c(&combat, "p1").controller, Controller::Dm);
    // The DM reorders the sub-order of one owner's creatures.
    dm(
        &mut combat,
        Command::SwapTie {
            a: id("p1"),
            b: id("p2"),
        },
    );
    assert_eq!(
        order(&combat),
        ["w", "owl", "mote", "m", "p2", "p1", "g", "f", "fw"]
    );
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::SwapTie {
                a: id("p1"),
                b: id("owl")
            }
        ),
        CombatError::Invalid(_)
    ));
    // One wolf switches to its own initiative: it needs a roll, typed or in the app.
    let own = |roll| Command::SetInitiativeMode {
        id: id("p1"),
        mode: InitiativeMode::Own,
        roll,
    };
    assert!(matches!(
        dm_err(&mut combat, own(None)),
        CombatError::Invalid(_)
    ));
    dm(&mut combat, own(Some(D20Input::typed(20))));
    assert_eq!(
        order(&combat),
        ["p1", "w", "owl", "mote", "m", "p2", "g", "f", "fw"]
    );
    dm(
        &mut combat,
        Command::SetInitiativeMode {
            id: id("p1"),
            mode: InitiativeMode::AfterOwner,
            roll: None,
        },
    );
    assert_eq!(
        order(&combat),
        ["w", "owl", "mote", "m", "p2", "p1", "g", "f", "fw"]
    );
    // A summon with its own initiative from the start.
    let mut die = party_dice::SequenceDie::new(vec![1]);
    let cmd = Command::Summon {
        owner: id("g"),
        combatant: Box::new(goblin("g2")),
        initiative: Some(D20Input::app(2, Default::default())),
        visible: None,
    };
    combat.execute_with(&Actor::Dm, &cmd, &mut die).unwrap();
    assert_eq!(c(&combat, "g2").initiative, Some(3));
    assert_eq!(order(&combat).last().map(String::as_str), Some("g2"));
    // No loops through owners and links.
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::Link {
                id: id("w"),
                to: Some(id("mote"))
            }
        ),
        CombatError::Invalid(_)
    ));
}

#[test]
fn turns_go_through_controlled_creatures_and_tell_their_player() {
    let mut combat = fight();
    dm(&mut combat, Command::Start);
    assert_eq!(current(&combat), "w");
    let ev = dm(&mut combat, Command::NextTurn);
    assert_eq!(current(&combat), "owl");
    assert!(ev.players.iter().any(
        |p| matches!(p, PlayerEvent::YourTurn { id: i, owner } if i == &id("owl") && owner == ALICE)
    ));
    dm(&mut combat, Command::NextTurn);
    assert_eq!(current(&combat), "mote");
    // The hidden mage and its hidden wolves: three hidden turns, no player events, the pointer stays on the mote.
    for _ in 0..3 {
        let ev = dm(&mut combat, Command::NextTurn);
        assert!(ev.players.is_empty(), "{:?}", ev.players);
    }
    assert_eq!(current(&combat), "p2");
    assert_eq!(
        turn_pointer(&combat.state, ViewerKind::Player),
        TurnPointer::Combatant {
            id: id("mote"),
            round: 1
        }
    );
}

#[test]
fn the_player_acts_only_for_their_own_pc_and_its_creatures() {
    let mut combat = fight();
    dm(&mut combat, Command::Reveal { id: id("g") });
    let may =
        |combat: &mut Combat, actor: &Actor, cmd: Command| combat.execute(actor, &cmd).is_ok();
    let effect_on = |target: &str| {
        let mut s = effect("", target);
        s.conditions = vec![Condition::Prone];
        Command::AddCondition {
            effect: Box::new(s),
        }
    };
    // Allowed: her creatures' HP, temp HP, conditions, effects, rolls and attacks.
    assert!(may(&mut combat, &alice(), hit("owl", 0, None)));
    assert!(may(
        &mut combat,
        &alice(),
        Command::GrantTempHp {
            id: id("owl"),
            amount: 3
        }
    ));
    assert!(may(
        &mut combat,
        &alice(),
        Command::Heal {
            target: id("owl"),
            amount: DiceInput::typed(1)
        }
    ));
    assert!(may(
        &mut combat,
        &alice(),
        Command::SetHp {
            id: id("mote"),
            current: 1,
            max: None,
            temp: None
        }
    ));
    assert!(may(&mut combat, &alice(), effect_on("owl")));
    assert!(may(
        &mut combat,
        &alice(),
        Command::RemoveCondition {
            target: id("owl"),
            condition: Condition::Prone
        }
    ));
    let roll = Command::Roll {
        id: id("owl"),
        label: "Perception".into(),
        roll: RollInput::D20(D20Input::typed(14)),
    };
    assert!(may(&mut combat, &alice(), roll));
    let atk = |attacker: &str, target: &str| Command::Attack {
        attacker: id(attacker),
        target: id(target),
        label: "Talons".into(),
        roll: D20Input::typed(12),
    };
    assert!(may(&mut combat, &alice(), atk("owl", "g")));
    assert!(may(&mut combat, &alice(), atk("w", "g")));
    // Refused: her own PC's HP (D7), others' creatures, the DM's creatures, hidden targets, DM commands.
    for cmd in [
        hit("w", 3, None),
        hit("fw", 3, None),
        hit("p1", 3, None),
        effect_on("fw"),
        summon("m", wolf("x")),
        summon("f", wolf("x")),
        Command::Dismiss { id: id("fw") },
        Command::Dismiss { id: id("p1") },
        atk("owl", "m"), // hidden
        atk("fw", "g"),
        Command::Hide { id: id("owl") },
        Command::NextTurn,
        Command::Rename {
            id: id("owl"),
            label: "x".into(),
        },
    ] {
        assert_eq!(
            combat.execute(&alice(), &cmd),
            Err(CombatError::NotAllowed),
            "{cmd:?}"
        );
    }
    // A hidden target is refused like a made-up id: nothing tells the player it exists.
    assert_eq!(
        combat.execute(&alice(), &atk("owl", "nobody")),
        Err(CombatError::NotAllowed)
    );
    // Bob can't touch Alice's familiar; Alice dismisses it herself.
    assert_eq!(
        combat.execute(&bob(), &hit("owl", 1, None)),
        Err(CombatError::NotAllowed)
    );
    assert!(may(
        &mut combat,
        &alice(),
        Command::Dismiss { id: id("mote") }
    ));
    assert!(c(&combat, "mote").removed);
    // The DM overrides anything.
    dm(&mut combat, hit("owl", 1, None));
    dm(&mut combat, Command::Dismiss { id: id("fw") });
}

#[test]
fn a_summons_hp_lives_in_combat_state() {
    let mut combat = fight();
    let ev = dm(&mut combat, hit("owl", 1, None));
    assert!(c(&combat, "owl").life.is_dead());
    assert!(
        !ev.players
            .iter()
            .any(|p| matches!(p, PlayerEvent::DamagePrompt { .. })),
        "prompts are for PCs only"
    );
    assert!(ev.players.contains(&PlayerEvent::Changed { id: id("owl") }));
}

#[test]
fn pets_of_hidden_villains_stay_hidden() {
    let mut combat = fight();
    let players: Vec<String> = visible_order(&combat.state, ViewerKind::Player)
        .into_iter()
        .map(|c| c.0)
        .collect();
    assert_eq!(players, ["w", "owl", "mote", "f", "fw"]);
    // Revealing the owner reveals no pets.
    dm(&mut combat, Command::Reveal { id: id("m") });
    assert!(!is_visible(&combat.state, &id("p1"), ViewerKind::Player));
    // A pet summoned while its owner is visible starts visible; one can be revealed while its owner is hidden.
    dm(&mut combat, summon("m", wolf("p3")));
    assert!(is_visible(&combat.state, &id("p3"), ViewerKind::Player));
    dm(&mut combat, Command::Hide { id: id("m") });
    dm(&mut combat, Command::Reveal { id: id("p1") });
    assert!(is_visible(&combat.state, &id("p1"), ViewerKind::Player));
    // Its player view doesn't point at the hidden owner.
    let view = player_view_of(&combat.state, &id("p1")).unwrap();
    assert!(view.get("owner_id").is_none(), "{view}");
    assert!(!view.to_string().contains("\"m\""), "{view}");
    assert!(player_view_of(&combat.state, &id("p2")).is_none());
    // A PC's creatures are always visible.
    let cmd = Command::Summon {
        owner: id("w"),
        combatant: Box::new(wolf("w2")),
        initiative: None,
        visible: Some(false),
    };
    dm(&mut combat, cmd);
    assert!(is_visible(&combat.state, &id("w2"), ViewerKind::Player));
    assert!(matches!(
        dm_err(&mut combat, Command::Hide { id: id("w2") }),
        CombatError::Invalid(_)
    ));
}

#[test]
fn a_hidden_villains_hidden_pet_leaves_no_trace() {
    let mut combat = fight();
    dm(&mut combat, Command::Start);
    let quiet = [
        summon("m", wolf("p4")),
        hit("p1", 3, Some(DamageType::Piercing)),
        Command::Dismiss { id: id("p4") },
        Command::SetInitiativeMode {
            id: id("p2"),
            mode: InitiativeMode::Own,
            roll: Some(D20Input::typed(30)),
        },
        hit("m", 100, None),
    ];
    for cmd in quiet {
        let ev = dm(&mut combat, cmd.clone());
        assert!(ev.players.is_empty(), "{cmd:?} -> {:?}", ev.players);
    }
    for i in ["m", "p1", "p2", "p4"] {
        assert!(player_view_of(&combat.state, &id(i)).is_none());
    }
}

/// When an owner goes down, the DM gets a prompt for each of its creatures; nothing changes on its own.
#[test]
fn owner_defeated_is_a_dm_prompt() {
    let mut combat = fight();
    dm(&mut combat, Command::Start);
    let ev = dm(&mut combat, hit("m", 100, None));
    let prompted: Vec<&CombatantId> = ev
        .dm
        .iter()
        .filter_map(|e| match &e.kind {
            DmEventKind::OwnerDefeated { owner } if owner == &id("m") => e.subject.as_ref(),
            _ => None,
        })
        .collect();
    assert_eq!(prompted, [&id("p1"), &id("p2")]);
    assert!(ev.players.is_empty());
    // Kept after the dead owner, still taking turns.
    assert_eq!(
        order(&combat),
        ["w", "owl", "mote", "m", "p1", "p2", "g", "f", "fw"]
    );
    for _ in 0..3 {
        dm(&mut combat, Command::NextTurn);
    }
    assert_eq!(
        current(&combat),
        "p1",
        "the dead mage is skipped, its wolves are not"
    );
    // A dismissed owner prompts too.
    let ev = combat
        .execute(&alice(), &Command::Dismiss { id: id("owl") })
        .unwrap();
    assert!(ev
        .dm
        .iter()
        .any(|e| matches!(&e.kind, DmEventKind::OwnerDefeated { owner } if owner == &id("owl"))));
    assert!(!c(&combat, "mote").removed);
}

#[test]
fn undo_across_summon_and_dismiss() {
    let mut combat = fight();
    let before = combat.state.clone();
    combat.execute(&alice(), &summon("w", wolf("w3"))).unwrap();
    let after_summon = combat.state.clone();
    combat.undo(&Actor::Dm, true).unwrap();
    assert_eq!(combat.state, before);
    combat.execute(&alice(), &summon("w", wolf("w3"))).unwrap();
    assert_eq!(combat.state, after_summon);
    dm(&mut combat, Command::Dismiss { id: id("w3") });
    combat.undo(&Actor::Dm, false).unwrap();
    assert_eq!(combat.state, after_summon);
    // Dismiss is for controlled creatures; others are removed.
    assert!(matches!(
        dm_err(&mut combat, Command::Dismiss { id: id("g") }),
        CombatError::Invalid(_)
    ));
    assert!(matches!(
        dm_err(&mut combat, summon("w", wizard_pc("pc2"))),
        CombatError::Invalid(_)
    ));
}

#[test]
fn import_checks_owners_and_controllers() {
    let combat = fight();
    let mut v: serde_json::Value = serde_json::from_str(&export_json(&combat)).unwrap();
    let pets = v["state"]["combatants"].as_array_mut().unwrap();
    let owl = pets.iter_mut().find(|c| c["id"] == "owl").unwrap();
    owl["owner_id"] = serde_json::json!("ghost");
    assert!(
        matches!(import_json(&v.to_string(), 1), Err(CombatError::Import(m)) if m.contains("unknown owner"))
    );
    let mut v: serde_json::Value = serde_json::from_str(&export_json(&combat)).unwrap();
    let pets = v["state"]["combatants"].as_array_mut().unwrap();
    let owl = pets.iter_mut().find(|c| c["id"] == "owl").unwrap();
    owl["controller"] = serde_json::json!({"kind": "player", "pc": "g"});
    assert!(import_json(&v.to_string(), 1).is_err());
    assert_eq!(
        import_json(&export_json(&combat), 1).unwrap().state,
        combat.state
    );
}
