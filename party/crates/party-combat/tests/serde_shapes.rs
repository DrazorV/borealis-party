//! The JSON shapes wave 2 (server routes and UI) builds against. Changing one of these is a breaking change.

mod common;
use common::*;
use party_combat::*;
use serde_json::json;

#[test]
fn command_shapes() {
    let cmd: Command = serde_json::from_value(json!({
        "command": "damage",
        "target": "g",
        "parts": [
            {"amount": {"kind": "typed", "total": 7}, "damage_type": "slashing"},
            {"amount": {"kind": "app", "notation": "1d6", "critical": true}, "damage_type": "fire"}
        ],
        "magical": true
    }))
    .unwrap();
    let Command::Damage {
        parts,
        magical,
        critical_hit,
        ..
    } = &cmd
    else {
        panic!()
    };
    assert_eq!(parts.len(), 2);
    assert!(*magical && !critical_hit);
    let back = serde_json::to_value(&cmd).unwrap();
    assert_eq!(back["command"], "damage");

    let cmd: Command = serde_json::from_value(json!({
        "command": "set_initiative", "id": "w", "roll": {"kind": "typed", "total": 17, "natural": 14}
    }))
    .unwrap();
    assert!(cmd.player_allowed());
    assert_eq!(
        serde_json::to_value(Command::NextTurn).unwrap(),
        json!({"command": "next_turn"})
    );

    let cmd: Command = serde_json::from_value(json!({
        "command": "add_effect",
        "effect": {
            "name": "Bless", "source": "m", "target": "w", "concentration": true,
            "duration": {"kind": "source_turns", "remaining": 10, "at": "start"},
            "modifiers": [{"kind": "ac", "value": 2}, {"kind": "cover", "value": "half"}, {"kind": "speed_zero"}],
            "conditions": ["prone"]
        }
    }))
    .unwrap();
    assert!(matches!(cmd, Command::AddEffect { .. }));
}

#[test]
fn state_and_event_shapes() {
    let combat = small_fight(Edition::Srd521);
    let v = serde_json::to_value(&combat.state).unwrap();
    assert_eq!(v["edition"], "2024");
    assert_eq!(v["status"], "active");
    let w = v["combatants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "w")
        .unwrap();
    assert_eq!(w["kind"], "character");
    assert_eq!(w["owner"], ALICE);
    assert_eq!(w["life"], json!({"state": "alive"}));
    assert_eq!(w["hp_authority"], "player");
    assert_eq!(
        serde_json::to_value(Life::Dying {
            successes: 1,
            failures: 2
        })
        .unwrap(),
        json!({"state": "dying", "successes": 1, "failures": 2})
    );

    let p = serde_json::to_value(TurnPointer::Combatant {
        id: id("w"),
        round: 2,
    })
    .unwrap();
    assert_eq!(p, json!({"kind": "combatant", "id": "w", "round": 2}));
    let e = serde_json::to_value(PlayerEvent::YourTurn {
        id: id("w"),
        owner: ALICE.into(),
    })
    .unwrap();
    assert_eq!(e, json!({"event": "your_turn", "id": "w", "owner": ALICE}));
    let u = serde_json::to_value(Unavailable::NoSlot(3)).unwrap();
    assert_eq!(u, json!({"reason": "no_slot", "detail": 3}));
}

#[test]
fn damage_clauses_read_upstream_monster_json() {
    // The shape of borealis_content::monster_types::DamageClause.
    let clauses: Vec<DamageClause> = serde_json::from_value(json!([
        {"types": ["fire"]},
        {"types": ["bludgeoning", "piercing", "slashing"], "text": "from nonmagical attacks"}
    ]))
    .unwrap();
    assert_eq!(clauses[0].scope(), ClauseScope::Always);
    assert_eq!(clauses[1].scope(), ClauseScope::Nonmagical);
}

#[test]
fn health_bands_match_the_party_hub() {
    let band = |current, max| {
        Hp {
            current,
            max,
            temp: 0,
        }
        .band()
    };
    assert_eq!(band(20, 20), Some(HealthBand::Unhurt));
    assert_eq!(band(11, 20), Some(HealthBand::Unhurt));
    assert_eq!(band(10, 20), Some(HealthBand::Bloodied));
    assert_eq!(band(1, 20), Some(HealthBand::Bloodied));
    assert_eq!(band(0, 20), Some(HealthBand::Down));
    assert_eq!(band(0, 0), None);
}
