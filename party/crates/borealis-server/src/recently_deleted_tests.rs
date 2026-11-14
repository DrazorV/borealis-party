//! Recently deleted (0.3.6, `recently_deleted.rs`): a deleted party or encounter disappears from every list and route,
//! waits 30 days in its DM's Recently deleted, and comes back whole with Restore (members, encounters, creatures,
//! effects, a combat that was live comes back ended); "Delete forever" and the 30-day sweep remove it for good; a
//! deleted character is listed, restored and removed the same way; nobody sees or restores another account's items.
//! The upgrade: a database a 0.3.5 server left at step 10 gets step 11 with its data kept. On SQLite, and on
//! PostgreSQL when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.

use std::path::Path;

use axum::http::StatusCode;
use serde_json::{json, Value};
use sqlx::Row;

use super::{drop_all, new_character, router_on, sign_in, sign_up, Client, POSTGRES};
use crate::db::{Db, Location};

const GOBLIN: &str = "ID_SRD521_MONSTER_GOBLIN_WARRIOR";

async fn ok(client: &Client, method: &str, path: &str, body: Value) -> Value {
    let (status, out) = super::send(&client.router, Some(&client.token), method, path, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {out}");
    out
}

async fn character(client: &Client, name: &str) -> String {
    let out = ok(
        client,
        "POST",
        "/api/rpc/create_character",
        new_character(name),
    )
    .await;
    let id = out["id"].as_str().unwrap().to_string();
    ok(
        client,
        "POST",
        "/api/rpc/save_character",
        json!({ "characterId": id }),
    )
    .await;
    id
}

async fn new_party(dm: &Client, name: &str) -> String {
    ok(dm, "POST", "/api/me/roles/dm", json!({})).await;
    let out = ok(dm, "POST", "/api/parties", json!({ "name": name })).await;
    out["id"].as_str().unwrap().to_string()
}

async fn join(dm: &Client, party: &str, player: &Client, username: &str, who: &str) -> String {
    let c = character(player, who).await;
    ok(
        dm,
        "POST",
        &format!("/api/parties/{party}/invites"),
        json!({ "kind": "username", "username": username }),
    )
    .await;
    let invites = ok(player, "GET", "/api/invites", Value::Null).await;
    let invite = invites["invites"][0]["id"].as_str().unwrap().to_string();
    ok(
        player,
        "POST",
        &format!("/api/invites/{invite}/accept"),
        json!({ "characterId": c }),
    )
    .await;
    c
}

async fn new_encounter(dm: &Client, party: &str, name: &str) -> String {
    let out = ok(
        dm,
        "POST",
        &format!("/api/parties/{party}/encounters"),
        json!({ "name": name, "publicName": name }),
    )
    .await;
    let e = out["id"].as_str().unwrap().to_string();
    let v = ok(
        dm,
        "POST",
        &format!("/api/encounters/{e}/creatures"),
        json!({ "elementId": GOBLIN, "count": 2, "hidden": false }),
    )
    .await;
    let creature = v["creatures"][0]["id"].as_str().unwrap().to_string();
    ok(
        dm,
        "POST",
        &format!("/api/encounters/{e}/effects"),
        json!({ "creatureId": creature, "name": "Bless" }),
    )
    .await;
    e
}

async fn party_ids(client: &Client) -> Vec<String> {
    let out = ok(client, "GET", "/api/parties", Value::Null).await;
    out["parties"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_string())
        .collect()
}

async fn encounter_ids(dm: &Client, party: &str) -> Vec<String> {
    let out = ok(
        dm,
        "GET",
        &format!("/api/parties/{party}/encounters"),
        Value::Null,
    )
    .await;
    out["encounters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_string())
        .collect()
}

async fn deleted(client: &Client) -> Vec<Value> {
    let out = ok(client, "GET", "/api/me/deleted", Value::Null).await;
    assert_eq!(out["retentionDays"], 30);
    out["items"].as_array().unwrap().clone()
}

async fn restore(client: &Client, kind: &str, id: &str) -> (StatusCode, Value) {
    client
        .post("/api/me/deleted/restore", json!({ "kind": kind, "id": id }))
        .await
}

/// The whole round trip on one database.
async fn round_trip(db: Db, data: &Path) {
    let router = router_on(data.to_path_buf(), db.clone());
    let dm = sign_up(&router, "del-dm").await;
    let player = sign_up(&router, "del-player").await;
    let other = sign_up(&router, "del-other").await;
    let party = new_party(&dm, "The Lantern").await;
    let hero = join(&dm, &party, &player, "del-player", "Hero").await;
    let fight = new_encounter(&dm, &party, "Bridge").await;
    let spare = new_encounter(&dm, &party, "Cellar").await;
    ok(
        &dm,
        "PATCH",
        &format!("/api/encounters/{fight}"),
        json!({ "status": "ready", "playerVisible": true }),
    )
    .await;
    let combat = ok(
        &dm,
        "POST",
        &format!("/api/encounters/{fight}/start"),
        json!({ "requestId": uuid::Uuid::new_v4().to_string() }),
    )
    .await;
    assert!(combat["id"].is_string(), "{combat}");

    // An encounter on its own: gone from the list and its route, back with its creatures and effect.
    ok(
        &dm,
        "DELETE",
        &format!("/api/encounters/{spare}"),
        Value::Null,
    )
    .await;
    assert!(!encounter_ids(&dm, &party).await.contains(&spare));
    let (status, _) = dm.get(&format!("/api/encounters/{spare}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let items = deleted(&dm).await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["kind"], "encounter");
    assert_eq!(items[0]["id"], spare.as_str());
    assert_eq!(items[0]["name"], "Cellar");
    assert_eq!(items[0]["detail"], "The Lantern");
    assert!(items[0].get("snapshot").is_none() && items[0].get("snapshot_json").is_none());
    let deleted_at = items[0]["deletedAtMs"].as_u64().unwrap();
    assert_eq!(
        items[0]["purgeAtMs"].as_u64().unwrap(),
        deleted_at + 30 * 24 * 60 * 60 * 1000
    );
    // Nobody else sees it or gets it back.
    assert!(deleted(&player).await.is_empty());
    assert!(deleted(&other).await.is_empty());
    let (status, _) = restore(&other, "encounter", &spare).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = other
        .delete(&format!("/api/me/deleted/encounter/{spare}"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, out) = restore(&dm, "encounter", &spare).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["name"], "Cellar");
    let back = ok(&dm, "GET", &format!("/api/encounters/{spare}"), Value::Null).await;
    assert_eq!(back["creatures"].as_array().unwrap().len(), 2, "{back}");
    assert!(back.to_string().contains("Bless"), "{back}");
    assert!(deleted(&dm).await.is_empty());
    let (status, _) = restore(&dm, "encounter", &spare).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a second restore finds nothing"
    );

    // The party, mid-combat: gone for the DM and the player, every route 404.
    ok(
        &dm,
        "DELETE",
        &format!("/api/encounters/{spare}"),
        Value::Null,
    )
    .await;
    ok(&dm, "DELETE", &format!("/api/parties/{party}"), Value::Null).await;
    assert!(party_ids(&dm).await.is_empty());
    assert!(party_ids(&player).await.is_empty());
    for path in [
        format!("/api/parties/{party}"),
        format!("/api/parties/{party}/encounters"),
        format!("/api/encounters/{fight}"),
    ] {
        let (status, _) = dm.get(&path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        let (status, _) = player.get(&path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let left: i64 = sqlx::query("SELECT COUNT(*) FROM party_members WHERE party_id = $1")
        .bind(&party)
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(left, 0, "the rows left their tables");
    let mut kinds: Vec<String> = deleted(&dm)
        .await
        .iter()
        .map(|i| i["kind"].as_str().unwrap().to_string())
        .collect();
    kinds.sort();
    assert_eq!(kinds, ["encounter", "party"]);
    // Its encounter waits for the party.
    let (status, out) = restore(&dm, "encounter", &spare).await;
    assert_eq!(status, StatusCode::CONFLICT, "{out}");
    assert!(out.to_string().contains("Restore the party first"), "{out}");

    // Restore: the party, its member, its encounters and creatures; the live combat comes back ended.
    let (status, out) = restore(&dm, "party", &party).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["name"], "The Lantern");
    assert_eq!(party_ids(&dm).await, vec![party.clone()]);
    assert_eq!(party_ids(&player).await, vec![party.clone()]);
    let detail = ok(&dm, "GET", &format!("/api/parties/{party}"), Value::Null).await;
    assert!(detail.to_string().contains(&hero), "{detail}");
    assert_eq!(encounter_ids(&dm, &party).await, vec![fight.clone()]);
    let enc = ok(&dm, "GET", &format!("/api/encounters/{fight}"), Value::Null).await;
    assert_eq!(enc["creatures"].as_array().unwrap().len(), 2, "{enc}");
    let live: i64 =
        sqlx::query("SELECT COUNT(*) FROM combats WHERE party_id = $1 AND status = 'live'")
            .bind(&party)
            .fetch_one(db.pool())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    assert_eq!(live, 0);
    let ended: i64 =
        sqlx::query("SELECT COUNT(*) FROM combats WHERE party_id = $1 AND ended_at IS NOT NULL")
            .bind(&party)
            .fetch_one(db.pool())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
    assert_eq!(ended, 1);
    // Now the encounter can come back too, and a new combat can start.
    let (status, out) = restore(&dm, "encounter", &spare).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    ok(
        &dm,
        "PATCH",
        &format!("/api/encounters/{spare}"),
        json!({ "status": "ready" }),
    )
    .await;
    ok(
        &dm,
        "POST",
        &format!("/api/encounters/{spare}/start"),
        json!({ "requestId": uuid::Uuid::new_v4().to_string() }),
    )
    .await;

    // "Delete forever": a party takes its deleted encounters with it.
    let second = new_party(&dm, "Second").await;
    let lost = new_encounter(&dm, &second, "Lost").await;
    ok(
        &dm,
        "DELETE",
        &format!("/api/encounters/{lost}"),
        Value::Null,
    )
    .await;
    ok(
        &dm,
        "DELETE",
        &format!("/api/parties/{second}"),
        Value::Null,
    )
    .await;
    assert_eq!(deleted(&dm).await.len(), 2);
    let (status, _) = other
        .delete(&format!("/api/me/deleted/party/{second}"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    ok(
        &dm,
        "DELETE",
        &format!("/api/me/deleted/party/{second}"),
        Value::Null,
    )
    .await;
    assert!(deleted(&dm).await.is_empty());
    let (status, _) = restore(&dm, "party", &second).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The sweep: 30 days on, it's gone.
    let third = new_party(&dm, "Third").await;
    ok(&dm, "DELETE", &format!("/api/parties/{third}"), Value::Null).await;
    let at = crate::db::now();
    assert_eq!(crate::recently_deleted::sweep(&db, at).await.unwrap(), 0);
    assert_eq!(deleted(&dm).await.len(), 1);
    sqlx::query("UPDATE deleted_items SET deleted_at = $1 WHERE id = $2")
        .bind(at - 30 * 24 * 60 * 60 + 60)
        .bind(&third)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        crate::recently_deleted::sweep(&db, at).await.unwrap(),
        0,
        "29 days 23 hours"
    );
    assert_eq!(
        crate::recently_deleted::sweep(&db, at + 120).await.unwrap(),
        1,
        "30 days"
    );
    assert!(deleted(&dm).await.is_empty());

    // Characters: listed from the trash, restored, or deleted forever.
    let kept = character(&player, "Keeper").await;
    let gone = character(&player, "Goner").await;
    for id in [&kept, &gone] {
        ok(
            &player,
            "POST",
            "/api/rpc/delete_character",
            json!({ "characterId": id }),
        )
        .await;
    }
    let items = deleted(&player).await;
    let names: Vec<&str> = items.iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert_eq!(items.len(), 2, "{items:?}");
    assert!(
        names.contains(&"Keeper") && names.contains(&"Goner"),
        "{names:?}"
    );
    assert!(items.iter().all(|i| i["kind"] == "character"));
    let (status, _) = restore(&other, "character", &kept).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, out) = restore(&player, "character", &kept).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["name"], "Keeper");
    let list = ok(&player, "POST", "/api/rpc/list_characters", json!({})).await;
    assert!(list.to_string().contains(&kept), "{list}");
    ok(
        &player,
        "DELETE",
        &format!("/api/me/deleted/character/{gone}"),
        Value::Null,
    )
    .await;
    assert!(deleted(&player).await.is_empty());
    let (status, _) = restore(&player, "character", &gone).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = player.delete("/api/me/deleted/character/..%2Fevil").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = restore(&player, "pack", "x").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn deleted_items_come_back_or_go_for_good() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Sqlite(dir.path().join("party.db")))
        .await
        .unwrap();
    round_trip(db, &dir.path().join("data")).await;
}

/// A database a 0.3.5 server left (steps 1 to 10, with a party and an encounter) gets step 11 and keeps its data;
/// the step can run again (its marker) and a delete works right after.
async fn upgrade(location: &Location, data: &Path) {
    assert_eq!(Db::schema_version(), 11);
    let old = Db::open_at(location, 10).await.unwrap();
    let party = {
        let router = router_on(data.to_path_buf(), old.clone());
        let dm = sign_up(&router, "up-dm").await;
        let party = new_party(&dm, "Old Table").await;
        new_encounter(&dm, &party, "Old Fight").await;
        party
    };
    assert_eq!(old.pending_steps().await.unwrap(), Some(vec![11]));
    old.pool().close().await;

    let db = Db::open(location).await.unwrap();
    assert_eq!(db.pending_steps().await.unwrap(), Some(vec![]));
    let router = router_on(data.to_path_buf(), db.clone());
    let dm = sign_in(&router, "up-dm").await;
    assert_eq!(party_ids(&dm).await, vec![party.clone()]);
    assert_eq!(encounter_ids(&dm, &party).await.len(), 1);
    assert!(deleted(&dm).await.is_empty());
    ok(&dm, "DELETE", &format!("/api/parties/{party}"), Value::Null).await;
    assert_eq!(deleted(&dm).await.len(), 1);

    // Recorded but missing its table (another numbering): step 11 runs again, the rest stays.
    sqlx::query("DROP TABLE deleted_items")
        .execute(db.pool())
        .await
        .unwrap();
    db.pool().close().await;
    let again = Db::open(location).await.unwrap();
    let router = router_on(data.to_path_buf(), again.clone());
    let dm = sign_in(&router, "up-dm").await;
    assert!(deleted(&dm).await.is_empty());
    let fresh = new_party(&dm, "After").await;
    ok(&dm, "DELETE", &format!("/api/parties/{fresh}"), Value::Null).await;
    let (status, out) = restore(&dm, "party", &fresh).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    again.pool().close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn step_11_arrives_on_a_0_3_5_database() {
    let dir = tempfile::tempdir().unwrap();
    upgrade(
        &Location::Sqlite(dir.path().join("party.db")),
        &dir.path().join("data"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn recently_deleted_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = POSTGRES.lock().await;
    let dir = tempfile::tempdir().unwrap();
    drop_all(&url).await;
    upgrade(&Location::Postgres(url.clone()), &dir.path().join("up")).await;
    drop_all(&url).await;
    let db = Db::open(&Location::Postgres(url.clone())).await.unwrap();
    round_trip(db.clone(), &dir.path().join("data")).await;
    db.pool().close().await;
    drop_all(&url).await;
}
