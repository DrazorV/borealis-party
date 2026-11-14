//! A book installed twice under two abbreviations (an Aurora file's and a converted pack's, both
//! naming the same source): the proxy ids of either abbreviation resolve, each to its own copy
//! of the target, and the catalogue lists each copy once.

use borealis_content::{ContentFile, ContentStoreBuilder, PackOrigin, Ruleset};
use borealis_engine::Engine;
use serde_json::{json, Value};
use std::path::Path;

const BOOK: &str = "Book of Frost";

fn pack(elements: Vec<Value>) -> ContentFile {
    serde_json::from_value(
        json!({"format": "borealis-content/1", "ruleset": "dnd2014", "elements": elements}),
    )
    .expect("synthetic content")
}

/// The converted pack first (it sorts first among installed packs), then the Aurora file.
fn engine(with_aurora: bool) -> Engine {
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/core");
    let converted = pack(vec![
        json!({"id":"ID_CNV_BOF_SOURCE","name":BOOK,"type":"Source","source":"Internal",
               "setters":[{"name":"abbreviation","value":"BOF"}]}),
        json!({"id":"ID_CNV_BOF_SPELL_FROST_KNOT","name":"Frost Knot","type":"Spell","source":BOOK,
               "setters":[{"name":"level","value":"2"}]}),
    ]);
    let aurora = pack(vec![
        json!({"id":"ID_HB_BOFR_SOURCE_BOOK_OF_FROST","name":BOOK,"type":"Source","source":"Core",
               "setters":[{"name":"abbreviation","value":"BOFR"}]}),
        json!({"id":"ID_HB_BOFR_SPELL_FROST_KNOT","name":"Frost Knot","type":"Spell","source":BOOK,
               "setters":[{"name":"level","value":"2"}]}),
    ]);
    let mut b = ContentStoreBuilder::new()
        .add_pack_dir(&core)
        .expect("core pack")
        .add_file("converted.json", PackOrigin::Custom, converted);
    if with_aurora {
        b = b.add_file("aurora.json", PackOrigin::Custom, aurora);
    }
    Engine::new(&b.build())
}

const AURORA_PROXY: &str = "ID_BOFR_INTERNAL_ITEM__SPELL_PROXY_SPELL_FROST_KNOT";
const CONVERTED_PROXY: &str = "ID_BOF_INTERNAL_ITEM__SPELL_PROXY_SPELL_FROST_KNOT";

/// The element the proxy item grants.
fn granted(uni: &borealis_engine::universe::Universe, proxy: &str) -> String {
    let item = uni.get(proxy).unwrap_or_else(|| panic!("{proxy} resolves"));
    let rules = format!("{:?}", item.el.rules);
    ["ID_HB_BOFR_SPELL_FROST_KNOT", "ID_CNV_BOF_SPELL_FROST_KNOT"]
        .into_iter()
        .find(|id| rules.contains(id))
        .unwrap_or_else(|| panic!("{proxy} grants a copy: {rules}"))
        .to_string()
}

#[test]
fn both_abbreviations_resolve_each_to_its_own_copy() {
    let e = engine(true);
    let uni = e.universe(Ruleset::Dnd2014);
    assert_eq!(granted(uni, AURORA_PROXY), "ID_HB_BOFR_SPELL_FROST_KNOT");
    assert_eq!(granted(uni, CONVERTED_PROXY), "ID_CNV_BOF_SPELL_FROST_KNOT");
}

#[test]
fn the_catalogue_lists_each_copy_once() {
    let e = engine(true);
    let uni = e.universe(Ruleset::Dnd2014);
    let mut ids: Vec<String> = uni
        .proxies()
        .all()
        .filter(|p| p.source == BOOK)
        .map(|p| p.id)
        .collect();
    ids.sort();
    let mut want = [AURORA_PROXY, CONVERTED_PROXY];
    want.sort_unstable();
    assert_eq!(ids, want);
}

#[test]
fn an_aurora_proxy_resolves_to_the_converted_copy_when_only_that_is_installed() {
    let e = engine(false);
    let uni = e.universe(Ruleset::Dnd2014);
    assert_eq!(granted(uni, AURORA_PROXY), "ID_CNV_BOF_SPELL_FROST_KNOT");
}
