//! The default 5etools source (`import_source.rs`) and bulk pack removal (`pack_removal.rs`): who may read and set
//! the default, what an address must be, and that an account removes only its own packs, together, never built-in ones.

use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use serde_json::{json, Value};

use super::{router, sign_up, spell_pack, Client};

/// A pack file named `file` (its pack id is the name without `.xml`).
fn pack_named(file: &str) -> Value {
    let mut pack = spell_pack();
    pack["request"]["fileName"] = json!(file);
    pack
}

fn own_ids(packs: &Value) -> Vec<String> {
    let mut ids: Vec<String> = packs
        .as_array()
        .unwrap_or_else(|| panic!("{packs}"))
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_string())
        .filter(|id| id != "core" && !id.starts_with("srd-"))
        .collect();
    ids.sort();
    ids
}

async fn packs(client: &Client) -> Vec<String> {
    let (status, out) = client.rpc("list_packs", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    own_ids(&out)
}

/// The folder called `name` somewhere under `root`.
fn find_dir(root: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().is_some_and(|n| n == name) {
            return Some(path);
        }
        if let Some(found) = find_dir(&path, name) {
            return Some(found);
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
async fn the_default_5etools_source_is_the_admins_to_set() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let admin = sign_up(&router, "source-admin").await;
    let player = sign_up(&router, "source-player").await;

    // None ships with the server.
    let (status, out) = player.get("/api/import/default-5etools").await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["url"], Value::Null, "{out}");
    let (status, out) = admin.get("/api/admin/default-5etools").await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["source"], "none", "{out}");

    // Players can't read the admin view or set it.
    let (status, _) = player.get("/api/admin/default-5etools").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = player
        .post(
            "/api/admin/default-5etools",
            json!({ "url": "https://github.com/example/srd-data" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Addresses the importer can't read, or the server may not download from, are refused in words.
    for (url, words) in [
        ("http://github.com/example/srd-data", "https://"),
        (
            "https://github.com/example/srd-data/blob/main/data/books.json",
            "repository",
        ),
        ("https://lan.example.invalid/data/", ""),
        ("https://127.0.0.1/data/", "127.0.0.1"),
    ] {
        let (status, out) = admin
            .post("/api/admin/default-5etools", json!({ "url": url }))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url}: {out}");
        let message = out["message"].as_str().unwrap_or_default();
        assert!(
            !message.is_empty() && message.contains(words),
            "{url}: {out}"
        );
    }

    // A good one: saved, and offered to every account.
    let (status, out) = admin
        .post(
            "/api/admin/default-5etools",
            json!({ "url": " https://github.com/example/srd-data " }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["url"], "https://github.com/example/srd-data", "{out}");
    assert_eq!(out["source"], "admin", "{out}");
    let (_, out) = player.get("/api/import/default-5etools").await;
    assert_eq!(out["url"], "https://github.com/example/srd-data", "{out}");

    // "" is none, saved; null goes back to the config file (none here).
    let (status, out) = admin
        .post("/api/admin/default-5etools", json!({ "url": "" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(
        (out["url"].clone(), out["source"].clone()),
        (Value::Null, json!("admin")),
        "{out}"
    );
    let (status, out) = admin
        .post("/api/admin/default-5etools", json!({ "url": null }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["source"], "none", "{out}");
    let (_, out) = player.get("/api/import/default-5etools").await;
    assert_eq!(out["url"], Value::Null, "{out}");

    // Signed out: nothing.
    let (status, _) = super::send(
        &router,
        None,
        "GET",
        "/api/import/default-5etools",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn packs_are_removed_together_from_the_callers_own_content_only() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let alice = sign_up(&router, "bulk-alice").await;
    let bob = sign_up(&router, "bulk-bob").await;
    for file in ["first.xml", "second.xml", "third.xml"] {
        let (status, out) = alice.rpc("import_pack", pack_named(file)).await;
        assert_eq!(status, StatusCode::OK, "{out}");
    }
    let (status, out) = bob.rpc("import_pack", pack_named("first.xml")).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let mine = packs(&alice).await;
    assert_eq!(mine.len(), 3, "{mine:?}");

    // One of alice's packs came from an index (as an index download records it).
    let account = dir.path().join("users").join(&alice.id);
    let second = find_dir(&account, &mine[1])
        .unwrap_or_else(|| panic!("no folder for {} under {}", mine[1], account.display()));
    std::fs::write(
        second.join("pack-meta.json"),
        json!({ "kind": "downloaded", "indexUrl": "https://content.example/srd.index" })
            .to_string(),
    )
    .unwrap();
    let (status, out) = alice.get("/api/content/pack-sources").await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(
        out["packs"],
        json!([{ "id": mine[1], "indexUrl": "https://content.example/srd.index", "fiveToolsUrl": null }]),
        "{out}"
    );
    let (_, out) = bob.get("/api/content/pack-sources").await;
    assert_eq!(out["packs"], json!([]), "bob sees only his own: {out}");

    // Built-in and unknown ids are skipped; the rest go in one request.
    let (status, out) = alice
        .post(
            "/api/content/remove-packs",
            json!({ "packIds": [mine[0], mine[1], "core", "not-installed"] }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["removed"], 2, "{out}");
    assert_eq!(out["skipped"], json!(["core", "not-installed"]), "{out}");
    assert_eq!(own_ids(&out["packs"]), vec![mine[2].clone()], "{out}");
    assert_eq!(packs(&alice).await, vec![mine[2].clone()]);
    assert!(!second.exists(), "the folder is gone");

    // Bob's pack of the same name is his: alice's removal never touched it, and bob can't remove alice's.
    assert_eq!(packs(&bob).await.len(), 1);
    let (status, out) = bob
        .post("/api/content/remove-packs", json!({ "packIds": [mine[2]] }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(
        packs(&alice).await,
        vec![mine[2].clone()],
        "alice keeps hers"
    );

    // Nothing to remove, or signed out: refused.
    let (status, _) = alice
        .post("/api/content/remove-packs", json!({ "packIds": [] }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = super::send(
        &router,
        None,
        "POST",
        "/api/content/remove-packs",
        json!({ "packIds": ["x"] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
