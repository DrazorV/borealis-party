//! The access rules, end to end through the router and the real Borealis backend.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::auth::LoginLimiter;
use crate::db::{Db, Location};
use crate::workspaces::Workspaces;
use crate::{api_routes, AppState, Config};

struct Client {
    router: Router,
    token: String,
    id: String,
}

async fn router(data: PathBuf) -> Router {
    router_on(data, Db::open(&Location::Memory).await.unwrap())
}

fn router_on(data: PathBuf, db: Db) -> Router {
    let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../content");
    let state = AppState {
        db,
        workspaces: Arc::new(Workspaces::new(data, content)),
        config: Arc::new(Config {
            open_registration: true,
        }),
        login_limiter: LoginLimiter::default(),
        membership: Arc::new(tokio::sync::Mutex::new(())),
    };
    api_routes(state, 64 * 1024 * 1024).layer(MockConnectInfo(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        9,
    ))))
}

async fn send(
    router: &Router,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn sign_up(router: &Router, name: &str, role: &str) -> Client {
    let (status, out) = send(
        router,
        None,
        "POST",
        "/api/auth/register",
        json!({ "username": name, "password": "correct horse battery", "role": role }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    Client {
        router: router.clone(),
        token: out["token"].as_str().unwrap().into(),
        id: out["user"]["id"].as_str().unwrap().into(),
    }
}

impl Client {
    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        send(&self.router, Some(&self.token), "POST", path, body).await
    }
    async fn get(&self, path: &str) -> (StatusCode, Value) {
        send(&self.router, Some(&self.token), "GET", path, Value::Null).await
    }
    async fn delete(&self, path: &str) -> (StatusCode, Value) {
        send(&self.router, Some(&self.token), "DELETE", path, Value::Null).await
    }
    async fn rpc(&self, scope: &str, command: &str, args: Value) -> (StatusCode, Value) {
        self.post(&format!("/api/rpc/{scope}/{command}"), args)
            .await
    }
}

fn new_character(name: &str) -> Value {
    json!({ "request": { "name": name, "ruleset": "dnd2024", "abilityMethod": "point_buy" } })
}

fn spell_pack() -> Value {
    use base64::Engine as _;
    let xml = r#"<?xml version="1.0" encoding="utf-8"?><elements>
      <info><update version="0.0.1"><file name="party.xml" url="https://example.invalid/party.xml"/></update></info>
      <element name="Party Spark" type="Spell" source="Party Book" id="ID_PARTY_SPARK">
        <supports>Wizard</supports>
        <setters><set name="level">0</set><set name="school">Evocation</set><set name="time">1 action</set><set name="duration">Instantaneous</set><set name="range">Self</set></setters>
      </element></elements>"#;
    json!({ "request": {
        "fileName": "party.xml",
        "contentBase64": base64::engine::general_purpose::STANDARD.encode(xml),
    }})
}

/// Other workspaces reload in the background.
async fn eventually(client: &Client, scope: &str, id: &str, want: StatusCode) {
    let mut last = (StatusCode::IM_A_TEAPOT, Value::Null);
    for _ in 0..100 {
        last = client
            .rpc(scope, "get_element", json!({ "elementId": id }))
            .await;
        if last.0 == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{id} in {scope}: wanted {want}, last {last:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn accounts_groups_and_characters_follow_the_rules() {
    let dir = tempfile::tempdir().unwrap();
    full_flow(router(dir.path().to_path_buf()).await).await;
}

async fn some_accounts(router: &Router) {
    let dm = sign_up(router, "moving-dm", "dm").await;
    let player = sign_up(router, "moving-player", "player").await;
    let (_, group) = dm
        .post("/api/groups", json!({ "name": "Moving day" }))
        .await;
    let group = group["id"].as_str().unwrap().to_string();
    let (_, invite) = dm
        .post(
            &format!("/api/groups/{group}/invites"),
            json!({ "username": "moving-player" }),
        )
        .await;
    player
        .post(
            &format!("/api/invites/{}/accept", invite["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    let (status, out) = dm
        .rpc(
            "library",
            "save_settings",
            json!({ "patch": { "accentColor": "green" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let other = sign_up(router, "invited-only", "player").await;
    dm.post(
        &format!("/api/groups/{group}/invites"),
        json!({ "username": "invited-only" }),
    )
    .await;
    drop(other);
}

/// After a copy the new database answers exactly like the old one.
async fn check_moved(router: &Router) {
    let (status, out) = send(
        router,
        None,
        "POST",
        "/api/auth/login",
        json!({ "username": "MOVING-DM", "password": "correct horse battery" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let token = out["token"].as_str().unwrap();
    let (_, groups) = send(router, Some(token), "GET", "/api/groups", Value::Null).await;
    assert_eq!(groups[0]["name"], "Moving day", "{groups}");
    assert_eq!(groups[0]["memberCount"], 1);
    let (_, detail) = send(
        router,
        Some(token),
        "GET",
        &format!("/api/groups/{}", groups[0]["id"].as_str().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(detail["invites"][0]["username"], "invited-only", "{detail}");
    let (_, settings) = send(
        router,
        Some(token),
        "POST",
        "/api/rpc/library/get_settings",
        json!({}),
    )
    .await;
    assert_eq!(
        settings["accentColor"], "green",
        "account settings came too"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sqlite_instance_moves_with_its_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("party.db");
    let old = Db::open(&Location::Sqlite(file.clone())).await.unwrap();
    some_accounts(&router_on(dir.path().join("data"), old.clone())).await;
    let new = Db::open(&Location::Memory).await.unwrap();
    let report = crate::migrate::adopt_sqlite(&new, &file)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::migrate::describe(&report),
        "users 3, sessions 3, party_groups 1, invites 1, members 1, user_settings 1"
    );
    check_moved(&router_on(dir.path().join("data"), new.clone())).await;
    // Once the new database has accounts, the old file is left alone.
    assert!(crate::migrate::adopt_sqlite(&new, &file)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        crate::migrate::copy(&old, &new).await,
        Err(crate::migrate::MigrateError::TargetNotEmpty)
    ));
    // A missing file is nothing to do.
    let empty = Db::open(&Location::Memory).await.unwrap();
    assert!(
        crate::migrate::adopt_sqlite(&empty, &dir.path().join("none.db"))
            .await
            .unwrap()
            .is_none()
    );
}

/// The same rules on PostgreSQL. Runs when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.
#[tokio::test(flavor = "multi_thread")]
async fn accounts_groups_and_characters_follow_the_rules_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    {
        sqlx::any::install_default_drivers();
        let pool = sqlx::AnyPool::connect(&url).await.unwrap();
        for table in [
            "members",
            "invites",
            "party_groups",
            "sessions",
            "users",
            "schema_version",
        ] {
            sqlx::query(&format!("DROP TABLE IF EXISTS {table} CASCADE"))
                .execute(&pool)
                .await
                .unwrap();
        }
    }
    let db = Db::open(&Location::Postgres(url.clone())).await.unwrap();
    // Opening again finds the schema already there.
    Db::open(&Location::Postgres(url)).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    // First a move from SQLite, as when an instance switches databases.
    let file = dir.path().join("party.db");
    let old = Db::open(&Location::Sqlite(file.clone())).await.unwrap();
    some_accounts(&router_on(dir.path().join("old"), old)).await;
    let report = crate::migrate::adopt_sqlite(&db, &file)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::migrate::describe(&report),
        "users 3, sessions 3, party_groups 1, invites 1, members 1, user_settings 1"
    );
    check_moved(&router_on(dir.path().join("old"), db.clone())).await;
    assert!(crate::migrate::adopt_sqlite(&db, &file)
        .await
        .unwrap()
        .is_none());
    full_flow(router_on(dir.path().to_path_buf(), db)).await;
}

async fn full_flow(router: Router) {
    let dm = sign_up(&router, "dungeon-master", "dm").await;
    let other_dm = sign_up(&router, "other-dm", "dm").await;
    let alice = sign_up(&router, "alice", "player").await;
    let bob = sign_up(&router, "bob", "player").await;

    // Roles decide who runs groups.
    let (status, _) = alice.post("/api/groups", json!({ "name": "Nope" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, group) = dm.post("/api/groups", json!({ "name": "Tuesday" })).await;
    assert_eq!(status, StatusCode::OK);
    let group_id = group["id"].as_str().unwrap().to_string();
    let scope = format!("group-{group_id}");
    let invites = format!("/api/groups/{group_id}/invites");

    // Invitations.
    let (status, _) = dm.post(&invites, json!({ "username": "other-dm" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "only players join groups");
    let (status, _) = other_dm
        .post(&invites, json!({ "username": "alice" }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "not their group");
    let (status, invite) = dm.post(&invites, json!({ "username": "alice" })).await;
    assert_eq!(status, StatusCode::OK);
    let accept = format!("/api/invites/{}/accept", invite["id"].as_str().unwrap());
    let (_, pending) = alice.get("/api/invites").await;
    assert_eq!(pending.as_array().unwrap().len(), 1);
    let (status, _) = bob.post(&accept, json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "someone else's invitation");
    let (status, _) = alice.post(&accept, json!({})).await;
    assert_eq!(status, StatusCode::OK);

    // A player owns many characters, one per group.
    let (status, solo) = alice
        .rpc("personal", "create_character", new_character("Solo"))
        .await;
    assert_eq!(status, StatusCode::OK, "{solo}");
    assert!(
        solo.get("displayPath").is_none(),
        "server paths stay private"
    );
    alice
        .rpc(
            "personal",
            "save_character",
            json!({ "characterId": solo["id"] }),
        )
        .await;
    let (status, hero) = alice
        .rpc(&scope, "create_character", new_character("Hero"))
        .await;
    assert_eq!(status, StatusCode::OK, "{hero}");
    alice
        .rpc(
            &scope,
            "save_character",
            json!({ "characterId": hero["id"] }),
        )
        .await;
    let (status, _) = alice
        .rpc(&scope, "create_character", new_character("Second"))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let character = format!("/api/groups/{group_id}/character");
    let (status, _) = alice
        .post(&character, json!({ "characterId": solo["id"] }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "the group's slot is taken");

    // The DM sees the party and cannot change it.
    let (status, party) = dm.get(&format!("/api/groups/{group_id}/party")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(party[0]["name"], "Hero");
    assert!(party[0]["stats"]["maxHp"].is_number(), "{party}");
    let view = format!("/api/groups/{group_id}/members/{}/view", alice.id);
    let (status, sheet) = dm.post(&format!("{view}/get_snapshot"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sheet["name"], "Hero");
    let (status, _) = dm
        .post(&format!("{view}/set_xp"), json!({ "xp": 900 }))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = dm
        .rpc(
            &scope,
            "set_xp",
            json!({ "characterId": hero["id"], "xp": 900 }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = dm
        .rpc(
            &scope,
            "delete_character",
            json!({ "characterId": hero["id"] }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = dm
        .rpc(&scope, "create_character", new_character("Theirs"))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Strangers see nothing.
    for (who, what) in [(&bob, "player"), (&other_dm, "dm")] {
        let (status, _) = who.rpc(&scope, "list_characters", json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{what}");
        let (status, _) = who.get(&format!("/api/groups/{group_id}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{what}");
    }
    let (status, _) = other_dm.get(&format!("/api/groups/{group_id}/party")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = other_dm
        .post(&format!("{view}/get_snapshot"), json!({}))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Commands that touch the server's own machine do not exist here.
    for command in [
        "pick_path",
        "open_app_folder",
        "vault_read",
        "set_documents_dir",
        "get_data_dirs",
    ] {
        let (status, _) = dm.rpc("library", command, json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{command}");
    }

    // Only DMs add content, from their Library.
    let (status, _) = alice.rpc(&scope, "import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = alice
        .rpc(&scope, "stage_5etools_files", json!({ "files": [] }))
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "5etools homebrew files are content too"
    );
    let (status, out) = dm
        .rpc("library", "stage_5etools_files", json!({ "files": [] }))
        .await;
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "the server offers it to DMs: {out}"
    );
    let (status, _) = alice.rpc("library", "list_packs", json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = dm.rpc(&scope, "import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    eventually(&alice, &scope, "ID_PARTY_SPARK", StatusCode::NOT_FOUND).await;
    let (status, imported) = dm.rpc("library", "import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::OK, "{imported}");

    // It reaches the group, and only the group: a character outside it sees bundled content.
    eventually(&alice, &scope, "ID_PARTY_SPARK", StatusCode::OK).await;
    let (status, _) = alice
        .rpc(
            "personal",
            "get_element",
            json!({ "elementId": "ID_PARTY_SPARK" }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The DM chooses the group's packs.
    let (_, packs) = dm.rpc(&scope, "list_packs", json!({})).await;
    let pack = packs
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] != "core" && !p["id"].as_str().unwrap().starts_with("srd-"))
        .unwrap_or_else(|| panic!("{packs}"));
    let (status, out) = dm
        .rpc(
            &scope,
            "set_pack_enabled",
            json!({ "packId": pack["id"], "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    eventually(&alice, &scope, "ID_PARTY_SPARK", StatusCode::NOT_FOUND).await;
    let (status, _) = dm
        .rpc(
            "library",
            "get_element",
            json!({ "elementId": "ID_PARTY_SPARK" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "the Library keeps it");

    // Leaving the group hands the character back.
    let (status, _) = alice
        .delete(&format!("/api/groups/{group_id}/members/{}", alice.id))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, mine) = alice.get("/api/characters").await;
    let names: Vec<&str> = mine
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Hero") && names.contains(&"Solo"), "{mine}");

    // An existing character can come back in, and deleting the group returns it.
    let (status, invite) = dm.post(&invites, json!({ "username": "alice" })).await;
    assert_eq!(status, StatusCode::OK, "{invite}");
    alice
        .post(
            &format!("/api/invites/{}/accept", invite["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    let (status, out) = alice
        .post(&character, json!({ "characterId": hero["id"] }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, _) = dm.delete(&format!("/api/groups/{group_id}")).await;
    assert_eq!(status, StatusCode::OK);
    let (_, mine) = alice.get("/api/characters").await;
    assert_eq!(mine.as_array().unwrap().len(), 2, "{mine}");
    assert!(mine
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["scope"] == "personal"));
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_are_required_and_passwords_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let (status, _) = send(&router, None, "GET", "/api/me", Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(&router, Some("nonsense"), "GET", "/api/groups", Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &router,
        None,
        "POST",
        "/api/auth/register",
        json!({ "username": "weak", "password": "password", "role": "player" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    sign_up(&router, "carol", "player").await;
    for _ in 0..10 {
        let (status, _) = send(
            &router,
            None,
            "POST",
            "/api/auth/login",
            json!({ "username": "carol", "password": "wrong password" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _) = send(
        &router,
        None,
        "POST",
        "/api/auth/login",
        json!({ "username": "carol", "password": "correct horse battery" }),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

/// A setting as `client` sees it in `scope`, once the background reloads have caught up.
async fn eventually_setting(client: &Client, scope: &str, field: &str, want: &Value) {
    let mut last = Value::Null;
    for _ in 0..100 {
        let (status, settings) = client.rpc(scope, "get_settings", json!({})).await;
        assert_eq!(status, StatusCode::OK, "{settings}");
        last = settings[field].clone();
        if last == *want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{field} in {scope}: wanted {want}, last {last}");
}

fn patch(fields: Value) -> Value {
    json!({ "patch": fields })
}

#[test]
fn every_setting_has_one_owner() {
    use crate::settings::{DM_FIELDS, PLATFORM_FIELDS, RULE_FIELDS, SERVER_FIELDS, USER_FIELDS};
    let all = serde_json::to_value(borealis_lib::settings::AppSettings::default()).unwrap();
    for field in all.as_object().unwrap().keys() {
        let owners = [
            &USER_FIELDS[..],
            &DM_FIELDS,
            &RULE_FIELDS,
            &SERVER_FIELDS,
            &PLATFORM_FIELDS,
        ]
        .iter()
        .filter(|list| list.contains(&field.as_str()))
        .count();
        assert_eq!(owners, 1, "{field}: say in settings.rs who owns it");
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn settings_are_scoped_by_account_group_and_server() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let dm = sign_up(&router, "settings-dm", "dm").await;
    let alice = sign_up(&router, "settings-alice", "player").await;
    let bob = sign_up(&router, "settings-bob", "player").await;
    let (_, group) = dm.post("/api/groups", json!({ "name": "Rules" })).await;
    let group_id = group["id"].as_str().unwrap().to_string();
    let scope = format!("group-{group_id}");
    for (who, name) in [(&alice, "settings-alice"), (&bob, "settings-bob")] {
        let (_, invite) = dm
            .post(
                &format!("/api/groups/{group_id}/invites"),
                json!({ "username": name }),
            )
            .await;
        let (status, _) = who
            .post(
                &format!("/api/invites/{}/accept", invite["id"].as_str().unwrap()),
                json!({}),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Server-wide settings are nobody's to see or change.
    let (status, settings) = alice.rpc(&scope, "get_settings", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["documentsDir"], Value::Null);
    assert_eq!(settings["ai"]["enabled"], false);
    assert_eq!(settings["checkAppUpdates"], false);
    assert_eq!(settings["mobile"], true);
    for (who, place) in [
        (&dm, "library"),
        (&dm, scope.as_str()),
        (&alice, "personal"),
    ] {
        for change in [
            json!({ "checkAppUpdates": true }),
            json!({ "documentsDir": "/etc" }),
            json!({ "vaultDir": "/home" }),
            json!({ "ai": { "enabled": true } }),
        ] {
            let (status, out) = who.rpc(place, "save_settings", patch(change.clone())).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{change} in {place}: {out}");
            assert_eq!(
                out["message"],
                "That setting is managed by the server's administrator."
            );
        }
    }

    // An account's own settings follow it into every workspace, and stay its own.
    let (status, out) = alice
        .rpc(
            "personal",
            "save_settings",
            patch(json!({ "theme": "dark", "sheet": { "formFillable": true } })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["theme"], "dark");
    let (_, there) = alice.rpc(&scope, "get_settings", json!({})).await;
    assert_eq!(there["theme"], "dark");
    assert_eq!(there["sheet"]["formFillable"], true);
    let (_, theirs) = bob.rpc(&scope, "get_settings", json!({})).await;
    assert_eq!(theirs["theme"], "system", "bob's own");
    let (_, dms) = dm.rpc(&scope, "get_settings", json!({})).await;
    assert_eq!(dms["theme"], "system", "the DM's own");
    let (status, out) = dm
        .rpc(
            "library",
            "save_settings",
            patch(json!({ "accentColor": "green" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, dms) = dm.rpc(&scope, "get_settings", json!({})).await;
    assert_eq!(dms["accentColor"], "green");
    let (_, theirs) = alice.rpc(&scope, "get_settings", json!({})).await;
    assert_eq!(
        theirs["accentColor"], "default",
        "the member's file is the DM's, the look is not"
    );
    let (status, _) = alice
        .rpc(
            "personal",
            "save_settings",
            patch(json!({ "theme": "purple" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "values are still checked");

    // A group's house rules are its DM's: members read them and cannot change them.
    let (status, out) = dm
        .rpc(
            &scope,
            "save_settings",
            patch(json!({ "pointBuyBudget": 32, "abilityMethod": "point_buy" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    eventually_setting(&alice, &scope, "pointBuyBudget", &json!(32)).await;
    eventually_setting(&bob, &scope, "abilityMethod", &json!("point_buy")).await;
    let (status, out) = alice
        .rpc(
            &scope,
            "save_settings",
            patch(json!({ "pointBuyBudget": 40 })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(
        out["message"],
        "Your Dungeon Master sets this group's house rules."
    );
    let (status, out) = dm
        .rpc(
            "library",
            "save_settings",
            patch(json!({ "pointBuyBudget": 40 })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");

    // The settings window sends everything back: what the caller was shown is no change.
    let (_, mut everything) = alice.rpc(&scope, "get_settings", json!({})).await;
    everything["tileSize"] = json!(200);
    let (status, out) = alice.rpc(&scope, "save_settings", patch(everything)).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["tileSize"], 200);
    assert_eq!(out["pointBuyBudget"], 32);

    // Outside groups a player keeps their own rules.
    let (_, personal) = alice.rpc("personal", "get_settings", json!({})).await;
    assert_eq!(personal["pointBuyBudget"], 27);
    let (status, out) = alice
        .rpc(
            "personal",
            "save_settings",
            patch(json!({ "pointBuyBudget": 30 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, there) = alice.rpc(&scope, "get_settings", json!({})).await;
    assert_eq!(
        there["pointBuyBudget"], 32,
        "the group's rules are untouched"
    );

    // The DM and the member's character derive the same numbers; the DM's view can't change settings.
    let (status, hero) = alice
        .rpc(&scope, "create_character", new_character("Ruled"))
        .await;
    assert_eq!(status, StatusCode::OK, "{hero}");
    let view = format!("/api/groups/{group_id}/members/{}/view", alice.id);
    let (status, seen) = dm.post(&format!("{view}/get_snapshot"), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!(hero["abilities"]["pointBuy"]["budget"], 32, "{hero}");
    assert_eq!(seen["abilities"], hero["abilities"]);
    let (status, _) = dm
        .post(
            &format!("{view}/save_settings"),
            patch(json!({ "theme": "dark" })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The content library's settings are the DM's account's; players neither see nor change them.
    let (status, out) = dm
        .rpc(
            "library",
            "save_settings",
            patch(json!({ "showImportNotice": false })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, dms) = dm.rpc(&scope, "get_settings", json!({})).await;
    assert_eq!(dms["showImportNotice"], false);
    let (_, players) = alice.rpc("personal", "get_settings", json!({})).await;
    assert_eq!(players["autoContentUpdate"], false, "hidden from players");
    let (status, out) = alice
        .rpc(
            "personal",
            "save_settings",
            patch(json!({ "autoContentUpdate": true })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(
        out["message"],
        "Only Dungeon Masters have a content library."
    );
    for place in ["personal", scope.as_str()] {
        let (status, _) = alice.rpc(place, "clear_caches", json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{place}");
    }
    for place in ["library", scope.as_str()] {
        let (status, out) = dm.rpc(place, "clear_caches", json!({})).await;
        assert_eq!(status, StatusCode::OK, "{place}: {out}");
    }

    // Restore defaults resets only what the caller owns.
    let (status, out) = alice
        .rpc(&scope, "restore_default_settings", json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["theme"], "system");
    assert_eq!(out["tileSize"], 160);
    assert_eq!(out["pointBuyBudget"], 32, "still the DM's");
    let (_, dms) = dm.rpc(&scope, "get_settings", json!({})).await;
    assert_eq!(dms["pointBuyBudget"], 32);
    assert_eq!(dms["accentColor"], "green");
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn a_character_joining_a_group_takes_the_groups_rules() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let dm = sign_up(&router, "rules-dm", "dm").await;
    let alice = sign_up(&router, "rules-alice", "player").await;
    let (_, group) = dm.post("/api/groups", json!({ "name": "Strict" })).await;
    let group_id = group["id"].as_str().unwrap().to_string();
    let scope = format!("group-{group_id}");
    let (_, invite) = dm
        .post(
            &format!("/api/groups/{group_id}/invites"),
            json!({ "username": "rules-alice" }),
        )
        .await;
    alice
        .post(
            &format!("/api/invites/{}/accept", invite["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    let (status, out) = dm
        .rpc(
            &scope,
            "save_settings",
            patch(json!({ "abilityMethod": "point_buy", "pointBuyBudget": 20 })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");

    // Outside any group a player's character has its own house rules and spends the usual 27 points.
    let (_, solo) = alice
        .rpc("personal", "create_character", new_character("Solo"))
        .await;
    let id = solo["id"].clone();
    let (status, out) = alice
        .rpc(
            "personal",
            "set_character_options",
            json!({ "characterId": id, "options": {
                "pointBuyUnrestricted": true, "ignoreFeatPrerequisites": true, "attunementLimit": 5,
            }}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    for ability in ["str", "dex", "con"] {
        let (status, out) = alice
            .rpc(
                "personal",
                "set_base_score",
                json!({ "characterId": id, "ability": ability, "value": 15 }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{out}");
    }
    alice
        .rpc("personal", "save_character", json!({ "characterId": id }))
        .await;

    // In the group the DM's rules apply: what they don't allow is taken out, and the player is told once.
    let (status, out) = alice
        .post(
            &format!("/api/groups/{group_id}/character"),
            json!({ "characterId": id }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, joined) = alice
        .rpc(&scope, "load_character", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    assert_eq!(joined["options"]["pointBuyUnrestricted"], false, "{joined}");
    assert_eq!(joined["options"]["ignoreFeatPrerequisites"], false);
    assert_ne!(joined["options"]["attunementLimit"], 5);
    assert_eq!(joined["abilities"]["pointBuy"]["budget"], 20);
    assert_eq!(joined["abilities"]["pointBuy"]["spent"], 0, "scores reset");
    assert_eq!(joined["dirty"], false, "and saved");
    let (status, notes) = alice
        .rpc(&scope, "take_group_notes", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK, "{notes}");
    let notes = notes.as_array().unwrap().clone();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes
        .iter()
        .any(|n| n.as_str().unwrap().contains("house rules")));
    assert!(notes
        .iter()
        .any(|n| n.as_str().unwrap().contains("point buy with 20 points")));
    let (_, again) = alice
        .rpc(&scope, "take_group_notes", json!({ "characterId": id }))
        .await;
    assert_eq!(again, json!([]), "told once");
    let (status, _) = dm
        .rpc(&scope, "take_group_notes", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "the player's notes");

    // Nor can the player switch them back on in the group.
    for (command, args) in [
        (
            "set_character_options",
            json!({ "characterId": id, "options": { "ignoreFeatPrerequisites": true } }),
        ),
        (
            "set_character_options",
            json!({ "characterId": id, "options": { "pointBuyUnrestricted": true } }),
        ),
        (
            "set_ability_method",
            json!({ "characterId": id, "method": "roll_4d6" }),
        ),
    ] {
        let (status, out) = alice.rpc(&scope, command, args.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{args}: {out}");
        assert_eq!(
            out["message"],
            "Your Dungeon Master sets this group's house rules."
        );
    }
    let (status, out) = alice
        .rpc(
            &scope,
            "set_character_options",
            json!({ "characterId": id, "options": { "averageHp": false } }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "other options are the player's: {out}"
    );
    for ability in ["str", "dex"] {
        let (status, out) = alice
            .rpc(
                &scope,
                "set_base_score",
                json!({ "characterId": id, "ability": ability, "value": 15 }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{out}");
    }
    let (status, _) = alice
        .rpc(
            &scope,
            "set_base_score",
            json!({ "characterId": id, "ability": "con", "value": 15 }),
        )
        .await;
    assert_ne!(status, StatusCode::OK, "27 points is over the group's 20");
}

fn slot<'a>(snapshot: &'a Value, title: &str) -> &'a Value {
    snapshot["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == title)
        .unwrap_or_else(|| panic!("no slot {title}: {snapshot}"))
}

fn has_class(snapshot: &Value, name: &str) -> bool {
    snapshot["level"]["classes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["name"] == name)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pick_made_by_ignoring_prerequisites_is_taken_out_in_a_group() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let dm = sign_up(&router, "prereq-dm", "dm").await;
    let alice = sign_up(&router, "prereq-alice", "player").await;
    let (_, group) = dm
        .post("/api/groups", json!({ "name": "By the book" }))
        .await;
    let group_id = group["id"].as_str().unwrap().to_string();
    let scope = format!("group-{group_id}");
    let (_, invite) = dm
        .post(
            &format!("/api/groups/{group_id}/invites"),
            json!({ "username": "prereq-alice" }),
        )
        .await;
    alice
        .post(
            &format!("/api/invites/{}/accept", invite["id"].as_str().unwrap()),
            json!({}),
        )
        .await;
    // Scores by hand, so only the multiclass is at odds with the group.
    let (status, out) = dm
        .rpc(
            &scope,
            "save_settings",
            patch(json!({ "abilityMethod": "manual" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");

    // Outside any group: a CHA 8 wizard multiclasses into Bard by ignoring its CHA 13 prerequisite.
    let (status, wizard) = alice
        .rpc(
            "personal",
            "create_character",
            json!({ "request": {
                "name": "Seraphine", "ruleset": "dnd2014", "abilityMethod": "manual",
                "options": { "multiclassing": true },
            }}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{wizard}");
    let id = wizard["id"].clone();
    let mut snapshot = wizard;
    for (title, element) in [
        ("Race", "ID_RACE_ELF"),
        ("Class", "ID_WOTC_PHB_CLASS_WIZARD"),
        ("Background", "ID_BACKGROUND_ACOLYTE"),
    ] {
        let (status, out) = alice
            .rpc(
                "personal",
                "apply_selection",
                json!({ "characterId": id, "slotId": slot(&snapshot, title)["id"], "elementId": element }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{title}: {out}");
        snapshot = out;
    }
    for (ability, value) in [("str", 13), ("cha", 8), ("int", 13)] {
        let (status, out) = alice
            .rpc(
                "personal",
                "set_base_score",
                json!({ "characterId": id, "ability": ability, "value": value }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{out}");
    }
    let (status, out) = alice
        .rpc(
            "personal",
            "set_character_options",
            json!({ "characterId": id, "options": { "ignoreMulticlassPrerequisites": true } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = alice
        .rpc("personal", "add_multiclass", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let multiclass = slot(&out, "Multiclass (Level 2)")["id"].clone();
    let (status, bard) = alice
        .rpc(
            "personal",
            "apply_selection",
            json!({ "characterId": id, "slotId": multiclass, "elementId": "ID_WOTC_PHB_MULTICLASS_BARD" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{bard}");
    assert!(has_class(&bard, "Bard"), "{bard}");
    assert_eq!(
        slot(&bard, "Multiclass (Level 2)")["prerequisitesUnmet"],
        true
    );
    alice
        .rpc("personal", "save_character", json!({ "characterId": id }))
        .await;

    // In the group the DM's rules hold: the Bard levels go, and the player is told to choose again.
    let (status, out) = alice
        .post(
            &format!("/api/groups/{group_id}/character"),
            json!({ "characterId": id }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, joined) = alice
        .rpc(&scope, "load_character", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    assert!(!has_class(&joined, "Bard"), "{joined}");
    assert!(has_class(&joined, "Wizard"), "the main class stays");
    assert_eq!(joined["options"]["ignoreMulticlassPrerequisites"], false);
    assert!(joined["slots"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["prerequisitesUnmet"] == false || s["chosen"].is_null()));
    assert_eq!(joined["dirty"], false, "and saved");
    let (_, notes) = alice
        .rpc(&scope, "take_group_notes", json!({ "characterId": id }))
        .await;
    let notes: Vec<&str> = notes
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("Bard") && n.contains("Choose again")),
        "{notes:?}"
    );
    assert!(notes.iter().any(|n| n.contains("house rules")), "{notes:?}");
    assert_eq!(notes.len(), 2, "the scores were fine: {notes:?}");

    // It stays out after a reopen.
    let (_, again) = alice
        .rpc(&scope, "load_character", json!({ "characterId": id }))
        .await;
    assert!(!has_class(&again, "Bard"));
}
