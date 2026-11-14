//! The sync commands for the apps: state, one character, one pack, delete records, events, isolation.

use super::*;

use std::collections::BTreeMap;

use base64::Engine as _;
use borealis_bundle::{list_packs, scan_characters, scan_packs, sha256_hex, Limits};
use tauri::Manager as _;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(text: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(text.as_str().unwrap_or_else(|| panic!("{text}")))
        .unwrap()
}

fn account_dir(data: &Path, account: &str) -> PathBuf {
    data.join("users").join(account)
}

fn trash(data: &Path, account: &str) -> PathBuf {
    account_dir(data, account).join("sync").join("trash")
}

fn ids(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap_or_else(|| panic!("{list}"))
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect()
}

/// The account's event stream, open.
async fn event_stream(client: &Client) -> Body {
    let request = Request::builder()
        .method("GET")
        .uri("/api/events")
        .header("authorization", format!("Bearer {}", client.token))
        .body(Body::empty())
        .unwrap();
    let response = client.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body()
}

/// The payload of the next `name` event on the stream.
async fn next_event(body: &mut Body, buffer: &mut String, name: &str) -> Value {
    loop {
        while let Some(end) = buffer.find("\n\n") {
            let message: String = buffer.drain(..end + 2).collect();
            for line in message.lines() {
                if let Some(data) = line.strip_prefix("data:") {
                    let event: Value = serde_json::from_str(data.trim()).unwrap();
                    if event["event"] == name {
                        return event["payload"].clone();
                    }
                }
            }
        }
        let frame = tokio::time::timeout(Duration::from_secs(20), body.frame())
            .await
            .unwrap_or_else(|_| panic!("no {name} event"))
            .expect("the stream stays open")
            .unwrap();
        if let Ok(data) = frame.into_data() {
            buffer.push_str(&String::from_utf8_lossy(&data));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_apps_read_an_account_through_the_sync_commands() {
    let dir = tempfile::tempdir().unwrap();
    sync_flow(router(dir.path().to_path_buf()).await, dir.path()).await;
}

/// Also run on PostgreSQL, from the test there.
pub(super) async fn sync_flow(router: Router, data: &Path) {
    let alice = sign_up(&router, "sync-alice").await;
    let bob = sign_up(&router, "sync-bob").await;
    let mut stream = event_stream(&alice).await;
    let mut buffer = String::new();

    // An empty account.
    let (status, empty) = alice.rpc("party_sync_state", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert!(empty["nowMs"].as_u64().unwrap() > 0);
    assert!(!empty["appVersion"].as_str().unwrap().is_empty());
    for list in ["characters", "packs", "deleted"] {
        assert_eq!(empty[list], json!([]), "{list}: {empty}");
    }

    // A saved character is listed with the digest and stamp of its file, and saving says so on the event stream.
    let (status, made) = alice.rpc("create_character", new_character("Mirror")).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let id = made["id"].as_str().unwrap().to_string();
    let changed = next_event(&mut stream, &mut buffer, crate::sync::CHARACTERS_CHANGED).await;
    assert_eq!(changed["ids"], json!([id]), "{changed}");
    let character = json!({ "characterId": id });
    let (status, out) = alice.rpc("save_character", character.clone()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let changed = next_event(&mut stream, &mut buffer, crate::sync::CHARACTERS_CHANGED).await;
    assert_eq!(changed["ids"], json!([id]), "{changed}");

    // An unsaved edit is saved before the state is read: the state is the file's.
    let (status, out) = alice
        .rpc("set_xp", json!({ "characterId": id, "xp": 2700 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, state) = alice.rpc("party_sync_state", json!({})).await;
    let file = character_file(data, &alice.id, &id);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("2700"), "the edit reached the file");
    let scanned = scan_characters(file.parent().unwrap()).unwrap();
    assert_eq!(scanned.len(), 1);
    assert_eq!(
        state["characters"],
        json!([{ "id": id, "editedAtMs": scanned[0].edited_at_ms, "sha256": scanned[0].sha256 }]),
    );

    // One character's file: the bytes as they are, with meta.id set to the id.
    let (status, got) = alice.rpc("party_sync_get", json!({ "id": id })).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["sha256"], scanned[0].sha256);
    assert_eq!(got["sha256"], sha256_hex(text.as_bytes()));
    assert_eq!(got["editedAtMs"], scanned[0].edited_at_ms);
    let content: Value = serde_json::from_slice(&unb64(&got["contentBase64"])).unwrap();
    assert_eq!(content["meta"]["id"], id.as_str());
    let mut on_disk: Value = serde_json::from_str(&text).unwrap();
    on_disk["meta"]["id"] = json!(id);
    assert_eq!(content, on_disk, "nothing else changes");

    // The device imports it under an id of its own and keeps meta.id: the id map can be rebuilt from its files.
    let device = Desktop::new().await;
    let imported = device
        .run(
            "import_character",
            json!({ "fileName": format!("{id}.json"), "contentBase64": got["contentBase64"] }),
        )
        .await;
    let local = imported["summary"]["id"].as_str().unwrap();
    assert_ne!(local, id);
    let local_file: Value = serde_json::from_str(
        &std::fs::read_to_string(
            device
                .dir
                .path()
                .join("characters")
                .join(format!("{local}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(local_file["meta"]["id"], id.as_str());

    // User packs only, never the bundled ones.
    let (status, out) = alice.rpc("import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, state) = alice.rpc("party_sync_state", json!({})).await;
    let library = account_dir(data, &alice.id).join("library");
    let packs = scan_packs(&library, &Limits::default()).unwrap();
    assert_eq!(packs.len(), 1);
    assert_eq!(
        state["packs"],
        json!([{
            "id": "party",
            "editedAtMs": packs[0].edited_at_ms,
            "sha256": packs[0].sha256,
            "enabled": true,
            "indexUrl": null,
        }]),
    );
    let (status, out) = alice
        .rpc(
            "set_pack_enabled",
            json!({ "packId": "party", "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, state) = alice.rpc("party_sync_state", json!({})).await;
    assert_eq!(state["packs"][0]["enabled"], false, "{state}");

    // One pack as one content file, for the device's own import.
    let (status, merged) = alice.rpc("party_sync_pack", json!({ "id": "party" })).await;
    assert_eq!(status, StatusCode::OK, "{merged}");
    assert_eq!(merged["fileName"], "party.json");
    assert_eq!(merged["sha256"], packs[0].sha256);
    let file: Value = serde_json::from_slice(&unb64(&merged["contentBase64"])).unwrap();
    assert_eq!(ids(&file["elements"]), ["ID_PARTY_SPARK"], "{file}");

    // Another account sees none of it.
    let (_, theirs) = bob.rpc("party_sync_state", json!({})).await;
    for list in ["characters", "packs", "deleted"] {
        assert_eq!(theirs[list], json!([]), "{list}: {theirs}");
    }
    let (status, _) = bob.rpc("party_sync_get", json!({ "id": id })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = bob.rpc("party_sync_pack", json!({ "id": "party" })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Ids are plain names, never paths.
    for bad in [
        json!({}),
        json!({ "id": "../x" }),
        json!({ "id": "a/b" }),
        json!({ "id": 7 }),
    ] {
        for command in ["party_sync_get", "party_sync_pack"] {
            let (status, out) = alice.rpc(command, bad.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{command} {bad}: {out}");
        }
    }
    // Bundles leave only as a download (bundle_export), never by share sheet or link.
    for command in ["bundle_share", "link_status"] {
        let (status, out) = alice.rpc(command, json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{command}: {out}");
    }

    // A delete leaves a copy in the account's trash: the delete record, with the edits not yet saved.
    let saved = std::fs::read_to_string(character_file(data, &alice.id, &id)).unwrap();
    let (status, out) = alice
        .rpc("set_xp", json!({ "characterId": id, "xp": 4321 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert!(!saved.contains("4321"), "{saved}");
    let before = crate::db::now();
    let (status, out) = alice.rpc("delete_character", character.clone()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let changed = next_event(&mut stream, &mut buffer, crate::sync::CHARACTERS_CHANGED).await;
    assert_eq!(changed["ids"], json!([id]), "{changed}");
    let records: Vec<PathBuf> = std::fs::read_dir(trash(data, &alice.id))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(records.len(), 1, "{records:?}");
    let kept = std::fs::read_to_string(&records[0]).unwrap();
    assert!(kept.contains("4321"), "{kept}");
    let (_, state) = alice.rpc("party_sync_state", json!({})).await;
    assert_eq!(state["characters"], json!([]));
    let deleted_at = state["deleted"][0]["deletedAtMs"].as_u64().unwrap();
    assert_eq!(
        state["deleted"],
        json!([{ "id": id, "deletedAtMs": deleted_at }])
    );
    assert!(deleted_at / 1000 >= u64::try_from(before).unwrap());
    assert_eq!(
        records[0].file_name().unwrap().to_str().unwrap(),
        format!("{deleted_at}-{id}.json")
    );
    // Deleting what is not there leaves no record.
    let (status, _) = alice.rpc("delete_character", character.clone()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        std::fs::read_dir(trash(data, &alice.id)).unwrap().count(),
        1
    );
    let (status, _) = alice.rpc("party_sync_get", json!({ "id": id })).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Bob's trash is his own.
    let (_, theirs) = bob.rpc("party_sync_state", json!({})).await;
    assert_eq!(theirs["deleted"], json!([]));

    // After 30 days the record goes, with the copy.
    let old = trash(data, &alice.id).join(format!("{}-{id}.json", deleted_at - 31 * DAY_MS));
    std::fs::rename(&records[0], &old).unwrap();
    let (_, state) = alice.rpc("party_sync_state", json!({})).await;
    assert_eq!(state["deleted"], json!([]), "{state}");
    assert!(!old.exists());
    // A trash entry of a live character is not a delete record.
    let (_, made) = alice.rpc("create_character", new_character("Back")).await;
    let back = made["id"].as_str().unwrap().to_string();
    std::fs::write(
        trash(data, &alice.id).join(format!("{}-{back}.json", deleted_at)),
        b"{}",
    )
    .unwrap();
    let (_, state) = alice.rpc("party_sync_state", json!({})).await;
    assert_eq!(state["deleted"], json!([]), "{state}");
    assert_eq!(ids(&state["characters"]), [back.as_str()]);
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// The digest and stamp of a character's file on the server.
fn on_file(data: &Path, account: &str, id: &str) -> (String, u64) {
    let file = character_file(data, account, id);
    (
        sha256_hex(&std::fs::read(&file).unwrap()),
        borealis_bundle::mtime_ms(&file),
    )
}

/// A device's copy of the server's file of `id`, edited: its player is `who`.
fn device_copy(data: &Path, account: &str, id: &str, who: &str) -> Vec<u8> {
    let mut value: Value =
        serde_json::from_slice(&std::fs::read(character_file(data, account, id)).unwrap()).unwrap();
    value["meta"]["id"] = json!(id);
    value["meta"]["player"] = json!(who);
    serde_json::to_vec_pretty(&value).unwrap()
}

/// The trash entries of an account whose text names `who`, by file name.
fn trashed(data: &Path, account: &str, who: &str) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(trash(data, account))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains(&format!("\"{who}\"")))
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    found.sort();
    found
}

fn player(data: &Path, account: &str, id: &str) -> String {
    let value: Value =
        serde_json::from_slice(&std::fs::read(character_file(data, account, id)).unwrap()).unwrap();
    value["meta"]["player"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

async fn push(
    client: &Client,
    id: &str,
    base: Option<&str>,
    edited_at_ms: u64,
    content: &[u8],
) -> (StatusCode, Value) {
    client
        .rpc(
            "party_sync_push",
            json!({ "id": id, "baseSha": base, "editedAtMs": edited_at_ms, "contentBase64": b64(content) }),
        )
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_apps_send_their_edits_back() {
    let dir = tempfile::tempdir().unwrap();
    push_flow(router(dir.path().to_path_buf()).await, dir.path()).await;
}

/// Sending edits back: fast-forward, conflicts, ties, clamped times, repeats, deletes, races, unsaved server edits,
/// refusals and isolation. Also run on PostgreSQL, from the test there.
pub(super) async fn push_flow(router: Router, data: &Path) {
    let alice = sign_up(&router, "push-alice").await;
    let bob = sign_up(&router, "push-bob").await;
    let mut stream = event_stream(&alice).await;
    let mut buffer = String::new();
    let (_, made) = alice.rpc("create_character", new_character("Pusher")).await;
    let id = made["id"].as_str().unwrap().to_string();
    let (status, out) = alice
        .rpc("save_character", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let _ = next_event(&mut stream, &mut buffer, crate::sync::CHARACTERS_CHANGED).await;
    let _ = next_event(&mut stream, &mut buffer, crate::sync::CHARACTERS_CHANGED).await;

    // Fast-forward: the server still holds the device's base, so the device's copy goes on even with a clock far
    // behind; it is stamped after the old file, which goes to the trash.
    let (base, base_ms) = on_file(data, &alice.id, &id);
    let first = device_copy(data, &alice.id, &id, "first");
    let (status, out) = push(&alice, &id, Some(&base), 1000, &first).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["result"], "applied", "{out}");
    let (sha, ms) = on_file(data, &alice.id, &id);
    assert_eq!(sha, sha256_hex(&first));
    assert_eq!(ms, base_ms + 1);
    assert_eq!(out["sha256"], sha.as_str());
    assert_eq!(out["editedAtMs"], ms);
    let changed = next_event(&mut stream, &mut buffer, crate::sync::CHARACTERS_CHANGED).await;
    assert_eq!(changed["ids"], json!([id]), "{changed}");
    assert_eq!(changed["replaced"], true, "{changed}");
    let kept: Vec<String> = std::fs::read_dir(trash(data, &alice.id))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].ends_with(&format!("-{id}.json")), "{kept:?}");
    // The live file and a trash copy of it are no delete record.
    let (_, state) = alice.rpc("party_sync_state", json!({})).await;
    assert_eq!(state["deleted"], json!([]), "{state}");

    // The same push again (its answer was lost): identical, nothing written.
    let (_, out) = push(&alice, &id, Some(&base), 1000, &first).await;
    assert_eq!(out["result"], "identical", "{out}");
    assert_eq!(on_file(data, &alice.id, &id), (sha.clone(), ms));

    // Both changed: an older device edit loses and is kept in the trash, as the device's copy.
    let stale = device_copy(data, &alice.id, &id, "stale");
    let (_, out) = push(&alice, &id, Some(&base), ms - 10, &stale).await;
    assert_eq!(out["result"], "kept-server", "{out}");
    assert_eq!(out["sha256"], sha.as_str());
    assert_eq!(player(data, &alice.id, &id), "first");
    let lost = trashed(data, &alice.id, "stale");
    assert_eq!(lost.len(), 1);
    assert!(lost[0].ends_with(&format!("-{id}.device.json")), "{lost:?}");
    // A tie keeps the server's copy too.
    let (_, out) = push(
        &alice,
        &id,
        Some(&base),
        ms,
        &device_copy(data, &alice.id, &id, "tie"),
    )
    .await;
    assert_eq!(out["result"], "kept-server", "{out}");
    assert_eq!(trashed(data, &alice.id, "tie").len(), 1);
    // A newer device edit wins, and the server's copy goes to the trash.
    let newer = device_copy(data, &alice.id, &id, "newer");
    let (_, out) = push(&alice, &id, None, ms + 10, &newer).await;
    assert_eq!(out["result"], "applied", "{out}");
    assert_eq!(on_file(data, &alice.id, &id), (sha256_hex(&newer), ms + 10));
    assert_eq!(trashed(data, &alice.id, "first").len(), 1);

    // A time in the future counts as now.
    let (sha, _) = on_file(data, &alice.id, &id);
    let before = now_ms();
    let (_, out) = push(
        &alice,
        &id,
        Some("not the base"),
        before + 3_600_000,
        &device_copy(data, &alice.id, &id, "future"),
    )
    .await;
    assert_eq!(out["result"], "applied", "{out}");
    let (_, ms) = on_file(data, &alice.id, &id);
    assert!(ms >= before && ms <= now_ms(), "{ms} {before}");
    assert_ne!(on_file(data, &alice.id, &id).0, sha);

    // Edits the server has not saved yet are saved first and take part: an older device edit loses to them.
    let (base, _) = on_file(data, &alice.id, &id);
    let (_, out) = alice
        .rpc("load_character", json!({ "characterId": id }))
        .await;
    assert!(out.is_object(), "{out}");
    let (status, out) = alice
        .rpc("set_xp", json!({ "characterId": id, "xp": 6543 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, out) = push(
        &alice,
        &id,
        Some(&base),
        now_ms() - 60_000,
        &device_copy(data, &alice.id, &id, "behind"),
    )
    .await;
    assert_eq!(out["result"], "kept-server", "{out}");
    let text = std::fs::read_to_string(character_file(data, &alice.id, &id)).unwrap();
    assert!(text.contains("6543"), "the unsaved edit reached the file");
    // Saved, the session is no edit of its own: a device copy made from that file goes on, the old file (with the
    // edit) goes to the trash, and the open session shows the device's copy.
    let (base, _) = on_file(data, &alice.id, &id);
    let device = device_copy(data, &alice.id, &id, "ahead");
    let (_, out) = push(&alice, &id, Some(&base), now_ms(), &device).await;
    assert_eq!(out["result"], "applied", "{out}");
    let kept = std::fs::read_dir(trash(data, &alice.id))
        .unwrap()
        .filter_map(|e| std::fs::read_to_string(e.unwrap().path()).ok())
        .filter(|t| t.contains("6543") && !t.contains("\"ahead\""))
        .count();
    assert!(kept >= 1, "the server's copy with the edit is in the trash");
    let (_, sheet) = alice
        .rpc("get_snapshot", json!({ "characterId": id }))
        .await;
    assert!(sheet.to_string().contains("\"ahead\""), "{sheet}");
    assert_eq!(sheet["dirty"], false, "{sheet}");
    let (_, state) = alice.rpc("party_sync_state", json!({})).await;
    assert_eq!(state["characters"][0]["sha256"], sha256_hex(&device));
    // The next edit of the open page lands on the device's copy.
    let (status, out) = alice
        .rpc("set_xp", json!({ "characterId": id, "xp": 7654 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, sheet) = alice
        .rpc("get_snapshot", json!({ "characterId": id }))
        .await;
    assert!(sheet.to_string().contains("\"ahead\""), "{sheet}");
    assert!(sheet.to_string().contains("7654"), "{sheet}");
    alice
        .rpc(
            "close_character",
            json!({ "characterId": id, "discard": false }),
        )
        .await;

    // Deleted after the device's edit: stays deleted, and the device's copy goes to the trash.
    let (status, _) = alice
        .rpc("delete_character", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, state) = alice.rpc("party_sync_state", json!({})).await;
    let deleted_at = state["deleted"][0]["deletedAtMs"].as_u64().unwrap();
    let (_, out) = push(
        &alice,
        &id,
        Some(&sha256_hex(&device)),
        deleted_at - 1,
        &device,
    )
    .await;
    assert_eq!(out["result"], "deleted", "{out}");
    assert_eq!(out["sha256"], Value::Null);
    assert!(!character_file(data, &alice.id, &id).exists());
    let (_, state) = alice.rpc("party_sync_state", json!({})).await;
    assert_eq!(
        state["deleted"],
        json!([{ "id": id, "deletedAtMs": deleted_at }]),
        "a lost copy is no newer delete record"
    );
    // Edited after the delete: it comes back.
    let back = device_copy_from(&device, "back");
    let (_, out) = push(&alice, &id, Some("gone"), deleted_at + 1, &back).await;
    assert_eq!(out["result"], "applied", "{out}");
    assert_eq!(player(data, &alice.id, &id), "back");
    let (_, state) = alice.rpc("party_sync_state", json!({})).await;
    assert_eq!(ids(&state["characters"]), [id.as_str()]);
    assert_eq!(state["deleted"], json!([]));

    // Two devices at once, from the same base: one after the other, and neither copy is lost.
    let (base, _) = on_file(data, &alice.id, &id);
    let one = device_copy(data, &alice.id, &id, "device one");
    let two = device_copy(data, &alice.id, &id, "device two");
    let at = now_ms();
    let (a, b) = tokio::join!(
        push(&alice, &id, Some(&base), at, &one),
        push(&alice, &id, Some(&base), at + 5, &two),
    );
    let results = [a.1["result"].clone(), b.1["result"].clone()];
    assert!(results.contains(&json!("applied")), "{results:?}");
    let live = player(data, &alice.id, &id);
    let other = if live == "device one" {
        "device two"
    } else {
        "device one"
    };
    assert!(live == "device one" || live == "device two", "{live}");
    assert_eq!(trashed(data, &alice.id, other).len(), 1, "{results:?}");

    // What is not a character of this account is refused, and changes nothing.
    let (sha, ms) = on_file(data, &alice.id, &id);
    let mut wrong_id: Value = serde_json::from_slice(&one).unwrap();
    wrong_id["meta"]["id"] = json!("c_someone_else");
    for (bad, why) in [
        (
            json!({ "id": "../x", "editedAtMs": 1, "contentBase64": b64(&one) }),
            "path",
        ),
        (json!({ "id": id, "contentBase64": b64(&one) }), "no time"),
        (
            json!({ "id": id, "editedAtMs": 1, "contentBase64": "%%%" }),
            "not base64",
        ),
        (
            json!({ "id": id, "editedAtMs": 1, "contentBase64": b64(b"not json") }),
            "not JSON",
        ),
        (
            json!({ "id": id, "editedAtMs": 1, "contentBase64": b64(b"[1,2]") }),
            "not a character",
        ),
        (
            json!({ "id": id, "editedAtMs": now_ms(), "contentBase64": b64(&serde_json::to_vec(&wrong_id).unwrap()) }),
            "another id",
        ),
    ] {
        let (status, out) = alice.rpc("party_sync_push", bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {out}");
    }
    assert_eq!(on_file(data, &alice.id, &id), (sha.clone(), ms));

    // Another account's push lands in its own folder only.
    let (_, out) = push(&bob, &id, None, now_ms(), &one).await;
    assert_eq!(out["result"], "applied", "{out}");
    assert_eq!(on_file(data, &alice.id, &id), (sha, ms));
    assert_eq!(player(data, &bob.id, &id), "device one");
}

/// A 0.3.6 device's push (`ask`, and `force` for "Keep mine").
async fn ask_push(
    client: &Client,
    id: &str,
    base: Option<&str>,
    content: &[u8],
    force: bool,
) -> (StatusCode, Value) {
    client
        .rpc(
            "party_sync_push",
            json!({ "id": id, "baseSha": base, "editedAtMs": now_ms(), "contentBase64": b64(content), "ask": true, "force": force }),
        )
        .await
}

/// The trash entries of an account (none when there is no trash yet).
fn trash_count(data: &Path, account: &str) -> usize {
    std::fs::read_dir(trash(data, account)).map_or(0, Iterator::count)
}

/// Offline conflict safety (0.3.6): a device that asks is never decided for. A stale push (another device or the DM
/// changed the file, or it was deleted) is a `conflict` that writes nothing anywhere, whatever the clocks say; a fresh
/// one goes on; "Keep mine" (`force`) writes over the server's copy, which goes to the trash.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_push_from_a_device_that_asks_is_a_conflict_and_never_applied() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let router = router(data.to_path_buf()).await;
    let alice = sign_up(&router, "ask-alice").await;
    let (_, made) = alice.rpc("create_character", new_character("Asker")).await;
    let id = made["id"].as_str().unwrap().to_string();
    let (status, out) = alice
        .rpc("save_character", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (base, _) = on_file(data, &alice.id, &id);

    // Fresh: the server still holds the base, so the device's edit goes on.
    let fresh = device_copy(data, &alice.id, &id, "fresh");
    let (status, out) = ask_push(&alice, &id, Some(&base), &fresh, false).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["result"], "applied", "{out}");
    let (server_sha, server_ms) = on_file(data, &alice.id, &id);
    assert_eq!(server_sha, sha256_hex(&fresh));

    // Stale: made from the old base while the server moved on. Even with a newer edit time it is not applied, and
    // nothing goes to the trash: the device keeps its copy.
    let trash_before = trash_count(data, &alice.id);
    let stale = device_copy_from(&fresh, "stale");
    let (status, out) = ask_push(&alice, &id, Some(&base), &stale, false).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["result"], "conflict", "{out}");
    assert_eq!(out["sha256"], server_sha.as_str());
    assert_eq!(
        on_file(data, &alice.id, &id),
        (server_sha.clone(), server_ms)
    );
    assert_eq!(player(data, &alice.id, &id), "fresh");
    assert_eq!(trash_count(data, &alice.id), trash_before);
    // An unknown base is no base: a conflict too.
    let (_, out) = ask_push(&alice, &id, None, &stale, false).await;
    assert_eq!(out["result"], "conflict", "{out}");
    assert_eq!(player(data, &alice.id, &id), "fresh");

    // The same content as the server's is no conflict.
    let (_, out) = ask_push(&alice, &id, Some(&base), &fresh, false).await;
    assert_eq!(out["result"], "identical", "{out}");

    // "Keep mine": written over the server's copy, which is kept in the trash.
    let (_, out) = ask_push(&alice, &id, Some(&base), &stale, true).await;
    assert_eq!(out["result"], "applied", "{out}");
    assert_eq!(player(data, &alice.id, &id), "stale");
    assert_eq!(out["sha256"], sha256_hex(&stale).as_str());
    assert_eq!(trashed(data, &alice.id, "fresh").len(), 1);

    // Deleted on the server after the copy was made: a conflict with no server copy, and it stays deleted.
    let (status, _) = alice
        .rpc("delete_character", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK);
    let late = device_copy_from(&stale, "late");
    let (_, out) = ask_push(&alice, &id, Some(&sha256_hex(&stale)), &late, false).await;
    assert_eq!(out["result"], "conflict", "{out}");
    assert_eq!(out["sha256"], Value::Null);
    assert!(!character_file(data, &alice.id, &id).exists());
    assert!(
        trashed(data, &alice.id, "late").is_empty(),
        "nothing of the device's copy is kept here"
    );
    // "Keep mine" brings it back.
    let (_, out) = ask_push(&alice, &id, Some(&sha256_hex(&stale)), &late, true).await;
    assert_eq!(out["result"], "applied", "{out}");
    assert_eq!(player(data, &alice.id, &id), "late");

    // A device without `ask` (before 0.3.6) is decided by the clocks as before: an older edit loses to the trash.
    let (_, out) = push(
        &alice,
        &id,
        Some(&base),
        1000,
        &device_copy_from(&late, "old app"),
    )
    .await;
    assert_eq!(out["result"], "kept-server", "{out}");
    assert_eq!(trashed(data, &alice.id, "old app").len(), 1);
}

fn device_copy_from(copy: &[u8], who: &str) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(copy).unwrap();
    value["meta"]["player"] = json!(who);
    serde_json::to_vec_pretty(&value).unwrap()
}

/// A desktop data folder holding `files` (paths under the data folder) before it opens.
async fn desktop_with(files: &[(&str, Value)]) -> Desktop {
    let dir = tempfile::tempdir().unwrap();
    for (rel, value) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }
    let data = dir.path().to_path_buf();
    let app = tokio::task::spawn_blocking(move || {
        crate::backend::open_workspace(data, content_root(), |_, _| {}).unwrap()
    })
    .await
    .unwrap();
    Desktop { app, dir }
}

impl Desktop {
    /// Every loaded element of `pack`, as stored after loading.
    fn elements(&self, pack: &str) -> BTreeMap<String, Value> {
        let prefix = format!("{pack}/");
        let hub = self
            .app
            .state::<borealis_lib::state::AppState>()
            .hub()
            .unwrap();
        hub.store
            .iter()
            .filter(|s| s.file.starts_with(&prefix))
            .map(|s| {
                let mut out = serde_json::to_value(&s.element).unwrap();
                out["extended"] = json!(s.extended);
                out["appendedSupports"] = json!(s.appended_supports);
                (s.element.id.clone(), out)
            })
            .collect()
    }

    /// Install every user pack of `from` here, as a device does it: merged, then upstream's `import_pack`.
    async fn copy_packs(&self, from: &Desktop) -> Vec<String> {
        let mut copied = Vec::new();
        for (key, dir) in list_packs(&from.dir.path().join("library")).unwrap() {
            let chosen = crate::sync::chosen_editions(
                &from.app.state::<borealis_lib::state::AppState>(),
                &key,
            )
            .unwrap();
            let (file_name, bytes) = crate::sync::merge_pack(&dir, &key, &chosen)
                .unwrap_or_else(|e| panic!("{key}: {e:?}"));
            let out = self
                .run(
                    "import_pack",
                    json!({ "request": { "fileName": file_name, "contentBase64": b64(&bytes) } }),
                )
                .await;
            assert_eq!(out["pack"]["id"], key.as_str(), "{out}");
            let loaded = from.elements(&key);
            assert!(!loaded.is_empty(), "{key}");
            assert_eq!(self.elements(&key), loaded, "{key}");
            eprintln!("{key}: {} elements load the same", loaded.len());
            copied.push(key);
        }
        copied
    }

    /// The character as it opens here, with its id and the data folder taken out.
    async fn sheet(&self, id: &str) -> String {
        let sheet = self
            .run("load_character", json!({ "characterId": id }))
            .await;
        serde_json::to_string_pretty(&sheet)
            .unwrap()
            .replace(id, "<id>")
            .replace(self.dir.path().to_str().unwrap(), "<data>")
    }

    /// Copy every character of `from` here as a device does it, and check each opens the same.
    async fn copy_characters(&self, from: &Desktop) -> usize {
        let characters = scan_characters(&from.dir.path().join("characters")).unwrap();
        for c in &characters {
            let app = from.app.clone();
            let key = c.key.clone();
            let got = tokio::task::spawn_blocking(move || crate::sync::character(&app, &key))
                .await
                .unwrap()
                .unwrap_or_else(|e| panic!("{e:?}"));
            let imported = self
                .run(
                    "import_character",
                    json!({ "fileName": format!("{}.json", c.key), "contentBase64": got["contentBase64"] }),
                )
                .await;
            let local = imported["summary"]["id"].as_str().unwrap();
            let (here, there) = (self.sheet(local).await, from.sheet(&c.key).await);
            let differ: Vec<String> = here
                .lines()
                .zip(there.lines())
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .take(20)
                .map(|(n, (a, b))| format!("line {n}: device {a} / server {b}"))
                .collect();
            assert!(differ.is_empty(), "{}:\n{}", c.key, differ.join("\n"));
            assert_eq!(here.lines().count(), there.lines().count(), "{}", c.key);
        }
        characters.len()
    }
}

fn element(id: &str, ty: &str, name: &str) -> Value {
    json!({ "id": id, "name": name, "type": ty, "source": "Sync Book" })
}

fn item(id: &str, name: &str) -> Value {
    let mut out = element(id, "Item", name);
    out["setters"] = json!([
        { "name": "category", "value": "Adventuring Gear" },
        { "name": "cost", "value": "1", "attributes": { "currency": "gp" } },
        { "name": "weight", "value": "1 lb.", "attributes": { "lb": "1" } },
    ]);
    out
}

fn content(ruleset: Option<&str>, elements: Vec<Value>) -> Value {
    let mut out = json!({ "format": "borealis-content/1", "elements": elements });
    if let Some(ruleset) = ruleset {
        out["ruleset"] = json!(ruleset);
    }
    out
}

fn manifest(id: &str, rulesets: &[&str], files: &[&str]) -> Value {
    json!({
        "format": "borealis-pack/1",
        "id": id,
        "name": "Sync Mixed",
        "version": "3.1.4",
        "origin": "custom",
        "rulesets": rulesets,
        "license": "CC-BY-4.0",
        "files": files.iter().map(|f| json!({ "path": f, "kind": "content" })).collect::<Vec<_>>(),
    })
}

/// S8: a pack of several files and editions, with a replaced element, an append, an element of its own edition and
/// an ignored file, merged and installed through upstream's import on a fresh device: the same elements load, and
/// a character that uses them opens the same.
#[tokio::test(flavor = "multi_thread")]
async fn a_merged_pack_loads_the_same_elements_on_a_device() {
    let mut appended = content(
        Some("dnd2014"),
        vec![element("ID_SYNC_TONGUE", "Language", "Sync Tongue")],
    );
    appended["appends"] = json!([{ "id": "ID_SYNC_BLADE", "supports": ["ID_SYNC_TAG"] }]);
    let mut ignored = content(None, vec![element("ID_SYNC_IGNORED", "Language", "Never")]);
    ignored["ignore"] = json!(true);
    let mut neutral = element("ID_SYNC_ANY", "Language", "Sync Common");
    neutral["ruleset"] = json!("any");
    let server = desktop_with(&[
        (
            "library/sync-mixed/pack.json",
            manifest(
                "sync-mixed",
                &["dnd2014", "dnd2024"],
                &["a.json", "sub/b.json", "c.json"],
            ),
        ),
        ("library/sync-mixed/a.json", appended),
        (
            "library/sync-mixed/sub/b.json",
            content(
                Some("dnd2024"),
                vec![
                    item("ID_SYNC_BLADE", "Sync Blade"),
                    neutral,
                    element("ID_SYNC_TONGUE", "Language", "Sync Tongue, revised"),
                ],
            ),
        ),
        ("library/sync-mixed/c.json", ignored),
        // One file, one edition from the manifest only.
        (
            "library/sync-plain/pack.json",
            manifest("sync-plain", &["dnd2024"], &["x.json"]),
        ),
        (
            "library/sync-plain/x.json",
            content(None, vec![item("ID_SYNC_ROPE", "Sync Rope")]),
        ),
    ])
    .await;
    let loaded = server.elements("sync-mixed");
    assert_eq!(
        loaded.keys().map(String::as_str).collect::<Vec<_>>(),
        ["ID_SYNC_ANY", "ID_SYNC_BLADE", "ID_SYNC_TONGUE"]
    );
    assert_eq!(
        loaded["ID_SYNC_TONGUE"]["ruleset"], "dnd2024",
        "the later file wins"
    );
    assert_eq!(loaded["ID_SYNC_BLADE"]["extended"], true, "{loaded:?}");

    let made = server
        .run("create_character", new_character("Packer"))
        .await;
    let id = made["id"].as_str().unwrap().to_string();
    for item in ["ID_SYNC_BLADE", "ID_SYNC_ROPE"] {
        server
            .run(
                "add_item",
                json!({ "characterId": id, "itemId": item, "quantity": 2, "buy": false }),
            )
            .await;
    }
    server
        .run("save_character", json!({ "characterId": id }))
        .await;
    assert!(server.sheet(&id).await.contains("Sync Blade"));

    let device = Desktop::new().await;
    assert_eq!(
        device.copy_packs(&server).await,
        ["sync-mixed", "sync-plain"]
    );
    assert_eq!(device.copy_characters(&server).await, 1);
}

/// An edition the account chose for a book (Sources page) applies only while content loads here; the merged pack
/// carries it, so the device loads the book's elements, its helper included, under the chosen edition too.
#[tokio::test(flavor = "multi_thread")]
async fn a_chosen_book_edition_reaches_the_device() {
    let mut helper = element("ID_SYNC_HELPER", "Internal", "Sync Helper");
    helper["source"] = json!("Internal");
    let server = desktop_with(&[
        (
            "library/sync-book/pack.json",
            manifest("sync-book", &["dnd2014"], &["x.json"]),
        ),
        (
            "library/sync-book/x.json",
            content(
                Some("dnd2014"),
                vec![
                    element("ID_SYNC_BOOK", "Source", "Sync Book"),
                    item("ID_SYNC_LANTERN", "Sync Lantern"),
                    helper,
                ],
            ),
        ),
    ])
    .await;
    let library = server.dir.path().join("library");
    let before = scan_packs(&library, &Limits::default()).unwrap()[0]
        .sha256
        .clone();
    server
        .run(
            "set_source_edition",
            json!({ "sourceId": "Sync Book", "edition": "dnd2024" }),
        )
        .await;
    let loaded = server.elements("sync-book");
    assert_eq!(
        loaded["ID_SYNC_LANTERN"]["ruleset"], "dnd2024",
        "{loaded:?}"
    );
    let backend = server.app.state::<borealis_lib::state::AppState>();
    let chosen = crate::sync::chosen_editions(&backend, "sync-book").unwrap();
    assert!(chosen.contains_key("ID_SYNC_LANTERN"), "{chosen:?}");
    // The files are unchanged, but the device must fetch the pack again: the state's digest is not the files'.
    assert_eq!(
        scan_packs(&library, &Limits::default()).unwrap()[0].sha256,
        before
    );
    let (digest, _) =
        crate::sync::pack_digest(&backend, "sync-book", &library.join("sync-book")).unwrap();
    assert_ne!(digest, before);

    let made = server
        .run("create_character", new_character("Reader"))
        .await;
    let id = made["id"].as_str().unwrap().to_string();
    server
        .run(
            "add_item",
            json!({ "characterId": id, "itemId": "ID_SYNC_LANTERN", "quantity": 1, "buy": false }),
        )
        .await;
    server
        .run("save_character", json!({ "characterId": id }))
        .await;
    assert!(server.sheet(&id).await.contains("Sync Lantern"));

    let device = Desktop::new().await;
    assert_eq!(device.copy_packs(&server).await, ["sync-book"]);
    assert_eq!(device.copy_characters(&server).await, 1);
}

#[test]
fn packs_that_cannot_be_copied_faithfully_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let write = |rel: &str, value: Value| {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
    };
    // No edition anywhere, in a pack of two.
    write(
        "loose/pack.json",
        manifest("loose", &["dnd2014", "dnd2024"], &["x.json"]),
    );
    write(
        "loose/x.json",
        content(None, vec![element("ID_LOOSE", "Language", "Loose")]),
    );
    // An id that is not the folder's.
    write(
        "other/pack.json",
        manifest("not-other", &["dnd2014"], &["x.json"]),
    );
    write("other/x.json", content(None, vec![]));
    // Core load rules.
    let mut core = manifest("core-ish", &["dnd2014"], &["x.json"]);
    core["origin"] = json!("core");
    write("core-ish/pack.json", core);
    write("core-ish/x.json", content(None, vec![]));
    // A file outside the folder.
    write(
        "escape/pack.json",
        manifest("escape", &["dnd2014"], &["../loose/x.json"]),
    );
    for key in ["loose", "other", "core-ish", "escape"] {
        match crate::sync::merge_pack(&dir.path().join(key), key, &BTreeMap::new()) {
            Err(crate::error::ApiError::Invalid(message)) => {
                assert!(
                    message.contains("cannot be copied to a device"),
                    "{message}"
                );
            }
            other => panic!("{key}: {other:?}"),
        }
    }
}

/// S8 on a real desktop bundle, when `BOREALIS_TEST_BUNDLE` names one: every pack in it, merged and installed on a
/// fresh device, loads the same elements, and every character in it opens the same.
#[tokio::test(flavor = "multi_thread")]
async fn a_real_bundle_copies_to_a_device_the_same() {
    let Ok(file) = std::env::var("BOREALIS_TEST_BUNDLE") else {
        eprintln!("skipped: set BOREALIS_TEST_BUNDLE to a .borealis file");
        return;
    };
    let bytes = std::fs::read(&file).unwrap();
    let server = Desktop::new().await;
    let staged = server
        .run(
            "stage_bundle",
            json!({ "fileName": "real.borealis", "contentBase64": b64(&bytes) }),
        )
        .await;
    let preview = server
        .run("bundle_open", json!({ "token": staged["token"] }))
        .await;
    server
        .run("bundle_accept", json!({ "bundleId": preview["bundleId"] }))
        .await;
    let device = Desktop::new().await;
    let packs = device.copy_packs(&server).await;
    let characters = device.copy_characters(&server).await;
    eprintln!("real bundle: packs {packs:?} and {characters} characters copy the same");
    assert!(characters > 0);
}
