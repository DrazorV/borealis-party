//! A bundle's packs from indexes (`index_subs.rs`, `bundle_accept`): all of their indexes are followed and named
//! after their packs, a pack removal of another pack leaves them, packs already here bring their index too, an index
//! removed on purpose stays removed, and adding it again (Additional Content's repair) clears that.

use std::path::Path;

use axum::http::StatusCode;
use axum::Router;
use serde_json::{json, Value};

use super::{drop_all, router, router_on, sign_up, spell_pack, Client, Db, Location, POSTGRES};
use crate::index_subs::INSTALLED_NOT_CHECKED;

/// Stand-ins for the user's four index packs: (file, index address, files the index lists).
const PACKS: [(&str, &str, u64); 4] = [
    (
        "legacy.xml",
        "https://example.invalid/o/r/master/Legacy.index",
        740,
    ),
    (
        "homebrew.xml",
        "https://example.invalid/o/r/main/homebrew.index",
        7,
    ),
    (
        "heavy.xml",
        "https://example.invalid/o/r/master/Heavy.index",
        8,
    ),
    (
        "third.xml",
        "https://example.invalid/o/r/master/third-party.index",
        239,
    ),
];

fn pack_named(file: &str) -> Value {
    let mut pack = spell_pack();
    pack["request"]["fileName"] = json!(file);
    pack
}

fn meta_path(data: &Path, account: &str, pack: &str) -> std::path::PathBuf {
    data.join("users")
        .join(account)
        .join("library")
        .join(pack)
        .join("pack-meta.json")
}

/// Each index in `list`: (address, name, file count, note).
fn rows(list: &Value) -> Vec<(String, String, u64, Option<String>)> {
    let mut out: Vec<_> = list
        .as_array()
        .unwrap_or_else(|| panic!("{list}"))
        .iter()
        .map(|i| {
            (
                i["url"].as_str().unwrap().to_string(),
                i["name"].as_str().unwrap().to_string(),
                i["fileCount"].as_u64().unwrap(),
                i["problem"].as_str().map(str::to_string),
            )
        })
        .collect();
    out.sort();
    out
}

fn urls(list: &Value) -> Vec<String> {
    rows(list).into_iter().map(|r| r.0).collect()
}

async fn indexes(client: &Client) -> Value {
    let (status, list) = client.rpc("list_indexes", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list
}

/// `alice`'s four packs, each written as an index download writes it; their ids and names, in [`PACKS`] order.
async fn index_packs(alice: &Client, data: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (file, url, files) in PACKS {
        let (status, imported) = alice.rpc("import_pack", pack_named(file)).await;
        assert_eq!(status, StatusCode::OK, "{imported}");
        let id = imported["pack"]["id"].as_str().unwrap().to_string();
        let name = imported["pack"]["name"].as_str().unwrap().to_string();
        let meta = meta_path(data, &alice.id, &id);
        let mut written: Value =
            serde_json::from_str(&std::fs::read_to_string(&meta).unwrap()).unwrap();
        written["indexUrl"] = json!(url);
        written["files"] = json!(files);
        written["format"] = json!("aurora-index");
        written["sha256"] = json!("00");
        std::fs::write(&meta, written.to_string()).unwrap();
        out.push((id, name));
    }
    out
}

fn all() -> Vec<String> {
    let mut v: Vec<String> = PACKS.iter().map(|p| p.1.to_string()).collect();
    v.sort();
    v
}

async fn scenario(router: Router, data: &Path) {
    let alice = sign_up(&router, "subs-alice").await;
    let packs = index_packs(&alice, data).await;
    let bytes = alice.export_bundle(&[], "all").await;

    // Accepted: all four indexes, each named after its pack until it is downloaded here.
    let bob = sign_up(&router, "subs-bob").await;
    let preview = bob.open_bundle("content.borealis", &bytes).await;
    bob.accept_bundle(&preview).await;
    let list = indexes(&bob).await;
    assert_eq!(urls(&list), all(), "{list}");
    for ((_, url, files), (_, name)) in PACKS.iter().zip(&packs) {
        let row = rows(&list).into_iter().find(|r| r.0 == *url).unwrap();
        assert_eq!(row.1, *name, "{list}");
        assert_eq!(row.2, *files, "{list}");
        assert_eq!(row.3.as_deref(), Some(INSTALLED_NOT_CHECKED), "{list}");
    }

    // Removing one pack (and only that) leaves every index followed, the other packs' names included.
    let (status, out) = bob
        .post(
            "/api/content/remove-packs",
            json!({ "packIds": [packs[0].0] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["removed"], 1, "{out}");
    let list = indexes(&bob).await;
    assert_eq!(urls(&list), all(), "{list}");
    let third = rows(&list).into_iter().find(|r| r.0 == PACKS[3].1).unwrap();
    assert_eq!(third.1, packs[3].1, "{list}");
    // Its pack gone, the first index shows its address again, as upstream does.
    let legacy = rows(&list).into_iter().find(|r| r.0 == PACKS[0].1).unwrap();
    assert_eq!(legacy.1, PACKS[0].1, "{list}");

    // The account's case: packs already here (from before bundles brought indexes) with one index missing.
    let carol = sign_up(&router, "subs-carol").await;
    let preview = carol.open_bundle("content.borealis", &bytes).await;
    carol.accept_bundle(&preview).await;
    let prefs = data
        .join("users")
        .join(&carol.id)
        .join("content-prefs.json");
    let mut written: Value =
        serde_json::from_str(&std::fs::read_to_string(&prefs).unwrap()).unwrap();
    written["indexes"]
        .as_array_mut()
        .unwrap()
        .retain(|i| i["url"] != PACKS[3].1);
    std::fs::write(&prefs, written.to_string()).unwrap();
    let list = indexes(&carol).await;
    assert_eq!(urls(&list).len(), 3, "{list}");
    // The pack still says where it came from: Additional Content offers to add that index.
    let (status, sources) = carol.get("/api/content/pack-sources").await;
    assert_eq!(status, StatusCode::OK, "{sources}");
    assert!(
        sources["packs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == packs[3].0.as_str() && p["indexUrl"] == PACKS[3].1),
        "{sources}"
    );
    // One more index removed on purpose.
    let (status, list) = carol
        .rpc("remove_index", json!({ "url": PACKS[2].1 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(urls(&list).len(), 2, "{list}");

    // The same bundle again (every pack identical): the missing index comes back, the removed one does not.
    let preview = carol.open_bundle("again.borealis", &bytes).await;
    assert!(
        super::actions(&preview)
            .iter()
            .all(|(_, action)| action == "identical"),
        "{preview}"
    );
    carol.accept_bundle(&preview).await;
    let list = indexes(&carol).await;
    let mut want: Vec<String> = vec![PACKS[0].1.into(), PACKS[1].1.into(), PACKS[3].1.into()];
    want.sort();
    assert_eq!(urls(&list), want, "{list}");

    // Adding it again by hand (the repair's "Add its index") follows it, and a later bundle keeps it.
    let (status, list) = carol.rpc("add_index", json!({ "url": PACKS[2].1 })).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(urls(&list), all(), "{list}");
    let heavy = rows(&list).into_iter().find(|r| r.0 == PACKS[2].1).unwrap();
    assert_eq!(heavy.1, packs[2].1, "the answer is named too: {list}");
    let preview = carol.open_bundle("third.borealis", &bytes).await;
    carol.accept_bundle(&preview).await;
    assert_eq!(urls(&indexes(&carol).await), all());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bundle_follows_and_names_every_index_its_packs_came_from() {
    let dir = tempfile::tempdir().unwrap();
    scenario(router(dir.path().to_path_buf()).await, dir.path()).await;
}

/// The same on PostgreSQL. Runs when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.
#[tokio::test(flavor = "multi_thread")]
async fn a_bundle_follows_and_names_every_index_its_packs_came_from_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = POSTGRES.lock().await;
    drop_all(&url).await;
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Postgres(url.clone())).await.unwrap();
    scenario(router_on(dir.path().to_path_buf(), db), dir.path()).await;
    drop_all(&url).await;
}
