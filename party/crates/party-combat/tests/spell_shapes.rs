//! The 0.3.2 contract: spell definitions (the fixtures), formulas, and the cast command shapes. Changing one of
//! these is a breaking change for the data, server and UI sessions.

mod common;
use common::*;
use party_combat::spell::*;
use party_combat::*;
use serde_json::json;

#[test]
fn every_fixture_parses_and_round_trips() {
    for name in FIXTURES {
        for edition in [Edition::Srd51, Edition::Srd521] {
            let m = mech(name, edition);
            assert_eq!(m.v, 1, "{name}");
            let back: SpellMech =
                serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
            assert_eq!(back, m, "{name} round trip");
        }
    }
}

#[test]
fn formulas() {
    let v = |slot: u8, level: u8| FormulaVars {
        slot,
        slot_above: slot - level,
        modifier: 3,
        cantrip_tier: 2,
    };
    let f = Formula::parse("8d6+(slot_above)d6").unwrap();
    assert_eq!(f.notation(&v(3, 3)).unwrap(), "8d6");
    assert_eq!(f.notation(&v(4, 3)).unwrap(), "9d6");
    let f = Formula::parse("2d8+(2*slot_above)d8").unwrap();
    assert_eq!(f.notation(&v(3, 1)).unwrap(), "6d8");
    assert_eq!(
        Formula::parse("1+slot_above")
            .unwrap()
            .int(&v(4, 2))
            .unwrap(),
        3
    );
    assert_eq!(
        Formula::parse("11+slot").unwrap().int(&v(4, 2)).unwrap(),
        15
    );
    assert_eq!(
        Formula::parse("1d8+mod")
            .unwrap()
            .notation(&v(1, 1))
            .unwrap(),
        "1d8+3"
    );
    assert_eq!(
        Formula::parse("(cantrip_tier)d10")
            .unwrap()
            .notation(&v(0, 0))
            .unwrap(),
        "2d10"
    );
    assert_eq!(
        Formula::parse("d20").unwrap().notation(&v(0, 0)).unwrap(),
        "1d20"
    );
    assert!(Formula::parse("1d6").unwrap().int(&v(1, 1)).is_err());
    assert!(Formula::parse("1d6*1d6")
        .unwrap()
        .notation(&v(1, 1))
        .is_err());
    assert!(Formula::parse("level+1").is_err());
    assert!(Formula::parse("").is_err());
    assert!(Formula::parse("2d6)").is_err());
    // A bad formula in a definition is refused when it deserializes.
    let bad = json!({"kind": "heal", "dice": "2d8+bogus"});
    assert!(serde_json::from_value::<Primitive>(bad).is_err());
    // Numbers are accepted as formulas.
    let ok: Primitive = serde_json::from_value(json!({"kind": "max_hp", "n": 5})).unwrap();
    assert_eq!(
        ok,
        Primitive::MaxHp {
            n: Formula::constant(5)
        }
    );
}

#[test]
fn definitions_are_closed() {
    let mut m = serde_json::to_value(mech("fireball", Edition::Srd51)).unwrap();
    m["script"] = json!("anything");
    assert!(serde_json::from_value::<SpellMech>(m).is_err());
    let p = json!({"kind": "ac_bonus", "n": 2, "extra": true});
    assert!(serde_json::from_value::<Primitive>(p).is_err());
}

#[test]
fn cast_command_shapes() {
    let spec = spec_of("fireball", Edition::Srd521, 15, None);
    let cmd: Command = serde_json::from_value(json!({
        "command": "cast_spell",
        "caster": "m",
        "spec": serde_json::to_value(&spec).unwrap(),
        "slot": 4,
        "targets": ["a", "w"],
        "rows": [
            {"target": "a", "roll": {"kind": "typed", "total": 12}},
            {"target": "w", "roll": {"kind": "outcome", "outcome": "saved"}}
        ],
        "amounts": [{"kind": "typed", "total": 31}]
    }))
    .unwrap();
    assert!(!cmd.player_allowed());
    assert_eq!(cmd.label(), "cast spell");
    let back = serde_json::to_value(&cmd).unwrap();
    assert_eq!(back["rows"][1]["roll"]["outcome"], "saved");
    assert_eq!(back["spec"]["mech"]["key"], "ID_2024_SPELL_FIREBALL");

    let row: Command = serde_json::from_value(json!({
        "command": "resolve_cast_row", "cast": "c1", "target": "w", "roll": {"kind": "app"}
    }))
    .unwrap();
    assert!(row.player_allowed());
    for c in [
        json!({"command": "apply_cast", "cast": "c1", "amounts": [{"kind": "app"}]}),
        json!({"command": "cancel_cast", "cast": "c1", "refund": true}),
        json!({"command": "apply_zone", "effect": 3, "targets": ["a"]}),
        json!({"command": "perform_statblock", "id": "o", "ability": "beak", "targets": ["w"]}),
        json!({"command": "standard_action", "id": "o", "action": "dodge"}),
        json!({"command": "bind_to_concentration", "id": "wolf"}),
    ] {
        let cmd: Command = serde_json::from_value(c.clone()).unwrap_or_else(|e| panic!("{c}: {e}"));
        assert_eq!(serde_json::to_value(&cmd).unwrap()["command"], c["command"]);
    }
}

#[test]
fn model_fields_default_and_round_trip() {
    // A 0.3.1 combatant (no stats, casting, spell links) still loads.
    let old: Combatant = serde_json::from_value(json!({
        "id": "g", "kind": "creature", "label": "Goblin", "ac": 15,
        "abilities": [{"key": "scimitar", "name": "Scimitar", "kind": "action"}]
    }))
    .unwrap();
    assert!(old.stats.is_none() && old.casting.is_empty() && old.abilities[0].mech.is_none());

    let mut c = mage("m");
    c.stats = Some(CombatStats {
        mods: [-1, 2, 0, 3, 1, -1],
        saves: [-1, 2, 0, 6, 4, -1],
        magic_resistance: false,
        creature_type: Some("humanoid".into()),
        armor: None,
    });
    c.casting = vec![Casting {
        ability: AbilityName::Int,
        dc: Some(14),
        attack: Some(6),
        caster_level: Some(9),
        cantrip_tier: None,
    }];
    let mut fb = Ability::new("fireball", "Fireball", AbilityKind::Action);
    fb.spell = Some(SpellLink {
        key: Some("ID_PHB_SPELL_FIREBALL".into()),
        level: 3,
        frequency: SpellFrequency::Slot,
        fixed_slot: None,
        baked_in: false,
    });
    fb.mech = Some(Box::new(spec_of("fireball", Edition::Srd51, 14, None)));
    c.abilities.push(fb);
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!(v["stats"]["mods"][3], 3);
    assert_eq!(v["abilities"][2]["spell"]["level"], 3);
    let back: Combatant = serde_json::from_value(v).unwrap();
    assert_eq!(back, c);

    let mut e = EffectSpec::new("Mage Armor", "m");
    e.modifiers = vec![
        Modifier::AcBase {
            base: 13,
            add_dex: true,
        },
        Modifier::RollDice {
            on: RollOn::Saves,
            ability: None,
            term: DiceTerm {
                count: 1,
                sides: 4,
                sign: party_dice::Sign::Plus,
            },
        },
    ];
    e.dismiss_on_end = true;
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(
        v["modifiers"][0],
        json!({"kind": "ac_base", "value": {"base": 13, "add_dex": true}})
    );
    assert_eq!(v["dismiss_on_end"], true);
    assert_eq!(serde_json::from_value::<EffectSpec>(v).unwrap(), e);

    let s = CombatState::new(Edition::Srd51);
    let v = serde_json::to_value(&s).unwrap();
    assert!(v.get("casts").is_none(), "empty casts are left out");
}
