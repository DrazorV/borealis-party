//! Copies of the bundled SRD monsters and of a companion block, overrides and refresh conflicts on both, and what
//! players see of a revealed creature.

use std::collections::BTreeMap;

use borealis_content::monster::Ability;
use borealis_content::{Element, MonsterData};
use party_encounter::creature::{player_view, Kind, Snapshot, SnapshotError};
use party_encounter::overrides::{
    refresh, AddedTrait, Choice, HpValue, OverrideError, Overrides, OverridesForm, Printed,
};
use serde_json::{json, Value};

/// An element of the bundled SRD 5.2.1 monsters (`content/srd-5.2.1-monsters`).
fn srd(id: &str) -> Element {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../content/srd-5.2.1-monsters");
    for file in ["monsters.json", "animals.json"] {
        let text = std::fs::read_to_string(dir.join(file)).unwrap();
        let pack: Value = serde_json::from_str(&text).unwrap();
        if let Some(e) = pack["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == id)
        {
            return serde_json::from_value(e.clone()).unwrap();
        }
    }
    panic!("{id} is not in the SRD 5.2.1 monsters");
}

fn monster(id: &str) -> Snapshot {
    Snapshot::of_monster(&srd(id)).unwrap()
}

fn data(id: &str) -> MonsterData {
    monster(id).data.unwrap()
}

/// A made-up Aurora companion element (an owl familiar) and its printed block, with some numbers not printed.
fn owl() -> Snapshot {
    let element: Element = serde_json::from_value(json!({
        "id": "ID_TEST_COMPANION_OWL", "name": "Owl", "type": "Companion", "source": "Test Companions",
    }))
    .unwrap();
    let block = json!({
        "ac": 11, "acText": "11", "hp": 1, "hpText": "1 (1d4-1)", "hpFormula": "1d4-1",
        "speed": "5 ft., fly 60 ft.", "speeds": { "walk": 5, "fly": 60 },
        "abilities": { "str": 3, "dex": 13, "con": 8, "int": 2, "wis": 12, "cha": null },
        "saves": {}, "proficiency": 2, "skills": "Perception +3", "senses": "darkvision 120 ft.",
        "languages": null, "challenge": "0", "size": "Tiny", "type": "Beast", "alignment": "unaligned",
        "defenses": [], "traits": [{ "name": "Flyby", "text": "No opportunity attacks when it flies out of reach." }],
        "actions": [{ "name": "Talons", "text": "1 slashing damage." }], "reactions": [],
    });
    Snapshot::of_companion(&element, block).unwrap()
}

const GOBLIN: &str = "ID_SRD521_MONSTER_GOBLIN_WARRIOR";
const OWLBEAR: &str = "ID_SRD521_MONSTER_OWLBEAR";

fn form(v: Value) -> OverridesForm {
    serde_json::from_value(v).unwrap()
}

#[test]
fn copies_of_srd_monsters_round_trip_through_the_model() {
    let snap = monster(GOBLIN);
    assert_eq!(snap.kind, Kind::Monster);
    assert_eq!(snap.name, "Goblin Warrior");
    assert_eq!(snap.ruleset, "dnd2024");
    assert_eq!(snap.maps_to.as_deref(), Some("ID_SRD51_MONSTER_GOBLIN"));
    assert_eq!(snap.type_word(), "Fey");
    assert_eq!(snap.sizes(), ["small"]);
    assert_eq!(snap.challenge().as_deref(), Some("1/4"));
    let text = serde_json::to_string(&snap).unwrap();
    let back: Snapshot = serde_json::from_str(&text).unwrap();
    assert_eq!(back, snap);
    let data = back.data.as_ref().unwrap();
    assert!(data.validate().is_empty(), "{:?}", data.validate());
    assert_eq!(back.rev(), snap.rev());
    assert_eq!(snap.rev().len(), 16);
    // XP is never part of a block anyone is shown (D16).
    let block = snap.block(&Overrides::default());
    assert!(block.get("xp").is_none() && block["cr"] == "1/4", "{block}");
}

#[test]
fn only_creatures_are_copied() {
    let mut e = srd(GOBLIN);
    e.ty = "Language".into();
    assert_eq!(Snapshot::of_monster(&e), Err(SnapshotError::NotACreature));
    assert_eq!(
        Snapshot::of_companion(&e, json!({})),
        Err(SnapshotError::NotACreature)
    );
    let mut e = srd(GOBLIN);
    e.data.insert("not_a_field".into(), json!(1));
    assert_eq!(Snapshot::of_monster(&e), Err(SnapshotError::Unreadable));
}

#[test]
fn a_companion_copy_keeps_what_it_prints_and_nothing_more() {
    let snap = owl();
    assert_eq!(snap.kind, Kind::Companion);
    assert_eq!(
        (snap.type_word(), snap.sizes()),
        ("Beast".to_string(), vec!["tiny".to_string()])
    );
    assert_eq!(snap.ruleset, "any");
    assert_eq!(snap.challenge().as_deref(), Some("0"));
    let printed = snap.printed();
    assert_eq!(
        printed.hp,
        Some(HpValue {
            average: 1,
            formula: Some("1d4-1".into())
        })
    );
    assert_eq!(printed.ac.as_ref().unwrap().value, 11);
    // A save from its score; a score it doesn't print gives none.
    assert_eq!(printed.saves[&Ability::Dex], Some(1));
    assert_eq!(printed.saves[&Ability::Cha], None);
    assert_eq!(snap.average_hp(&Overrides::default()), 1);
    let back: Snapshot = serde_json::from_str(&serde_json::to_string(&snap).unwrap()).unwrap();
    assert_eq!(back, snap);
}

#[test]
fn overrides_change_hp_ac_saves_and_traits_and_keep_the_source_values() {
    let snap = monster(OWLBEAR);
    let owlbear = data(OWLBEAR);
    let o = Overrides::default()
        .edit(
            form(json!({
                "hp": { "average": 80, "formula": "9d10 + 31" },
                "ac": { "value": 15, "source": "scarred hide" },
                "saves": { "dex": 4 },
                "traits": [{ "name": "Battle Scarred", "text": "It can't be frightened." }],
                "notes": "Tougher, as the adventure's sidebar says.",
            })),
            &snap.printed(),
        )
        .unwrap();
    assert_eq!(
        o.hp.as_ref().unwrap().base,
        Some(HpValue {
            average: 59,
            formula: Some("7d10+21".into())
        })
    );
    assert_eq!(
        o.hp.as_ref().unwrap().value.formula.as_deref(),
        Some("9d10+31")
    );
    assert_eq!(o.ac.as_ref().unwrap().base.as_ref().unwrap().value, 13);
    assert_eq!(
        o.saves[&Ability::Dex].base,
        Some(owlbear.save_bonus(Ability::Dex))
    );
    assert!(o.changes_block());

    let used = o.apply(&owlbear);
    assert_eq!(used.hp.as_ref().unwrap().average, 80);
    assert_eq!((used.ac.len(), used.ac[0].value), (1, 15));
    assert_eq!(used.save_bonus(Ability::Dex), 4);
    assert_eq!(used.traits.last().unwrap().name, "Battle Scarred");
    assert!(used.validate().is_empty(), "{:?}", used.validate());
    assert_eq!((snap.average_hp(&o), snap.ac(&o)), (80, Some(15)));
    // The note is the DM's, not part of the block.
    assert!(!snap.block(&o).to_string().contains("sidebar"));

    // null drops an override; absent leaves it.
    let o = o
        .edit(
            form(json!({ "hp": null, "saves": { "dex": null } })),
            &snap.printed(),
        )
        .unwrap();
    assert!(o.hp.is_none() && o.saves.is_empty() && o.ac.is_some());
    assert_eq!(o.traits.len(), 1);
}

#[test]
fn overrides_apply_to_a_companion_block_too() {
    let snap = owl();
    let o = Overrides::default()
        .edit(
            form(json!({ "hp": { "average": 4 }, "saves": { "cha": 1 }, "traits": [{ "name": "Keen Sight", "text": "x" }] })),
            &snap.printed(),
        )
        .unwrap();
    assert_eq!(o.saves[&Ability::Cha].base, None, "the owl prints no CHA");
    let block = snap.block(&o);
    assert_eq!(
        (block["hp"].clone(), block["hpFormula"].clone()),
        (json!(4), Value::Null)
    );
    assert_eq!(block["saves"]["cha"], 1);
    assert_eq!(
        block["traits"].as_array().unwrap().last().unwrap()["name"],
        "Keen Sight"
    );
    assert_eq!(
        block["abilities"]["cha"],
        Value::Null,
        "nothing is filled in"
    );
}

#[test]
fn overrides_refuse_what_a_block_cant_say() {
    let printed = monster(OWLBEAR).printed();
    let none = Overrides::default();
    assert!(matches!(
        none.edit(form(json!({ "hp": { "average": 0 } })), &printed),
        Err(OverrideError::OutOfRange(_))
    ));
    assert_eq!(
        none.edit(
            form(json!({ "hp": { "average": 5, "formula": "lots" } })),
            &printed
        ),
        Err(OverrideError::BadFormula)
    );
    assert!(none
        .edit(form(json!({ "ac": { "value": 99 } })), &printed)
        .is_err());
    assert!(none
        .edit(form(json!({ "saves": { "str": 31 } })), &printed)
        .is_err());
    assert!(none
        .edit(
            form(json!({ "traits": [{ "name": "", "text": "x" }] })),
            &printed
        )
        .is_err());
    assert!(none
        .edit(form(json!({ "notes": "a\u{0}b" })), &printed)
        .is_err());
    let many: Vec<AddedTrait> = (0..13)
        .map(|i| AddedTrait {
            name: format!("T{i}"),
            text: String::new(),
        })
        .collect();
    assert!(none
        .edit(
            OverridesForm {
                traits: Some(many),
                ..OverridesForm::default()
            },
            &printed
        )
        .is_err());
    assert!(serde_json::from_value::<OverridesForm>(json!({ "xp": 1 })).is_err());
}

#[test]
fn refresh_reports_conflicts_only_where_both_sides_changed() {
    let old = monster(OWLBEAR);
    let o = Overrides::default()
        .edit(
            form(json!({ "hp": { "average": 80 }, "ac": { "value": 15 }, "saves": { "con": 7 } })),
            &old.printed(),
        )
        .unwrap();
    let r = refresh(&o, &old.printed(), &BTreeMap::new());
    assert!(r.conflicts.is_empty());
    assert_eq!(r.overrides.as_ref(), Some(&o));

    // The source changed its HP and its AC, not its CON save.
    let mut fresh = old.data.clone().unwrap();
    fresh.hp.as_mut().unwrap().average = 65;
    fresh.ac[0].value = 14;
    let fresh = Printed::of_monster(&fresh);
    let r = refresh(&o, &fresh, &BTreeMap::new());
    let fields: Vec<&str> = r.conflicts.iter().map(|c| c.field.as_str()).collect();
    assert_eq!(fields, ["hp", "ac"]);
    assert_eq!(r.conflicts[0].was["average"], 59);
    assert_eq!(r.conflicts[0].source["average"], 65);
    assert_eq!(r.conflicts[0].value["average"], 80);
    assert!(r.overrides.is_none(), "undecided conflicts change nothing");

    let choices = BTreeMap::from([
        ("hp".to_string(), Choice::Keep),
        ("ac".to_string(), Choice::Source),
    ]);
    let after = refresh(&o, &fresh, &choices).overrides.unwrap();
    assert_eq!(after.hp.as_ref().unwrap().value.average, 80);
    assert_eq!(
        after.hp.as_ref().unwrap().base.as_ref().unwrap().average,
        65
    );
    assert!(after.ac.is_none());
    assert_eq!(after.saves[&Ability::Con].value, 7);
    assert!(refresh(&after, &fresh, &BTreeMap::new())
        .conflicts
        .is_empty());
}

#[test]
fn players_see_a_revealed_creature_whole_but_nothing_of_the_dm() {
    let goblin = monster(GOBLIN);
    let o = Overrides::default()
        .edit(form(json!({ "notes": "SECRET NOTE" })), &goblin.printed())
        .unwrap();
    let v = player_view("c-1", "Small fey", &goblin, &o, (10, 10), &[], None);
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "conditions",
            "hp",
            "id",
            "kind",
            "publicName",
            "size",
            "statBlock",
            "type"
        ]
    );
    assert_eq!(
        (v["type"].clone(), v["hp"]["max"].clone()),
        (json!("Fey"), json!(10))
    );
    assert!(v["statBlock"]["abilities"].is_object());
    let text = v.to_string();
    for leak in ["Goblin Warrior", "ID_SRD521", "SECRET", "\"xp\""] {
        assert!(!text.contains(leak), "{leak} in {text}");
    }
    let pet = player_view(
        "c-2",
        "Bird",
        &owl(),
        &Overrides::default(),
        (1, 1),
        &[],
        Some("c-1"),
    );
    assert_eq!(
        (pet["kind"].clone(), pet["ownerId"].clone()),
        (json!("companion"), json!("c-1"))
    );
    assert!(!pet.to_string().contains("ID_TEST"));
}
