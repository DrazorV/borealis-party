//! The party's additions: difficulty against hand-computed SRD 5.2.1 budgets, snapshots of the bundled SRD monsters,
//! overrides and refresh conflicts, and what players see at each reveal level.

use std::collections::BTreeMap;

use borealis_content::monster::Ability;
use borealis_content::{Element, MonsterData};
use party_encounter::creature::{
    block_rev, group, health_band, player_view, type_word, RevealLevel, Snapshot, SnapshotError,
};
use party_encounter::overrides::{
    refresh, AcValue, AddedTrait, Choice, HpValue, OverrideError, Overrides, OverridesForm,
};
use party_encounter::{evaluate_encounter, Band, MonsterGroup, Ruleset};
use serde_json::{json, Value};

/// An element of the bundled SRD 5.2.1 monsters (`content/srd-5.2.1-monsters`).
fn srd(id: &str) -> Element {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content/srd-5.2.1-monsters");
    for file in ["monsters.json", "animals.json"] {
        let text = std::fs::read_to_string(dir.join(file)).unwrap();
        let pack: Value = serde_json::from_str(&text).unwrap();
        if let Some(e) = pack["elements"].as_array().unwrap().iter().find(|e| e["id"] == id) {
            return serde_json::from_value(e.clone()).unwrap();
        }
    }
    panic!("{id} is not in the SRD 5.2.1 monsters");
}

fn block(id: &str) -> MonsterData {
    Snapshot::from_element(&srd(id)).unwrap().data
}

fn groups(ids: &[(&str, u32)]) -> Vec<MonsterGroup> {
    ids.iter()
        .map(|(id, n)| {
            let mut g = group(&block(id)).unwrap();
            g.count = *n;
            g
        })
        .collect()
}

const GOBLIN: &str = "ID_SRD521_MONSTER_GOBLIN_WARRIOR";
const OWLBEAR: &str = "ID_SRD521_MONSTER_OWLBEAR";
const RED_DRAGON: &str = "ID_SRD521_MONSTER_ANCIENT_RED_DRAGON";

#[test]
fn four_level_1_characters_against_goblins_and_an_owlbear() {
    // SRD 5.2.1 p. 202, level 1: 50 / 75 / 100 per character, so 200 / 300 / 400 for four.
    // Four Goblin Warriors (CR 1/4, 50 XP each) are 200 XP: Low. An Owlbear (CR 3, 700 XP) on top: past High.
    let party = [1; 4];
    let r = evaluate_encounter(Ruleset::Edition2024, &party, &groups(&[(GOBLIN, 4)])).unwrap();
    assert_eq!((r.thresholds.low, r.thresholds.moderate, r.thresholds.high), (200, 300, 400));
    assert_eq!((r.total_xp, r.band), (200, Band::Low));
    let r = evaluate_encounter(Ruleset::Edition2024, &party, &groups(&[(GOBLIN, 4), (OWLBEAR, 1)])).unwrap();
    assert_eq!((r.total_xp, r.band), (900, Band::Extreme));
    assert_eq!(r.remaining.high, -500);
}

#[test]
fn four_level_5_characters() {
    // Level 5: 500 / 750 / 1,100 per character, 2,000 / 3,000 / 4,400 for four.
    let party = [5; 4];
    let r = evaluate_encounter(Ruleset::Edition2024, &party, &groups(&[(GOBLIN, 4), (OWLBEAR, 1)])).unwrap();
    assert_eq!((r.thresholds.low, r.thresholds.moderate, r.thresholds.high), (2_000, 3_000, 4_400));
    assert_eq!((r.total_xp, r.band), (900, Band::Low));
    let r = evaluate_encounter(Ruleset::Edition2024, &party, &groups(&[(OWLBEAR, 3)])).unwrap();
    assert_eq!((r.total_xp, r.band), (2_100, Band::Moderate));
    let r = evaluate_encounter(Ruleset::Edition2024, &party, &groups(&[(OWLBEAR, 6)])).unwrap();
    assert_eq!((r.total_xp, r.band), (4_200, Band::High));
}

#[test]
fn five_level_20_characters_against_an_ancient_red_dragon() {
    // Level 20: 6,400 / 13,200 / 22,000 per character, 32,000 / 66,000 / 110,000 for five.
    // The dragon is CR 24, 62,000 XP: Moderate. Two of them: past High.
    let party = [20; 5];
    let r = evaluate_encounter(Ruleset::Edition2024, &party, &groups(&[(RED_DRAGON, 1)])).unwrap();
    assert_eq!((r.thresholds.low, r.thresholds.moderate, r.thresholds.high), (32_000, 66_000, 110_000));
    assert_eq!((r.total_xp, r.band), (62_000, Band::Moderate));
    let r = evaluate_encounter(Ruleset::Edition2024, &party, &groups(&[(RED_DRAGON, 2)])).unwrap();
    assert_eq!((r.total_xp, r.band), (124_000, Band::Extreme));
}

#[test]
fn a_mixed_party_sums_each_characters_budget() {
    // 1 + 1 + 5 + 20: Low 50+50+500+6,400 = 7,000; Moderate 75+75+750+13,200 = 14,100; High 100+100+1,100+22,000.
    let r = evaluate_encounter(Ruleset::Edition2024, &[1, 1, 5, 20], &groups(&[(OWLBEAR, 1)])).unwrap();
    assert_eq!((r.thresholds.low, r.thresholds.moderate, r.thresholds.high), (7_000, 14_100, 23_300));
}

#[test]
fn snapshots_of_srd_monsters_round_trip_through_the_model() {
    let snap = Snapshot::from_element(&srd(GOBLIN)).unwrap();
    assert_eq!(snap.name, "Goblin Warrior");
    assert_eq!(snap.ruleset(), "dnd2024");
    assert_eq!(snap.maps_to.as_deref(), Some("ID_SRD51_MONSTER_GOBLIN"));
    assert_eq!(type_word(&snap.data), "Fey");
    let text = serde_json::to_string(&snap).unwrap();
    let back: Snapshot = serde_json::from_str(&text).unwrap();
    assert_eq!(back, snap);
    assert!(back.data.validate().is_empty(), "{:?}", back.data.validate());
    assert_eq!(block_rev(&back.data), block_rev(&snap.data));
    assert_eq!(block_rev(&snap.data).len(), 16);
}

#[test]
fn only_monsters_are_snapshotted() {
    let mut e = srd(GOBLIN);
    e.ty = "Companion".into();
    assert_eq!(Snapshot::from_element(&e), Err(SnapshotError::NotAMonster));
    let mut e = srd(GOBLIN);
    e.data.insert("not_a_field".into(), json!(1));
    assert_eq!(Snapshot::from_element(&e), Err(SnapshotError::Unreadable));
}

fn form(v: Value) -> OverridesForm {
    serde_json::from_value(v).unwrap()
}

#[test]
fn overrides_change_hp_ac_saves_and_traits_and_keep_the_source_values() {
    let owlbear = block(OWLBEAR);
    let o = Overrides::default()
        .edit(
            form(json!({
                "hp": { "average": 80, "formula": "9d10 + 31" },
                "ac": { "value": 15, "source": "scarred hide" },
                "saves": { "dex": 4 },
                "traits": [{ "name": "Battle Scarred", "text": "It can't be frightened." }],
                "notes": "Tougher, as the module's sidebar says.",
            })),
            &owlbear,
        )
        .unwrap();
    assert_eq!(o.hp.as_ref().unwrap().base, Some(HpValue { average: 59, formula: Some("7d10+21".into()) }));
    assert_eq!(o.hp.as_ref().unwrap().value.formula.as_deref(), Some("9d10+31"));
    assert_eq!(o.ac.as_ref().unwrap().base.as_ref().unwrap().value, 13);
    assert_eq!(o.saves[&Ability::Dex].base, Some(owlbear.save_bonus(Ability::Dex)));
    assert!(o.changes_numbers());

    let used = o.apply(&owlbear);
    assert_eq!(used.hp.as_ref().unwrap().average, 80);
    assert_eq!(used.ac.len(), 1);
    assert_eq!(used.ac[0].value, 15);
    assert_eq!(used.save_bonus(Ability::Dex), 4);
    assert_eq!(used.traits.last().unwrap().name, "Battle Scarred");
    assert!(used.validate().is_empty(), "{:?}", used.validate());
    // The note is the DM's, not part of the block.
    assert!(!serde_json::to_string(&used).unwrap().contains("sidebar"));

    // null drops an override; absent leaves it.
    let o = o.edit(form(json!({ "hp": null, "saves": { "dex": null } })), &owlbear).unwrap();
    assert!(o.hp.is_none() && o.saves.is_empty() && o.ac.is_some());
    assert_eq!(o.traits.len(), 1);
}

#[test]
fn overrides_refuse_what_a_block_cant_say() {
    let owlbear = block(OWLBEAR);
    let none = Overrides::default();
    assert!(matches!(none.edit(form(json!({ "hp": { "average": 0 } })), &owlbear), Err(OverrideError::OutOfRange(_))));
    assert_eq!(none.edit(form(json!({ "hp": { "average": 5, "formula": "lots" } })), &owlbear), Err(OverrideError::BadFormula));
    assert!(none.edit(form(json!({ "ac": { "value": 99 } })), &owlbear).is_err());
    assert!(none.edit(form(json!({ "saves": { "str": 31 } })), &owlbear).is_err());
    assert!(none.edit(form(json!({ "traits": [{ "name": "", "text": "x" }] })), &owlbear).is_err());
    assert!(none.edit(form(json!({ "notes": "a\u{0}b" })), &owlbear).is_err());
    let many: Vec<AddedTrait> = (0..13).map(|i| AddedTrait { name: format!("T{i}"), text: String::new() }).collect();
    assert!(none.edit(OverridesForm { traits: Some(many), ..OverridesForm::default() }, &owlbear).is_err());
    // An unknown key is refused, not dropped.
    assert!(serde_json::from_value::<OverridesForm>(json!({ "xp": 1 })).is_err());
}

#[test]
fn refresh_reports_conflicts_only_where_both_sides_changed() {
    let old = block(OWLBEAR);
    let o = Overrides::default()
        .edit(form(json!({ "hp": { "average": 80 }, "ac": { "value": 15 }, "saves": { "con": 7 } })), &old)
        .unwrap();
    // Unchanged source: nothing to decide.
    let r = refresh(&o, &old, &old, &BTreeMap::new());
    assert!(!r.changed && r.conflicts.is_empty());
    assert_eq!(r.overrides.as_ref(), Some(&o));

    // The source changed its HP and its AC, not its CON save.
    let mut fresh = old.clone();
    fresh.hp.as_mut().unwrap().average = 65;
    fresh.ac[0].value = 14;
    let r = refresh(&o, &old, &fresh, &BTreeMap::new());
    assert!(r.changed);
    let fields: Vec<&str> = r.conflicts.iter().map(|c| c.field.as_str()).collect();
    assert_eq!(fields, ["hp", "ac"]);
    assert_eq!(r.conflicts[0].was["average"], 59);
    assert_eq!(r.conflicts[0].source["average"], 65);
    assert_eq!(r.conflicts[0].value["average"], 80);
    assert!(r.overrides.is_none(), "undecided conflicts change nothing");

    // Keep the DM's HP, take the source's AC.
    let choices = BTreeMap::from([("hp".to_string(), Choice::Keep), ("ac".to_string(), Choice::Source)]);
    let after = refresh(&o, &old, &fresh, &choices).overrides.unwrap();
    assert_eq!(after.hp.as_ref().unwrap().value.average, 80);
    assert_eq!(after.hp.as_ref().unwrap().base.as_ref().unwrap().average, 65);
    assert!(after.ac.is_none());
    assert_eq!(after.saves[&Ability::Con].value, 7);
    // Applied to the new block, the kept HP and the source's AC show.
    let used = after.apply(&fresh);
    assert_eq!((used.hp.unwrap().average, used.ac[0].value), (80, 14));
    // A second refresh against the same source has nothing left to decide.
    assert!(refresh(&after, &fresh, &fresh, &BTreeMap::new()).conflicts.is_empty());

    // A save the source changed is a conflict of its own.
    let mut fresh2 = old.clone();
    fresh2.saves.insert(Ability::Con, 9);
    let r = refresh(&o, &old, &fresh2, &BTreeMap::new());
    assert_eq!(r.conflicts.iter().map(|c| c.field.as_str()).collect::<Vec<_>>(), ["saves.con"]);
    let _ = AcValue { value: 1, source: None };
}

#[test]
fn players_see_only_what_the_reveal_level_allows() {
    let goblin = block(GOBLIN);
    let at = |level| player_view("c-1", "Small fey", level, &goblin, (10, 10), &[]);
    let name = at(RevealLevel::Name);
    assert_eq!(name["publicName"], "Small fey");
    assert_eq!(name["type"], "Fey");
    assert!(name.get("band").is_none() && name.get("statBlock").is_none() && name.get("hp").is_none());
    let band = at(RevealLevel::Band);
    assert_eq!(band["band"], "unhurt");
    assert!(band.get("statBlock").is_none() && band.get("hp").is_none());
    let full = at(RevealLevel::Full);
    assert_eq!(full["hp"]["max"], 10);
    assert!(full["statBlock"].get("xp").is_none(), "XP never reaches players");
    assert_eq!(full["statBlock"]["cr"], "1/4");
    for v in [&name, &band, &full] {
        let text = v.to_string();
        assert!(!text.contains("Goblin Warrior") && !text.contains("ID_SRD521"), "{text}");
    }
    assert_eq!((health_band(0, 10), health_band(5, 10), health_band(6, 10)), ("down", "bloodied", "unhurt"));
    assert_eq!(RevealLevel::default(), RevealLevel::Name);
}
