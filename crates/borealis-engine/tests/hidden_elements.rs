//! `EngineSettings::hidden_elements`: listed elements are never offered for new picks, as a superseded element
//! isn't, while a stored choice of one still replays. An app uses it to show one copy of an element printed in
//! several installed sources. Bundled core + SRD packs plus a small book with two copies of one feat.

use std::collections::BTreeSet;
use std::path::Path;

use borealis_content::{ContentFile, ContentStore, ContentStoreBuilder, PackOrigin, Ruleset};
use borealis_engine::commands;
use borealis_engine::snapshot::CharacterSnapshot;
use borealis_engine::state::Choice;
use borealis_engine::{Engine, EngineSettings};
use serde_json::json;

const CLASS: &str = "ID_T_WALKER";
const COPY_A: &str = "ID_T_GIFT_A";
const COPY_B: &str = "ID_T_GIFT_B";

fn store() -> ContentStore {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let file: ContentFile = serde_json::from_value(json!({
    "format": "borealis-content/1",
    "elements": [
        {"id": CLASS, "name": "Walker", "type": "Class", "source": "Book A", "ruleset": "dnd2014",
         "setters": [{"name": "hd", "value": "d8"}],
         "rules": [{"kind": "select", "type": "Feat", "name": "Walker's Gift", "supports": "T Gift", "level": 1}]},
        {"id": COPY_A, "name": "Walker's Gift", "type": "Feat", "source": "Book A", "ruleset": "dnd2014",
         "supports": ["T Gift"]},
        {"id": COPY_B, "name": "Walker's Gift", "type": "Feat", "source": "Book B", "ruleset": "dnd2014",
         "supports": ["T Gift"]}
    ]}))
    .expect("synthetic content");
    ContentStoreBuilder::new()
        .add_pack_dir(&root.join("core"))
        .and_then(|b| b.add_pack_dir(&root.join("srd-5.1")))
        .expect("bundled packs")
        .add_file("book.json", PackOrigin::Custom, file)
        .build()
}

fn hiding(store: &ContentStore, ids: &[&str]) -> Engine {
    Engine::with_settings(
        store,
        EngineSettings {
            hidden_elements: ids
                .iter()
                .map(|s| (*s).to_string())
                .collect::<BTreeSet<_>>(),
            ..EngineSettings::default()
        },
    )
}

/// A 2014 character with the Walker class.
fn walker(e: &Engine) -> borealis_engine::state::CharacterState {
    let s = commands::new_character(e, Ruleset::Dnd2014);
    let key = commands::slot_key(&e.derive(&s), "ID_LEVEL_1", "Class", 1).expect("class slot");
    commands::select(e, &s, &key, Choice::Element(CLASS.into()))
        .expect("class")
        .state
}

fn gift_options(snap: &CharacterSnapshot) -> Vec<String> {
    let mut ids: Vec<String> = snap
        .slots
        .iter()
        .find(|x| x.name == "Walker's Gift")
        .expect("gift slot")
        .options
        .iter()
        .map(|o| o.id.clone())
        .collect();
    ids.sort();
    ids
}

#[test]
fn hidden_elements_are_not_offered() {
    let store = store();
    let plain = Engine::new(&store);
    assert_eq!(
        gift_options(&plain.derive(&walker(&plain))),
        [COPY_A, COPY_B]
    );
    let e = hiding(&store, &[COPY_B]);
    assert_eq!(gift_options(&e.derive(&walker(&e))), [COPY_A]);
    assert!(
        e.universe(Ruleset::Dnd2014)
            .get(COPY_B)
            .expect("loaded")
            .hidden
    );
    assert!(
        !e.universe(Ruleset::Dnd2014)
            .get(COPY_A)
            .expect("loaded")
            .hidden
    );
}

/// A character that chose a copy before it was hidden keeps it.
#[test]
fn a_stored_choice_of_a_hidden_element_still_replays() {
    let store = store();
    let plain = Engine::new(&store);
    let s = walker(&plain);
    let key = commands::slot_key(&plain.derive(&s), CLASS, "Walker's Gift", 1).expect("gift slot");
    let s = commands::select(&plain, &s, &key, Choice::Element(COPY_B.into()))
        .expect("gift")
        .state;
    let e = hiding(&store, &[COPY_B]);
    let snap = e.derive(&s);
    let slot = snap
        .slots
        .iter()
        .find(|x| x.name == "Walker's Gift")
        .expect("gift slot");
    assert_eq!(slot.chosen.as_ref().map(|c| c.id.as_str()), Some(COPY_B));
    assert!(slot.rejected.is_none() && slot.missing.is_none());
}

// ---------------------------------------------------------------------------------------------------------------
// Stand-ins: content that names a hidden copy by id gets the visible copy
// ---------------------------------------------------------------------------------------------------------------

const WARD_A: &str = "ID_T_SPELL_WARD_A";
const WARD_B: &str = "ID_T_SPELL_WARD_B";

fn ward(id: &str, source: &str) -> serde_json::Value {
    json!({"id": id, "name": "Ward of Testing", "type": "Spell", "source": source, "ruleset": "dnd2014",
           "supports": ["Other", "1"], "setters": [{"name": "level", "value": "1"}]})
}

/// A caster class whose optional feature ("Additional <name> Spells") extends its list by the id
/// of one copy (Book B's) of a spell printed in two books.
/// - `Keeper` prepares from its list and has two selects naming that copy by id.
/// - `Singer` knows spells: a `$(spellcasting:list),1` select.
/// - `Warden` prepares, and its extension makes that copy always prepared (`prepared="true"`).
fn caster(
    name: &str,
    prepare: bool,
    prepared_extension: bool,
    selects: Vec<serde_json::Value>,
) -> Vec<serde_json::Value> {
    let lc = name.to_lowercase();
    let up = name.to_uppercase();
    let mut rules =
        vec![json!({"kind": "stat", "name": format!("{lc}:spellcasting:slots:1"), "value": "2"})];
    if prepare {
        rules.push(
            json!({"kind": "stat", "name": format!("{lc}:spellcasting:prepare"), "value": "3"}),
        );
    }
    rules.extend(selects);
    let mut extend = json!({"supports": WARD_B});
    if prepared_extension {
        extend["prepared"] = json!(true);
    }
    vec![
        json!({"id": format!("ID_T_{up}"), "name": name, "type": "Class", "source": "Book A", "ruleset": "dnd2014",
        "setters": [{"name": "hd", "value": "d8"}],
        "rules": [
            {"kind": "grant", "id": format!("ID_T_{up}_CASTING"), "type": "Class Feature"},
            {"kind": "grant", "id": format!("ID_T_{up}_EXTRA"), "type": "Class Feature"}
        ]}),
        json!({"id": format!("ID_T_{up}_CASTING"), "name": format!("{name} Spellcasting"), "type": "Class Feature",
               "source": "Book A", "ruleset": "dnd2014",
               "spellcasting": {"name": name, "ability": "Wisdom", "prepare": prepare,
                                "list": {"supports": name, "known": prepare}},
               "rules": rules}),
        json!({"id": format!("ID_T_{up}_EXTRA"), "name": format!("Additional {name} Spells"), "type": "Class Feature",
               "source": "Book B", "ruleset": "dnd2014",
               "spellcasting": {"name": name, "extend": true, "extends": [extend]}}),
    ]
}

fn stand_in_store() -> ContentStore {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let named = |n: &str| json!({"kind": "select", "type": "Spell", "name": n, "supports": WARD_B, "spellcasting": "Keeper"});
    let mut elements = caster(
        "Keeper",
        true,
        false,
        vec![named("Keeper Ward"), named("Keeper Ward Again")],
    );
    elements.extend(caster(
        "Singer",
        false,
        false,
        vec![
            json!({"kind": "select", "type": "Spell", "name": "Singer Spell",
                    "supports": "$(spellcasting:list),1", "spellcasting": "Singer"}),
        ],
    ));
    elements.extend(caster("Warden", true, true, Vec::new()));
    elements.push(ward(WARD_A, "Book A"));
    elements.push(ward(WARD_B, "Book B"));
    let file: ContentFile =
        serde_json::from_value(json!({"format": "borealis-content/1", "elements": elements}))
            .expect("synthetic content");
    ContentStoreBuilder::new()
        .add_pack_dir(&root.join("core"))
        .expect("core pack")
        .add_file("book.json", PackOrigin::Custom, file)
        .build()
}

fn hiding_with(store: &ContentStore, hidden: &[&str], stand_ins: &[(&str, &str)]) -> Engine {
    Engine::with_settings(
        store,
        EngineSettings {
            hidden_elements: hidden.iter().map(|s| (*s).to_string()).collect(),
            hidden_stand_ins: stand_ins
                .iter()
                .map(|(h, v)| ((*h).to_string(), (*v).to_string()))
                .collect(),
            ..EngineSettings::default()
        },
    )
}

/// The hidden copy B, with A standing in for it.
fn b_hidden(store: &ContentStore) -> Engine {
    hiding_with(store, &[WARD_B], &[(WARD_B, WARD_A)])
}

/// The other way round: the app swapped which copy is visible.
fn a_hidden(store: &ContentStore) -> Engine {
    hiding_with(store, &[WARD_A], &[(WARD_A, WARD_B)])
}

fn with_class(e: &Engine, class: &str) -> borealis_engine::state::CharacterState {
    let mut s = commands::new_character(e, Ruleset::Dnd2014);
    s.abilities.base = [10, 10, 10, 10, 14, 10];
    let key = commands::slot_key(&e.derive(&s), "ID_LEVEL_1", "Class", 1).expect("class slot");
    commands::select(e, &s, &key, Choice::Element(class.into()))
        .expect("class")
        .state
}

fn options(snap: &CharacterSnapshot, slot: &str) -> Vec<String> {
    let mut ids: Vec<String> = snap
        .slots
        .iter()
        .find(|x| x.name == slot)
        .unwrap_or_else(|| panic!("{slot} slot"))
        .options
        .iter()
        .map(|o| o.id.clone())
        .collect();
    ids.sort();
    ids
}

/// A caster's spells as `(id, always prepared)`.
fn spells_of(snap: &CharacterSnapshot, class: &str) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = snap
        .caster(class)
        .unwrap_or_else(|| panic!("{class} caster"))
        .spells
        .iter()
        .map(|s| (s.id.clone(), s.always_prepared))
        .collect();
    out.sort();
    out
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn a_list_extension_naming_a_hidden_copy_offers_its_stand_in() {
    let store = stand_in_store();
    // Nothing hidden: the extension's own copy is preparable, and the select offers it.
    let plain = Engine::new(&store);
    let snap = plain.derive(&with_class(&plain, "ID_T_KEEPER"));
    assert_eq!(spells_of(&snap, "Keeper"), [(WARD_B.to_string(), false)]);
    assert_eq!(options(&snap, "Keeper Ward"), ids(&[WARD_B]));
    // Its copy hidden, with the other copy standing in: the spell stays, as the visible copy.
    let e = b_hidden(&store);
    let snap = e.derive(&with_class(&e, "ID_T_KEEPER"));
    assert_eq!(spells_of(&snap, "Keeper"), [(WARD_A.to_string(), false)]);
    assert_eq!(options(&snap, "Keeper Ward"), ids(&[WARD_A]));
    let uni = e.universe(Ruleset::Dnd2014);
    assert_eq!(uni.get(WARD_A).expect("loaded").copy_ids, [WARD_B]);
    assert_eq!(uni.get(WARD_B).expect("loaded").copy_ids, [WARD_A]);
}

/// A known caster's `$(spellcasting:list)` select (the class-list check of the graph).
#[test]
fn a_known_casters_list_select_offers_the_stand_in() {
    let store = stand_in_store();
    let plain = Engine::new(&store);
    assert_eq!(
        options(
            &plain.derive(&with_class(&plain, "ID_T_SINGER")),
            "Singer Spell"
        ),
        ids(&[WARD_B])
    );
    let e = b_hidden(&store);
    assert_eq!(
        options(&e.derive(&with_class(&e, "ID_T_SINGER")), "Singer Spell"),
        ids(&[WARD_A])
    );
    let e = hiding_with(&store, &[WARD_B], &[]);
    assert!(options(&e.derive(&with_class(&e, "ID_T_SINGER")), "Singer Spell").is_empty());
}

/// An always-prepared extension naming the hidden copy lists the spell once, as the visible copy.
#[test]
fn a_prepared_extension_naming_a_hidden_copy_lists_the_spell_once() {
    let store = stand_in_store();
    let plain = Engine::new(&store);
    assert_eq!(
        spells_of(&plain.derive(&with_class(&plain, "ID_T_WARDEN")), "Warden"),
        [(WARD_B.to_string(), true)]
    );
    let e = b_hidden(&store);
    assert_eq!(
        spells_of(&e.derive(&with_class(&e, "ID_T_WARDEN")), "Warden"),
        [(WARD_A.to_string(), true)]
    );
}

/// A pick made through a stand-in still replays after the app swaps which copy is visible.
#[test]
fn a_choice_made_through_a_stand_in_replays_after_a_swap() {
    let store = stand_in_store();
    let e = b_hidden(&store);
    let s = with_class(&e, "ID_T_KEEPER");
    let key = commands::slot_key(&e.derive(&s), "ID_T_KEEPER_CASTING", "Keeper Ward", 1)
        .expect("ward slot");
    let s = commands::select(&e, &s, &key, Choice::Element(WARD_A.into()))
        .expect("the stand-in")
        .state;
    let s = commands::set_prepared(&e, &s, "Keeper", WARD_A, true)
        .expect("prepare the stand-in")
        .state;
    let swapped = a_hidden(&store);
    let snap = swapped.derive(&s);
    let slot = snap
        .slots
        .iter()
        .find(|x| x.name == "Keeper Ward")
        .expect("ward slot");
    assert_eq!(slot.chosen.as_ref().map(|c| c.id.as_str()), Some(WARD_A));
    assert!(slot.rejected.is_none() && slot.missing.is_none());
    // the prepared copy stays prepared, and its now-visible copy isn't listed beside it
    let keeper = snap.caster("Keeper").expect("Keeper caster");
    let listed: Vec<(&str, bool)> = keeper
        .spells
        .iter()
        .map(|x| (x.id.as_str(), x.prepared))
        .collect();
    assert_eq!(listed, [(WARD_A, true)]);
}

/// A character that holds the hidden copy isn't offered its stand-in as a second copy.
#[test]
fn a_held_hidden_copy_is_not_offered_again_as_its_stand_in() {
    let store = stand_in_store();
    let plain = Engine::new(&store);
    let s = with_class(&plain, "ID_T_KEEPER");
    let key = commands::slot_key(&plain.derive(&s), "ID_T_KEEPER_CASTING", "Keeper Ward", 1)
        .expect("ward slot");
    let s = commands::select(&plain, &s, &key, Choice::Element(WARD_B.into()))
        .expect("the copy")
        .state;
    let e = b_hidden(&store);
    let snap = e.derive(&s);
    assert!(options(&snap, "Keeper Ward Again").is_empty());
}

/// Without a stand-in, a prepared spell whose copy is hidden drops off the list, as before stand-ins.
#[test]
fn without_stand_ins_a_prepared_hidden_copy_stays_out() {
    let store = stand_in_store();
    let plain = Engine::new(&store);
    let s = with_class(&plain, "ID_T_KEEPER");
    let s = commands::set_prepared(&plain, &s, "Keeper", WARD_B, true)
        .expect("prepare the copy")
        .state;
    let e = hiding_with(&store, &[WARD_B], &[]);
    assert!(spells_of(&e.derive(&s), "Keeper").is_empty());
    let e = hiding_with(&store, &[WARD_A], &[]);
    assert_eq!(
        spells_of(&e.derive(&s), "Keeper"),
        [(WARD_B.to_string(), false)]
    );
}

#[test]
fn without_stand_ins_hiding_works_as_before() {
    let store = stand_in_store();
    let view = |e: &Engine| {
        let snap = e.derive(&with_class(e, "ID_T_KEEPER"));
        (spells_of(&snap, "Keeper"), options(&snap, "Keeper Ward"))
    };
    // No stand-in: the named copy is hidden and nothing answers for it (the earlier behaviour).
    assert_eq!(
        view(&hiding_with(&store, &[WARD_B], &[])),
        (Vec::new(), Vec::new())
    );
    // A stand-in that is hidden too, with nothing behind it, is ignored.
    assert_eq!(
        view(&hiding_with(&store, &[WARD_A, WARD_B], &[(WARD_B, WARD_A)])),
        (Vec::new(), Vec::new())
    );
    // A stand-in for an id that isn't hidden changes nothing.
    let e = hiding_with(&store, &[], &[(WARD_B, WARD_A)]);
    assert_eq!(
        view(&e),
        (vec![(WARD_B.to_string(), false)], ids(&[WARD_B]))
    );
    assert!(e
        .universe(Ruleset::Dnd2014)
        .get(WARD_A)
        .expect("loaded")
        .copy_ids
        .is_empty());
}
