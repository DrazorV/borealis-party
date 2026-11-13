//! The combat wrapper: undo (C8), the DM-only log, who may do what, export and import (D45), and what players
//! may see (D5).

mod common;
use common::*;
use party_combat::party_dice::D20Input;
use party_combat::*;

#[test]
fn undo_returns_the_exact_previous_state() {
    let mut combat = small_fight(Edition::Srd521);
    let before = combat.state.clone();
    dm(&mut combat, hit("o", 20, Some(DamageType::Slashing)));
    let mid = combat.state.clone();
    dm(&mut combat, Command::NextTurn);
    assert_ne!(combat.state, mid);
    let ev = combat.undo(&Actor::Dm, false).unwrap();
    assert_eq!(combat.state, mid);
    assert!(matches!(&ev.dm[0].kind, DmEventKind::Undone { label } if label == "next turn"));
    combat.undo(&Actor::Dm, false).unwrap();
    assert_eq!(combat.state, before);
}

#[test]
fn undo_keeps_fifty_steps_and_never_replays_a_roll() {
    let mut combat = small_fight(Edition::Srd521);
    for _ in 0..60 {
        dm(&mut combat, Command::NextTurn);
    }
    assert_eq!(combat.journal.len(), UNDO_LIMIT);
    for _ in 0..UNDO_LIMIT {
        combat.undo(&Actor::Dm, false).unwrap();
    }
    assert_eq!(
        combat.undo(&Actor::Dm, false),
        Err(CombatError::NothingToUndo)
    );

    // The die is outside the state: a roll after an undo is a fresh roll.
    let mut combat = Combat::new(Edition::Srd521, 3);
    add(&mut combat, goblin("g"));
    let roll = Command::RollInitiative {
        ids: vec![],
        only_missing: false,
    };
    let rolls: Vec<Option<i32>> = (0..6)
        .map(|_| {
            dm(&mut combat, roll.clone());
            let v = c(&combat, "g").initiative;
            combat.undo(&Actor::Dm, false).unwrap();
            v
        })
        .collect();
    assert!(rolls.windows(2).any(|w| w[0] != w[1]), "{rolls:?}");
}

#[test]
fn only_the_dm_undoes_and_a_players_write_needs_force() {
    let mut combat = small_fight(Edition::Srd521);
    let alice = Actor::Player {
        account: ALICE.into(),
    };
    combat
        .execute(
            &alice,
            &Command::SetInitiative {
                id: id("w"),
                roll: D20Input::typed(18),
            },
        )
        .unwrap();
    assert_eq!(combat.undo(&alice, false), Err(CombatError::NotAllowed));
    assert_eq!(
        combat.undo(&Actor::Dm, false),
        Err(CombatError::UndoWouldRevertPlayer)
    );
    combat.undo(&Actor::Dm, true).unwrap();
    assert_eq!(c(&combat, "w").initiative, Some(10));
}

#[test]
fn players_act_only_on_their_own_pc() {
    let mut combat = small_fight(Edition::Srd521);
    add(&mut combat, fighter_pc("f"));
    let alice = Actor::Player {
        account: ALICE.into(),
    };
    let set = |who: &str| Command::SetInitiative {
        id: id(who),
        roll: D20Input::typed(12),
    };
    combat.execute(&alice, &set("w")).unwrap();
    assert_eq!(
        combat.execute(&alice, &set("f")),
        Err(CombatError::NotAllowed)
    );
    assert_eq!(
        combat.execute(&alice, &set("a")),
        Err(CombatError::NotAllowed)
    );
    assert_eq!(
        combat.execute(&alice, &hit("w", 1, None)),
        Err(CombatError::NotAllowed)
    );
    assert_eq!(
        combat.execute(&alice, &Command::NextTurn),
        Err(CombatError::NotAllowed)
    );
    assert_eq!(
        combat.execute(&alice, &set("nobody")),
        Err(CombatError::NotAllowed)
    );
    combat
        .execute(
            &alice,
            &Command::SetConcentration {
                id: id("w"),
                name: "Bless".into(),
            },
        )
        .unwrap();
    combat
        .execute(&alice, &Command::EndConcentration { id: id("w") })
        .unwrap();
}

#[test]
fn a_refused_command_changes_nothing() {
    let mut combat = small_fight(Edition::Srd521);
    let (state, seq, log) = (combat.state.clone(), combat.seq, combat.log.len());
    assert!(combat.execute(&Actor::Dm, &hit("nobody", 5, None)).is_err());
    assert_eq!(
        (combat.state.clone(), combat.seq, combat.log.len()),
        (state, seq, log)
    );
}

#[test]
fn the_log_keeps_dm_events_with_their_write() {
    let mut combat = small_fight(Edition::Srd521);
    let seq = combat.seq;
    dm(&mut combat, hit("o", 5, None));
    let last = combat.log.last().unwrap();
    assert_eq!(last.seq, seq + 1);
    assert!(
        last.event.text.contains("Owlbear takes 5"),
        "{}",
        last.event.text
    );
    assert_eq!(LOG_RETENTION_DAYS, 90);
}

#[test]
fn json_round_trip_with_a_schema_version() {
    let mut combat = small_fight(Edition::Srd51);
    let mut spec = effect("Bless", "w");
    spec.source = Some(id("w"));
    spec.concentration = true;
    spec.duration = Duration::SourceTurns {
        remaining: 10,
        at: Boundary::Start,
    };
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(spec),
        },
    );
    dm(&mut combat, hit("o", 9, Some(DamageType::Fire)));
    let text = export_json(&combat);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["format"], "borealis-party-combat");
    assert_eq!(value["schema_version"], SCHEMA_VERSION);
    assert_eq!(value["state"]["edition"], "2014");

    let back = import_json(&text, 42).unwrap();
    assert_eq!(back.state, combat.state);
    assert_eq!(back.log, combat.log);
    // A new combat: no undo history, its own die and counter.
    assert!(back.journal.is_empty());
    assert_eq!(back.seq, 0);
    assert_eq!(export_json(&back), text);

    // The whole Combat serializes too (the server stores it as one value).
    let whole = serde_json::to_string(&combat).unwrap();
    assert_eq!(serde_json::from_str::<Combat>(&whole).unwrap(), combat);
}

#[test]
fn import_refuses_newer_other_and_inconsistent_files() {
    let combat = small_fight(Edition::Srd521);
    let mut value: serde_json::Value = serde_json::from_str(&export_json(&combat)).unwrap();
    let try_import = |v: &serde_json::Value| import_json(&v.to_string(), 1);

    let mut newer = value.clone();
    newer["schema_version"] = serde_json::json!(SCHEMA_VERSION + 1);
    assert!(matches!(try_import(&newer), Err(CombatError::Import(m)) if m.contains("newer")));
    let mut other = value.clone();
    other["format"] = serde_json::json!("something-else");
    assert!(try_import(&other).is_err());
    let mut none = value.clone();
    none.as_object_mut().unwrap().remove("schema_version");
    assert!(try_import(&none).is_err());
    assert!(import_json("not json", 1).is_err());

    value["state"]["combatants"][1]["linked_to"] = serde_json::json!("ghost");
    assert!(matches!(try_import(&value), Err(CombatError::Import(m)) if m.contains("unknown")));
}

#[test]
fn same_seed_same_commands_same_result() {
    let run = |seed| {
        let mut combat = Combat::new(Edition::Srd521, seed);
        add(&mut combat, adult_red_dragon("d"));
        add(&mut combat, owlbear("o"));
        add(&mut combat, goblin("g"));
        dm(
            &mut combat,
            Command::RollInitiative {
                ids: vec![],
                only_missing: false,
            },
        );
        dm(&mut combat, Command::Start);
        for _ in 0..4 {
            let parts = vec![DamagePart {
                amount: party_dice::DiceInput::app("2d6+3"),
                damage_type: Some(DamageType::Fire),
            }];
            let target = combat.state.current.clone().unwrap();
            let _ = combat.execute(
                &Actor::Dm,
                &Command::Damage {
                    target,
                    parts,
                    magical: true,
                    apply_qualified: false,
                    critical_hit: false,
                },
            );
            dm(&mut combat, Command::NextTurn);
        }
        combat
    };
    assert_eq!(run(11), run(11));
    assert_ne!(run(11).state, run(12).state);
}

// ----- visibility (D5) -----

fn hidden_fight() -> Combat {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, goblin("a")); // hidden
    add(&mut combat, wizard_pc("w"));
    add(&mut combat, owlbear("o")); // hidden
    add(&mut combat, fighter_pc("f"));
    for (k, v) in [("a", 20), ("w", 15), ("o", 12), ("f", 5)] {
        init(&mut combat, k, v);
    }
    combat
}

#[test]
fn players_never_see_hidden_creatures() {
    let mut combat = hidden_fight();
    assert_eq!(visible_order(&combat.state, ViewerKind::Dm).len(), 4);
    let players: Vec<String> = visible_order(&combat.state, ViewerKind::Player)
        .into_iter()
        .map(|c| c.0)
        .collect();
    assert_eq!(players, ["w", "f"]);
    let ev = dm(&mut combat, Command::Start);
    // The goblin acts first, hidden: players see the combat start but no turn yet.
    assert_eq!(
        turn_pointer(&combat.state, ViewerKind::Player),
        TurnPointer::Waiting { round: 1 }
    );
    assert_eq!(
        turn_pointer(&combat.state, ViewerKind::Dm),
        TurnPointer::Combatant {
            id: id("a"),
            round: 1
        }
    );
    assert!(ev.players.contains(&PlayerEvent::Started));
    assert!(!ev
        .players
        .iter()
        .any(|p| format!("{p:?}").contains("\"a\"")));
    let ev = dm(&mut combat, Command::NextTurn);
    assert_eq!(
        turn_pointer(&combat.state, ViewerKind::Player),
        TurnPointer::Combatant {
            id: id("w"),
            round: 1
        }
    );
    assert!(ev
        .players
        .iter()
        .any(|p| matches!(p, PlayerEvent::YourTurn { owner, .. } if owner == ALICE)));
    // The owlbear's hidden turn: the pointer stays on the wizard, and players get no event at all.
    let ev = dm(&mut combat, Command::NextTurn);
    assert_eq!(current(&combat), "o");
    assert_eq!(
        turn_pointer(&combat.state, ViewerKind::Player),
        TurnPointer::Combatant {
            id: id("w"),
            round: 1
        }
    );
    assert!(ev.players.is_empty(), "{:?}", ev.players);
    dm(&mut combat, Command::NextTurn); // fighter
                                        // Round 2 starts with the hidden goblin: players keep seeing the fighter's round-1 turn.
    let ev = dm(&mut combat, Command::NextTurn);
    assert_eq!(combat.state.round, 2);
    assert_eq!(
        turn_pointer(&combat.state, ViewerKind::Player),
        TurnPointer::Combatant {
            id: id("f"),
            round: 1
        }
    );
    assert!(ev.players.is_empty());
}

#[test]
fn dm_only_writes_produce_no_player_events() {
    let mut combat = hidden_fight();
    dm(&mut combat, Command::Start);
    let quiet = [
        hit("o", 10, Some(DamageType::Slashing)),
        Command::Rename {
            id: id("w"),
            label: "Alice's wizard".into(),
        },
        Command::SetNotes {
            id: id("a"),
            notes: "flees at half".into(),
        },
        Command::GrantTempHp {
            id: id("o"),
            amount: 5,
        },
        Command::SetPublicName {
            id: id("a"),
            name: "Shadow".into(),
        },
        Command::AddCondition {
            effect: Box::new({
                let mut s = effect("", "o");
                s.conditions = vec![Condition::Prone];
                s
            }),
        },
        Command::SetLegendaryPool {
            pool: "p".into(),
            value: LegendaryPool {
                max: 3,
                remaining: 3,
                members: vec![id("o")],
            },
        },
        Command::SetEconomy {
            id: id("a"),
            action: Some(true),
            bonus_action: None,
            reaction: None,
        },
        hit("a", 100, None),
    ];
    for cmd in quiet {
        let ev = dm(&mut combat, cmd.clone());
        assert!(ev.players.is_empty(), "{cmd:?} -> {:?}", ev.players);
    }
}

#[test]
fn reveal_and_hide_after_reveal() {
    let mut combat = hidden_fight();
    dm(&mut combat, Command::Start);
    let ev = dm(&mut combat, Command::Reveal { id: id("a") });
    // Revealed during its own turn: players now see the goblin's turn.
    assert!(ev.players.contains(&PlayerEvent::Appeared { id: id("a") }));
    assert!(ev.players.contains(&PlayerEvent::Order));
    assert_eq!(
        turn_pointer(&combat.state, ViewerKind::Player),
        TurnPointer::Combatant {
            id: id("a"),
            round: 1
        }
    );
    let ev = dm(&mut combat, Command::Hide { id: id("a") });
    assert!(ev
        .dm
        .iter()
        .any(|e| e.kind == DmEventKind::Hidden { was_seen: true }));
    assert!(ev
        .players
        .contains(&PlayerEvent::Disappeared { id: id("a") }));
    assert_eq!(
        turn_pointer(&combat.state, ViewerKind::Player),
        TurnPointer::Waiting { round: 1 }
    );
    assert!(matches!(
        dm_err(&mut combat, Command::Hide { id: id("w") }),
        CombatError::Invalid(_)
    ));
    // A visible creature's change is a player event; a removal makes it disappear.
    dm(&mut combat, Command::Reveal { id: id("o") });
    let ev = dm(&mut combat, hit("o", 40, None));
    assert_eq!(ev.players, vec![PlayerEvent::Changed { id: id("o") }]);
    let ev = dm(&mut combat, Command::RemoveCombatant { id: id("o") });
    assert!(ev
        .players
        .contains(&PlayerEvent::Disappeared { id: id("o") }));
}

#[test]
fn the_enemy_turn_slot_is_opt_in() {
    let mut combat = hidden_fight();
    dm(
        &mut combat,
        Command::SetSettings {
            settings: Settings {
                show_enemy_turn: true,
                auto_recharge: true,
            },
        },
    );
    dm(&mut combat, Command::Start);
    assert_eq!(
        turn_pointer(&combat.state, ViewerKind::Player),
        TurnPointer::EnemyTurn { round: 1 }
    );
}

/// Two combats that differ only in hidden creatures look the same to players at every visible turn.
#[test]
fn hidden_creatures_change_nothing_players_see() {
    let mut plain = Combat::new(Edition::Srd521, 5);
    add(&mut plain, wizard_pc("w"));
    add(&mut plain, fighter_pc("f"));
    let mut with_hidden = Combat::new(Edition::Srd521, 5);
    add(&mut with_hidden, adult_red_dragon("hidden-1"));
    add(&mut with_hidden, wizard_pc("w"));
    add(&mut with_hidden, goblin("hidden-2"));
    add(&mut with_hidden, fighter_pc("f"));
    for combat in [&mut plain, &mut with_hidden] {
        init(combat, "w", 14);
        init(combat, "f", 14);
    }
    init(&mut with_hidden, "hidden-1", 20);
    init(&mut with_hidden, "hidden-2", 14);
    dm(&mut plain, Command::Start);
    dm(&mut with_hidden, Command::Start);
    let view = |combat: &Combat| {
        let order = visible_order(&combat.state, ViewerKind::Player);
        let seen: Vec<_> = order
            .iter()
            .map(|i| {
                let c = combat.state.combatant(i).unwrap();
                (
                    c.public_name.clone(),
                    c.hp.band(),
                    derive::conditions(&combat.state, i),
                    c.life,
                )
            })
            .collect();
        serde_json::to_string(&(order, seen, turn_pointer(&combat.state, ViewerKind::Player)))
            .unwrap()
    };
    for _ in 0..6 {
        // Step the hidden combat until a visible turn.
        while !is_visible(
            &with_hidden.state,
            with_hidden.state.current.as_ref().unwrap(),
            ViewerKind::Player,
        ) {
            dm(&mut with_hidden, Command::NextTurn);
        }
        assert_eq!(view(&plain), view(&with_hidden));
        dm(&mut plain, Command::NextTurn);
        dm(&mut with_hidden, Command::NextTurn);
    }
    // The number of player-visible turns equals PCs plus revealed creatures.
    assert_eq!(
        visible_order(&with_hidden.state, ViewerKind::Player).len(),
        2
    );
}

/// D20 b: a revealed creature shows its full stat block, so an exact HP change (inside a band) or a legendary
/// action spent is a player event; DM-only fields still aren't.
#[test]
fn revealed_means_the_full_stat_block() {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, adult_red_dragon("d"));
    add(&mut combat, wizard_pc("w"));
    init(&mut combat, "d", 15);
    init(&mut combat, "w", 10);
    let pool = LegendaryPool {
        max: 3,
        remaining: 3,
        members: vec![id("d")],
    };
    dm(
        &mut combat,
        Command::SetLegendaryPool {
            pool: "d".into(),
            value: pool,
        },
    );
    dm(&mut combat, Command::Reveal { id: id("d") });
    dm(&mut combat, Command::Start);
    let ev = dm(&mut combat, hit("d", 1, Some(DamageType::Cold)));
    assert_eq!(
        ev.players,
        vec![PlayerEvent::Changed { id: id("d") }],
        "256 -> 255 is still unhurt"
    );
    dm(&mut combat, Command::NextTurn);
    let ev = dm(
        &mut combat,
        Command::UseAbility {
            id: id("d"),
            ability: "tail".into(),
            slot: None,
        },
    );
    assert_eq!(ev.players, vec![PlayerEvent::Changed { id: id("d") }]);
    let view = player_view_of(&combat.state, &id("d")).unwrap();
    assert_eq!(view["hp"]["current"], 255);
    assert_eq!(
        view["legendary_actions"],
        serde_json::json!({"remaining": 2, "max": 3})
    );
    for dm_only in ["label", "notes", "tiebreak"] {
        assert!(view.get(dm_only).is_none(), "{dm_only} in {view}");
    }
    let ev = dm(
        &mut combat,
        Command::SetNotes {
            id: id("d"),
            notes: "flees at 50".into(),
        },
    );
    assert!(ev.players.is_empty());
}
