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

/// A homebrew copy of Fireball (no ruleset: an `any` copy), answering its id.
async fn homebrew_fireball(client: &Client) -> String {
    let pack = ok(
        client,
        "POST",
        "/api/homebrew/packs",
        json!({ "name": "Spell Test", "description": "Made up for tests.", "author": "Tester" }),
    )
    .await;
    let pack = pack["id"].as_str().unwrap().to_string();
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
    assert_eq!(fireball["chosenBy"], "rule");
    assert_eq!(fireball["coverage"], "ruling", "nothing defined yet");
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
    assert_eq!(chosen["spell"]["chosenBy"], "dm");
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
    let (status, _) = call(dm, "GET", "/api/spells/ID_NO_SUCH_SPELL", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

fn def_row(key: &str, ruleset: &str, def: Value) -> Value {
    json!({ "kind": "def", "spellKey": key, "ruleset": ruleset, "def": def })
}

/// Defs, links and aliases; what combat would use.
async fn mechanics_checks(state: &crate::AppState, dm: &Client, other: &Client) {
    let fireball = json!({ "v": 1, "delivery": { "save": { "ability": "dex", "success": "half" } },
                           "onLand": [{ "damage": { "dice": "8d6+(slot_above)d6", "type": "fire" } }] });
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
    assert_eq!(got[FIREBALL_2024].def, fireball);
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
    assert!(other_got.is_empty(), "another account's rows are not mine");
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
        def_row(DELAYED_2024, "dnd2024", json!({ "v": 1, "notes": ["own"] })),
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
        def_row(FIREBALL_2014, "dnd2014", json!({ "v": 1 })),
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
    let enc = ok(
        &dm,
        "POST",
        &format!("/api/parties/{party}/encounters"),
        json!({ "name": "Old one" }),
    )
    .await;
    let enc = enc["id"].as_str().unwrap().to_string();
    let v = ok(
        &dm,
        "POST",
        &format!("/api/encounters/{enc}/creatures"),
        json!({ "elementId": GOBLIN }),
    )
    .await;
    let creature = v["creatures"][0]["id"].as_str().unwrap().to_string();
    ok(
        &dm,
        "POST",
        &format!("/api/encounters/{enc}/effects"),
        json!({ "creatureId": creature, "name": "Old ward", "slotLevel": 1 }),
    )
    .await;
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
    assert_eq!(version, 9);
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
        def_row(FIREBALL_2024, "dnd2024", json!({ "v": 1 })),
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
        def_row(FIREBALL_2024, "dnd2024", json!({ "v": 1 })),
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
