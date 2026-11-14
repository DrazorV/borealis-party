//! The bestiary layer (`bestiary.rs`): coverage and search over the bundled SRD packs, Aurora companions listed beside
//! the monsters (D17 b), a 5etools-format import whose dropped details are captured into `monster_meta`, the
//! upgrade from the 0.2.x schema (SQLite and PostgreSQL), and two opt-in suites over real data that never comes into
//! the repository:
//! - `BOREALIS_TEST_5ETOOLS_DIR`: a local 5etools source checkout (the folder that holds `data/`);
//! - `BOREALIS_TEST_AURORA_DIR`: a folder where the Aurora Legacy indexes and their files are cached.
//!
//! Every creature named here is an SRD creature; the fixture's extra details (mythic actions, a coven CR, ...) are
//! made up for the test. The opt-in suites print counts only.

use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use base64::Engine;
use serde_json::{json, Value};
use sqlx::Row;
use tauri::Manager;

use super::{drop_all, router_of, router_on, sign_in, sign_up, state_with, Client, Setup};
use crate::bestiary::{self, meta_of, read_creatures};
use crate::db::{Db, Location};
use borealis_lib::state::AppState as Backend;

/// The bundled SRD stat blocks.
const SRD51: usize = 317;
const SRD521: usize = 330;
/// The made-up 5etools source of the fixture, and its pack.
const SOURCE: &str = "BTEST";
const PACK: &str = "5etools-btest";

/// The SRD 5.1 Wolf as 5etools writes a creature, renamed and with `extra` fields on top.
fn creature(name: &str, extra: &Value) -> Value {
    let mut c = json!({
        "name": name, "source": SOURCE,
        "size": ["M"], "type": "beast", "alignment": ["U"],
        "ac": [{"ac": 13, "from": ["natural armor"]}],
        "hp": {"average": 11, "formula": "2d8 + 2"},
        "speed": {"walk": 40},
        "str": 12, "dex": 15, "con": 12, "int": 3, "wis": 12, "cha": 6,
        "skill": {"perception": "+3", "stealth": "+4"},
        "passive": 13, "cr": "1/4",
        "trait": [{"name": "Pack Tactics", "entries": ["The creature has advantage on attack rolls against a creature if at least one of its allies is within 5 feet of the creature."]}],
        "action": [{"name": "Bite", "entries": ["{@atk mw} {@hit 4} to hit, reach 5 ft., one target. {@h}7 ({@damage 2d4 + 2}) piercing damage."]}]
    });
    for (k, v) in extra.as_object().unwrap() {
        c[k] = v.clone();
    }
    c
}

/// A 5etools-format homebrew file with five SRD-named creatures: four come in (one with mythic actions, one with a
/// coven CR and a variant sidebar, one with an alternate speed), one is skipped (an ability score that is no number).
fn fixture() -> Value {
    json!({
        "_meta": { "sources": [{ "json": SOURCE, "full": "Bestiary Test" }] },
        "monster": [
            creature("Wolf", &json!({ "page": 341, "environment": ["forest", "grassland", "hill"] })),
            creature("Werewolf", &json!({
                "cr": "3", "type": {"type": "humanoid", "tags": ["human", "shapechanger"]},
                "speed": {"walk": 30, "alternate": {"walk": [{"number": 40, "condition": "(wolf form only)"}]}},
                "environment": ["forest"], "hasToken": true
            })),
            creature("Green Hag", &json!({
                "cr": {"cr": "3", "coven": "5"}, "type": "fey", "environment": ["swamp", "forest"],
                "variant": [{"type": "inset", "name": "Hag Coven", "entries": ["Hags gather in covens of three."]}],
                "hasFluff": true,
                "spellcasting": [{
                    "name": "Innate Spellcasting", "type": "spellcasting", "ability": "cha",
                    "headerEntries": ["The hag's innate spellcasting ability is Charisma ({@dc 12})."],
                    "will": ["{@spell dancing lights}", "{@spell minor illusion}", "{@spell vicious mockery}"],
                    "displayAs": "trait"
                }]
            })),
            creature("Adult Red Dragon", &json!({
                "cr": "17", "size": ["H"], "type": "dragon", "environment": ["mountain", "hill"],
                "legendary": [{"name": "Detect", "entries": ["The dragon makes a Wisdom (Perception) check."]}],
                "mythicHeader": ["If the dragon's mythic trait is active, it can use the options below as legendary actions."],
                "mythic": [{"name": "Searing Wing", "entries": ["The dragon beats its wings."]}]
            })),
            creature("Goblin", &json!({ "str": "varies" })),
        ]
    })
}

async fn ok(client: &Client, method: &str, path: &str, body: Value) -> Value {
    let (status, out) = match method {
        "GET" => client.get(path).await,
        _ => client.post(path, body).await,
    };
    assert_eq!(status, StatusCode::OK, "{method} {path}: {out}");
    out
}

async fn search(client: &Client, query: Value) -> Value {
    ok(client, "POST", "/api/bestiary/search", query).await
}

fn names(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_string())
        .collect()
}

fn pack<'a>(coverage: &'a Value, id: &str) -> &'a Value {
    coverage["packs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["packId"] == id)
        .unwrap_or_else(|| panic!("no {id} in {coverage}"))
}

/// Import a 5etools-format homebrew file through the account's own commands, as the Import section does.
async fn import_file(client: &Client, file: &Value) -> Value {
    let bytes = serde_json::to_vec(file).unwrap();
    let (status, staged) = client
        .rpc(
            "stage_5etools_files",
            json!({ "files": [{
                "fileName": "bestiary-test.json",
                "contentBase64": base64::engine::general_purpose::STANDARD.encode(bytes),
            }] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{staged}");
    import_token(client, staged["token"].as_str().unwrap(), &[SOURCE]).await
}

async fn import_token(client: &Client, token: &str, sources: &[&str]) -> Value {
    let (status, out) = client
        .rpc(
            "import_5etools",
            json!({ "request": { "folder": token, "sources": sources, "includeFluff": false } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Always on: SRD packs and an SRD-named fixture
// ---------------------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn the_srd_bestiaries_are_complete_and_searchable() {
    let dir = tempfile::tempdir().unwrap();
    let router = super::router(dir.path().to_path_buf()).await;
    let dm = sign_up(&router, "bestiary-srd").await;
    srd_checks(&dm).await;
}

async fn srd_checks(dm: &Client) {
    let coverage = ok(dm, "GET", "/api/bestiary/coverage", Value::Null).await;
    for (id, n, ruleset) in [
        ("srd-5.1-monsters", SRD51, "dnd2014"),
        ("srd-5.2.1-monsters", SRD521, "dnd2024"),
    ] {
        let p = pack(&coverage, id);
        assert_eq!(
            (p["status"].as_str(), p["kind"].as_str()),
            (Some("complete"), Some("bundled")),
            "{p}"
        );
        assert_eq!(
            (p["monsters"].as_u64(), p["inData"].as_u64()),
            (Some(n as u64), Some(n as u64))
        );
        assert_eq!(
            p["rulesets"],
            json!([{ "value": ruleset, "count": n }]),
            "{p}"
        );
        let by_cr: u64 = p["byCr"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["count"].as_u64().unwrap())
            .sum();
        let by_type: u64 = p["byType"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["count"].as_u64().unwrap())
            .sum();
        assert_eq!(
            (by_cr, by_type),
            (n as u64, n as u64),
            "every SRD block has a CR and a type"
        );
        assert_eq!(p["captured"], false);
    }
    assert_eq!(
        coverage["totals"]["monsters"].as_u64(),
        Some((SRD51 + SRD521) as u64)
    );

    // Everything, a page at a time, in name order.
    let all = search(dm, json!({ "limit": 200 })).await;
    assert_eq!(all["total"].as_u64(), Some((SRD51 + SRD521) as u64));
    assert_eq!(all["items"].as_array().unwrap().len(), 200);
    let second = search(dm, json!({ "limit": 200, "offset": 200 })).await;
    assert!(names(&all)[199] <= names(&second)[0]);
    let past = search(dm, json!({ "offset": 5000 })).await;
    assert_eq!(
        (
            past["total"].as_u64(),
            past["items"].as_array().unwrap().len()
        ),
        (Some((SRD51 + SRD521) as u64), 0)
    );

    // Editions.
    let old = search(dm, json!({ "ruleset": "dnd2014", "limit": 1 })).await;
    let new = search(dm, json!({ "ruleset": "dnd2024", "limit": 1 })).await;
    assert_eq!(
        (old["total"].as_u64(), new["total"].as_u64()),
        (Some(SRD51 as u64), Some(SRD521 as u64))
    );

    // Name: a prefix match comes first.
    let goblins = search(dm, json!({ "text": "goblin", "ruleset": "dnd2024" })).await;
    let found = names(&goblins);
    assert!(
        found.iter().all(|n| n.to_lowercase().contains("goblin")),
        "{found:?}"
    );
    assert!(found.contains(&"Goblin Warrior".to_string()), "{found:?}");
    let warrior = goblins["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "Goblin Warrior")
        .unwrap();
    assert_eq!(warrior["id"], "ID_SRD521_MONSTER_GOBLIN_WARRIOR");
    assert_eq!(warrior["kind"], "monster");
    assert_eq!(warrior["cr"].as_str(), Some("1/4"));
    assert!(warrior.get("xp").is_none(), "no XP anywhere (D16)");
    assert_eq!(
        (warrior["packId"].as_str(), warrior["ruleset"].as_str()),
        (Some("srd-5.2.1-monsters"), Some("dnd2024"))
    );
    assert!(
        warrior["packRev"].as_str().is_some_and(|r| r.len() == 16),
        "{warrior}"
    );
    let dragons = search(dm, json!({ "text": "red dragon" })).await;
    assert!(
        names(&dragons).contains(&"Adult Red Dragon".to_string()),
        "{dragons}"
    );

    // CR range, inclusive, and CR order.
    let low = search(
        dm,
        json!({ "crMin": "1/8", "crMax": "1/2", "sort": "cr", "limit": 200 }),
    )
    .await;
    let values: Vec<f64> = low["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["crValue"].as_f64().unwrap())
        .collect();
    assert!(
        !values.is_empty() && values.iter().all(|v| (0.125..=0.5).contains(v)),
        "{values:?}"
    );
    assert!(values.windows(2).all(|w| w[0] <= w[1]), "sorted by CR");
    assert!(
        values.contains(&0.125) && values.contains(&0.5),
        "bounds are inclusive"
    );

    // Type, size, legendary.
    let dragons = search(
        dm,
        json!({ "types": ["dragon"], "sizes": ["huge"], "legendary": true, "limit": 200 }),
    )
    .await;
    assert!(dragons["total"].as_u64().unwrap() > 0);
    for i in dragons["items"].as_array().unwrap() {
        assert_eq!(
            (i["type"].as_str(), i["legendary"].as_bool()),
            (Some("dragon"), Some(true))
        );
        assert!(i["size"].as_array().unwrap().contains(&json!("huge")));
    }
    let plain = search(dm, json!({ "legendary": false, "limit": 1 })).await;
    let legendary = search(dm, json!({ "legendary": true, "limit": 1 })).await;
    assert_eq!(
        plain["total"].as_u64().unwrap() + legendary["total"].as_u64().unwrap(),
        (SRD51 + SRD521) as u64
    );

    // Source by pack id or source name; the SRD packs have no environments.
    let by_pack = search(dm, json!({ "sources": ["srd-5.1-monsters"], "limit": 1 })).await;
    assert_eq!(by_pack["total"].as_u64(), Some(SRD51 as u64));
    let name = warrior["source"].as_str().unwrap();
    let by_name = search(dm, json!({ "sources": [name], "limit": 1 })).await;
    assert_eq!(by_name["total"].as_u64(), Some(SRD521 as u64));
    let swamp = search(dm, json!({ "environments": ["swamp"] })).await;
    assert_eq!(swamp["total"].as_u64(), Some(0));

    // Bad filters are refused in words.
    for bad in [
        json!({ "crMin": "1/3" }),
        json!({ "types": ["dinosaur"] }),
        json!({ "sizes": ["enormous"] }),
        json!({ "ruleset": "dnd3" }),
        json!({ "sort": "hp" }),
        json!({ "limit": 0 }),
        json!({ "limit": 201 }),
        json!({ "text": "x".repeat(101) }),
        json!({ "types": vec!["beast"; 51] }),
        json!({ "colour": "red" }),
    ] {
        let (status, out) = dm.post("/api/bestiary/search", bad.clone()).await;
        assert!(status.is_client_error(), "{bad}: {status} {out}");
    }

    // Facets.
    let facets = ok(dm, "GET", "/api/bestiary/facets", Value::Null).await;
    assert_eq!(facets["total"].as_u64(), Some((SRD51 + SRD521) as u64));
    let crs: Vec<&str> = facets["crs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert_eq!(&crs[..4], ["0", "1/8", "1/4", "1/2"], "{crs:?}");
    let types = facets["types"].as_array().unwrap();
    assert!(types.len() >= 13, "{types:?}");
    let sources = facets["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 2, "{sources:?}");

    // One creature: the stat block an encounter snapshots.
    let detail = ok(
        dm,
        "GET",
        "/api/bestiary/monsters/ID_SRD521_MONSTER_GOBLIN_WARRIOR",
        Value::Null,
    )
    .await;
    assert_eq!(
        (detail["name"].as_str(), detail["packRev"].clone()),
        (Some("Goblin Warrior"), warrior["packRev"].clone())
    );
    let block: borealis_content::MonsterData =
        serde_json::from_value(detail["monster"].clone()).unwrap();
    // The block is sent without XP (D16); upstream's validation asks for it with a CR, and nothing else may fail.
    let issues: Vec<_> = block
        .validate()
        .into_iter()
        .filter(|i| !i.field.starts_with("xp"))
        .collect();
    assert!(issues.is_empty(), "{issues:?}");
    assert!(block.xp.is_none() && block.xp_lair.is_none(), "no XP (D16)");
    assert_eq!(detail["meta"], Value::Null);
    assert_eq!(
        (detail["kind"].as_str(), detail["companion"].clone()),
        (Some("monster"), Value::Null)
    );
    for id in [
        "ID_SRD521_MONSTER_NOPE",
        "ID_SRD521_SPELL_FIREBALL",
        "../etc",
    ] {
        let (status, _) = dm.get(&format!("/api/bestiary/monsters/{id}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{id}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn signed_out_calls_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let router = super::router(dir.path().to_path_buf()).await;
    for (method, path) in [
        ("GET", "/api/bestiary/coverage"),
        ("POST", "/api/bestiary/search"),
        ("GET", "/api/bestiary/facets"),
        (
            "GET",
            "/api/bestiary/monsters/ID_SRD521_MONSTER_GOBLIN_WARRIOR",
        ),
    ] {
        let (status, _) = super::send(&router, None, method, path, json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_5etools_import_keeps_what_the_stat_block_drops() {
    let dir = tempfile::tempdir().unwrap();
    let router = super::router(dir.path().to_path_buf()).await;
    let dm = sign_up(&router, "bestiary-dm").await;
    let other = sign_up(&router, "bestiary-other").await;
    import_checks(&dm, &other, dir.path()).await;
}

async fn import_checks(dm: &Client, other: &Client, data: &Path) {
    let report = import_file(dm, &fixture()).await;
    assert_eq!(
        report["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["kind"] == "monster")
            .count(),
        1,
        "{report}"
    );
    let spool = data.join("users").join(&dm.id).join(bestiary::SPOOL_DIR);
    assert!(
        spool.join(format!("{PACK}.json")).is_file(),
        "the import spooled its capture"
    );

    let coverage = ok(dm, "GET", "/api/bestiary/coverage", Value::Null).await;
    assert!(
        !spool.join(format!("{PACK}.json")).exists(),
        "and the next call stored it"
    );
    let p = pack(&coverage, PACK);
    assert_eq!(
        (
            p["kind"].as_str(),
            p["status"].as_str(),
            p["captured"].as_bool()
        ),
        (Some("5etools"), Some("partial"), Some(true)),
        "{p}"
    );
    assert_eq!(
        (
            p["monsters"].as_u64(),
            p["inData"].as_u64(),
            p["skipped"].as_u64(),
            p["partial"].as_u64()
        ),
        (Some(4), Some(5), Some(1), Some(3)),
        "{p}"
    );
    let notes: Vec<&str> = p["partialNotes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["value"].as_str().unwrap())
        .collect();
    assert!(
        notes.contains(&"mythic actions") && notes.contains(&"variant sidebars"),
        "{notes:?}"
    );
    assert_eq!(p["skippedReasons"].as_array().unwrap().len(), 1, "{p}");
    assert_eq!(
        p["extras"],
        json!({ "environment": 4, "page": 1, "mythic": 1, "covenCr": 1, "altSpeeds": 1, "variants": 1,
                "reactionIntro": 0, "fluff": 1, "tokens": 1, "spellcasting": 1 }),
    );
    assert_eq!(
        p["byCr"],
        json!([{ "value": "1/4", "count": 1 }, { "value": "3", "count": 2 }, { "value": "17", "count": 1 }])
    );

    // Environment, the mythic flag and the page in search.
    let swamp = search(dm, json!({ "environments": ["swamp"] })).await;
    assert_eq!(names(&swamp), ["Green Hag"]);
    let hills = search(dm, json!({ "environments": ["HILL"], "sources": [PACK] })).await;
    assert_eq!(names(&hills), ["Adult Red Dragon", "Wolf"]);
    let dragon = &hills["items"][0];
    assert_eq!(
        (dragon["mythic"].as_bool(), dragon["legendary"].as_bool()),
        (Some(true), Some(true))
    );
    assert_eq!(hills["items"][1]["page"].as_i64(), Some(341));
    let werewolf = search(dm, json!({ "text": "werewolf", "sources": [PACK] })).await;
    let id = werewolf["items"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(id, "ID_5ET_BTEST_MONSTER_WEREWOLF");

    // The details of one creature.
    let detail = ok(
        dm,
        "GET",
        &format!("/api/bestiary/monsters/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        detail["meta"]["altSpeeds"],
        json!({ "walk": [{ "feet": 40, "condition": "(wolf form only)" }] })
    );
    assert_eq!(detail["meta"]["token"], true);
    let hag = ok(
        dm,
        "GET",
        "/api/bestiary/monsters/ID_5ET_BTEST_MONSTER_GREEN_HAG",
        Value::Null,
    )
    .await;
    assert_eq!(hag["meta"]["covenCr"], json!({ "cr": "5" }));
    // The raw spellcasting blocks are kept for the spellbook parser (0.3.2), as the data has them.
    assert_eq!(
        hag["meta"]["spellcasting"][0]["will"][2], "{@spell vicious mockery}",
        "{}",
        hag["meta"]
    );
    assert_eq!(hag["meta"]["spellcasting"][0]["ability"], "cha");
    assert!(
        detail["meta"].get("spellcasting").is_none(),
        "none for a creature without it"
    );
    assert!(
        hag["monster"].is_object()
            && hag["monster"].get("xp").is_none()
            && hag["monster"].get("xp_lair").is_none(),
        "the stat block has no XP (D16): {}",
        hag["monster"]
    );
    assert_eq!(
        hag["meta"]["variants"],
        json!([{ "name": "Hag Coven", "text": "Hags gather in covens of three." }])
    );
    assert_eq!(hag["monster"]["cr"], "3", "the stat block keeps its own CR");
    let red = ok(
        dm,
        "GET",
        "/api/bestiary/monsters/ID_5ET_BTEST_MONSTER_ADULT_RED_DRAGON",
        Value::Null,
    )
    .await;
    assert_eq!(
        red["meta"]["mythic"]["actions"],
        json!([{ "name": "Searing Wing", "text": "The dragon beats its wings." }])
    );
    assert!(red["meta"]["mythic"]["intro"]
        .as_str()
        .unwrap()
        .starts_with("If the dragon's mythic trait"));

    // Another account has none of it.
    let (status, _) = other.get(&format!("/api/bestiary/monsters/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let theirs = search(other, json!({ "sources": [PACK] })).await;
    assert_eq!(theirs["total"].as_u64(), Some(0));
    let theirs = ok(other, "GET", "/api/bestiary/coverage", Value::Null).await;
    assert!(theirs["packs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["packId"] != PACK));

    // Imported again with a change: the pack's rows are replaced whole, never doubled.
    let mut changed = fixture();
    changed["monster"][0]["environment"] = json!(["arctic"]);
    let before = search(dm, json!({ "sources": [PACK] })).await;
    import_file(dm, &changed).await;
    let after = search(dm, json!({ "sources": [PACK] })).await;
    assert_eq!(after["total"], before["total"]);
    assert_ne!(
        after["items"][0]["packRev"], before["items"][0]["packRev"],
        "a new pack revision"
    );
    let arctic = search(dm, json!({ "environments": ["arctic"] })).await;
    assert_eq!(names(&arctic), ["Wolf"]);
    let hills = search(dm, json!({ "environments": ["hill"] })).await;
    assert_eq!(names(&hills), ["Adult Red Dragon"]);
    let coverage = ok(dm, "GET", "/api/bestiary/coverage", Value::Null).await;
    assert_eq!(pack(&coverage, PACK)["status"], "partial");
}

/// A 5etools pack imported before this version (no record) is "unknown" until imported again; the rows of an older
/// pack revision are ignored.
#[tokio::test(flavor = "multi_thread")]
async fn a_pack_without_its_import_record_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    let router = router_on(dir.path().to_path_buf(), db.clone());
    let dm = sign_up(&router, "bestiary-old").await;
    import_file(&dm, &fixture()).await;
    ok(&dm, "GET", "/api/bestiary/coverage", Value::Null).await;
    sqlx::query("UPDATE monster_meta SET pack_rev = 'older'")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM bestiary_imports")
        .execute(db.pool())
        .await
        .unwrap();
    // As after a server restart: the account's list is built again from the database.
    bestiary::forget_for_tests(&dm.id);
    let coverage = ok(&dm, "GET", "/api/bestiary/coverage", Value::Null).await;
    let p = pack(&coverage, PACK);
    assert_eq!(
        (
            p["status"].as_str(),
            p["captured"].as_bool(),
            p["inData"].clone()
        ),
        (Some("unknown"), Some(false), Value::Null),
        "{p}"
    );
    let swamp = search(&dm, json!({ "environments": ["swamp"] })).await;
    assert_eq!(
        swamp["total"].as_u64(),
        Some(0),
        "rows of another revision are ignored"
    );
}

/// Aurora companions (D17 b) as the custom content import installs them: three SRD creatures with the setters Aurora
/// requires (six scores, type, size, challenge, alignment, AC): one printed in full with linked traits and an action,
/// one without traits, one whose challenge and AC are a dash and that prints no HP or speed.
fn companion_pack() -> Value {
    let xml = r#"<?xml version="1.0" encoding="utf-8"?><elements>
      <element name="Owl" type="Companion" source="Companion Test" id="ID_TEST_COMPANION_OWL">
        <supports>Familiar</supports>
        <setters>
          <set name="strength">3</set><set name="dexterity">13</set><set name="constitution">8</set>
          <set name="intelligence">2</set><set name="wisdom">12</set><set name="charisma">7</set>
          <set name="ac">11</set><set name="hp">1 (1d4 - 1)</set><set name="speed">5 ft., fly 60 ft.</set>
          <set name="skills">Perception +3, Stealth +3</set>
          <set name="senses">darkvision 120 ft., passive Perception 13</set>
          <set name="type">Beast</set><set name="size">Tiny</set><set name="alignment">unaligned</set>
          <set name="challenge">0</set>
          <set name="traits">ID_TEST_COMPANION_TRAIT_FLYBY,ID_TEST_COMPANION_TRAIT_KEEN</set>
          <set name="actions">ID_TEST_COMPANION_ACTION_TALONS</set>
        </setters>
      </element>
      <element name="Flyby" type="Companion Trait" source="Companion Test" id="ID_TEST_COMPANION_TRAIT_FLYBY">
        <description><p>The owl doesn't provoke opportunity attacks when it flies out of an enemy's reach.</p></description>
      </element>
      <element name="Keen Hearing and Sight" type="Companion Trait" source="Companion Test" id="ID_TEST_COMPANION_TRAIT_KEEN">
        <description><p>The owl has advantage on Wisdom (Perception) checks that rely on hearing or sight.</p></description>
      </element>
      <element name="Talons" type="Companion Action" source="Companion Test" id="ID_TEST_COMPANION_ACTION_TALONS">
        <description><p>Melee Weapon Attack: +3 to hit, reach 5 ft., one target. Hit: 1 slashing damage.</p></description>
      </element>
      <element name="Mastiff" type="Companion" source="Companion Test" id="ID_TEST_COMPANION_MASTIFF">
        <setters>
          <set name="strength">13</set><set name="dexterity">14</set><set name="constitution">12</set>
          <set name="intelligence">3</set><set name="wisdom">12</set><set name="charisma">7</set>
          <set name="ac">12</set><set name="hp">5 (1d8 + 1)</set><set name="speed">40 ft.</set>
          <set name="type">Beast</set><set name="size">Medium</set><set name="alignment">unaligned</set>
          <set name="challenge">1/8</set>
        </setters>
      </element>
      <element name="Pony" type="Companion" source="Companion Test" id="ID_TEST_COMPANION_PONY">
        <setters>
          <set name="strength">15</set><set name="dexterity">10</set><set name="constitution">13</set>
          <set name="intelligence">2</set><set name="wisdom">11</set><set name="charisma">7</set>
          <set name="ac">—</set><set name="type">Beast</set><set name="size">—</set>
          <set name="alignment">unaligned</set><set name="challenge">—</set>
        </setters>
      </element>
    </elements>"#;
    json!({ "request": {
        "fileName": "companion-test.xml",
        "contentBase64": base64::engine::general_purpose::STANDARD.encode(xml),
    }})
}

#[tokio::test(flavor = "multi_thread")]
async fn aurora_companions_are_listed_beside_monsters() {
    let dir = tempfile::tempdir().unwrap();
    let router = super::router(dir.path().to_path_buf()).await;
    let dm = sign_up(&router, "bestiary-companions").await;
    companion_checks(&dm).await;
}

async fn companion_checks(dm: &Client) {
    let (status, out) = dm.rpc("import_pack", companion_pack()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["report"]["converted"].as_u64(), Some(6), "{out}");

    let page = search(dm, json!({ "kinds": ["companion"] })).await;
    assert_eq!(names(&page), ["Mastiff", "Owl", "Pony"]);
    let item = |name: &str| {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == name)
            .unwrap()
            .clone()
    };
    let owl = item("Owl");
    assert_eq!(owl["kind"], "companion");
    assert_eq!(
        (owl["cr"].as_str(), owl["crValue"].as_f64()),
        (Some("0"), Some(0.0))
    );
    assert!(owl.get("xp").is_none(), "no XP anywhere (D16)");
    assert_eq!(
        (
            owl["ac"].as_u64(),
            owl["hp"].as_u64(),
            owl["hpFormula"].as_str()
        ),
        (Some(11), Some(1), Some("1d4-1"))
    );
    assert_eq!(
        (owl["type"].as_str(), owl["size"].clone()),
        (Some("beast"), json!(["tiny"]))
    );
    assert_eq!(
        (owl["legendary"].as_bool(), owl["environments"].clone()),
        (Some(false), json!([]))
    );
    let pack_id = owl["packId"].as_str().unwrap().to_string();
    let pony = item("Pony");
    for field in ["cr", "crValue", "ac", "hp", "hpFormula"] {
        assert_eq!(pony[field], Value::Null, "nothing filled in: {field}");
    }
    assert_eq!(
        (pony["type"].as_str(), pony["size"].clone()),
        (Some("beast"), json!([]))
    );

    // Filters work across both kinds; CR filters leave out companions without a CR.
    let low = search(dm, json!({ "kinds": ["companion"], "crMax": "1/8" })).await;
    assert_eq!(names(&low), ["Mastiff", "Owl"]);
    let tiny = search(
        dm,
        json!({ "sizes": ["tiny"], "types": ["beast"], "limit": 200 }),
    )
    .await;
    let kinds: std::collections::BTreeSet<&str> = tiny["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["companion", "monster"].into_iter().collect());
    let monsters = search(dm, json!({ "kinds": ["monster"], "limit": 1 })).await;
    assert_eq!(monsters["total"].as_u64(), Some((SRD51 + SRD521) as u64));
    let both = search(dm, json!({ "limit": 1 })).await;
    assert_eq!(both["total"].as_u64(), Some((SRD51 + SRD521 + 3) as u64));
    let legendary = search(dm, json!({ "kinds": ["companion"], "legendary": true })).await;
    assert_eq!(legendary["total"].as_u64(), Some(0));
    let (status, _) = dm
        .post("/api/bestiary/search", json!({ "kinds": ["npc"] }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The companion's block: what it prints, and its linked traits and actions.
    let detail = ok(
        dm,
        "GET",
        "/api/bestiary/monsters/ID_TEST_COMPANION_OWL",
        Value::Null,
    )
    .await;
    assert_eq!(
        (detail["kind"].as_str(), detail["monster"].clone()),
        (Some("companion"), Value::Null)
    );
    let c = &detail["companion"];
    assert_eq!(
        c["abilities"],
        json!({ "str": 3, "dex": 13, "con": 8, "int": 2, "wis": 12, "cha": 7 })
    );
    assert_eq!(c["speeds"], json!({ "walk": 5, "fly": 60 }));
    assert_eq!(
        (c["ac"].as_u64(), c["hpText"].as_str()),
        (Some(11), Some("1 (1d4 - 1)"))
    );
    assert_eq!(c["senses"], "darkvision 120 ft., passive Perception 13");
    assert_eq!(c["proficiency"].as_i64(), Some(2));
    let traits: Vec<&str> = c["traits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(traits, ["Flyby", "Keen Hearing and Sight"]);
    assert_eq!(c["actions"][0]["name"], "Talons");
    assert!(
        c["actions"][0]["text"]
            .as_str()
            .unwrap()
            .contains("+3 to hit"),
        "{c}"
    );
    let bare = ok(
        dm,
        "GET",
        "/api/bestiary/monsters/ID_TEST_COMPANION_PONY",
        Value::Null,
    )
    .await;
    let b = &bare["companion"];
    assert_eq!(b["abilities"]["str"].as_u64(), Some(15));
    for field in ["ac", "hp", "hpText", "speed", "proficiency"] {
        assert_eq!(b[field], Value::Null, "nothing filled in: {field}");
    }
    assert_eq!(
        (b["acText"].as_str(), b["challenge"].as_str()),
        (Some("—"), Some("—")),
        "printed as is"
    );
    assert_eq!(
        (b["speeds"].clone(), b["traits"].clone()),
        (json!({}), json!([]))
    );
    let (status, _) = dm
        .get("/api/bestiary/monsters/ID_TEST_COMPANION_TRAIT_FLYBY")
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a companion's trait is not a creature"
    );

    // Coverage counts both kinds.
    let coverage = ok(dm, "GET", "/api/bestiary/coverage", Value::Null).await;
    let p = pack(&coverage, &pack_id);
    assert_eq!(
        (
            p["status"].as_str(),
            p["monsters"].as_u64(),
            p["companions"].as_u64()
        ),
        (Some("companions_only"), Some(0), Some(3)),
        "{p}"
    );
    assert_eq!(
        p["byCr"],
        json!([{ "value": "0", "count": 1 }, { "value": "1/8", "count": 1 }])
    );
    assert_eq!(p["byType"], json!([{ "value": "beast", "count": 3 }]));
    assert_eq!(coverage["totals"]["companions"].as_u64(), Some(3));
    let facets = ok(dm, "GET", "/api/bestiary/facets", Value::Null).await;
    assert_eq!(
        facets["kinds"],
        json!([{ "value": "companion", "count": 3 }, { "value": "monster", "count": SRD51 + SRD521 }])
    );
}

#[test]
fn details_are_read_from_a_creature() {
    let empty = meta_of(&json!({ "name": "Wolf", "source": SOURCE }));
    assert_eq!(empty, bestiary::Meta::default());
    let m = meta_of(&json!({
        "environment": ["Forest", "forest", " hill "], "page": 12,
        "cr": { "cr": "1/2", "coven": "1" },
        "speed": { "walk": 30, "alternate": { "fly": [40, { "number": 60, "condition": "({@spell fly})" }], "swim": [] } },
        "reactionHeader": ["The creature can take one reaction."],
        "hasFluffImages": true
    }));
    assert_eq!(m.environments, ["forest", "hill"]);
    assert_eq!(m.page, Some(12));
    assert_eq!(m.coven_cr, Some(bestiary::CovenCr { cr: "1".into() }));
    let fly = &m.alt_speeds["fly"];
    assert_eq!(
        (fly[0].feet, fly[1].feet, fly[1].condition.as_str()),
        (40, 60, "(fly)")
    );
    assert!(!m.alt_speeds.contains_key("swim"));
    assert_eq!(m.reaction_intro, "The creature can take one reaction.");
    assert!(m.fluff && !m.token);
    assert!(m.spellcasting.is_none());
    let caster = meta_of(&json!({
        "spellcasting": [{ "name": "Spellcasting", "spells": { "1": { "slots": 4, "spells": ["{@spell shield}"] } } }]
    }));
    assert_eq!(
        caster.spellcasting.unwrap()[0]["spells"]["1"]["slots"],
        4,
        "kept raw"
    );
    let none = meta_of(&json!({ "spellcasting": [] }));
    assert!(
        none.spellcasting.is_none(),
        "an empty list is no spellcasting"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// The upgrade from 0.2.x (schema 3) on SQLite and PostgreSQL
// ---------------------------------------------------------------------------------------------------------------

/// Schema 3 (0.2.x) with an account and a character; HEAD then adds step 4 and the bestiary works over it. A database
/// that says it is at 4 but has no `monster_meta` gets the step again.
async fn upgrade_from_0_2(location: &Location, data: &Path, url: Option<&str>) {
    let old = Db::open_at(location, 3).await.unwrap();
    let router = router_on(data.to_path_buf(), old.clone());
    sign_up(&router, "up-bestiary").await;
    old.pool().close().await;

    let db = Db::open(location).await.unwrap();
    let version: i64 = sqlx::query("SELECT MAX(version) FROM schema_version")
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(version, i64::try_from(Db::schema_version()).unwrap());
    let router = router_on(data.to_path_buf(), db.clone());
    let dm = sign_in(&router, "up-bestiary").await;
    import_file(&dm, &fixture()).await;
    let coverage = ok(&dm, "GET", "/api/bestiary/coverage", Value::Null).await;
    assert_eq!(pack(&coverage, PACK)["captured"], true);
    let rows: i64 = sqlx::query("SELECT COUNT(*) FROM monster_meta")
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(rows, 4);
    db.pool().close().await;

    // Recorded as done, but the tables are gone (another numbering left the database): the step runs again.
    if let Some(url) = url {
        sqlx::any::install_default_drivers();
        let pool = sqlx::AnyPool::connect(url).await.unwrap();
        for t in ["bestiary_imports", "monster_meta"] {
            sqlx::query(&format!("DROP TABLE {t}"))
                .execute(&pool)
                .await
                .unwrap();
        }
        pool.close().await;
        let db = Db::open(location).await.unwrap();
        let rows: i64 = sqlx::query("SELECT COUNT(*) FROM monster_meta")
            .fetch_one(db.pool())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        assert_eq!(rows, 0);
        db.pool().close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_0_2_database_upgrades_to_the_bestiary() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("party.sqlite");
    let url = format!("sqlite://{}", file.display());
    upgrade_from_0_2(
        &Location::Sqlite(file),
        &dir.path().join("data"),
        Some(&url),
    )
    .await;
}

/// The same on PostgreSQL, with the always-on flows. Runs when `BOREALIS_TEST_POSTGRES_URL` names a database it may
/// wipe.
#[tokio::test(flavor = "multi_thread")]
async fn the_bestiary_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = super::POSTGRES.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let location = Location::Postgres(url.clone());
    drop_all(&url).await;
    upgrade_from_0_2(&location, &dir.path().join("data"), Some(&url)).await;
    drop_all(&url).await;
    let db = Db::open(&location).await.unwrap();
    let data = dir.path().join("data2");
    let router = router_on(data.clone(), db);
    let dm = sign_up(&router, "pg-bestiary-dm").await;
    let other = sign_up(&router, "pg-bestiary-other").await;
    srd_checks(&dm).await;
    import_checks(&dm, &other, &data).await;
    let companions = sign_up(&router, "pg-bestiary-companions").await;
    companion_checks(&companions).await;
    drop_all(&url).await;
}

// ---------------------------------------------------------------------------------------------------------------
// Opt-in: a real 5etools checkout (BOREALIS_TEST_5ETOOLS_DIR). Counts only; never names a creature.
// ---------------------------------------------------------------------------------------------------------------

/// The `data/` folder of the checkout in `BOREALIS_TEST_5ETOOLS_DIR`, or `None` (test skipped).
fn fivetools_data() -> Option<PathBuf> {
    let Ok(dir) = std::env::var("BOREALIS_TEST_5ETOOLS_DIR") else {
        eprintln!("skipped: set BOREALIS_TEST_5ETOOLS_DIR to a local 5etools source checkout");
        return None;
    };
    let root = PathBuf::from(dir);
    let data = if root.join("data").is_dir() {
        root.join("data")
    } else {
        root
    };
    assert!(
        data.join("bestiary").is_dir(),
        "{} has no bestiary folder",
        data.display()
    );
    Some(data)
}

#[test]
fn every_5etools_creature_reads() {
    let Some(data) = fivetools_data() else { return };
    let (creatures, failed) = read_creatures(&data).unwrap();
    let (mut parsed, mut skipped) = (0usize, 0usize);
    let mut crs = std::collections::BTreeMap::<String, usize>::new();
    let mut types = std::collections::BTreeSet::new();
    let mut sizes = std::collections::BTreeSet::new();
    let (mut mythic, mut coven, mut alt, mut env, mut page, mut variants) = (0, 0, 0, 0, 0, 0);
    for raw in &creatures {
        let ruleset = if raw["edition"] == "one" {
            borealis_content::Ruleset::Dnd2024
        } else {
            borealis_content::Ruleset::Dnd2014
        };
        let meta = meta_of(raw);
        mythic += usize::from(meta.mythic.is_some());
        coven += usize::from(meta.coven_cr.is_some());
        alt += usize::from(!meta.alt_speeds.is_empty());
        env += usize::from(!meta.environments.is_empty());
        page += usize::from(meta.page.is_some());
        variants += usize::from(!meta.variants.is_empty());
        if let Some(m) = &meta.mythic {
            assert!(
                !m.actions.is_empty() && m.actions.iter().all(|a| !a.text.is_empty()),
                "mythic options have text"
            );
        }
        match borealis_import_5etools::convert::monster::stat_block(raw, ruleset) {
            Ok((m, _)) => {
                parsed += 1;
                *crs.entry(
                    m.cr.map_or_else(|| "none".to_string(), borealis_content::Cr::as_text),
                )
                .or_default() += 1;
                types.insert(m.creature_type.as_str());
                for s in &m.size {
                    sizes.insert(s.as_str());
                }
                assert!(m.validate().is_empty());
            }
            Err(_) => skipped += 1,
        }
    }
    let total = creatures.len();
    println!(
        "5etools corpus: {total} creatures ({} unresolved copies), {parsed} read, {skipped} skipped; \
         details: mythic {mythic}, coven CR {coven}, alternate speeds {alt}, environment {env}, page {page}, \
         variants {variants}; CRs {}, types {}, sizes {}",
        failed.len(),
        crs.len(),
        types.len(),
        sizes.len()
    );
    assert!(total > 4000, "a full bestiary: {total}");
    assert!(
        parsed * 100 >= total * 95,
        "at least 95% read: {parsed}/{total}"
    );
    assert_eq!(types.len(), 14, "every creature type occurs");
    assert_eq!(sizes.len(), 6, "every size occurs");
    for cr in ["0", "1/8", "1/4", "1/2", "1", "5", "10", "17", "20", "30"] {
        assert!(crs.get(cr).is_some_and(|n| *n > 0), "CR {cr} occurs");
    }
    let low: usize = ["0", "1/8", "1/4", "1/2", "1", "2", "3", "4", "5"]
        .iter()
        .filter_map(|c| crs.get(*c))
        .sum();
    assert!(
        low * 2 > parsed,
        "most creatures are CR 5 or lower: {low}/{parsed}"
    );
    assert!(
        mythic >= 15 && coven >= 5 && alt >= 5,
        "mythic {mythic}, coven {coven}, alternate {alt}"
    );
    assert!(env * 4 >= total, "environments: {env}/{total}");
    assert!(page * 10 >= total * 9, "pages: {page}/{total}");
}

/// The books whose creatures are the SRD's (the importer's `SRD_MONSTER_BOOKS`, one per edition) imported through the
/// server (the wrapped `import_5etools`): every creature the data has is installed or skipped with a reason, and the
/// details are captured and searchable.
#[tokio::test(flavor = "multi_thread")]
async fn a_5etools_bestiary_import_is_covered() {
    let Some(data) = fivetools_data() else { return };
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    let state = state_with(dir.path().to_path_buf(), db, Setup::default());
    let router = router_of(state.clone(), Setup::default().max_upload);
    let dm = sign_up(&router, "corpus-dm").await;
    let workspace = state.workspaces.get(&dm.id).await.unwrap();
    let token = workspace
        .blocking(move |h| {
            let picked = h
                .state::<Backend>()
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
    let books = borealis_import_5etools::convert::monster::SRD_MONSTER_BOOKS;
    import_token(&dm, &token, books).await;
    let [old, new] = [books[0], books[1]].map(borealis_import_5etools::pack::pack_id);
    let coverage = ok(&dm, "GET", "/api/bestiary/coverage", Value::Null).await;
    for id in [old.as_str(), new.as_str()] {
        let p = pack(&coverage, id);
        let (monsters, in_data, skipped, partial) = (
            p["monsters"].as_u64().unwrap(),
            p["inData"].as_u64().unwrap(),
            p["skipped"].as_u64().unwrap(),
            p["partial"].as_u64().unwrap(),
        );
        println!(
            "{id}: {monsters} installed of {in_data} in the data, {skipped} skipped, {partial} partial, extras {}",
            p["extras"]
        );
        assert_eq!(p["captured"], true);
        assert!(monsters > 300, "{id}: {monsters}");
        assert_eq!(
            monsters + skipped,
            in_data,
            "{id}: every creature is installed or skipped"
        );
        assert!(
            p["extras"]["environment"].as_u64().unwrap() * 2 > monsters,
            "{id}: environments"
        );
        assert!(
            p["extras"]["page"].as_u64().unwrap() * 10 > monsters * 9,
            "{id}: pages"
        );
    }
    let forest = search(
        &dm,
        json!({ "environments": ["forest"], "sources": [old], "limit": 1 }),
    )
    .await;
    let edition = search(
        &dm,
        json!({ "ruleset": "dnd2024", "sources": [old], "limit": 1 }),
    )
    .await;
    println!(
        "forest {} in {old}, {old} as 2024: {}",
        forest["total"], edition["total"]
    );
    assert!(forest["total"].as_u64().unwrap() > 10);
    assert_eq!(edition["total"].as_u64(), Some(0), "the 2014 book is 2014");
    let goblin = search(&dm, json!({ "text": "goblin", "sources": [new] })).await;
    assert!(
        names(&goblin).contains(&"Goblin Warrior".to_string()),
        "{goblin}"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// Opt-in: the Aurora Legacy indexes (BOREALIS_TEST_AURORA_DIR caches them). They have no Monster elements.
// ---------------------------------------------------------------------------------------------------------------

/// The Aurora Legacy indexes the user subscribes to.
pub(super) const AURORA_INDEXES: [&str; 4] = [
    "https://raw.githubusercontent.com/AuroraLegacy/elements/master/core.index",
    "https://raw.githubusercontent.com/AuroraLegacy/elements/master/supplements.index",
    "https://raw.githubusercontent.com/AuroraLegacy/elements/master/unearthed-arcana.index",
    "https://raw.githubusercontent.com/AuroraLegacy/elements/master/AuroraLegacy.index",
];

/// Downloads through a folder: a URL fetched once is read from there afterwards.
pub(super) struct Cached(pub(super) PathBuf);

impl Cached {
    fn file(&self, url: &str) -> PathBuf {
        let name: String = url
            .trim_start_matches("https://")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.0.join(name)
    }
}

impl borealis_lib::content_index::Fetcher for Cached {
    fn get(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
        let file = self.file(url);
        if let Ok(bytes) = std::fs::read(&file) {
            return Ok(bytes);
        }
        let bytes = borealis_lib::content_index::Fetcher::get(
            &borealis_lib::content_index::HttpsFetcher,
            url,
            max_bytes,
        )?;
        let _ = std::fs::write(&file, &bytes);
        Ok(bytes)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_aurora_indexes_hold_no_bestiary() {
    let Ok(cache) = std::env::var("BOREALIS_TEST_AURORA_DIR") else {
        eprintln!("skipped: set BOREALIS_TEST_AURORA_DIR to a folder for the cached Aurora Legacy indexes");
        return;
    };
    let cache = PathBuf::from(cache);
    std::fs::create_dir_all(&cache).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    let state = state_with(dir.path().to_path_buf(), db, Setup::default());
    let router = router_of(state.clone(), Setup::default().max_upload);
    let dm = sign_up(&router, "aurora-dm").await;
    let workspace = state.workspaces.get(&dm.id).await.unwrap();
    let result = workspace
        .blocking(move |h| {
            let backend = h.state::<Backend>();
            for url in AURORA_INDEXES {
                backend
                    .add_index(url)
                    .map_err(|e| crate::error::ApiError::Internal(e.message))?;
            }
            let sink = borealis_lib::content_packs::NullSink;
            backend
                .update_indexes(&Cached(cache), &sink)
                .map_err(|e| crate::error::ApiError::Internal(e.message))?;
            let packs = backend
                .list_packs()
                .map_err(|e| crate::error::ApiError::Internal(e.message))?;
            Ok(packs
                .iter()
                .filter(|p| !matches!(p.kind, borealis_lib::content_dto::PackKind::Bundled))
                .count())
        })
        .await
        .unwrap();
    assert!(result > 0, "the indexes installed packs");
    let coverage = ok(&dm, "GET", "/api/bestiary/coverage", Value::Null).await;
    let mut companions = 0;
    for p in coverage["packs"].as_array().unwrap() {
        if p["kind"] == "bundled" {
            continue;
        }
        assert_eq!(
            p["monsters"].as_u64(),
            Some(0),
            "an Aurora pack with Monster elements: {}",
            p["packId"]
        );
        assert_eq!(p["status"], "companions_only");
        companions += p["companions"].as_u64().unwrap();
    }
    assert!(companions > 0, "the Aurora indexes have companions");
    // Monsters are still just the SRD's; the companions are listed beside them (D17 b).
    let monsters = search(&dm, json!({ "kinds": ["monster"], "limit": 1 })).await;
    assert_eq!(monsters["total"].as_u64(), Some((SRD51 + SRD521) as u64));
    let all = search(&dm, json!({ "kinds": ["companion"], "limit": 200 })).await;
    assert_eq!(all["total"].as_u64(), Some(companions));
    let mut listed = Vec::new();
    let mut offset = 0;
    loop {
        let page = search(
            &dm,
            json!({ "kinds": ["companion"], "limit": 200, "offset": offset }),
        )
        .await;
        let items = page["items"].as_array().unwrap().clone();
        if items.is_empty() {
            break;
        }
        offset += items.len();
        listed.extend(items);
    }
    let have = |k: &str| listed.iter().filter(|i| !i[k].is_null()).count();
    println!(
        "Aurora indexes: {result} packs, {companions} companions, no Monster elements; with AC {}, HP {}, CR {}, type {}",
        have("ac"),
        have("hp"),
        have("cr"),
        have("type")
    );
    for i in &listed {
        assert_eq!(i["kind"], "companion");
        assert_eq!(
            (i["legendary"].as_bool(), i["mythic"].as_bool()),
            (Some(false), Some(false))
        );
        if let Some(t) = i["type"].as_str() {
            assert_eq!(t, t.to_lowercase(), "types are lowercase");
        }
    }
    assert!(
        have("ac") * 10 >= listed.len() * 9,
        "most companions print an AC"
    );
    let detail = ok(
        &dm,
        "GET",
        &format!(
            "/api/bestiary/monsters/{}",
            listed[0]["id"].as_str().unwrap()
        ),
        Value::Null,
    )
    .await;
    assert_eq!(
        (detail["kind"].as_str(), detail["monster"].clone()),
        (Some("companion"), Value::Null)
    );
    assert!(detail["companion"].is_object(), "{detail}");
    let facets = ok(&dm, "GET", "/api/bestiary/facets", Value::Null).await;
    assert_eq!(
        facets["kinds"].as_array().unwrap().len(),
        2,
        "{}",
        facets["kinds"]
    );
    for q in [
        json!({ "text": "wolf" }),
        json!({ "environments": ["forest"] }),
        json!({ "types": ["beast"], "sort": "cr" }),
    ] {
        search(&dm, q).await;
    }
}
