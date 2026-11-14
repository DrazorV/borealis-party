//! Admin › Trash (`admin_trash.rs`): a deleted character is listed for the admin with names and dates only, restored
//! into its owner's list, and gone from the trash after that. Non-admins get 403.

use axum::http::StatusCode;
use serde_json::{json, Value};

use super::{new_character, router, sign_up, Client};

async fn character(client: &Client, name: &str) -> String {
    let (status, out) = client.rpc("create_character", new_character(name)).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    let (status, out) = client
        .rpc("save_character", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    id
}

async fn ids(client: &Client) -> Vec<String> {
    let (status, list) = client.rpc("list_characters", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list.as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn admins_see_and_restore_deleted_characters() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let admin = sign_up(&router, "trash-admin").await;
    let owner = sign_up(&router, "trash-owner").await;
    let kept = character(&owner, "Kept Hero").await;
    let gone = character(&owner, "Lost Hero").await;
    let (status, out) = owner
        .rpc("delete_character", json!({ "characterId": gone }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(ids(&owner).await, vec![kept.clone()]);

    // Only admins.
    let (status, _) = owner.get("/api/admin/trash").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = owner
        .post(
            "/api/admin/trash/restore",
            json!({ "accountId": owner.id, "characterId": gone }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, trash) = admin.get("/api/admin/trash").await;
    assert_eq!(status, StatusCode::OK, "{trash}");
    assert_eq!(trash["retentionDays"], 30);
    let items = trash["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{trash}");
    let item = &items[0];
    assert_eq!(item["accountId"], owner.id.as_str());
    assert_eq!(item["username"], "trash-owner");
    assert_eq!(item["kind"], "character");
    assert_eq!(item["characterId"], gone.as_str());
    assert_eq!(item["name"], "Lost Hero");
    let deleted = item["deletedAtMs"].as_u64().unwrap();
    assert_eq!(
        item["purgeAtMs"].as_u64().unwrap(),
        deleted + 30 * 24 * 60 * 60 * 1000
    );
    // Names and dates only: nothing of the sheet.
    let mut keys: Vec<&str> = item
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "accountId",
            "characterId",
            "deletedAtMs",
            "kind",
            "name",
            "purgeAtMs",
            "username"
        ]
    );

    // Unknown ids and bad ones: 404; an unknown account: 404; extra fields: refused.
    for (account, character) in [
        (owner.id.as_str(), "c_00000000000000000000000000000000"),
        (owner.id.as_str(), "../../etc/passwd"),
        ("no-such-account", gone.as_str()),
    ] {
        let (status, out) = admin
            .post(
                "/api/admin/trash/restore",
                json!({ "accountId": account, "characterId": character }),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{account} {character}: {out}"
        );
    }
    let (status, _) = admin
        .post(
            "/api/admin/trash/restore",
            json!({ "accountId": owner.id, "characterId": gone, "overwrite": true }),
        )
        .await;
    assert!(status.is_client_error());

    // Restore: the character is back in the owner's list, under its own id and name, and out of the trash.
    let (status, out) = admin
        .post(
            "/api/admin/trash/restore",
            json!({ "accountId": owner.id, "characterId": gone }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out, json!({ "characterId": gone, "name": "Lost Hero" }));
    let mut now = ids(&owner).await;
    now.sort();
    let mut want = vec![kept.clone(), gone.clone()];
    want.sort();
    assert_eq!(now, want);
    let (_, list) = owner.rpc("list_characters", json!({})).await;
    assert!(list.to_string().contains("Lost Hero"), "{list}");
    let (_, trash) = admin.get("/api/admin/trash").await;
    assert_eq!(trash["items"], json!([]), "{trash}");

    // Twice: the character is live again, so there is nothing to restore (409); the sync state no longer lists it
    // as deleted.
    let (status, _) = admin
        .post(
            "/api/admin/trash/restore",
            json!({ "accountId": owner.id, "characterId": gone }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, state) = owner.rpc("party_sync_state", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert!(!state["deleted"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["id"] == gone.as_str()));
    assert!(state["characters"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"] == gone.as_str()));

    // The activity log names the admin, the account and the character id.
    let (_, log) = admin.get("/api/admin/activity").await;
    let restored: Vec<&Value> = log["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "character_restored")
        .collect();
    assert_eq!(restored.len(), 1, "{log}");
    assert_eq!(restored[0]["actorName"], "trash-admin");
    assert_eq!(restored[0]["targetName"], "trash-owner");
    assert_eq!(restored[0]["detail"], gone.as_str());
}
