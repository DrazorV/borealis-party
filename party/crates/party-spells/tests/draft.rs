//! Drafts from 5etools-format spell tags: what the tags give, what is left to the DM, and (opt-in, counts only) a run
//! over a local 5etools copy (`BOREALIS_TEST_5ETOOLS_DIR`).

use party_combat::spell::{Delivery, Primitive, SaveSuccess, Targeting, Tier};
use party_combat::{Condition, DamageType, Edition};
use party_spells::draft::{from_5etools, Check};
use serde_json::{json, Value};

/// A 5etools-format spell from a made-up source, shaped like the SRD Fireball.
fn fireball() -> Value {
    json!({
        "name": "Fireball", "source": "TestBook", "level": 3, "school": "V",
        "time": [{ "number": 1, "unit": "action" }],
        "range": { "type": "point", "distance": { "type": "feet", "amount": 150 } },
        "duration": [{ "type": "instant" }],
        "entries": ["Each creature in the area makes a saving throw, taking {@damage 8d6} fire damage."],
        "entriesHigherLevel": [{ "type": "entries", "entries": ["The damage increases by {@scaledamage 8d6|3-9|1d6} per level."] }],
        "damageInflict": ["fire"], "savingThrow": ["dexterity"], "areaTags": ["S"]
    })
}

#[test]
fn damage_and_save_from_tags() {
    let d = from_5etools("ID_TEST_FIREBALL", Edition::Srd51, &fireball());
    let m = &d.mech;
    assert_eq!(m.level, 3);
    assert_eq!(m.tier, Tier::Partial, "a draft is never auto");
    assert!(matches!(m.targeting, Targeting::Area { size_ft: 0, .. }));
    // Half damage on a save is prose only: the draft says none and asks.
    assert!(matches!(
        m.delivery,
        Delivery::Save {
            success: SaveSuccess::None,
            ..
        }
    ));
    match &m.on_land[0] {
        Primitive::Damage {
            dice, damage_type, ..
        } => {
            assert_eq!(dice.as_str(), "8d6+(slot_above)d6");
            assert_eq!(*damage_type, Some(DamageType::Fire));
        }
        p => panic!("{p:?}"),
    }
    assert_eq!(d.checks, [Check::HalfOnSave, Check::AreaSize]);
}

#[test]
fn conditions_types_and_cantrips() {
    let hold = json!({
        "name": "Hold Person", "level": 2, "time": [{ "number": 1, "unit": "action" }],
        "range": { "type": "point", "distance": { "type": "feet", "amount": 60 } },
        "duration": [{ "type": "timed", "duration": { "type": "minute", "amount": 1 }, "concentration": true }],
        "entries": ["Choose a humanoid. It must succeed on a saving throw or be {@condition paralyzed}."],
        "savingThrow": ["wisdom"], "conditionInflict": ["paralyzed"], "affectsCreatureType": ["humanoid"]
    });
    let d = from_5etools("ID_TEST_HOLD", Edition::Srd51, &hold);
    assert!(d.mech.concentration);
    assert!(matches!(
        &d.mech.on_land[0],
        Primitive::Condition {
            condition: Condition::Paralyzed,
            repeat_save: None,
            ..
        }
    ));
    assert!(d.checks.contains(&Check::Targets));
    assert!(d.checks.contains(&Check::Condition));

    let bolt = json!({
        "name": "Fire Bolt", "level": 0, "time": [{ "number": 1, "unit": "action" }],
        "range": { "type": "point", "distance": { "type": "feet", "amount": 120 } },
        "duration": [{ "type": "instant" }],
        "entries": ["On a hit, the target takes {@damage 1d10} fire damage."],
        "scalingLevelDice": { "label": "fire damage", "scaling": { "1": "1d10", "5": "2d10", "11": "3d10", "17": "4d10" } },
        "damageInflict": ["fire"], "spellAttack": ["R"]
    });
    let d = from_5etools("ID_TEST_BOLT", Edition::Srd521, &bolt);
    assert!(matches!(d.mech.delivery, Delivery::Attack { .. }));
    match &d.mech.on_land[0] {
        Primitive::Damage { dice, .. } => assert_eq!(dice.as_str(), "(cantrip_tier)d10"),
        p => panic!("{p:?}"),
    }
}

#[test]
fn buffs_are_left_to_the_dm() {
    let armor = json!({
        "name": "Mage Armor", "level": 1, "time": [{ "number": 1, "unit": "action" }],
        "range": { "type": "point", "distance": { "type": "touch" } },
        "duration": [{ "type": "timed", "duration": { "type": "hour", "amount": 8 } }],
        "entries": ["The target's base AC becomes 13 + its Dexterity modifier."],
        "miscTags": ["MAC"]
    });
    let d = from_5etools("ID_TEST_ARMOR", Edition::Srd51, &armor);
    // MAC only says "changes AC": no number is guessed; the AC card opens.
    assert!(d.mech.on_land.is_empty() && d.mech.always.is_empty());
    assert_eq!(d.mech.tier, Tier::Ruling);
    assert!(d.checks.contains(&Check::Ac));

    let cure = json!({
        "name": "Cure Wounds", "level": 1, "time": [{ "number": 1, "unit": "action" }],
        "range": { "type": "point", "distance": { "type": "touch" } }, "duration": [{ "type": "instant" }],
        "entries": ["A creature regains {@dice 1d8} hit points."],
        "entriesHigherLevel": [{ "type": "entries", "entries": ["{@scaledice 1d8|1-9|1d8}"] }],
        "miscTags": ["HL"]
    });
    let d = from_5etools("ID_TEST_CURE", Edition::Srd51, &cure);
    match &d.mech.on_land[0] {
        Primitive::Heal { dice, add_mod } => {
            assert_eq!(dice.as_str(), "1d8+(slot_above)d8");
            assert!(!add_mod, "the modifier is prose: the DM confirms it");
        }
        p => panic!("{p:?}"),
    }
    assert!(d.checks.contains(&Check::Heal));

    let summon = json!({ "name": "Call Beasts", "level": 3, "time": [{ "number": 1, "unit": "action" }],
        "range": { "type": "point", "distance": { "type": "feet", "amount": 60 } }, "duration": [{ "type": "instant" }],
        "entries": ["Spirits appear."], "miscTags": ["SMN"] });
    let d = from_5etools("ID_TEST_SUMMON", Edition::Srd51, &summon);
    assert!(matches!(d.mech.always[0], Primitive::Summon { .. }));
    assert!(d.checks.contains(&Check::Summon));
}

/// Opt-in: draft every spell of a local 5etools copy. Counts only; nothing is written.
#[test]
fn five_etools_corpus() {
    let Some(dir) = std::env::var_os("BOREALIS_TEST_5ETOOLS_DIR") else {
        eprintln!("BOREALIS_TEST_5ETOOLS_DIR not set: skipped");
        return;
    };
    let dir = std::path::Path::new(&dir).join("data/spells");
    let (mut n, mut partial, mut ruling, mut with_damage) = (0, 0, 0, 0);
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if !name.starts_with("spells-") {
            continue;
        }
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        for s in v["spell"].as_array().into_iter().flatten() {
            let d = from_5etools("ID_X", Edition::Srd51, s);
            // Every draft serializes and reads back under the closed schema.
            let back: party_combat::spell::SpellMech =
                serde_json::from_value(serde_json::to_value(&d.mech).unwrap()).unwrap();
            assert_eq!(back, d.mech);
            n += 1;
            match d.mech.tier {
                Tier::Partial => partial += 1,
                _ => ruling += 1,
            }
            if d
                .mech
                .on_land
                .iter()
                .any(|p| matches!(p, Primitive::Damage { .. }))
            {
                with_damage += 1;
            }
        }
    }
    println!(
        "5etools drafts: {n} spells, {partial} suggested, {ruling} left to the DM, {with_damage} with damage"
    );
    assert!(n > 0);
}
