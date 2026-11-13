//! D17 b: Aurora Companion creatures have thin stat blocks. HP, AC, initiative, conditions, effects, concentration
//! and turn flow always work; actions, recharge, legendary actions and slots exist only when the block has them.

mod common;
use common::*;
use party_combat::party_dice::D20Input;
use party_combat::*;

/// A companion block with no AC, unknown HP and no actions: the shape the server builds from a thin element.
fn thin(key: &str) -> Combatant {
    serde_json::from_value(serde_json::json!({
        "id": key, "kind": "creature", "label": "Companion"
    }))
    .expect("a minimal block deserializes")
}

#[test]
fn a_minimal_block_has_safe_defaults() {
    let t = thin("t");
    assert_eq!((t.ac, t.hp, t.life), (None, Hp::default(), Life::Alive));
    assert!(t.abilities.is_empty() && t.spell_slots.is_empty() && t.legendary_resistance.is_none());
    assert_eq!(t.hp.band(), None, "unknown HP has no band");
}

#[test]
fn a_thin_companion_fights_like_anyone() {
    let mut combat = Combat::new(Edition::Srd521, 4);
    add(&mut combat, wizard_pc("w"));
    init(&mut combat, "w", 12);
    combat
        .execute(
            &Actor::Player {
                account: ALICE.into(),
            },
            &Command::Summon {
                owner: id("w"),
                combatant: Box::new(thin("t")),
                initiative: None,
                visible: None,
            },
        )
        .unwrap();
    add(&mut combat, goblin("g"));
    dm(
        &mut combat,
        Command::RollInitiative {
            ids: vec![id("g")],
            only_missing: false,
        },
    );
    dm(&mut combat, Command::Start);
    for _ in 0..5 {
        dm(&mut combat, Command::NextTurn);
    }
    // Damage with unknown HP changes nothing and never panics; the DM sets HP when known.
    dm(&mut combat, hit("t", 5, Some(DamageType::Fire)));
    assert!(c(&combat, "t").life.is_alive());
    dm(
        &mut combat,
        Command::SetHp {
            id: id("t"),
            current: 9,
            max: Some(9),
            temp: None,
        },
    );
    dm(&mut combat, hit("t", 4, None));
    assert_eq!(c(&combat, "t").hp.current, 5);
    // Conditions, effects and concentration work.
    let mut spec = effect("", "t");
    spec.conditions = vec![Condition::Frightened];
    dm(
        &mut combat,
        Command::AddCondition {
            effect: Box::new(spec),
        },
    );
    let mut spec = effect("Barkskin", "t");
    spec.modifiers = vec![Modifier::Ac(2)];
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(spec),
        },
    );
    dm(
        &mut combat,
        Command::SetConcentration {
            id: id("t"),
            name: "Guidance".into(),
        },
    );
    // Unknown AC: effects still add up, but there is no total; attacks leave the verdict to the DM.
    let ac = derive::effective_ac(&combat.state, c(&combat, "t"));
    assert_eq!((ac.base, ac.effects, ac.total), (None, 2, None));
    let ev = dm(
        &mut combat,
        Command::Attack {
            attacker: id("g"),
            target: id("t"),
            label: String::new(),
            roll: D20Input::typed(14),
        },
    );
    assert!(matches!(
        &ev.dm[0].kind,
        DmEventKind::Attack {
            hit: None,
            target_ac: None,
            ..
        }
    ));
    let ev = dm(
        &mut combat,
        Command::Attack {
            attacker: id("g"),
            target: id("t"),
            label: String::new(),
            roll: D20Input::typed_natural(22, 20),
        },
    );
    assert!(
        matches!(
            &ev.dm[0].kind,
            DmEventKind::Attack {
                hit: Some(true),
                ..
            }
        ),
        "a natural 20 still hits"
    );
    // What the block doesn't have isn't invented: clear refusals, no panics.
    let use_x = Command::UseAbility {
        id: id("t"),
        ability: "bite".into(),
        slot: None,
    };
    assert!(matches!(
        dm_err(&mut combat, use_x),
        CombatError::UnknownAbility(_)
    ));
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::AdjustSlot {
                id: id("t"),
                level: 1,
                delta: -1
            }
        ),
        CombatError::Invalid(_)
    ));
    assert!(matches!(
        dm_err(
            &mut combat,
            Command::AdjustLegendaryResistance {
                id: id("t"),
                delta: -1
            }
        ),
        CombatError::Invalid(_)
    ));
    let recharge = Command::RollRecharge {
        id: id("t"),
        ability: "breath".into(),
        roll: party_dice::DiceInput::typed(6),
    };
    assert!(matches!(
        dm_err(&mut combat, recharge),
        CombatError::UnknownAbility(_)
    ));
    assert!(derive::pool_of(&combat.state, &id("t")).is_none());
    // Players see it (it's the wizard's), and it survives export.
    let view = player_view_of(&combat.state, &id("t")).unwrap();
    assert!(view.get("ac").is_none(), "{view}");
    let back = import_json(&export_json(&combat), 2).unwrap();
    assert_eq!(back.state, combat.state);
}

#[test]
fn speeds_and_saves_without_numbers() {
    let mut combat = Combat::new(Edition::Srd51, 1);
    add(&mut combat, thin("t"));
    assert_eq!(
        derive::effective_speed(&combat.state, c(&combat, "t")).walk,
        0
    );
    dm(
        &mut combat,
        Command::SetExhaustion {
            id: id("t"),
            level: 3,
        },
    );
    let mut spec = effect("Web", "t");
    spec.conditions = vec![Condition::Restrained];
    spec.repeat_save = Some(RepeatSave {
        ability: AbilityName::Str,
        dc: 12,
        at: Boundary::End,
    });
    dm(
        &mut combat,
        Command::AddEffect {
            effect: Box::new(spec),
        },
    );
    let e = combat.state.effects[0].id;
    dm(
        &mut combat,
        Command::ResolveEffectSave {
            effect: e,
            roll: D20Input::typed(12),
        },
    );
    assert!(combat.state.effects.is_empty());
}
