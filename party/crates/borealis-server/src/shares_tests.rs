//! Share codes (shares.rs) over HTTP: make and redeem, the safe receive, accounts kept apart, codes for one account,
//! decline and block, cancel, limits, expiry and the clean-up, links first (re-added through net_guard), the admin's
//! counts, and the M-shares migration on a database as 0.1.4 left it, on SQLite and PostgreSQL.

use super::*;

use crate::shares::{
    CREATES_ALLOWED, FAILED_REDEEMS_ALLOWED, NO_SUCH_CODE, OPEN_PER_SENDER, WAITING_PER_SENDER,
};

/// The schema version 0.1.4 shipped with (one step: accounts, sessions, settings, activity).
const SCHEMA_014: usize = 1;

fn outgoing(data: &Path, account: &str) -> Vec<PathBuf> {
    walk(
        &data
            .join("users")
            .join(account)
            .join("sync")
            .join("outgoing"),
    )
    .into_iter()
    .filter(|p| p.is_file())
    .collect()
}

impl Client {
    /// A share of `body`; the answer as it came.
    async fn share(&self, body: Value) -> (StatusCode, Value) {
        self.post("/api/shares", body).await
    }

    /// A share of `body` that must work; its code and id.
    async fn share_ok(&self, body: Value) -> (String, String) {
        let (status, out) = self.share(body).await;
        assert_eq!(status, StatusCode::OK, "{out}");
        (
            out["code"].as_str().unwrap().to_string(),
            out["share"]["id"].as_str().unwrap().to_string(),
        )
    }

    async fn redeem(&self, code: &str) -> (StatusCode, Value) {
        self.post("/api/shares/redeem", json!({ "code": code }))
            .await
    }

    async fn character_named(&self, name: &str) -> String {
        let (status, made) = self.rpc("create_character", new_character(name)).await;
        assert_eq!(status, StatusCode::OK, "{made}");
        let id = made["id"].as_str().unwrap().to_string();
        let (status, out) = self
            .rpc("save_character", json!({ "characterId": id }))
            .await;
        assert_eq!(status, StatusCode::OK, "{out}");
        self.rpc("close_character", json!({ "characterId": id }))
            .await;
        id
    }
}

fn of_character(id: &str) -> Value {
    json!({ "characterIds": [id], "packs": "required" })
}

/// Make the account's imported pack look downloaded from `url`, as an index download writes it.
fn from_index(data: &Path, account: &str, pack: &str, url: &str) {
    let meta = data
        .join("users")
        .join(account)
        .join("library")
        .join(pack)
        .join("pack-meta.json");
    let mut written: Value =
        serde_json::from_str(&std::fs::read_to_string(&meta).unwrap()).unwrap();
    written["indexUrl"] = json!(url);
    std::fs::write(&meta, written.to_string()).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_share_code_brings_a_character_to_another_account() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    let router = router_on(dir.path().to_path_buf(), db.clone());
    let admin = sign_up(&router, "share-admin").await;
    share_flow(router, dir.path(), &db, admin).await;
}

/// Make, redeem, accept; codes for one account; decline, block and cancel; accounts kept apart; admin counts.
async fn share_flow(router: Router, data: &Path, db: &Db, admin: Client) {
    let alice = sign_up(&router, "share-alice").await;
    let bob = sign_up(&router, "share-bob").await;
    let carol = sign_up(&router, "share-carol").await;
    let hero = alice.character_named("Shared Hero").await;

    // Nothing chosen is refused.
    let (status, out) = alice
        .share(json!({ "characterIds": [], "packs": "none" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");

    let (status, made) = alice.share(of_character(&hero)).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let code = made["code"].as_str().unwrap().to_string();
    let id = made["share"]["id"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 19, "{code}");
    assert_eq!(made["share"]["characters"], 1, "{made}");
    assert_eq!(made["share"]["status"], "pending");
    assert_eq!(made["link"], Value::Null, "no public address set, no link");
    assert_eq!(outgoing(data, &alice.id).len(), 1);
    // Only the code's hash is stored.
    let stored: Vec<String> = sqlx::query_scalar::<_, String>("SELECT code_hash FROM shares")
        .fetch_all(db.pool())
        .await
        .unwrap();
    assert!(
        stored.iter().all(|h| !h.contains(&code.replace('-', ""))),
        "{stored:?}"
    );

    // Wrong codes all get the same answer; the owner can't redeem its own.
    for wrong in ["", "nope", "0000-0000-0000-0000"] {
        let (status, out) = bob.redeem(wrong).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
        assert_eq!(out["message"], NO_SUCH_CODE);
    }
    let (status, out) = alice.redeem(&code).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");

    // Bob redeems (any case, spaces): the usual preview, as another player's bundle.
    let (status, got) = bob.redeem(&format!(" {} ", code.to_lowercase())).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["share"]["from"], "share-alice", "{got}");
    assert_eq!(got["share"]["status"], "claimed");
    let token = got["bundle"]["token"].clone();
    let (status, preview) = bob.rpc("bundle_open", json!({ "token": token })).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(
        preview["sourceDevice"], "another Borealis Party account",
        "{preview}"
    );
    assert_eq!(actions(&preview), one("character", "new"), "{preview}");

    // Nobody else can take it now, nor see it.
    let (status, out) = carol.redeem(&code).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
    let (_, theirs) = carol.get("/api/shares").await;
    assert_eq!(theirs["sent"], json!([]), "{theirs}");
    assert_eq!(theirs["received"], json!([]), "{theirs}");
    for path in [
        format!("/api/shares/{id}/open"),
        format!("/api/shares/{id}/accepted"),
        format!("/api/shares/{id}/links"),
    ] {
        let (status, out) = carol.post(&path, json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {out}");
    }
    let (status, out) = carol
        .post(&format!("/api/shares/{id}/decline"), json!({}))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
    let (status, out) = bob.delete(&format!("/api/shares/{id}")).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "only the sender cancels: {out}"
    );

    // Bob's list has it, and reopening it stages it again.
    let (_, list) = bob.get("/api/shares").await;
    assert_eq!(list["received"][0]["id"], id.as_str(), "{list}");
    let (status, again) = bob.post(&format!("/api/shares/{id}/open"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    let (status, preview) = bob
        .rpc("bundle_open", json!({ "token": again["bundle"]["token"] }))
        .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let summary = bob.accept_bundle(&preview).await;
    assert_eq!(summary["added"], 1, "{summary}");
    let (status, out) = bob
        .post(&format!("/api/shares/{id}/accepted"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert!(
        outgoing(data, &alice.id).is_empty(),
        "the file goes once accepted"
    );
    let (_, list) = bob.rpc("list_characters", json!({})).await;
    assert_eq!(names(&list), ["Shared Hero"], "{list}");
    let (_, sent) = alice.get("/api/shares").await;
    assert_eq!(sent["sent"][0]["status"], "done", "{sent}");
    assert_eq!(sent["sent"][0]["claimedBy"], "share-bob", "{sent}");
    // Used: a second redeem fails like any wrong code.
    let (status, _) = bob.redeem(&code).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A code for one account: others get the wrong-code answer and don't use it up. A name that doesn't exist is
    // accepted all the same, so the answer never tells whether it does.
    let (code, _) = alice
        .share_ok(
            json!({ "characterIds": [hero], "packs": "none", "forUsername": " Share-Carol " }),
        )
        .await;
    let (status, out) = bob.redeem(&code).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
    let (status, out) = carol.redeem(&code).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    alice
        .share_ok(json!({ "characterIds": [hero], "packs": "none", "forUsername": "nobody-has-this-name" }))
        .await;
    let (status, out) = alice
        .share(json!({ "characterIds": [hero], "packs": "none", "forUsername": "share-alice" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");

    // Decline and block: nothing comes in, and later codes from that sender are refused until unblocked.
    let (code, id) = alice.share_ok(of_character(&hero)).await;
    let (status, out) = bob.redeem(&code).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = bob
        .post(
            &format!("/api/shares/{id}/decline"),
            json!({ "block": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, sent) = alice.get("/api/shares").await;
    let row = sent["sent"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id.as_str())
        .unwrap()
        .clone();
    assert_eq!(row["status"], "declined", "{sent}");
    let (code, _) = alice.share_ok(of_character(&hero)).await;
    let (status, out) = bob.redeem(&code).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");
    let (_, list) = bob.get("/api/shares").await;
    assert_eq!(list["blocked"][0]["username"], "share-alice", "{list}");
    let (status, out) = bob
        .delete(&format!("/api/shares/blocks/{}", alice.id))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = bob.redeem(&code).await;
    assert_eq!(status, StatusCode::OK, "{out}");

    // Cancelled by the sender: the code stops working and its file goes.
    let before = outgoing(data, &alice.id).len();
    let (code, id) = alice.share_ok(of_character(&hero)).await;
    assert_eq!(outgoing(data, &alice.id).len(), before + 1);
    let (status, out) = alice.delete(&format!("/api/shares/{id}")).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(outgoing(data, &alice.id).len(), before);
    let (status, _) = carol.redeem(&code).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The admin sees counts and bytes, never codes, names or what was shared.
    let (status, stats) = admin.get("/api/admin/stats").await;
    assert_eq!(status, StatusCode::OK, "{stats}");
    let shares = &stats["shares"];
    for status in ["done", "declined", "cancelled", "claimed"] {
        assert!(
            shares["byStatus"][status].as_i64().unwrap() >= 1,
            "{status}: {stats}"
        );
    }
    assert!(shares["outgoingBytes"].as_u64().unwrap() > 0, "{stats}");
    assert!(shares["stagedBytes"].as_u64().unwrap() > 0, "{stats}");
    let text = shares.to_string();
    for secret in [
        "Shared Hero",
        "share-alice",
        "share-bob",
        "fileName",
        "code",
    ] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    // Not for players.
    let (status, _) = alice.get("/api/admin/stats").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Signed out: nothing.
    let (status, _) = send(&router, None, "GET", "/api/shares", Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn share_codes_are_limited() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let alice = sign_up(&router, "limit-alice").await;
    let bob = sign_up(&router, "limit-bob").await;
    let hero = alice.character_named("Limited").await;
    let body = json!({ "characterIds": [hero], "packs": "none" });

    // 20 open codes per sender.
    let mut codes = Vec::new();
    for _ in 0..OPEN_PER_SENDER {
        codes.push(alice.share_ok(body.clone()).await);
    }
    let (status, out) = alice.share(body.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{out}");

    // 8 redeemed and waiting from one sender.
    for (code, _) in codes
        .iter()
        .take(usize::try_from(WAITING_PER_SENDER).unwrap())
    {
        let (status, out) = bob.redeem(code).await;
        assert_eq!(status, StatusCode::OK, "{out}");
    }
    let next = &codes[usize::try_from(WAITING_PER_SENDER).unwrap()].0;
    let (status, out) = bob.redeem(next).await;
    assert_eq!(status, StatusCode::CONFLICT, "{out}");
    // Declining one frees a slot.
    let (_, first) = &codes[0];
    let (status, out) = bob
        .post(&format!("/api/shares/{first}/decline"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = bob.redeem(next).await;
    assert_eq!(status, StatusCode::OK, "{out}");

    // Making codes is rate-limited, cancelled ones included (21 tries so far, the refused one counted).
    for (_, id) in &codes[10..] {
        alice.delete(&format!("/api/shares/{id}")).await;
    }
    let mut made = OPEN_PER_SENDER + 1;
    let limited = loop {
        let (status, _) = alice.share(body.clone()).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            break true;
        }
        made += 1;
        if made > i64::try_from(CREATES_ALLOWED).unwrap() + 1 {
            break false;
        }
    };
    assert!(limited, "made {made} codes without a limit");

    // Wrong codes are rate-limited per account: after that even a right one waits.
    let carol = sign_up(&router, "limit-carol").await;
    let dave = sign_up(&router, "limit-dave").await;
    let hero = carol.character_named("Patient").await;
    let (good, _) = carol
        .share_ok(json!({ "characterIds": [hero], "packs": "none" }))
        .await;
    for _ in 0..FAILED_REDEEMS_ALLOWED {
        let (status, _) = dave.redeem("0000-0000-0000-0000").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, out) = dave.redeem(&good).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_shares_and_their_files_are_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let db = Db::open(&Location::Memory).await.unwrap();
    let router = router_on(data.to_path_buf(), db.clone());
    let alice = sign_up(&router, "expiry-alice").await;
    let bob = sign_up(&router, "expiry-bob").await;
    let hero = alice.character_named("Brief").await;
    let (code, id) = alice.share_ok(of_character(&hero)).await;
    let (kept, _) = alice.share_ok(of_character(&hero)).await;
    assert_eq!(outgoing(data, &alice.id).len(), 2);
    // A file no share names (left by a crash) goes too; an old half-written one as well.
    let stray = data
        .join("users")
        .join(&alice.id)
        .join("sync")
        .join("outgoing")
        .join("share-0123.borealis");
    std::fs::write(&stray, b"x").unwrap();

    sqlx::query("UPDATE shares SET expires_at = $1 WHERE id = $2")
        .bind(crate::db::now() - 1)
        .bind(&id)
        .execute(db.pool())
        .await
        .unwrap();
    // Past its end it is refused at once, before any clean-up.
    let (status, out) = bob.redeem(&code).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
    let (_, list) = alice.get("/api/shares").await;
    let row = list["sent"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id.as_str())
        .unwrap()
        .clone();
    assert_eq!(row["status"], "expired", "{list}");

    let workspaces = Arc::new(Workspaces::new(data.to_path_buf(), content_root()));
    let removed = crate::shares::sweep(&db, &workspaces).await.unwrap();
    assert_eq!(removed, 2, "the expired share's file and the stray one");
    assert_eq!(outgoing(data, &alice.id).len(), 1);
    let status: String = sqlx::query_scalar("SELECT status FROM shares WHERE id = $1")
        .bind(&id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(status, "expired");
    // The other one still works.
    let (status, out) = bob.redeem(&kept).await;
    assert_eq!(status, StatusCode::OK, "{out}");

    // Ended long ago: the row goes.
    sqlx::query("UPDATE shares SET ended_at = $1 WHERE id = $2")
        .bind(crate::db::now() - 31 * 24 * 60 * 60)
        .bind(&id)
        .execute(db.pool())
        .await
        .unwrap();
    crate::shares::sweep(&db, &workspaces).await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shares WHERE id = $1")
        .bind(&id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn index_packs_travel_as_links_and_are_added_through_the_guard() {
    const INDEX: &str = "https://content.example/homebrew.index";
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let db = Db::open(&Location::Memory).await.unwrap();
    let router = router_on(data.to_path_buf(), db.clone());
    let alice = sign_up(&router, "links-alice").await;
    let bob = sign_up(&router, "links-bob").await;
    let (status, imported) = alice.rpc("import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    let pack = imported["pack"]["id"].as_str().unwrap().to_string();
    let all = json!({ "characterIds": [], "packs": "all" });

    // A homebrew pack travels as a file.
    let (status, made) = alice.share(all.clone()).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(made["share"]["packs"], 1, "{made}");
    assert_eq!(made["share"]["links"], json!([]), "{made}");

    // From an index: as its address, with no file at all.
    from_index(data, &alice.id, &pack, INDEX);
    let (status, made) = alice.share(all.clone()).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(made["share"]["packs"], 0, "{made}");
    assert_eq!(made["share"]["bytes"], 0, "{made}");
    assert_eq!(made["share"]["links"][0]["kind"], "index", "{made}");
    assert_eq!(made["share"]["links"][0]["url"], INDEX, "{made}");
    let id = made["share"]["id"].as_str().unwrap().to_string();
    let (status, got) = bob.redeem(made["code"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["bundle"], Value::Null, "nothing to preview: {got}");
    let (status, added) = bob
        .post(&format!("/api/shares/{id}/links"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(added["indexes"][0]["added"], true, "{added}");
    let (_, list) = bob.rpc("list_indexes", json!({})).await;
    assert_eq!(index_urls(&list), [INDEX], "{list}");
    // Again: already there.
    let (_, added) = bob
        .post(&format!("/api/shares/{id}/links"), json!({}))
        .await;
    assert_eq!(added["indexes"][0]["already"], true, "{added}");

    // "Include the files": the pack as a file, no link.
    let (status, made) = alice
        .share(json!({ "characterIds": [], "packs": "all", "includeFiles": true }))
        .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(made["share"]["packs"], 1, "{made}");
    assert_eq!(made["share"]["links"], json!([]), "{made}");

    // An address that can't travel (a token in it, or the server's own network) is never sent: the pack is left out.
    for url in [
        "https://content.example/homebrew.index?token=secret",
        "https://lan.example.invalid/homebrew.index",
    ] {
        from_index(data, &alice.id, &pack, url);
        let (status, out) = alice.share(all.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url}: {out}");
        assert!(
            out["message"]
                .as_str()
                .unwrap()
                .contains("Include the files"),
            "{out}"
        );
        assert!(!out.to_string().contains("secret"), "{out}");
    }

    // A link that reaches the recipient anyway (an older share, a changed address) is checked again when added.
    let carol = sign_up(&router, "links-carol").await;
    sqlx::query(
        "INSERT INTO shares (id, from_id, code_hash, to_id, to_name, file_name, characters, packs, links, bytes,
            created_at, expires_at, claimed_at, ended_at, status)
         VALUES ('crafted', $1, 'crafted', $2, NULL, 'x.borealis', 0, 0, $3, 0, $4, $5, $4, NULL, 'claimed')",
    )
    .bind(&alice.id)
    .bind(&carol.id)
    .bind(json!([{ "kind": "index", "url": "https://lan.example.invalid/x.index", "packs": ["X"] },
                 { "kind": "5etools", "url": "https://127.0.0.1/data", "packs": ["Y"] }]).to_string())
    .bind(crate::db::now())
    .bind(crate::db::now() + 3600)
    .execute(db.pool())
    .await
    .unwrap();
    let (status, added) = carol.post("/api/shares/crafted/links", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{added}");
    assert_eq!(added["indexes"][0]["added"], false, "{added}");
    assert_eq!(added["fivetools"][0]["allowed"], false, "{added}");
    let (_, list) = carol.rpc("list_indexes", json!({})).await;
    assert!(index_urls(&list).is_empty(), "{list}");
}

// The migration.

/// What 0.1.4 holds: accounts in every state, sessions, settings and the activity log.
async fn seed_014(router: &Router) {
    some_accounts(router).await;
    // A pending account (approval is on now) and a disabled one.
    let (status, out) = register(router, "moving-pending", "moving-pending@example.com").await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["status"], "pending", "{out}");
    let admin = sign_in(router, "moving-admin").await;
    let (_, list) = admin.get("/api/admin/accounts?search=moving-other").await;
    let other = list["items"][0]["id"].as_str().unwrap().to_string();
    let (status, out) = admin
        .post(
            &format!("/api/admin/accounts/{other}/status"),
            json!({ "status": "disabled" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

/// Opened by this version, a 0.1.4 database keeps everything and gets share codes.
async fn check_upgraded(router: &Router, db: &Db) {
    let version: i64 = sqlx::query_scalar("SELECT MAX(version) FROM schema_version")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(version, i64::try_from(crate::db::SCHEMA_VERSION).unwrap());
    check_moved(router).await;
    let (status, out) = login(router, "moving-pending", PASSWORD).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "still pending: {out}");
    let (status, out) = login(router, "moving-other", PASSWORD).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "still disabled: {out}");
    let admin = sign_in(router, "moving-admin").await;
    let player = sign_in(router, "moving-player").await;
    let (_, list) = admin.rpc("list_characters", json!({})).await;
    let moved = list[0]["id"].as_str().unwrap().to_string();
    let (code, id) = admin.share_ok(of_character(&moved)).await;
    let (status, got) = player.redeem(&code).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let (_, preview) = player
        .rpc("bundle_open", json!({ "token": got["bundle"]["token"] }))
        .await;
    assert_eq!(player.accept_bundle(&preview).await["added"], 1);
    let (status, _) = player
        .post(&format!("/api/shares/{id}/accepted"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = player.rpc("list_characters", json!({})).await;
    assert_eq!(names(&list), ["Moved"], "{list}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_014_sqlite_database_upgrades_with_its_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("party.db");
    let data = dir.path().join("data");
    let location = Location::Sqlite(file);
    {
        let old = Db::open_at(&location, SCHEMA_014).await.unwrap();
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'share%'",
        )
        .fetch_all(old.pool())
        .await
        .unwrap();
        assert!(tables.is_empty(), "0.1.4 had no share tables: {tables:?}");
        seed_014(&router_on(data.clone(), old.clone())).await;
        old.pool().close().await;
    }
    let db = Db::open(&location).await.unwrap();
    check_upgraded(&router_on(data.clone(), db.clone()), &db).await;
    // Opening again changes nothing.
    let again = Db::open(&location).await.unwrap();
    let steps: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM schema_version")
        .fetch_one(again.pool())
        .await
        .unwrap();
    assert_eq!(steps, i64::try_from(crate::db::SCHEMA_VERSION).unwrap());
    schema_matches_the_copy_list(&again).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_014_postgres_database_upgrades_and_shares_work() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    sqlx::any::install_default_drivers();
    let server = sqlx::AnyPool::connect(&url).await.unwrap();
    let name = format!("bp_shares_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&server)
        .await
        .unwrap();
    let mut own = url::Url::parse(&url).unwrap();
    own.set_path(&format!("/{name}"));
    let location = Location::Postgres(own.to_string());
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    {
        let old = Db::open_at(&location, SCHEMA_014).await.unwrap();
        seed_014(&router_on(data.clone(), old.clone())).await;
        old.pool().close().await;
    }
    let db = Db::open(&location).await.unwrap();
    check_upgraded(&router_on(data.clone(), db.clone()), &db).await;
    schema_matches_the_copy_list(&db).await;
    // The whole flow on PostgreSQL too, with the upgraded database's admin and sign-up open again.
    let fresh = data.with_file_name("fresh");
    let router = router_on(fresh.clone(), db.clone());
    let admin = sign_in(&router, "moving-admin").await;
    let (status, out) = admin
        .patch("/api/admin/settings", json!({ "requireApproval": false }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    share_flow(router, &fresh, &db, admin).await;
    db.pool().close().await;
    sqlx::query(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .execute(&server)
        .await
        .unwrap();
}

/// Every table the migrations make is in migrate.rs's copy list with every column, numbers that are never NULL as
/// such, so a move from SQLite to PostgreSQL loses nothing.
async fn schema_matches_the_copy_list(db: &Db) {
    let listed = crate::migrate::table_columns();
    let sqlite = db.kind() == "sqlite";
    let tables: Vec<String> = sqlx::query_scalar(if sqlite {
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'"
    } else {
        "SELECT CAST(table_name AS TEXT) FROM information_schema.tables WHERE table_schema = current_schema()"
    })
    .fetch_all(db.pool())
    .await
    .unwrap();
    let mut tables: Vec<String> = tables
        .into_iter()
        .filter(|t| t != "schema_version" && t != "party_groups")
        .collect();
    tables.sort();
    let mut names: Vec<String> = listed.iter().map(|(t, _)| (*t).to_string()).collect();
    names.sort();
    assert_eq!(tables, names);
    for (table, columns) in &listed {
        let rows: Vec<(String, i64)> = if sqlite {
            sqlx::query(&format!(
                "SELECT name, \"notnull\" FROM pragma_table_info('{table}')"
            ))
            .fetch_all(db.pool())
            .await
            .unwrap()
            .iter()
            .map(|r| (sqlx::Row::get(r, 0), sqlx::Row::get(r, 1)))
            .collect()
        } else {
            sqlx::query(
                "SELECT CAST(column_name AS TEXT), CASE WHEN is_nullable = 'NO' THEN 1 ELSE 0 END FROM information_schema.columns
                 WHERE table_schema = current_schema() AND table_name = $1",
            )
            .bind(*table)
            .fetch_all(db.pool())
            .await
            .unwrap()
            .iter()
            .map(|r| (sqlx::Row::get(r, 0), sqlx::Row::get::<i32, _>(r, 1).into()))
            .collect()
        };
        let mut got: Vec<&str> = rows.iter().map(|(n, _)| n.as_str()).collect();
        got.sort_unstable();
        let mut want: Vec<&str> = columns.iter().map(|(n, _)| *n).collect();
        want.sort_unstable();
        assert_eq!(got, want, "{table}");
        for (column, int) in columns {
            if *int {
                let not_null = rows.iter().find(|(n, _)| n == column).unwrap().1;
                assert_eq!(not_null, 1, "{table}.{column} is listed as never NULL");
            }
        }
    }
}

#[test]
fn the_schema_version_file_matches_the_migrations() {
    let text = include_str!("../tests/schema_version.txt");
    assert_eq!(
        text.trim().parse::<usize>().unwrap(),
        crate::db::SCHEMA_VERSION,
        "tests/schema_version.txt must be the number of steps in db.rs MIGRATIONS (update both in the merging PR)"
    );
}

/// The versions recorded in `schema_version`, in order.
/// Every step this version has, as `schema_version` records them: 1 to [`Db::schema_version`].
fn every_step() -> Vec<i64> {
    (1..=i64::try_from(Db::schema_version()).unwrap()).collect()
}

async fn versions(db: &Db) -> Vec<i64> {
    sqlx::query_scalar("SELECT version FROM schema_version ORDER BY version")
        .fetch_all(db.pool())
        .await
        .unwrap()
}

/// A database the removed groups versions left: their steps 1 to 3 recorded (accounts, then groups tables), with an
/// account and groups rows, and none of this version's later tables.
async fn make_groups_era(location: &Location) {
    let old = Db::open_at(location, SCHEMA_014).await.unwrap();
    for sql in [
        "INSERT INTO users (id, username, password_hash, created_at, status, is_admin) VALUES ('old-1', 'old-player', 'x', 1, 'active', 0)",
        "CREATE TABLE party_groups (id TEXT PRIMARY KEY, name TEXT NOT NULL)",
        "CREATE TABLE invites (id TEXT PRIMARY KEY, group_id TEXT NOT NULL)",
        "CREATE TABLE members (group_id TEXT NOT NULL, user_id TEXT NOT NULL)",
        "CREATE TABLE user_settings (user_id TEXT PRIMARY KEY, value TEXT NOT NULL)",
        "INSERT INTO party_groups (id, name) VALUES ('g1', 'Old group')",
        "INSERT INTO invites (id, group_id) VALUES ('i1', 'g1')",
        "INSERT INTO members (group_id, user_id) VALUES ('g1', 'old-1')",
        "INSERT INTO user_settings (user_id, value) VALUES ('old-1', 'dark')",
        "INSERT INTO schema_version (version) VALUES (2)",
        "INSERT INTO schema_version (version) VALUES (3)",
    ] {
        sqlx::query(sql).execute(old.pool()).await.unwrap();
    }
    old.pool().close().await;
}

/// Opened by this version, a groups-era database at version 3 still gets the share tables, and keeps all it had.
async fn check_groups_era(location: &Location) {
    let db = Db::open(location).await.unwrap();
    schema_has_shares(&db).await;
    assert_eq!(
        versions(&db).await,
        every_step(),
        "no step recorded twice, none lost"
    );
    for (table, rows) in [
        ("users", 1),
        ("party_groups", 1),
        ("invites", 1),
        ("members", 1),
        ("user_settings", 1),
    ] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(n, rows, "{table} untouched");
    }
    db.pool().close().await;
    // Opening again finds everything in place and changes nothing.
    let again = Db::open(location).await.unwrap();
    assert_eq!(versions(&again).await, every_step());
    again.pool().close().await;
}

/// Every table steps 2 (M-shares) and 3 (M-parties) make is there, and empty.
async fn schema_has_shares(db: &Db) {
    for table in [
        "shares",
        "share_blocks",
        "account_roles",
        "parties",
        "party_invites",
        "party_invite_holds",
        "party_offers",
        "party_members",
    ] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await
            .unwrap_or_else(|e| panic!("{table}: {e}"));
        assert_eq!(n, 0, "{table}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_database_gets_the_share_tables() {
    // Fresh.
    let fresh = Db::open(&Location::Memory).await.unwrap();
    schema_has_shares(&fresh).await;
    assert_eq!(versions(&fresh).await, every_step());
    // A groups-era file at version 3 without them.
    let dir = tempfile::tempdir().unwrap();
    let location = Location::Sqlite(dir.path().join("groups.db"));
    make_groups_era(&location).await;
    check_groups_era(&location).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_groups_era_postgres_database_gets_the_share_tables() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    sqlx::any::install_default_drivers();
    let server = sqlx::AnyPool::connect(&url).await.unwrap();
    for era in ["fresh", "groups"] {
        let name = format!("bp_shares_{era}_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&server)
            .await
            .unwrap();
        let mut own = url::Url::parse(&url).unwrap();
        own.set_path(&format!("/{name}"));
        let location = Location::Postgres(own.to_string());
        if era == "fresh" {
            let db = Db::open(&location).await.unwrap();
            schema_has_shares(&db).await;
            assert_eq!(versions(&db).await, every_step());
            db.pool().close().await;
        } else {
            make_groups_era(&location).await;
            check_groups_era(&location).await;
        }
        sqlx::query(&format!("DROP DATABASE {name} WITH (FORCE)"))
            .execute(&server)
            .await
            .unwrap();
    }
}
