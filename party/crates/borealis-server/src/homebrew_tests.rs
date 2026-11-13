//! Homebrew authoring and index building (`homebrew.rs`), through the router and the real engine: every element type
//! round-trips, bad XML and bad references are reported precisely, ids stay and versions go up, an index is built for
//! the author's own repository (never this server: D47) and parses with Borealis's own index parser, the zip export
//! holds the same files, packs that are not the account's own homebrew are refused, and the tools match the routes.
//!
//! Every element here is made up (a "Lantern of Testing" and friends). The opt-in Aurora check
//! (`BOREALIS_TEST_AURORA_DIR`, the cache folder the bestiary suite fills) compares structure and prints counts only.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{router_of, sign_up, spell_pack, state_with, Client, Setup};
use crate::db::{Db, Location};
use crate::{AppState, Config};

const HOST: &str = "https://party.example.com";

/// A test server with a public address (which an index build must never point at).
fn server(data: PathBuf, db: Db) -> (AppState, Router) {
    let mut state = state_with(data, db, Setup::default());
    state.config = Arc::new(Config {
        open_registration: true,
        require_approval: false,
        public_url: Some(HOST.into()),
        app_download: None,
        trusted_proxies: Vec::new(),
    });
    let router = router_of(state.clone(), Setup::default().max_upload);
    (state, router)
}

async fn ok(client: &Client, method: &str, path: &str, body: Value) -> Value {
    let (status, out) = match method {
        "GET" => client.get(path).await,
        "POST" => client.post(path, body).await,
        "PATCH" => client.patch(path, body).await,
        "DELETE" => client.delete(path).await,
        _ => unreachable!(),
    };
    assert_eq!(status, StatusCode::OK, "{method} {path}: {out}");
    out
}

fn set(name: &str, value: &str) -> Value {
    json!({ "name": name, "value": value })
}

/// One made-up element of every type the editor makes.
fn every_type() -> Vec<Value> {
    vec![
        json!({ "type": "Spell", "name": "Spark of Testing", "supports": "Wizard",
                "description": "A spark leaps at a creature you can see.\n\nIt deals 1d6 fire damage.",
                "setters": [set("level", "0"), set("school", "Evocation"), set("time", "1 action"),
                            set("range", "60 feet"), set("duration", "Instantaneous"),
                            set("hasVerbalComponent", "true"), set("hasSomaticComponent", "true")] }),
        json!({ "type": "Feat", "name": "Tester's Focus", "prerequisite": "None",
                "description": "You gain a +1 bonus to initiative.",
                "sheet": { "description": "+1 to initiative." },
                "rules": [{ "kind": "stat", "attrs": { "name": "initiative", "value": "1" } }] }),
        json!({ "type": "Item", "name": "Testing Kit", "description": "A box of small tools.",
                "setters": [set("category", "Adventuring Gear"),
                            { "name": "cost", "value": "5", "attrs": { "currency": "gp" } },
                            set("weight", "3")] }),
        json!({ "type": "Magic Item", "name": "Lantern of Testing",
                "description": "This lantern sheds bright light on hidden bugs in a 30-foot radius.",
                "sheet": { "usage": "3/Long Rest", "description": "Reveal a hidden bug." },
                "setters": [set("category", "Wondrous Items"), set("type", "Wondrous Item"),
                            set("rarity", "Uncommon"), set("attunement", "true"), set("weight", "2")] }),
        json!({ "type": "Weapon", "name": "Testing Blade", "description": "A short, light blade.",
                "setters": [set("category", "Weapons"),
                            { "name": "damage", "value": "1d6", "attrs": { "type": "slashing" } },
                            set("slot", "onehand"), set("weight", "2"),
                            { "name": "cost", "value": "10", "attrs": { "currency": "gp" } }] }),
        json!({ "type": "Armor", "name": "Testing Mail", "description": "Interlocking test rings.",
                "setters": [set("category", "Armor"), set("armor", "Medium"), set("armorClass", "14"),
                            set("slot", "body"), set("weight", "20")] }),
        json!({ "type": "Race", "name": "Testfolk", "description": "Small, curious folk.",
                "rules": [{ "kind": "stat", "attrs": { "name": "dexterity", "value": "2" } }] }),
        json!({ "type": "Sub Race", "name": "Hill Testfolk", "supports": "Testfolk",
                "description": "Hardy testfolk of the hills.",
                "rules": [{ "kind": "stat", "attrs": { "name": "constitution", "value": "1" } }] }),
        json!({ "type": "Background", "name": "Test Pilot", "description": "You flew the first test flights.",
                "setters": [set("short", "A daring flyer.")] }),
        json!({ "type": "Class Feature", "name": "Testing Invocation", "supports": "Eldritch Invocation",
                "description": "You can cast detect magic at will." }),
        json!({ "type": "Archetype Feature", "name": "Testing Fury", "supports": "Path of Testing",
                "description": "Your rage tests your foes." }),
        json!({ "type": "Archetype", "name": "Path of Testing", "supports": "Primal Path",
                "description": "Barbarians who test everything.",
                "rules": [{ "kind": "grant", "attrs": { "type": "Archetype Feature",
                            "id": "ID_TEST_PACK_ARCHETYPE_FEATURE_TESTING_FURY", "level": "3" } }] }),
        json!({ "type": "Monster", "name": "Test Wolf", "description": "A wolf that tests its prey.",
                "setters": [set("size", "Medium"), set("type", "beast"), set("alignment", "unaligned"),
                            set("ac", "13"), set("hp", "11"), set("hd", "2d8+2"), set("speed", "40"),
                            set("str", "12"), set("dex", "15"), set("con", "12"), set("int", "3"),
                            set("wis", "12"), set("cha", "6"), set("cr", "1/4")] }),
    ]
}

/// The account's pack "Test Pack" (`hb-test-pack`).
async fn test_pack(client: &Client) -> String {
    let pack = ok(
        client,
        "POST",
        "/api/homebrew/packs",
        json!({ "name": "Test Pack", "description": "Made up for tests.", "author": "Tester" }),
    )
    .await;
    assert_eq!(pack["id"], "hb-test-pack");
    pack["id"].as_str().unwrap().to_string()
}

async fn create(client: &Client, pack: &str, form: &Value) -> Value {
    ok(
        client,
        "POST",
        &format!("/api/homebrew/packs/{pack}/elements"),
        json!({ "form": form }),
    )
    .await
}

fn report_codes(out: &Value) -> Vec<String> {
    out["report"]["issues"]
        .as_array()
        .unwrap_or_else(|| panic!("{out}"))
        .iter()
        .filter(|i| i["severity"] == "error")
        .map(|i| i["code"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn every_type_round_trips_through_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let (_, router) = server(
        dir.path().to_path_buf(),
        Db::open(&Location::Memory).await.unwrap(),
    );
    let author = sign_up(&router, "hb-author").await;
    let types = ok(&author, "GET", "/api/homebrew/types", Value::Null).await;
    assert_eq!(types["types"].as_array().unwrap().len(), every_type().len());
    let pack = test_pack(&author).await;
    for form in every_type() {
        let saved = create(&author, &pack, &form).await;
        let id = saved["id"].as_str().unwrap().to_string();
        assert!(id.starts_with("ID_TEST_PACK_"), "{id}");
        assert_eq!(saved["report"]["ok"], true, "{}: {saved}", form["name"]);
        // The form comes back as it went in, with the id and the pack's name as source filled in.
        let back = ok(
            &author,
            "GET",
            &format!("/api/homebrew/packs/{pack}/elements/{id}"),
            Value::Null,
        )
        .await;
        let mut want = form.clone();
        want["id"] = json!(id);
        want["source"] = json!("Test Pack");
        if want.get("setters").is_none() {
            want["setters"] = json!([]);
        }
        let got = &back["form"];
        for key in ["type", "id", "name", "source", "description", "supports"] {
            assert_eq!(
                got[key].as_str().unwrap_or(""),
                want[key].as_str().unwrap_or(""),
                "{key} of {id}"
            );
        }
        let names = |v: &Value| -> Vec<String> {
            v["setters"].as_array().map_or_else(Vec::new, |a| {
                a.iter()
                    .map(|s| format!("{}={}", s["name"], s["value"]))
                    .collect()
            })
        };
        assert_eq!(names(got), names(&want), "setters of {id}");
        // The engine loaded it: the account's compendium has it.
        let (status, found) = author.rpc("get_element", json!({ "elementId": id })).await;
        assert_eq!(status, StatusCode::OK, "{id}: {found}");
        assert_eq!(found["name"], form["name"], "{found}");
    }
    let detail = ok(
        &author,
        "GET",
        &format!("/api/homebrew/packs/{pack}"),
        Value::Null,
    )
    .await;
    assert_eq!(detail["elementCount"], every_type().len());
    // The library shows it as homebrew.
    let (_, packs) = author.rpc("list_packs", json!({})).await;
    let installed = packs
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == pack)
        .unwrap();
    assert_eq!(installed["kind"], "homebrew", "{installed}");
    // The whole pack validates as it is.
    let whole = ok(
        &author,
        "POST",
        "/api/homebrew/validate",
        json!({ "packId": pack }),
    )
    .await;
    assert_eq!(whole["report"]["ok"], true, "{whole}");
    assert_eq!(whole["report"]["converted"], every_type().len());
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_xml_and_bad_references_are_reported_precisely() {
    let dir = tempfile::tempdir().unwrap();
    let (_, router) = server(
        dir.path().to_path_buf(),
        Db::open(&Location::Memory).await.unwrap(),
    );
    let author = sign_up(&router, "hb-errors").await;
    let pack = test_pack(&author).await;
    let path = format!("/api/homebrew/packs/{pack}/elements");

    // Not well-formed: the line inside the element, nothing saved.
    let xml = "<element name=\"Broken\" type=\"Feat\" source=\"Test Pack\" id=\"ID_TEST_BROKEN\">\n<description>\n<p>x</p>\n</element>";
    let (status, out) = author.post(&path, json!({ "xml": xml })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    assert_eq!(out["code"], "invalid_homebrew");
    let issue = &out["report"]["issues"][0];
    assert_eq!(issue["code"], "xml_parse", "{out}");
    assert!(issue["elementLine"].as_u64().unwrap() >= 2, "{issue}");

    // A grant of an id nobody defines: the engine's unresolved reference, naming the id and the element.
    let form = json!({ "type": "Feat", "name": "Dangling Feat", "description": "Grants nothing real.",
        "rules": [{ "kind": "grant", "attrs": { "type": "Spell", "id": "ID_NOWHERE_SPELL" } }] });
    let (status, out) = author.post(&path, json!({ "form": form })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    let errors: Vec<&Value> = out["report"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["severity"] == "error")
        .collect();
    let dangling = errors
        .iter()
        .find(|i| i["code"] == "unresolved_reference")
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        dangling["message"]
            .as_str()
            .unwrap()
            .contains("ID_NOWHERE_SPELL"),
        "{dangling}"
    );
    assert_eq!(
        dangling["element"], "ID_TEST_PACK_FEAT_DANGLING_FEAT",
        "{dangling}"
    );

    // A spell without its required setters: the engine's own finding, at the element, with the line in the file.
    let form =
        json!({ "type": "Spell", "name": "Unfinished Spell", "description": "No level yet." });
    let checked = ok(
        &author,
        "POST",
        "/api/homebrew/validate",
        json!({ "packId": pack, "form": form }),
    )
    .await;
    assert_eq!(checked["report"]["ok"], false, "{checked}");
    let issue = checked["report"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["severity"] == "error")
        .unwrap();
    assert_eq!(
        issue["element"], "ID_TEST_PACK_SPELL_UNFINISHED_SPELL",
        "{checked}"
    );
    assert!(
        issue["file"].as_str().unwrap().ends_with("spells.xml"),
        "{issue}"
    );

    // A type the editor doesn't make, and a whole file instead of an element.
    let (_, out) = author
        .post(
            &path,
            json!({ "xml": "<element name=\"C\" type=\"Class\" source=\"S\" id=\"ID_C\"/>" }),
        )
        .await;
    assert_eq!(report_codes(&out), ["unknown_type"]);
    let (_, out) = author.post(&path, json!({ "xml": "<elements/>" })).await;
    assert_eq!(report_codes(&out), ["not_an_element"]);

    // Nothing of that was saved; the pack is still empty and never installed.
    let detail = ok(
        &author,
        "GET",
        &format!("/api/homebrew/packs/{pack}"),
        Value::Null,
    )
    .await;
    assert_eq!(detail["elementCount"], 0, "{detail}");
    assert_eq!(detail["version"], "1.0.0");
}

#[tokio::test(flavor = "multi_thread")]
async fn ids_stay_and_versions_go_up() {
    let dir = tempfile::tempdir().unwrap();
    let (_, router) = server(
        dir.path().to_path_buf(),
        Db::open(&Location::Memory).await.unwrap(),
    );
    let author = sign_up(&router, "hb-ids").await;
    let pack = test_pack(&author).await;
    let lantern = &every_type()[3];
    let saved = create(&author, &pack, lantern).await;
    let id = saved["id"].as_str().unwrap().to_string();
    assert_eq!(saved["pack"]["version"], "1.0.1");
    let path = format!("/api/homebrew/packs/{pack}/elements/{id}");

    // A rename keeps the id; the revision and the versions go up.
    let mut renamed = lantern.clone();
    renamed["name"] = json!("Great Lantern of Testing");
    let changed = ok(&author, "PATCH", &path, json!({ "form": renamed })).await;
    assert_eq!(changed["id"], id);
    assert_eq!(changed["revision"], 2);
    assert_eq!(changed["pack"]["version"], "1.0.2");
    let (_, found) = author.rpc("get_element", json!({ "elementId": id })).await;
    assert_eq!(found["name"], "Great Lantern of Testing");

    // The same element again changes nothing.
    let same = ok(&author, "PATCH", &path, json!({ "form": renamed })).await;
    assert_eq!(same["pack"]["version"], "1.0.2");

    // An edit that changes the id is refused, in XML and in the form.
    let xml = changed["xml"]
        .as_str()
        .unwrap()
        .replace(&id, "ID_SOMETHING_ELSE");
    let (status, out) = author.patch(&path, json!({ "xml": xml })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    assert_eq!(report_codes(&out), ["id_changed"]);
    let mut moved = renamed.clone();
    moved["id"] = json!("ID_SOMETHING_ELSE");
    let (status, out) = author.patch(&path, json!({ "form": moved })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{out}");

    // The same name again makes the same id: a duplicate. An id the bundled content has is refused too.
    let (status, out) = author
        .post(
            &format!("/api/homebrew/packs/{pack}/elements"),
            json!({ "form": renamed }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "a new name, so a new id: {out}");
    let (status, out) = author
        .post(
            &format!("/api/homebrew/packs/{pack}/elements"),
            json!({ "form": renamed }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    assert_eq!(report_codes(&out), ["duplicate_id"]);
    let (_, spells) = author
        .rpc(
            "search_compendium",
            json!({ "query": { "query": "", "types": ["spell"], "limit": 1 } }),
        )
        .await;
    let bundled = spells["items"][0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{spells}"))
        .to_string();
    let mut copy = every_type()[0].clone();
    copy["id"] = json!(bundled);
    let (status, out) = author
        .post(
            &format!("/api/homebrew/packs/{pack}/elements"),
            json!({ "form": copy }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    assert_eq!(report_codes(&out), ["id_taken"]);

    // A renamed pack keeps its id; the author may set the version.
    let renamed_pack = ok(
        &author,
        "PATCH",
        &format!("/api/homebrew/packs/{pack}"),
        json!({ "name": "Tested Things", "version": "2.0.0" }),
    )
    .await;
    assert_eq!(renamed_pack["id"], pack);
    assert_eq!(renamed_pack["version"], "2.0.0");

    // Deleting an element bumps the pack; the element is gone from the engine.
    let gone = ok(&author, "DELETE", &path, Value::Null).await;
    assert_eq!(gone["pack"]["version"], "2.0.1");
    let (status, _) = author.rpc("get_element", json!({ "elementId": id })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// The bytes of a GET as `client`, with its status.
async fn get_bytes(router: &Router, client: &Client, path: &str) -> (StatusCode, Vec<u8>) {
    let response = router
        .clone()
        .oneshot(
            Request::get(path)
                .header("authorization", format!("Bearer {}", client.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec())
}

const REPO: &str = "https://raw.githubusercontent.com/someone/test-homebrew/main";

#[tokio::test(flavor = "multi_thread")]
async fn an_index_is_built_for_the_authors_own_repository() {
    let dir = tempfile::tempdir().unwrap();
    let (_, router) = server(
        dir.path().to_path_buf(),
        Db::open(&Location::Memory).await.unwrap(),
    );
    let author = sign_up(&router, "hb-builder").await;
    let pack = test_pack(&author).await;
    create(&author, &pack, &every_type()[3]).await;
    create(&author, &pack, &every_type()[0]).await;

    let built = ok(
        &author,
        "GET",
        &format!("/api/homebrew/packs/{pack}/index?base={REPO}"),
        Value::Null,
    )
    .await;
    assert_eq!(built["base"], format!("{REPO}/"));
    let files = built["files"].as_array().unwrap();
    assert_eq!(files.len(), 3, "the index and two element files");
    let index_text = files[0]["text"].as_str().unwrap();
    let url = format!("{REPO}/test-pack.index");
    // Borealis's own index parser reads it, and every file it lists is one of the built files at that address.
    let index =
        borealis_lib::content_index_aurora::parse_index(index_text.as_bytes(), &url).unwrap();
    assert_eq!(index.files.len(), 2);
    assert!(
        index_text.contains(&format!("url=\"{url}\"")),
        "the index names its own address"
    );
    for f in &index.files {
        let path = f
            .url
            .strip_prefix(&format!("{REPO}/"))
            .unwrap_or_else(|| panic!("{}", f.url));
        let file = files
            .iter()
            .find(|x| x["path"] == path)
            .unwrap_or_else(|| panic!("{path}"));
        assert!(
            file["text"]
                .as_str()
                .unwrap()
                .contains(&format!("url=\"{}\"", f.url)),
            "each file's <info> names its own address"
        );
    }
    // Nothing in it points at this server.
    assert!(!built.to_string().contains("party.example.com"), "{built}");

    // The built files import as one pack, as a subscriber's Borealis would install them.
    let other = sign_up(&router, "hb-reader").await;
    let entries: Vec<(String, Vec<u8>)> = files
        .iter()
        .map(|f| {
            (
                format!("test-pack-copy/{}", f["path"].as_str().unwrap()),
                f["text"].as_str().unwrap().as_bytes().to_vec(),
            )
        })
        .collect();
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let zip = super::stored_zip(&refs);
    let (status, imported) = other
        .rpc("import_pack", json!({ "request": { "fileName": "test-pack-copy.zip", "contentBase64": super::b64(&zip) } }))
        .await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    let (status, found) = other
        .rpc(
            "get_element",
            json!({ "elementId": "ID_TEST_PACK_MAGIC_ITEM_LANTERN_OF_TESTING" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{found}");

    // The zip export holds the same files.
    let (status, bytes) = get_bytes(
        &router,
        &author,
        &format!("/api/homebrew/packs/{pack}/export?base={REPO}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<String> = borealis_lib::zip_read::read_zip(&bytes, |_| true)
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(
        names,
        [
            "test-pack.index",
            "test-pack/spells.xml",
            "test-pack/magic-items.xml"
        ]
    );

    // No address, a non-https one, or this server's own: refused. The server never hosts an index.
    for (base, status) in [
        ("", StatusCode::BAD_REQUEST),
        (
            "http://raw.githubusercontent.com/o/r/main",
            StatusCode::BAD_REQUEST,
        ),
        ("https://party.example.com/i/", StatusCode::BAD_REQUEST),
    ] {
        let (got, out) = author
            .get(&format!("/api/homebrew/packs/{pack}/index?base={base}"))
            .await;
        assert_eq!(got, status, "{base}: {out}");
        let (got, _) = get_bytes(
            &router,
            &author,
            &format!("/api/homebrew/packs/{pack}/export?base={base}"),
        )
        .await;
        assert_eq!(got, status, "export {base}");
    }
    // There is no publishing route.
    let (status, _) = author
        .post(&format!("/api/homebrew/packs/{pack}/publish"), Value::Null)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_accounts_own_homebrew_is_built_or_exported() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_path_buf();
    let (_, router) = server(data.clone(), Db::open(&Location::Memory).await.unwrap());
    let author = sign_up(&router, "hb-licence").await;
    let (status, imported) = author.rpc("import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    let imported_id = imported["pack"]["id"].as_str().unwrap().to_string();
    let refused = |out: &Value| out["message"].as_str().unwrap_or_default().to_string();
    let (status, out) = author
        .get(&format!(
            "/api/homebrew/packs/{imported_id}/index?base={REPO}"
        ))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(out["code"], "not_redistributable");
    assert!(refused(&out).contains("Only your own homebrew"), "{out}");
    let (status, _) = get_bytes(
        &router,
        &author,
        &format!("/api/homebrew/packs/{imported_id}/export?base={REPO}"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // The same pack marked as converted from 5etools data says so.
    std::fs::write(
        data.join("users")
            .join(&author.id)
            .join("library")
            .join(&imported_id)
            .join("import.json"),
        json!({ "origin": "5etools-import" }).to_string(),
    )
    .unwrap();
    let (_, out) = author
        .get(&format!(
            "/api/homebrew/packs/{imported_id}/index?base={REPO}"
        ))
        .await;
    assert!(refused(&out).contains("5etools"), "{out}");
    // A bundled pack too, and through the tool.
    let (_, packs) = author.rpc("list_packs", json!({})).await;
    let bundled = packs
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "bundled")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, out) = author
        .get(&format!("/api/homebrew/packs/{bundled}/index?base={REPO}"))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");
    // An empty homebrew pack has nothing to build; a made-up id is not found.
    let pack = test_pack(&author).await;
    let (status, _) = author
        .get(&format!("/api/homebrew/packs/{pack}/index?base={REPO}"))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = author
        .get(&format!("/api/homebrew/packs/hb-nothing/index?base={REPO}"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tools_do_what_the_routes_do() {
    let dir = tempfile::tempdir().unwrap();
    let (state, router) = server(
        dir.path().to_path_buf(),
        Db::open(&Location::Memory).await.unwrap(),
    );
    let author = sign_up(&router, "hb-tools").await;
    let user = state.db.account(&author.id).await.unwrap().unwrap().user;
    let specs = crate::homebrew::tools();
    let names: Vec<&str> = specs.iter().map(|t| t.name).collect();
    assert_eq!(
        names,
        [
            "homebrew_types",
            "homebrew_list",
            "homebrew_get",
            "homebrew_validate",
            "homebrew_create",
            "homebrew_update",
            "homebrew_save",
            "homebrew_delete",
            "index_build"
        ]
    );
    let scope = |name: &str| {
        serde_json::to_value(specs.iter().find(|t| t.name == name).unwrap().scope).unwrap()
    };
    assert_eq!(scope("index_build"), "homebrew:read");
    assert_eq!(scope("homebrew_save"), "homebrew:write");
    let call = |name: &'static str, args: Value| {
        let (state, user) = (state.clone(), user.clone());
        async move { crate::homebrew::call_tool(&state, &user, name, args).await }
    };
    // With no pack, the first element makes "My Homebrew".
    let made = call("homebrew_create", json!({ "form": every_type()[3] }))
        .await
        .unwrap();
    assert_eq!(made["pack"]["id"], "hb-my-homebrew");
    let id = made["id"].as_str().unwrap().to_string();
    let bad = call("homebrew_validate", json!({ "xml": "<element" }))
        .await
        .unwrap();
    assert_eq!(bad["report"]["ok"], false);
    let got = call(
        "homebrew_get",
        json!({ "packId": "hb-my-homebrew", "elementId": id }),
    )
    .await
    .unwrap();
    assert!(got["xml"].as_str().unwrap().contains("Lantern of Testing"));
    // homebrew_save: a change with elementId, refused with the report when the engine refuses it.
    let mut renamed = every_type()[3].clone();
    renamed["name"] = json!("Bright Lantern of Testing");
    let saved = call("homebrew_save", json!({ "elementId": id, "form": renamed }))
        .await
        .unwrap();
    assert_eq!(saved["id"], id.as_str());
    let refused = call("homebrew_save", json!({ "xml": "<element name=\"X\" type=\"Feat\" source=\"S\" id=\"ID_X\"><rules><grant type=\"Spell\" id=\"ID_NOWHERE\"/></rules></element>" })).await;
    let Err(crate::error::ApiError::Command(body)) = refused else {
        panic!("{refused:?}")
    };
    assert_eq!(body["code"], "invalid_homebrew");
    // index_build: the files for the agent, the zip for a person; a base address is required.
    let built = call("index_build", json!({ "baseUrl": REPO }))
        .await
        .unwrap();
    assert_eq!(built["indexUrl"], format!("{REPO}/my-homebrew.index"));
    assert_eq!(built["zip"]["fileName"], "my-homebrew.zip");
    use base64::Engine as _;
    let zip = base64::engine::general_purpose::STANDARD
        .decode(built["zip"]["contentBase64"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        borealis_lib::zip_read::read_zip(&zip, |_| true)
            .unwrap()
            .len(),
        built["files"].as_array().unwrap().len()
    );
    assert!(matches!(
        call("index_build", json!({})).await,
        Err(crate::error::ApiError::Invalid(_))
    ));
    for gone in ["index_publish", "index_unpublish", "index_list", "nope"] {
        assert!(
            matches!(
                call(gone, json!({})).await,
                Err(crate::error::ApiError::NotFound(_))
            ),
            "{gone}"
        );
    }
    let deleted = call(
        "homebrew_delete",
        json!({ "packId": "hb-my-homebrew", "elementId": id }),
    )
    .await
    .unwrap();
    assert_eq!(deleted["deleted"], id.as_str());
}

// ---- Opt-in: the generated index has the structure of the Aurora Legacy indexes. ----

/// Element paths and their attributes in an XML document: `index/info/update@version`, ...
fn shape(
    el: &borealis_migrate::dom::XmlElement,
    path: &str,
    out: &mut std::collections::BTreeSet<String>,
) {
    let here = if path.is_empty() {
        el.name.clone()
    } else {
        format!("{path}/{}", el.name)
    };
    out.insert(here.clone());
    for (k, _) in &el.attrs {
        out.insert(format!("{here}@{k}"));
    }
    for child in el.elements() {
        shape(child, &here, out);
    }
}

#[test]
fn the_index_has_the_aurora_legacy_shape() {
    let Ok(cache) = std::env::var("BOREALIS_TEST_AURORA_DIR") else {
        eprintln!(
            "skipped: set BOREALIS_TEST_AURORA_DIR to the folder of cached Aurora Legacy indexes"
        );
        return;
    };
    let mut theirs = std::collections::BTreeSet::new();
    let (mut indexes, mut listed) = (0, 0);
    let mut stack = vec![PathBuf::from(cache)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(root) = borealis_migrate::dom::parse_document(&text) else {
                continue;
            };
            if root.name != "index" {
                continue;
            }
            let parsed = borealis_lib::content_index_aurora::parse_index(
                text.as_bytes(),
                "https://example.invalid/x.index",
            )
            .expect("an Aurora index parses");
            indexes += 1;
            listed += parsed.files.len();
            shape(&root, "", &mut theirs);
        }
    }
    assert!(
        indexes > 0,
        "no cached .index files in BOREALIS_TEST_AURORA_DIR"
    );
    let pack = crate::homebrew::Pack {
        id: "hb-test-pack".into(),
        name: "Test Pack".into(),
        description: "Made up.".into(),
        author: "Tester".into(),
        author_url: "https://example.com/tester".into(),
        version: "1.0.0".into(),
        file_versions: std::collections::BTreeMap::new(),
        elements: vec![crate::homebrew::StoredElement {
            id: "ID_TEST".into(),
            ty: "Feat".into(),
            name: "T".into(),
            file: "feats.xml".into(),
            xml: "<element name=\"T\" type=\"Feat\" source=\"Test Pack\" id=\"ID_TEST\"><description><p>t</p></description></element>".into(),
            revision: 1,
            created_at: 0,
            updated_at: 0,
        }],
        created_at: 0,
        updated_at: 0,
    };
    let ours_text = crate::homebrew::index_file(&pack, "https://example.invalid/").text;
    let mut ours = std::collections::BTreeSet::new();
    shape(
        &borealis_migrate::dom::parse_document(&ours_text).unwrap(),
        "",
        &mut ours,
    );
    let missing: Vec<&String> = ours.difference(&theirs).collect();
    assert!(
        missing.is_empty(),
        "the generated index uses what no Aurora Legacy index has: {missing:?}"
    );
    eprintln!(
        "aurora: {indexes} indexes listing {listed} files; {} element/attribute paths, ours use {}",
        theirs.len(),
        ours.len()
    );
}
