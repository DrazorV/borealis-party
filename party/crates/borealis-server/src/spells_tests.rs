//! Spells as mechanics (0.3.2): one spell per edition with the DM's choice, the account's mechanics rows (def, link,
//! alias) and how combat resolves them, the JSON file export and import, account isolation, and the upgrade from the
//! 0.3.1 schema (steps 1 to 8) with rows present. Runs on SQLite, and on PostgreSQL too when
//! `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.
//!
//! Spells are the bundled SRD ones (SRD 5.1 Fireball is `ID_PHB_SPELL_FIREBALL`, SRD 5.2.1's is
//! `ID_2024_SPELL_FIREBALL`); a same-named copy comes from a made-up homebrew pack.

use std::path::Path;

use axum::http::StatusCode;
use serde_json::{json, Value};
use sqlx::Row;

use super::{drop_all, router_on, send, sign_in, sign_up, Client};
use crate::db::{Db, Location};

const FIREBALL_2014: &str = "ID_PHB_SPELL_FIREBALL";
const FIREBALL_2024: &str = "ID_2024_SPELL_FIREBALL";
const DELAYED_2024: &str = "ID_2024_SPELL_DELAYED_BLAST_FIREBALL";
const GOBLIN: &str = "ID_SRD521_MONSTER_GOBLIN_WARRIOR";

/// S1's fixture definitions: `[2014, 2024]` of one spell.
fn fixture(name: &str, edition: usize) -> Value {
    let text = match name {
        "fireball" => include_str!("../../party-combat/fixtures/spells/fireball.json"),
        "mage-armor" => include_str!("../../party-combat/fixtures/spells/mage-armor.json"),
        "hold-person" => include_str!("../../party-combat/fixtures/spells/hold-person.json"),
        _ => unreachable!(),
    };
    serde_json::from_str::<Value>(text).unwrap()[edition].clone()
}

async fn ok(client: &Client, method: &str, path: &str, body: Value) -> Value {
    let (status, out) = call(client, method, path, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {out}");
    out
}

async fn call(client: &Client, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    send(&client.router, Some(&client.token), method, path, body).await
}

fn set(name: &str, value: &str) -> Value {
    json!({ "name": name, "value": value })
}

/// The account's homebrew pack for these tests.
async fn spell_pack(client: &Client) -> String {
    let (status, out) = call(
        client,
        "POST",
        "/api/homebrew/packs",
        json!({ "name": "Spell Test", "description": "Made up for tests.", "author": "Tester" }),
    )
    .await;
    if status == StatusCode::OK {
        return out["id"].as_str().unwrap().to_string();
    }
    "hb-spell-test".to_string()
}

/// A made-up level 1 homebrew spell, answering its id.
async fn homebrew_spell(client: &Client, name: &str) -> String {
    let pack = spell_pack(client).await;
    let saved = ok(
        client,
        "POST",
        &format!("/api/homebrew/packs/{pack}/elements"),
        json!({ "form": { "type": "Spell", "name": name, "supports": "Wizard",
            "description": "A made-up spell for tests.",
            "setters": [set("level", "1"), set("school", "Evocation"), set("time", "1 action"),
                        set("range", "60 feet"), set("duration", "Instantaneous"),
                        set("hasVerbalComponent", "true")] } }),
    )
    .await;
    saved["id"].as_str().unwrap().to_string()
}

/// A homebrew copy of Fireball (Aurora XML: 2014 rules), answering its id.
async fn homebrew_fireball(client: &Client) -> String {
    let pack = spell_pack(client).await;
    let saved = ok(
        client,
        "POST",
        &format!("/api/homebrew/packs/{pack}/elements"),
        json!({ "form": { "type": "Spell", "name": "Fireball", "supports": "Wizard",
            "description": "A made-up copy of a spell for tests.",
            "setters": [set("level", "3"), set("school", "Evocation"), set("time", "1 action"),
                        set("range", "150 feet"), set("duration", "Instantaneous"),
                        set("hasVerbalComponent", "true")] } }),
    )
    .await;
    saved["id"].as_str().unwrap().to_string()
}

fn find<'a>(page: &'a Value, key: &str) -> Option<&'a Value> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["key"] == key)
}

/// The account's spells, one per name and edition, and the DM's choice of copy.
async fn one_spell_checks(state: &crate::AppState, dm: &Client, other: &Client) {
    // The editions stay apart, and different spells never merge.
    let new = ok(
        dm,
        "GET",
        "/api/spells?ruleset=dnd2024&text=fireball",
        Value::Null,
    )
    .await;
    let fireball = find(&new, FIREBALL_2024).unwrap_or_else(|| panic!("{new}"));
    assert_eq!(fireball["ruleset"], "dnd2024");
    assert_eq!(fireball["level"], 3);
    assert_eq!(fireball["chosenBy"], "only");
    assert_eq!(fireball["coverage"], "auto", "the curated SRD row");
    assert!(find(&new, DELAYED_2024).is_some(), "{new}");
    assert!(find(&new, FIREBALL_2014).is_none(), "{new}");
    let old = ok(
        dm,
        "GET",
        "/api/spells?ruleset=dnd2014&text=fireball",
        Value::Null,
    )
    .await;
    assert!(find(&old, FIREBALL_2014).is_some(), "{old}");
    assert!(find(&old, FIREBALL_2024).is_none(), "{old}");
    let cantrips = ok(
        dm,
        "GET",
        "/api/spells?ruleset=dnd2024&level=0&limit=500",
        Value::Null,
    )
    .await;
    assert!(cantrips["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["level"] == 0));
    let (status, _) = call(dm, "GET", "/api/spells?ruleset=dnd1999", Value::Null).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A same-named copy of the same edition is the same spell: listed once, its copy under alsoIn. Homebrew is
    // Aurora XML, whose content is 2014 rules unless it says otherwise.
    let copy = homebrew_fireball(dm).await;
    let old = ok(
        dm,
        "GET",
        "/api/spells?ruleset=dnd2014&text=fireball",
        Value::Null,
    )
    .await;
    let fireball = find(&old, FIREBALL_2014).unwrap_or_else(|| panic!("{old}"));
    assert_eq!(fireball["alsoIn"][0]["key"], copy, "{fireball}");
    assert!(
        find(&old, &copy).is_none(),
        "the copy is hidden, not listed: {old}"
    );
    let total = old["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["name"] == "Fireball")
        .count();
    assert_eq!(total, 1, "{old}");
    assert_eq!(
        fireball["chosenBy"], "srd",
        "a copy of no known date: the SRD one"
    );
    let all = ok(dm, "GET", "/api/spells?text=fireball", Value::Null).await;
    assert_eq!(all["total"], 4, "two editions, two spells each: {all}");
    let new = ok(
        dm,
        "GET",
        "/api/spells?ruleset=dnd2024&text=fireball",
        Value::Null,
    )
    .await;
    assert_eq!(
        find(&new, FIREBALL_2024).unwrap()["alsoIn"],
        json!([]),
        "never the other edition's copy"
    );

    // The DM picks the copy for 2014; 2024 keeps its own.
    let chosen = ok(
        dm,
        "POST",
        &format!("/api/spells/{copy}/choose"),
        Value::Null,
    )
    .await;
    assert_eq!(chosen["spell"]["key"], copy, "{chosen}");
    assert_eq!(chosen["spell"]["chosenBy"], "dmPick");
    assert_eq!(chosen["spell"]["alsoIn"][0]["key"], FIREBALL_2014);
    let old = ok(
        dm,
        "GET",
        "/api/spells?ruleset=dnd2014&text=fireball",
        Value::Null,
    )
    .await;
    assert!(
        find(&old, &copy).is_some() && find(&old, FIREBALL_2014).is_none(),
        "{old}"
    );
    let new = ok(
        dm,
        "GET",
        "/api/spells?ruleset=dnd2024&text=fireball",
        Value::Null,
    )
    .await;
    assert!(find(&new, FIREBALL_2024).is_some(), "{new}");
    // The choice is the spell's details too, from any copy.
    let detail = ok(
        dm,
        "GET",
        &format!("/api/spells/{FIREBALL_2014}"),
        Value::Null,
    )
    .await;
    assert_eq!(detail["spell"]["key"], copy, "{detail}");
    assert_eq!(detail["element"]["id"], FIREBALL_2014);
    assert!(detail["sourceHash"].as_str().unwrap().len() == 32);
    // Mechanics key on the one spell: the SRD definition applies to the chosen copy.
    let mine = ok(dm, "GET", &format!("/api/spells/{copy}"), Value::Null).await;
    assert_eq!(mine["mechanics"]["origin"], "srd", "{mine}");
    assert_eq!(mine["mechanics"]["fromKey"], FIREBALL_2014);
    assert_eq!(
        mine["suggested"],
        Value::Null,
        "a spell with mechanics needs no draft"
    );
    let named = crate::spells::resolve_name(state, &dm.id, "Fireball", "dnd2014")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        named.key, copy,
        "a spellbook line resolves to the chosen copy"
    );
    // Another account has neither the copy nor the choice.
    let theirs = ok(
        other,
        "GET",
        "/api/spells?ruleset=dnd2014&text=fireball",
        Value::Null,
    )
    .await;
    let f = find(&theirs, FIREBALL_2014).unwrap_or_else(|| panic!("{theirs}"));
    assert_eq!(f["alsoIn"], json!([]));
    let (status, _) = call(other, "GET", &format!("/api/spells/{copy}"), Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Back to the rule.
    let back = ok(
        dm,
        "DELETE",
        &format!("/api/spells/{copy}/choose"),
        Value::Null,
    )
    .await;
    assert_eq!(back["spell"]["key"], FIREBALL_2014, "{back}");
    // A spell with no definition anywhere gets a draft to confirm, never bound by itself.
    let bolt = homebrew_spell(dm, "Test Bolt").await;
    let detail = ok(dm, "GET", &format!("/api/spells/{bolt}"), Value::Null).await;
    assert_eq!(detail["mechanics"], Value::Null, "{detail}");
    assert_eq!(detail["suggested"]["mech"]["key"], bolt, "{detail}");
    assert!(detail["suggested"]["checks"].is_array(), "{detail}");
    let list = ok(dm, "GET", "/api/spells?text=test%20bolt", Value::Null).await;
    assert_eq!(list["items"][0]["coverage"], "ruling", "{list}");
    let (status, _) = call(dm, "GET", "/api/spells/ID_NO_SUCH_SPELL", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

fn def_row(key: &str, ruleset: &str, def: Value) -> Value {
    json!({ "kind": "def", "spellKey": key, "ruleset": ruleset, "def": def })
}

/// Defs, links and aliases; what combat would use.
async fn mechanics_checks(state: &crate::AppState, dm: &Client, other: &Client) {
    let fireball = fixture("fireball", 1);
    let saved = ok(
        dm,
        "PUT",
        "/api/spell-mechanics",
        def_row(FIREBALL_2024, "dnd2024", fireball.clone()),
    )
    .await;
    assert_eq!(saved["row"]["rev"], 1, "{saved}");
    assert_eq!(saved["row"]["origin"], "dm");
    let again = ok(
        dm,
        "PUT",
        "/api/spell-mechanics",
        def_row(FIREBALL_2024, "dnd2024", fireball.clone()),
    )
    .await;
    assert_eq!(again["row"]["rev"], 2, "every save is a new revision");
    let list = ok(
        dm,
        "GET",
        "/api/spells?ruleset=dnd2024&text=fireball",
        Value::Null,
    )
    .await;
    assert_eq!(find(&list, FIREBALL_2024).unwrap()["coverage"], "defined");

    // The 5.2.1 Delayed Blast Fireball uses Fireball's mechanics through a link.
    let link = ok(
        dm,
        "PUT",
        "/api/spell-mechanics",
        json!({ "kind": "link", "spellKey": DELAYED_2024, "ruleset": "dnd2024",
                "def": { "target": FIREBALL_2024, "ignored": true } }),
    )
    .await;
    assert_eq!(link["row"]["def"], json!({ "target": FIREBALL_2024 }));
    let got = crate::spells::mechanics_for(
        state,
        &dm.id,
        "dnd2024",
        &[
            FIREBALL_2024.to_string(),
            DELAYED_2024.to_string(),
            FIREBALL_2014.to_string(),
        ],
    )
    .await
    .unwrap();
    assert_eq!(got[FIREBALL_2024].origin, "def");
    assert_eq!(got[FIREBALL_2024].rev, 2);
    let mech =
        |v: &Value| serde_json::from_value::<party_combat::spell::SpellMech>(v.clone()).unwrap();
    assert_eq!(
        mech(&got[FIREBALL_2024].def),
        mech(&fireball),
        "stored as the engine reads it"
    );
    assert_eq!(got[DELAYED_2024].origin, "link");
    assert_eq!(got[DELAYED_2024].from_key, FIREBALL_2024);
    assert!(
        !got.contains_key(FIREBALL_2014),
        "a def of 2024 is not 2014's"
    );
    let other_got =
        crate::spells::mechanics_for(state, &other.id, "dnd2024", &[FIREBALL_2024.to_string()])
            .await
            .unwrap();
    assert_eq!(
        other_got[FIREBALL_2024].origin, "srd",
        "another account's rows are not mine: the curated row"
    );
    let detail = ok(
        dm,
        "GET",
        &format!("/api/spells/{DELAYED_2024}"),
        Value::Null,
    )
    .await;
    assert_eq!(detail["mechanics"]["origin"], "link", "{detail}");
    // Saving a def replaces the link: a spell is defined or linked, never both.
    ok(
        dm,
        "PUT",
        "/api/spell-mechanics",
        def_row(DELAYED_2024, "dnd2024", fixture("hold-person", 1)),
    )
    .await;
    let rows = ok(dm, "GET", "/api/spell-mechanics", Value::Null).await;
    let delayed: Vec<&Value> = rows["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["spellKey"] == DELAYED_2024)
        .collect();
    assert_eq!(delayed.len(), 1, "{rows}");
    assert_eq!(delayed[0]["kind"], "def");

    // An alias: a spellbook name the DM resolved once.
    ok(
        dm,
        "PUT",
        "/api/spell-mechanics",
        json!({ "kind": "alias", "spellKey": FIREBALL_2024, "ruleset": "dnd2024", "aliasName": "Fire Ball!" }),
    )
    .await;
    let named = crate::spells::resolve_name(state, &dm.id, "fire  ball", "dnd2024")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(named.key, FIREBALL_2024);
    let plain = crate::spells::resolve_name(state, &dm.id, "FIREBALL", "dnd2014")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(plain.key, FIREBALL_2014);
    assert!(
        crate::spells::resolve_name(state, &dm.id, "No Such Spell", "dnd2024")
            .await
            .unwrap()
            .is_none()
    );

    // Rows that don't fit are refused, and change nothing.
    for bad in [
        json!({ "kind": "script", "spellKey": FIREBALL_2024, "ruleset": "dnd2024" }),
        json!({ "kind": "def", "spellKey": FIREBALL_2024, "ruleset": "dnd2099", "def": {} }),
        json!({ "kind": "def", "spellKey": FIREBALL_2024, "ruleset": "dnd2024", "def": "text" }),
        json!({ "kind": "def", "spellKey": "", "ruleset": "dnd2024", "def": {} }),
        json!({ "kind": "link", "spellKey": FIREBALL_2024, "ruleset": "dnd2024", "def": { "target": FIREBALL_2024 } }),
        json!({ "kind": "link", "spellKey": FIREBALL_2024, "ruleset": "dnd2024", "def": {} }),
        json!({ "kind": "alias", "spellKey": FIREBALL_2024, "ruleset": "dnd2024", "aliasName": "..." }),
        json!({ "kind": "def", "spellKey": FIREBALL_2024, "ruleset": "dnd2024", "def": {}, "origin": "ai" }),
        json!({ "kind": "def", "spellKey": FIREBALL_2024, "ruleset": "dnd2024", "def": {}, "extra": 1 }),
        json!({ "kind": "def", "spellKey": FIREBALL_2024, "ruleset": "dnd2024",
                "def": { "big": "x".repeat(70 * 1024) } }),
        // Not the engine's schema: no scripting, no unknown fields, no wrong edition or version.
        json!({ "kind": "def", "spellKey": FIREBALL_2024, "ruleset": "dnd2024", "def": { "v": 1 } }),
        def_row(FIREBALL_2024, "dnd2024", {
            let mut d = fixture("fireball", 1);
            d["script"] = json!("x");
            d
        }),
        def_row(FIREBALL_2024, "dnd2024", fixture("fireball", 0)),
        def_row(FIREBALL_2024, "dnd2024", {
            let mut d = fixture("fireball", 1);
            d["v"] = json!(2);
            d
        }),
        def_row(FIREBALL_2024, "dnd2024", {
            let mut d = fixture("fireball", 1);
            d["on_land"][0]["dice"] = json!("8d6+system()");
            d
        }),
    ] {
        let (status, out) = call(dm, "PUT", "/api/spell-mechanics", bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {out}");
    }
    let after = ok(dm, "GET", "/api/spell-mechanics", Value::Null).await;
    assert_eq!(after["rows"].as_array().unwrap().len(), 3, "{after}");

    // Removing.
    let removed = ok(
        dm,
        "DELETE",
        "/api/spell-mechanics",
        json!({ "kind": "alias", "spellKey": FIREBALL_2024, "ruleset": "dnd2024", "aliasName": "fire ball" }),
    )
    .await;
    assert_eq!(removed["removed"], true);
    let removed = ok(
        dm,
        "DELETE",
        "/api/spell-mechanics",
        json!({ "kind": "alias", "spellKey": FIREBALL_2024, "ruleset": "dnd2024", "aliasName": "fire ball" }),
    )
    .await;
    assert_eq!(removed["removed"], false);
}

/// The file: export, import into another account (merge and replace), and refusals that write nothing.
async fn file_checks(dm: &Client, other: &Client) {
    let file = ok(dm, "GET", "/api/spell-mechanics/export", Value::Null).await;
    assert_eq!(file["format"], "borealis-party-spell-mechanics");
    assert_eq!(file["v"], 1);
    let mine = file["rows"].as_array().unwrap().len();
    assert!(mine >= 2, "{file}");
    // Something of their own first; merge keeps it.
    ok(
        other,
        "PUT",
        "/api/spell-mechanics",
        def_row(FIREBALL_2014, "dnd2014", fixture("fireball", 0)),
    )
    .await;
    let out = ok(other, "POST", "/api/spell-mechanics/import", file.clone()).await;
    assert_eq!(out["imported"], mine, "{out}");
    assert_eq!(out["total"], mine + 1, "{out}");
    let theirs = ok(other, "GET", "/api/spell-mechanics", Value::Null).await;
    let imported: Vec<&Value> = theirs["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["spellKey"] != FIREBALL_2014)
        .collect();
    assert!(imported.iter().all(|r| r["origin"] == "import"), "{theirs}");
    let defs = |rows: &Value| -> Vec<(String, Value)> {
        rows.as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "def")
            .map(|r| {
                (
                    r["spellKey"].as_str().unwrap().to_string(),
                    r["def"].clone(),
                )
            })
            .collect()
    };
    let want = defs(&file["rows"]);
    for (k, d) in &want {
        assert!(
            defs(&theirs["rows"]).contains(&(k.clone(), d.clone())),
            "{k}"
        );
    }
    // Replace: only the file's rows remain.
    let out = ok(
        other,
        "POST",
        "/api/spell-mechanics/import?mode=replace",
        file.clone(),
    )
    .await;
    assert_eq!(out["total"], mine, "{out}");
    // A file that isn't ours, a newer one, or one bad row: refused, nothing written.
    let before = ok(other, "GET", "/api/spell-mechanics", Value::Null).await;
    let mut bad_row = file.clone();
    bad_row["rows"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "kind": "def", "spellKey": "X", "ruleset": "nope", "def": {} }));
    let mut newer = file.clone();
    newer["v"] = json!(99);
    let mut wrong = file.clone();
    wrong["format"] = json!("something-else");
    for (bad, what) in [
        (bad_row, "a bad row"),
        (newer, "newer"),
        (wrong, "not ours"),
        (json!({ "rows": [] }), "no format"),
    ] {
        let (status, out) = call(
            other,
            "POST",
            "/api/spell-mechanics/import?mode=replace",
            bad,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {out}");
    }
    let (status, out) = call(
        other,
        "POST",
        "/api/spell-mechanics/import?mode=wipe",
        file.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    let after = ok(other, "GET", "/api/spell-mechanics", Value::Null).await;
    assert_eq!(before, after);
}

async fn flows(db: Db, data: &Path) {
    let state = super::state_with(data.to_path_buf(), db, super::Setup::default());
    let router = super::router_of(state.clone(), 64 * 1024 * 1024);
    let dm = sign_up(&router, "spell-dm").await;
    let other = sign_up(&router, "spell-other").await;
    // Signed out: refused.
    for path in [
        "/api/spells",
        "/api/spell-mechanics",
        "/api/spell-mechanics/export",
    ] {
        let (status, _) = send(&router, None, "GET", path, Value::Null).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
    one_spell_checks(&state, &dm, &other).await;
    mechanics_checks(&state, &dm, &other).await;
    file_checks(&dm, &other).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn spells_are_one_per_edition_with_the_dms_mechanics() {
    let dir = tempfile::tempdir().unwrap();
    flows(Db::open(&Location::Memory).await.unwrap(), dir.path()).await;
}

// ---------------------------------------------------------------------------------------------------------------
// The upgrade from the 0.3.1 schema (steps 1 to 8), with rows present
// ---------------------------------------------------------------------------------------------------------------

async fn has_table(db: &Db, name: &str) -> bool {
    sqlx::query(if db.kind() == "sqlite" {
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = $1"
    } else {
        "SELECT 1 FROM information_schema.tables WHERE table_schema = current_schema() AND table_name = $1"
    })
    .bind(name)
    .fetch_optional(db.pool())
    .await
    .unwrap()
    .is_some()
}

/// A database as 0.3.1 left it: a DM with a party, an encounter with a creature and a legacy starting effect.
async fn seed_0_3_1(location: &Location, data: &Path) -> (String, String) {
    let old = Db::open_at(location, 8).await.unwrap();
    assert!(!has_table(&old, "spell_mechanics").await);
    let router = router_on(data.to_path_buf(), old.clone());
    let _admin = sign_up(&router, "up-admin").await;
    let dm = sign_up(&router, "up-dm").await;
    let (status, out) = dm.post("/api/me/roles/dm", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, party) = dm
        .post("/api/parties", json!({ "name": "Before 0.3.2" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{party}");
    let party = party["id"].as_str().unwrap().to_string();
    // The encounter rows as 0.3.1 wrote them. HEAD's encounter routes read the step 9 tables, so the rows are written
    // directly; the creature's stat block copy comes from the same route on a scratch database.
    let scratch = Db::open(&Location::Memory).await.unwrap();
    let scratch_dir = tempfile::tempdir().unwrap();
    let r = router_on(scratch_dir.path().to_path_buf(), scratch.clone());
    let sdm = sign_up(&r, "scratch-dm").await;
    let (_, out) = sdm.post("/api/me/roles/dm", json!({})).await;
    assert!(out.is_object());
    let (_, sp) = sdm.post("/api/parties", json!({ "name": "Scratch" })).await;
    let se = ok(
        &sdm,
        "POST",
        &format!("/api/parties/{}/encounters", sp["id"].as_str().unwrap()),
        json!({ "name": "Scratch" }),
    )
    .await;
    ok(
        &sdm,
        "POST",
        &format!("/api/encounters/{}/creatures", se["id"].as_str().unwrap()),
        json!({ "elementId": GOBLIN }),
    )
    .await;
    let row = sqlx::query(
        "SELECT kind, dm_label, public_name, element_id, pack_id, pack_rev, statblock_json FROM encounter_creatures",
    )
    .fetch_one(scratch.pool())
    .await
    .unwrap();
    let enc = "11111111-1111-4111-8111-111111111111".to_string();
    let creature = "22222222-2222-4222-8222-222222222222".to_string();
    sqlx::query(
        "INSERT INTO encounters (id, party_id, name, public_name, created_at, updated_at)
         VALUES ($1, $2, 'Old one', 'Old one', 1, 1)",
    )
    .bind(&enc)
    .bind(&party)
    .execute(old.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO encounter_creatures (id, encounter_id, sort, kind, dm_label, public_name, element_id, pack_id,
            pack_rev, statblock_json, created_at, updated_at)
         VALUES ($1, $2, 0, $3, $4, $5, $6, $7, $8, $9, 1, 1)",
    )
    .bind(&creature)
    .bind(&enc)
    .bind(row.try_get::<String, _>(0).unwrap())
    .bind(row.try_get::<String, _>(1).unwrap())
    .bind(row.try_get::<String, _>(2).unwrap())
    .bind(row.try_get::<String, _>(3).unwrap())
    .bind(row.try_get::<String, _>(4).unwrap())
    .bind(row.try_get::<String, _>(5).unwrap())
    .bind(row.try_get::<String, _>(6).unwrap())
    .execute(old.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO encounter_effects (id, encounter_id, creature_id, sort, name, slot_level, created_at)
         VALUES ('33333333-3333-4333-8333-333333333333', $1, $2, 0, 'Old ward', 1, 1)",
    )
    .bind(&enc)
    .bind(&creature)
    .execute(old.pool())
    .await
    .unwrap();
    scratch.pool().close().await;
    old.pool().close().await;
    (enc, creature)
}

/// HEAD over the seeded database: the old rows, the new tables, and a re-run when a marker table is missing.
async fn check_upgraded(location: &Location, data: &Path, enc: &str, creature: &str) {
    let db = Db::open(location).await.unwrap();
    let version: i64 = sqlx::query("SELECT MAX(version) FROM schema_version")
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(version, i64::try_from(Db::schema_version()).unwrap());
    assert!(version >= 9, "the spell tables come with step 9");
    assert!(has_table(&db, "spell_mechanics").await && has_table(&db, "encounter_casts").await);
    let effects: i64 =
        sqlx::query("SELECT COUNT(*) FROM encounter_effects WHERE encounter_id = $1")
            .bind(enc)
            .fetch_one(db.pool())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    assert_eq!(effects, 1, "the legacy effect is kept");
    let router = router_on(data.to_path_buf(), db.clone());
    let dm = sign_in(&router, "up-dm").await;
    let view = ok(&dm, "GET", &format!("/api/encounters/{enc}"), Value::Null).await;
    assert_eq!(view["effects"][0]["name"], "Old ward", "{view}");
    ok(
        &dm,
        "PUT",
        "/api/spell-mechanics",
        def_row(FIREBALL_2024, "dnd2024", fixture("fireball", 1)),
    )
    .await;
    // Precast rows go with their encounter and creature.
    sqlx::query(
        "INSERT INTO encounter_casts (id, encounter_id, caster_creature_id, sort, spell_key, name, slot_level,
            targets_json, choices_json, created_at)
         VALUES ('cast-1', $1, $2, 0, $3, 'Fireball', 3, '[]', '{}', 1)",
    )
    .bind(enc)
    .bind(creature)
    .bind(FIREBALL_2024)
    .execute(db.pool())
    .await
    .unwrap();
    // A database that says step 9 ran but lacks its tables (another numbering) gets them again; nothing is lost.
    for table in ["encounter_casts", "spell_mechanics"] {
        sqlx::query(&format!("DROP TABLE {table}"))
            .execute(db.pool())
            .await
            .unwrap();
    }
    db.pool().close().await;
    let again = Db::open(location).await.unwrap();
    assert!(
        has_table(&again, "spell_mechanics").await && has_table(&again, "encounter_casts").await
    );
    let router = router_on(data.to_path_buf(), again.clone());
    let dm = sign_in(&router, "up-dm").await;
    let rows = ok(&dm, "GET", "/api/spell-mechanics", Value::Null).await;
    assert_eq!(rows["rows"], json!([]));
    let view = ok(&dm, "GET", &format!("/api/encounters/{enc}"), Value::Null).await;
    assert_eq!(view["effects"][0]["name"], "Old ward");
    // Opening once more changes nothing.
    again.pool().close().await;
    let third = Db::open(location).await.unwrap();
    let count: i64 = sqlx::query("SELECT COUNT(*) FROM schema_version WHERE version = 9")
        .fetch_one(third.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(count, 1);
    third.pool().close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn spells_arrive_on_a_0_3_1_database() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("party.db");
    let data = dir.path().join("data");
    let location = Location::Sqlite(file.clone());
    let (enc, creature) = seed_0_3_1(&location, &data).await;
    check_upgraded(&location, &data, &enc, &creature).await;
    // And the instance still moves to another database, spell rows and all.
    let db = Db::open(&location).await.unwrap();
    let router = router_on(data.clone(), db.clone());
    let dm = sign_in(&router, "up-dm").await;
    ok(
        &dm,
        "PUT",
        "/api/spell-mechanics",
        def_row(FIREBALL_2024, "dnd2024", fixture("fireball", 1)),
    )
    .await;
    db.pool().close().await;
    let moved = Db::open(&Location::Memory).await.unwrap();
    let report = crate::migrate::adopt_sqlite(&moved, &file)
        .await
        .unwrap()
        .unwrap();
    assert!(
        crate::migrate::describe(&report).contains("spell_mechanics 1"),
        "{}",
        crate::migrate::describe(&report)
    );
}

/// Everything above on PostgreSQL. Runs when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.
#[tokio::test(flavor = "multi_thread")]
async fn spells_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = super::POSTGRES.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let location = Location::Postgres(url.clone());
    drop_all(&url).await;
    let (enc, creature) = seed_0_3_1(&location, &dir.path().join("up")).await;
    check_upgraded(&location, &dir.path().join("up"), &enc, &creature).await;
    drop_all(&url).await;
    flows(
        Db::open(&location).await.unwrap(),
        &dir.path().join("flows"),
    )
    .await;
    drop_all(&url).await;
}

// ---------------------------------------------------------------------------------------------------------------
// Hidden copies: one spell per edition in the account's pickers
// ---------------------------------------------------------------------------------------------------------------

async fn hidden_now(client: &Client) -> Vec<String> {
    let (status, out) = client.rpc("get_settings", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let mut ids: Vec<String> = out["hiddenElements"]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
        .unwrap_or_default();
    ids.sort();
    ids
}

/// The account's stand-ins: each hidden copy with the chosen copy that answers for it.
async fn stand_ins_now(client: &Client) -> Value {
    let (status, out) = client.rpc("get_settings", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    out.get("hiddenStandIns")
        .cloned()
        .unwrap_or_else(|| json!({}))
}

#[test]
fn a_settings_file_from_before_stand_ins_still_loads() {
    let old: borealis_lib::settings::AppSettings =
        serde_json::from_value(json!({ "hiddenElements": ["ID_T_COPY"] })).unwrap();
    assert!(old.hidden_stand_ins.is_empty());
    assert!(old.engine_settings().hidden_stand_ins.is_empty());
    let saved = serde_json::to_value(&old).unwrap();
    assert!(
        saved.get("hiddenStandIns").is_none(),
        "an empty map is left out of the file"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_other_copies_of_a_spell_are_hidden_from_new_picks() {
    let dir = tempfile::tempdir().unwrap();
    let router = router_on(
        dir.path().to_path_buf(),
        Db::open(&Location::Memory).await.unwrap(),
    );
    let dm = sign_up(&router, "hide-dm").await;
    let other = sign_up(&router, "hide-other").await;
    // The bundled SRD has one copy of each spell per edition: nothing to hide.
    assert_eq!(hidden_now(&dm).await, Vec::<String>::new());
    // A second copy of Fireball (2014): the SRD one is the spell, the copy is hidden (on the next command).
    let copy = homebrew_fireball(&dm).await;
    assert_eq!(hidden_now(&dm).await, vec![copy.clone()]);
    // Content that names the hidden copy by id (a list extension) gets the chosen one.
    assert_eq!(
        stand_ins_now(&dm).await,
        json!({ copy.clone(): FIREBALL_2014 })
    );
    assert_eq!(
        hidden_now(&other).await,
        Vec::<String>::new(),
        "per account"
    );
    // A client can't change the list itself.
    let (status, out) = dm
        .rpc(
            "save_settings",
            json!({ "patch": { "hiddenElements": [],
                "hiddenStandIns": { copy.clone(): null, "ID_T_ANY": FIREBALL_2014 } } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(hidden_now(&dm).await, vec![copy.clone()]);
    assert_eq!(
        stand_ins_now(&dm).await,
        json!({ copy.clone(): FIREBALL_2014 }),
        "nor the stand-ins"
    );
    // The DM's pick swaps them; back to the rule swaps them back.
    ok(
        &dm,
        "POST",
        &format!("/api/spells/{copy}/choose"),
        Value::Null,
    )
    .await;
    assert_eq!(hidden_now(&dm).await, vec![FIREBALL_2014.to_string()]);
    assert_eq!(
        stand_ins_now(&dm).await,
        json!({ FIREBALL_2014: copy.clone() })
    );
    ok(
        &dm,
        "DELETE",
        &format!("/api/spells/{copy}/choose"),
        Value::Null,
    )
    .await;
    assert_eq!(hidden_now(&dm).await, vec![copy.clone()]);
    // Restoring the default settings hides them again.
    let (status, out) = dm.rpc("restore_default_settings", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(hidden_now(&dm).await, vec![copy.clone()]);
    // A character still loads with the list in place (upstream replays a stored pick of a hidden copy: its test).
    let (status, out) = dm
        .rpc("create_character", super::new_character("Hidden Test"))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    let (status, out) = dm.rpc("get_snapshot", json!({ "characterId": id })).await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

// ---------------------------------------------------------------------------------------------------------------
// Opt-in: the user's own installed content (the opt-in content folders of the other corpus tests). Counts only.
// ---------------------------------------------------------------------------------------------------------------

/// Every class-list extension that names a spell by id (an optional feature's or subclass's `<extend>`, an imported
/// expanded list) still reaches the class's lists when that copy is hidden: its chosen copy stands in for it.
#[tokio::test(flavor = "multi_thread")]
async fn list_extensions_reach_a_visible_copy_in_installed_content() {
    use tauri::Manager;
    let aurora = std::env::var("BOREALIS_TEST_AURORA_DIR").ok();
    let fivetools = std::env::var("BOREALIS_TEST_5ETOOLS_DIR").ok();
    if aurora.is_none() && fivetools.is_none() {
        eprintln!("skipped: set BOREALIS_TEST_AURORA_DIR (cached Aurora Legacy indexes) and/or BOREALIS_TEST_5ETOOLS_DIR");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    let state = super::state_with(dir.path().to_path_buf(), db, super::Setup::default());
    let router = super::router_of(state.clone(), super::Setup::default().max_upload);
    let dm = sign_up(&router, "corpus-dm").await;
    let workspace = state.workspaces.get(&dm.id).await.unwrap();
    if let Some(cache) = aurora {
        let cache = std::path::PathBuf::from(cache);
        workspace
            .blocking(move |h| {
                let backend = h.state::<borealis_lib::state::AppState>();
                for url in super::bestiary_tests::AURORA_INDEXES {
                    backend
                        .add_index(url)
                        .map_err(|e| crate::error::ApiError::Internal(e.message))?;
                }
                backend
                    .update_indexes(
                        &super::bestiary_tests::Cached(cache),
                        &borealis_lib::content_packs::NullSink,
                    )
                    .map_err(|e| crate::error::ApiError::Internal(e.message))?;
                Ok(())
            })
            .await
            .unwrap();
    }
    if let Some(root) = fivetools {
        let root = std::path::PathBuf::from(root);
        let data = if root.join("data").is_dir() {
            root.join("data")
        } else {
            root
        };
        let index: Value =
            serde_json::from_str(&std::fs::read_to_string(data.join("spells/index.json")).unwrap())
                .unwrap();
        let sources: Vec<String> = index.as_object().unwrap().keys().cloned().collect();
        let token = workspace
            .blocking(move |h| {
                let picked = h
                    .state::<borealis_lib::state::AppState>()
                    .picked
                    .grant(
                        data,
                        borealis_lib::picked_paths::PickPurpose::FivetoolsFolder,
                    )
                    .map_err(|e| crate::error::ApiError::Internal(e.message))?;
                Ok(picked.token)
            })
            .await
            .unwrap();
        let (status, out) = dm
            .rpc(
                "import_5etools",
                json!({ "request": { "folder": token, "sources": sources, "includeFluff": false } }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{}", out["message"]);
    }
    crate::spells::sync_hidden(&state, &dm.id).await.unwrap();
    let counts = workspace
        .blocking(|h| {
            let hub = h
                .state::<borealis_lib::state::AppState>()
                .hub()
                .map_err(|e| crate::error::ApiError::Internal(e.message))?;
            let mut out = Vec::new();
            for ruleset in [
                borealis_content::Ruleset::Dnd2014,
                borealis_content::Ruleset::Dnd2024,
            ] {
                let uni = hub.engine.universe(ruleset);
                let answered: std::collections::HashSet<&str> = uni
                    .iter()
                    .filter(|u| !u.hidden)
                    .flat_map(|u| u.copy_ids.iter().map(String::as_str))
                    .collect();
                let (mut refs, mut missing, mut hidden, mut unanswered) =
                    (0usize, 0usize, 0usize, 0usize);
                for u in uni.iter() {
                    let Some(sc) =
                        u.el.spellcasting
                            .as_ref()
                            .filter(|s| s.extend == Some(true))
                    else {
                        continue;
                    };
                    for l in sc.extends.iter().filter(|l| l.is_spell_id()) {
                        refs += 1;
                        let id = l.supports.trim();
                        match uni.get(id) {
                            None => missing += 1,
                            Some(t) if t.hidden => {
                                hidden += 1;
                                unanswered += usize::from(!answered.contains(id));
                            }
                            Some(_) => {}
                        }
                    }
                }
                out.push((ruleset, refs, missing, hidden, unanswered));
            }
            Ok(out)
        })
        .await
        .unwrap();
    for (ruleset, refs, missing, hidden, unanswered) in &counts {
        println!(
            "{ruleset:?}: {refs} list extensions by spell id; {missing} name no installed spell; \
             {hidden} name a hidden copy (unreachable before stand-ins); {unanswered} of them with no stand-in"
        );
    }
    assert!(
        counts.iter().all(|c| c.4 == 0),
        "a hidden copy named by a list extension has no stand-in"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// The DM tools follow the account's content rules (0.3.5): sources, toggles, one copy, edition
// ---------------------------------------------------------------------------------------------------------------

/// A made-up homebrew spell with its own id and source name.
async fn spell_in(client: &Client, id: &str, name: &str, source: &str, level: &str) -> String {
    let pack = spell_pack(client).await;
    let saved = ok(
        client,
        "POST",
        &format!("/api/homebrew/packs/{pack}/elements"),
        json!({ "form": { "type": "Spell", "name": name, "id": id, "source": source, "supports": "Wizard",
            "description": "A made-up spell for tests.",
            "setters": [set("level", level), set("school", "Evocation"), set("time", "1 action"),
                        set("range", "60 feet"), set("duration", "Instantaneous"),
                        set("hasVerbalComponent", "true")] } }),
    )
    .await;
    saved["id"].as_str().unwrap().to_string()
}

/// Every installed source name but `off`.
async fn sources_but(client: &Client, off: &[&str]) -> Vec<String> {
    let (status, out) = client.rpc("list_sources", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let all: Vec<String> = out
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    for name in off {
        assert!(
            all.iter().any(|s| s == name),
            "{name} is installed: {all:?}"
        );
    }
    all.into_iter()
        .filter(|s| !off.contains(&s.as_str()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_dm_tools_offer_only_what_the_accounts_characters_may_pick() {
    let dir = tempfile::tempdir().unwrap();
    let router = router_on(
        dir.path().to_path_buf(),
        Db::open(&Location::Memory).await.unwrap(),
    );
    let dm = sign_up(&router, "rules-dm").await;
    let zephyr = spell_in(
        &dm,
        "ID_TEST_SPELL_ZEPHYR_SPARK",
        "Zephyr Spark",
        "Off Book",
        "1",
    )
    .await;
    // Two copies of Fireball (2014) in one made-up book: the SRD copy is the spell, the book is named once.
    let a = spell_in(
        &dm,
        "ID_TEST_SPELL_FIREBALL_A",
        "Fireball",
        "Made Up Grimoire",
        "3",
    )
    .await;
    let b = spell_in(
        &dm,
        "ID_TEST_SPELL_FIREBALL_B",
        "Fireball",
        "Made Up Grimoire",
        "3",
    )
    .await;

    // Every source on: both listed, the made-up book once under "also in".
    let all = ok(
        &dm,
        "POST",
        "/api/spells/search",
        json!({ "text": "zephyr" }),
    )
    .await;
    assert!(find(&all, &zephyr).is_some(), "{all}");
    let page = ok(
        &dm,
        "POST",
        "/api/spells/search",
        json!({ "text": "fireball", "forRuleset": "dnd2014" }),
    )
    .await;
    let fireball = find(&page, FIREBALL_2014).unwrap_or_else(|| panic!("{page}"));
    let copies: Vec<&str> = fireball["alsoIn"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["key"].as_str().unwrap())
        .collect();
    assert!(
        copies.contains(&a.as_str()) && copies.contains(&b.as_str()),
        "{fireball}"
    );
    assert_eq!(
        fireball["alsoInSources"],
        json!(["Made Up Grimoire"]),
        "{fireball}"
    );
    // The edition rule of a new 2014 pick: no 2024 spells (a 2024 creature may take 2014 ones).
    assert!(find(&page, FIREBALL_2024).is_none(), "{page}");
    assert!(find(&page, DELAYED_2024).is_none(), "{page}");
    let new = ok(
        &dm,
        "POST",
        "/api/spells/search",
        json!({ "text": "fireball", "forRuleset": "dnd2024" }),
    )
    .await;
    assert!(find(&new, FIREBALL_2024).is_some(), "{new}");

    // The Sources page's sources: a book that is off is never offered, nor named under "also in".
    let on = sources_but(&dm, &["Off Book", "Made Up Grimoire"]).await;
    let none = ok(
        &dm,
        "POST",
        "/api/spells/search",
        json!({ "text": "zephyr", "enabledSources": on }),
    )
    .await;
    assert_eq!(none["total"], 0, "{none}");
    let page = ok(
        &dm,
        "POST",
        "/api/spells/search",
        json!({ "text": "fireball", "forRuleset": "dnd2014", "enabledSources": on }),
    )
    .await;
    let fireball = find(&page, FIREBALL_2014).unwrap_or_else(|| panic!("{page}"));
    assert_eq!(fireball["alsoIn"], json!([]), "{fireball}");
    assert_eq!(fireball["alsoInSources"], json!([]), "{fireball}");
    // The one copy the resolver chose is off: the spell is not offered (its other copies stay hidden), as in the
    // character builder.
    let on = sources_but(&dm, &["SRD 5.1"]).await;
    let page = ok(
        &dm,
        "POST",
        "/api/spells/search",
        json!({ "text": "fireball", "forRuleset": "dnd2014", "enabledSources": on }),
    )
    .await;
    assert!(find(&page, FIREBALL_2014).is_none(), "{page}");
    assert!(
        find(&page, &a).is_none() && find(&page, &b).is_none(),
        "{page}"
    );

    // The bestiary and the creature pickers: a creature of a book that is off is never listed.
    let goblins = ok(
        &dm,
        "POST",
        "/api/bestiary/search",
        json!({ "text": "goblin warrior" }),
    )
    .await;
    let goblin = goblins["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == GOBLIN)
        .unwrap_or_else(|| panic!("{goblins}"));
    let book = goblin["source"].as_str().unwrap().to_string();
    let on = sources_but(&dm, &[&book]).await;
    let off = ok(
        &dm,
        "POST",
        "/api/bestiary/search",
        json!({ "text": "goblin warrior", "enabledSources": on }),
    )
    .await;
    assert!(
        off["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["id"] != GOBLIN),
        "{off}"
    );
}
