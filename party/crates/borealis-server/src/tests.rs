//! The server end to end, through the router and the real Borealis backend.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::auth::RateLimiter;
use crate::db::{Db, Location};
use crate::mail::{Mailer, Recorder};
use crate::workspaces::Workspaces;
use crate::{api_routes, AppState, Config};

#[path = "sync_tests.rs"]
mod sync_tests;

const PASSWORD: &str = "correct horse battery";

struct Client {
    router: Router,
    token: String,
    id: String,
}

/// How a test server is set up.
struct Setup {
    require_approval: bool,
    mailer: Option<Arc<Recorder>>,
    signups_allowed: usize,
    max_upload: usize,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            require_approval: false,
            mailer: None,
            signups_allowed: 1000,
            max_upload: 64 * 1024 * 1024,
        }
    }
}

async fn router(data: PathBuf) -> Router {
    router_on(data, Db::open(&Location::Memory).await.unwrap())
}

fn router_on(data: PathBuf, db: Db) -> Router {
    router_with(data, db, Setup::default())
}

fn router_with(data: PathBuf, db: Db, setup: Setup) -> Router {
    let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../content");
    let mailer = setup.mailer.map(|m| m as Arc<dyn Mailer>);
    let state = AppState {
        db,
        workspaces: Arc::new(Workspaces::new(data, content)),
        config: Arc::new(Config {
            open_registration: true,
            require_approval: setup.require_approval,
            public_url: mailer
                .as_ref()
                .map(|_| "https://party.example.com".to_string()),
            app_download: Some(crate::config::DEFAULT_APP_DOWNLOAD.to_string()),
        }),
        login_limiter: RateLimiter::logins(),
        signup_limiter: RateLimiter::new(setup.signups_allowed, Duration::from_secs(3600)),
        mailer,
        accounts: Arc::new(tokio::sync::Mutex::new(())),
        started: std::time::Instant::now(),
    };
    api_routes(state, setup.max_upload).layer(MockConnectInfo(std::net::SocketAddr::from((
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

/// Ask for an account; the answer as it came.
async fn register(router: &Router, name: &str, email: &str) -> (StatusCode, Value) {
    send(
        router,
        None,
        "POST",
        "/api/auth/register",
        json!({ "username": name, "email": email, "password": PASSWORD }),
    )
    .await
}

/// Sign up an account that is active straight away.
async fn sign_up(router: &Router, name: &str) -> Client {
    let (status, out) = register(router, name, &format!("{name}@example.com")).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["status"], "active", "{out}");
    Client {
        router: router.clone(),
        token: out["token"].as_str().unwrap().into(),
        id: out["user"]["id"].as_str().unwrap().into(),
    }
}

async fn login(router: &Router, name: &str, password: &str) -> (StatusCode, Value) {
    send(
        router,
        None,
        "POST",
        "/api/auth/login",
        json!({ "username": name, "password": password }),
    )
    .await
}

async fn sign_in(router: &Router, name: &str) -> Client {
    let (status, out) = login(router, name, PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{name}: {out}");
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
    async fn patch(&self, path: &str, body: Value) -> (StatusCode, Value) {
        send(&self.router, Some(&self.token), "PATCH", path, body).await
    }
    async fn get(&self, path: &str) -> (StatusCode, Value) {
        send(&self.router, Some(&self.token), "GET", path, Value::Null).await
    }
    async fn delete(&self, path: &str) -> (StatusCode, Value) {
        send(&self.router, Some(&self.token), "DELETE", path, Value::Null).await
    }
    async fn rpc(&self, command: &str, args: Value) -> (StatusCode, Value) {
        self.post(&format!("/api/rpc/{command}"), args).await
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

fn patch(fields: Value) -> Value {
    json!({ "patch": fields })
}

fn character_file(data: &Path, account: &str, character: &str) -> PathBuf {
    data.join("users")
        .join(account)
        .join("characters")
        .join(format!("{character}.json"))
}

fn names(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap_or_else(|| panic!("{list}"))
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_does_not_wait_for_an_open_event_stream() {
    use axum::response::sse::{Event, Sse};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let endless = || async {
        Sse::new(tokio_stream::pending::<
            Result<Event, std::convert::Infallible>,
        >())
    };
    let router = Router::new().route("/api/events", axum::routing::get(endless));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(crate::serve(listener, router, async move {
        let _ = stopped.await;
    }));
    let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
    client
        .write_all(
            b"GET /api/events HTTP/1.1
Host: localhost

",
        )
        .await
        .unwrap();
    let mut head = [0u8; 12];
    client.read_exact(&mut head).await.unwrap();
    assert_eq!(&head, b"HTTP/1.1 200");
    stop.send(()).unwrap();
    tokio::time::timeout(crate::STOP_GRACE + Duration::from_secs(5), server)
        .await
        .expect("the server stops although the stream is open")
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_an_account_waits_for_its_workspace_to_open() {
    let dir = tempfile::tempdir().unwrap();
    let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../content");
    let workspaces = Arc::new(Workspaces::new(dir.path().to_path_buf(), content));
    let opening = tokio::spawn({
        let workspaces = Arc::clone(&workspaces);
        async move { workspaces.get("someone").await.map(|_| ()) }
    });
    // Opening loads the content, which takes far longer than this.
    tokio::time::sleep(Duration::from_millis(50)).await;
    workspaces.remove("someone").await.unwrap();
    opening.await.unwrap().unwrap();
    assert!(!workspaces.dir("someone").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn each_account_has_its_own_characters_and_content() {
    let dir = tempfile::tempdir().unwrap();
    full_flow(router(dir.path().to_path_buf()).await, dir.path()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn admins_approve_and_manage_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    // The first account on a new server is its admin, active at once; the next one is not.
    let admin = sign_up(&router, "the-admin").await;
    let (_, me) = admin.get("/api/me").await;
    assert_eq!(me["admin"], true, "{me}");
    let second = sign_up(&router, "the-second").await;
    let (_, me) = second.get("/api/me").await;
    assert_eq!(me["admin"], false, "{me}");
    admin_flow(&router, &admin, dir.path()).await;
}

/// Three accounts, a setting saved on the admin page, and a character and a setting in the first account's folder.
async fn some_accounts(router: &Router) {
    let admin = sign_up(router, "moving-admin").await;
    sign_up(router, "moving-player").await;
    sign_up(router, "moving-other").await;
    let (status, out) = admin
        .patch("/api/admin/settings", json!({ "requireApproval": true }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = admin
        .rpc("save_settings", patch(json!({ "accentColor": "green" })))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, moved) = admin.rpc("create_character", new_character("Moved")).await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    let (status, out) = admin
        .rpc("save_character", json!({ "characterId": moved["id"] }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

/// After a copy the new database answers exactly like the old one, over the same data folder.
async fn check_moved(router: &Router) {
    let (status, out) = login(router, "MOVING-ADMIN", PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["user"]["admin"], true, "the first account is the admin");
    assert_eq!(out["user"]["email"], "moving-admin@example.com");
    let admin = Client {
        router: router.clone(),
        token: out["token"].as_str().unwrap().into(),
        id: out["user"]["id"].as_str().unwrap().into(),
    };
    let (_, me) = sign_in(router, "moving-player").await.get("/api/me").await;
    assert_eq!(me["admin"], false, "{me}");
    let (_, settings) = admin.get("/api/admin/settings").await;
    assert_eq!(settings["requireApproval"], true, "{settings}");
    let (_, log) = admin.get("/api/admin/activity").await;
    assert!(
        log["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "setting_changed"),
        "the activity log moves too: {log}"
    );
    // The files never moved: the account id is the same, so its folder is found again.
    let (_, list) = admin.rpc("list_characters", json!({})).await;
    assert_eq!(names(&list), ["Moved"], "{list}");
    let (_, settings) = admin.rpc("get_settings", json!({})).await;
    assert_eq!(settings["accentColor"], "green");
}

/// The rows `some_accounts` makes, as the copy reports them.
const MOVED: &str = "users 3, sessions 3, server_settings 1, activity 4";

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
    assert_eq!(crate::migrate::describe(&report), MOVED);
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

/// A database with the table only the versions with groups had.
async fn make_old(url: &str) {
    sqlx::any::install_default_drivers();
    let pool = sqlx::AnyPool::connect(url).await.unwrap();
    sqlx::query("CREATE TABLE party_groups (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

/// True when the database still has the table only the versions with groups had.
async fn has_old_table(url: &str) -> bool {
    sqlx::any::install_default_drivers();
    let pool = sqlx::AnyPool::connect(url).await.unwrap();
    let found = sqlx::query("SELECT COUNT(*) FROM party_groups")
        .fetch_one(&pool)
        .await
        .is_ok();
    pool.close().await;
    found
}

/// A database that still has the table the versions with groups had opens like any other, its accounts and all, and
/// the table is left alone. Nor does it stop a move into a new database.
#[tokio::test(flavor = "multi_thread")]
async fn a_database_left_with_the_groups_table_opens() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("party.db");
    let data = dir.path().join("data");
    let url = format!("sqlite://{}?mode=rwc", file.display());
    let old = Db::open(&Location::Sqlite(file.clone())).await.unwrap();
    some_accounts(&router_on(data.clone(), old)).await;
    make_old(&url).await;
    let db = Db::open(&Location::Sqlite(file.clone())).await.unwrap();
    check_moved(&router_on(data.clone(), db)).await;
    assert!(has_old_table(&url).await);
    // A database with only that table opens too.
    let bare = dir.path().join("bare.db");
    make_old(&format!("sqlite://{}?mode=rwc", bare.display())).await;
    let fresh = Db::open(&Location::Sqlite(bare)).await.unwrap();
    assert_eq!(fresh.user_count().await.unwrap(), 0);
    // And the instance still moves to a new database.
    let new = Db::open(&Location::Memory).await.unwrap();
    let report = crate::migrate::adopt_sqlite(&new, &file)
        .await
        .unwrap()
        .unwrap();
    let moved = crate::migrate::describe(&report);
    assert!(moved.starts_with("users 3, "), "{moved}");
    check_moved(&router_on(data, new)).await;
}

async fn drop_all(url: &str) {
    sqlx::any::install_default_drivers();
    let pool = sqlx::AnyPool::connect(url).await.unwrap();
    for table in [
        "activity",
        "server_settings",
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

/// The same on PostgreSQL. Runs when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.
#[tokio::test(flavor = "multi_thread")]
async fn each_account_has_its_own_characters_and_content_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    drop_all(&url).await;
    // Left over from the versions with groups: the database opens all the same, and the table stays.
    make_old(&url).await;
    let db = Db::open(&Location::Postgres(url.clone())).await.unwrap();
    assert!(has_old_table(&url).await);
    // Opening again finds the schema already there.
    Db::open(&Location::Postgres(url)).await.unwrap();
    // First a move from SQLite, as when an instance switches databases.
    let file = dir.path().join("party.db");
    let old = Db::open(&Location::Sqlite(file.clone())).await.unwrap();
    some_accounts(&router_on(data.clone(), old)).await;
    let report = crate::migrate::adopt_sqlite(&db, &file)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(crate::migrate::describe(&report), MOVED);
    let router = router_on(data.clone(), db.clone());
    check_moved(&router).await;
    assert!(crate::migrate::adopt_sqlite(&db, &file)
        .await
        .unwrap()
        .is_none());
    // The moved first account is the admin here. This ends with sign-up open and no approval needed.
    let admin = sign_in(&router, "moving-admin").await;
    admin_flow(&router, &admin, &data).await;
    sessions_slide(&router, &db).await;
    bundle_flow(router.clone(), &data).await;
    receive_flow(router.clone(), &data).await;
    sync_tests::sync_flow(router.clone(), &data).await;
    sync_tests::push_flow(router.clone(), &data).await;
    full_flow(router, &data).await;
}

async fn full_flow(router: Router, data: &Path) {
    let alice = sign_up(&router, "alice").await;
    let bob = sign_up(&router, "bob").await;

    // An account creates, changes, saves and lists characters, in its own folder.
    let (status, hero) = alice.rpc("create_character", new_character("Hero")).await;
    assert_eq!(status, StatusCode::OK, "{hero}");
    assert!(
        hero.get("displayPath").is_none(),
        "server paths stay private"
    );
    let id = hero["id"].as_str().unwrap().to_string();
    let character = json!({ "characterId": id });
    let (status, out) = alice
        .rpc("set_xp", json!({ "characterId": id, "xp": 900 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = alice.rpc("save_character", character.clone()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert!(character_file(data, &alice.id, &id).is_file());
    let (status, list) = alice.rpc("list_characters", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(names(&list), ["Hero"], "{list}");

    // Another account sees none of them and cannot reach them by id.
    let (status, list) = bob.rpc("list_characters", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert!(names(&list).is_empty(), "{list}");
    for command in [
        "load_character",
        "get_snapshot",
        "save_character",
        "export_character",
        "delete_character",
    ] {
        let (status, out) = bob.rpc(command, character.clone()).await;
        assert!(!status.is_success(), "{command}: {out}");
    }
    let (status, out) = bob
        .rpc("set_xp", json!({ "characterId": id, "xp": 1 }))
        .await;
    assert!(!status.is_success(), "{out}");
    assert!(!character_file(data, &bob.id, &id).exists());
    assert!(character_file(data, &alice.id, &id).is_file());
    let (status, sheet) = alice.rpc("load_character", character.clone()).await;
    assert_eq!(status, StatusCode::OK, "{sheet}");
    assert_eq!(sheet["name"], "Hero");
    assert_eq!(sheet["level"]["xp"], 900, "untouched by anyone else");

    // Content an account imports is its own.
    let (status, imported) = alice.rpc("import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    let spark = json!({ "elementId": "ID_PARTY_SPARK" });
    let (status, out) = alice.rpc("get_element", spark.clone()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, _) = bob.rpc("get_element", spark).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let own_packs = |packs: &Value| {
        packs
            .as_array()
            .unwrap_or_else(|| panic!("{packs}"))
            .iter()
            .filter(|p| p["id"] != "core" && !p["id"].as_str().unwrap().starts_with("srd-"))
            .count()
    };
    let (_, packs) = alice.rpc("list_packs", json!({})).await;
    assert_eq!(own_packs(&packs), 1, "{packs}");
    let (_, packs) = bob.rpc("list_packs", json!({})).await;
    assert_eq!(own_packs(&packs), 0, "{packs}");
    // Every account may run the content commands, in its own folder.
    let (status, out) = bob.rpc("import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = bob.rpc("stage_5etools_files", json!({ "files": [] })).await;
    assert_ne!(status, StatusCode::NOT_FOUND, "the server offers it: {out}");

    // Commands that touch the server's own machine are not offered, nor is anything from before.
    for command in [
        "pick_path",
        "open_app_folder",
        "vault_read",
        "set_documents_dir",
        "get_data_dirs",
        "take_group_notes",
        // Bundles are opened from uploads and exported as downloads only: no share sheet, no OS inbox (one queue for the whole process), no link.
        "take_opened_bundles",
        "take_opened_files",
        "bundle_share",
        "link_start_receive",
        "link_start_send",
        "link_connect",
        "link_status",
        "link_accept",
        "link_decline",
        "link_cancel",
        "link_devices",
        "link_forget_device",
    ] {
        let (status, out) = alice.rpc(command, json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{command}");
        assert_eq!(out["message"], "The server does not offer that.", "{out}");
    }
    for path in [
        "/api/rpc/personal/list_characters",
        "/api/groups",
        "/api/invites",
        "/api/characters",
    ] {
        let (status, _) = alice.post(path, json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        let (status, out) = alice.get(path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(out["code"], "not_found", "{path}: {out}");
    }
    let (status, _) = send(&router, None, "GET", "/api/events", Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Deleting a character removes its file.
    let (status, out) = alice.rpc("delete_character", character).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert!(!character_file(data, &alice.id, &id).exists());
    let (_, list) = alice.rpc("list_characters", json!({})).await;
    assert!(names(&list).is_empty(), "{list}");
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_are_each_accounts_own_and_the_servers_stay_fixed() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let alice = sign_up(&router, "settings-alice").await;
    let bob = sign_up(&router, "settings-bob").await;
    let defaults = serde_json::to_value(borealis_lib::settings::AppSettings::default()).unwrap();
    let ai = serde_json::to_value(borealis_lib::settings::AiSettings::default()).unwrap();
    let fixed = |settings: &Value| {
        for folder in [
            "documentsDir",
            "secondaryContentDir",
            "portraitLibraryDir",
            "vaultDir",
        ] {
            assert_eq!(settings[folder], Value::Null, "{folder}: {settings}");
        }
        assert_eq!(settings["mobile"], true, "{settings}");
        assert_eq!(settings["storeBuild"], true, "{settings}");
        assert_eq!(settings["aiAvailable"], false, "{settings}");
        assert_eq!(settings["checkAppUpdates"], false, "{settings}");
        assert_eq!(settings["ai"], ai, "{settings}");
    };
    let (status, settings) = alice.rpc("get_settings", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    fixed(&settings);
    assert_eq!(settings["accentColor"], defaults["accentColor"]);

    // A patch that carries the server's settings saves the rest and ignores those.
    let unusual_updates = defaults["checkAppUpdates"] != true;
    let (status, saved) = alice
        .rpc(
            "save_settings",
            patch(json!({
                "documentsDir": "/etc",
                "secondaryContentDir": "/tmp",
                "portraitLibraryDir": "/tmp",
                "vaultDir": "/tmp",
                "checkAppUpdates": unusual_updates,
                "ai": { "enabled": true },
                "mobile": false,
                "storeBuild": false,
                "aiAvailable": true,
                "accentColor": "green",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    fixed(&saved);
    assert_eq!(saved["accentColor"], "green");
    let (status, saved) = alice
        .rpc("save_settings", patch(json!({ "documentsDir": "/etc" })))
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    fixed(&saved);
    let file = dir
        .path()
        .join("users")
        .join(&alice.id)
        .join("settings.json");
    let on_disk = || -> Value { serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap() };
    let stored = on_disk();
    assert_eq!(stored["accentColor"], "green", "{stored}");
    for key in [
        "documentsDir",
        "secondaryContentDir",
        "portraitLibraryDir",
        "vaultDir",
        "checkAppUpdates",
        "ai",
    ] {
        let kept = stored.get(key).unwrap_or(&defaults[key]);
        assert_eq!(*kept, defaults[key], "{key} never reaches disk: {stored}");
    }
    assert!(!std::fs::read_to_string(&file).unwrap().contains("/etc"));

    // An ordinary setting round-trips, for that account only.
    let (_, settings) = alice.rpc("get_settings", json!({})).await;
    assert_eq!(settings["accentColor"], "green");
    fixed(&settings);
    let (_, settings) = bob.rpc("get_settings", json!({})).await;
    assert_eq!(settings["accentColor"], defaults["accentColor"]);
    let (status, out) = alice
        .rpc(
            "save_settings",
            patch(json!({ "accentColor": "no such colour" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{out}");

    // Restore defaults is offered, and is the account's own too.
    bob.rpc("save_settings", patch(json!({ "accentColor": "green" })))
        .await;
    let (status, restored) = alice.rpc("restore_default_settings", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    fixed(&restored);
    assert_eq!(restored["accentColor"], defaults["accentColor"]);
    assert_eq!(on_disk()["accentColor"], defaults["accentColor"]);
    let (_, settings) = bob.rpc("get_settings", json!({})).await;
    assert_eq!(settings["accentColor"], "green");
}

async fn status_of(admin: &Client, name: &str) -> Value {
    let (status, list) = admin
        .get(&format!("/api/admin/accounts?search={name}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["username"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("{name} in {list}"))
}

/// Sign-up approval, the admin APIs and the activity log. `admin` is an active admin of a server with sign-up open;
/// `data` is its data folder.
async fn admin_flow(router: &Router, admin: &Client, data: &std::path::Path) {
    let (status, out) = admin
        .patch("/api/admin/settings", json!({ "requireApproval": true }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["requireApproval"], true);

    // New accounts wait, and can't sign in meanwhile.
    let (status, out) = register(router, "waiting", "waiting@example.com").await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["status"], "pending");
    assert!(out.get("token").is_none(), "{out}");
    let (status, out) = login(router, "waiting", PASSWORD).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(out["code"], "account_pending", "{out}");
    let (status, out) = login(router, "waiting", "wrong password here").await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a wrong password says nothing about the account: {out}"
    );
    // Email addresses are unique, whatever their case, and needed.
    let (status, _) = register(router, "copycat", "WAITING@example.com").await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = register(router, "no-email", "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = register(router, "bad-email", "not an address").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Approval lets them in; sign-in also takes the email address.
    let waiting = status_of(admin, "waiting").await;
    assert_eq!(waiting["status"], "pending");
    let waiting_id = waiting["id"].as_str().unwrap().to_string();
    let (status, out) = admin
        .post(
            &format!("/api/admin/accounts/{waiting_id}/status"),
            json!({ "status": "active" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let user = sign_in(router, "Waiting@Example.com").await;
    let (_, me) = user.get("/api/me").await;
    assert_eq!(me["admin"], false);

    // Rejected accounts stay out.
    register(router, "turned-down", "turned-down@example.com").await;
    let turned = status_of(admin, "turned-down").await;
    admin
        .post(
            &format!(
                "/api/admin/accounts/{}/status",
                turned["id"].as_str().unwrap()
            ),
            json!({ "status": "rejected" }),
        )
        .await;
    let (status, out) = login(router, "turned-down", PASSWORD).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(out["code"], "account_rejected");

    // Only admins reach the admin APIs.
    for (method, path) in [
        ("GET", "/api/admin/stats".to_string()),
        ("GET", "/api/admin/accounts".to_string()),
        ("GET", "/api/admin/activity".to_string()),
        ("GET", "/api/admin/settings".to_string()),
        ("PATCH", "/api/admin/settings".to_string()),
        ("POST", format!("/api/admin/accounts/{}/status", admin.id)),
        ("POST", format!("/api/admin/accounts/{}/admin", admin.id)),
        ("POST", format!("/api/admin/accounts/{}/password", admin.id)),
        ("POST", format!("/api/admin/accounts/{}/sign-out", admin.id)),
        ("DELETE", format!("/api/admin/accounts/{}", admin.id)),
    ] {
        let body = json!({ "status": "disabled", "admin": false, "openRegistration": false });
        let (status, _) = send(router, Some(&user.token), method, &path, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
    }

    // Never zero admins.
    let only_admin = admin.id.clone();
    let (status, _) = admin
        .post(
            &format!("/api/admin/accounts/{only_admin}/admin"),
            json!({ "admin": false }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "the last admin stays one");
    let (status, _) = admin
        .post(
            &format!("/api/admin/accounts/{only_admin}/status"),
            json!({ "status": "disabled" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = admin
        .delete(&format!("/api/admin/accounts/{only_admin}"))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, out) = admin
        .post(
            &format!("/api/admin/accounts/{}/admin", user.id),
            json!({ "admin": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, me) = user.get("/api/me").await;
    assert_eq!(me["admin"], true);
    let (status, _) = user
        .post(
            &format!("/api/admin/accounts/{}/admin", user.id),
            json!({ "admin": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "another admin exists now");
    let (status, _) = user.get("/api/admin/stats").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Disabling ends the sessions at once.
    let (status, _) = admin
        .post(
            &format!("/api/admin/accounts/{}/status", user.id),
            json!({ "status": "disabled" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = user.get("/api/me").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, out) = login(router, "waiting", PASSWORD).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(out["code"], "account_disabled");
    admin
        .post(
            &format!("/api/admin/accounts/{}/status", user.id),
            json!({ "status": "active" }),
        )
        .await;
    let user = sign_in(router, "waiting").await;

    // A temporary password: signed out, then a new password before anything else.
    let (status, _) = admin
        .post(
            &format!("/api/admin/accounts/{}/password", user.id),
            json!({ "password": "temporary words 42" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = user.get("/api/me").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = login(router, "waiting", PASSWORD).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old password is gone");
    let (status, out) = login(router, "waiting", "temporary words 42").await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["user"]["mustChangePassword"], true);
    let user = Client {
        router: router.clone(),
        token: out["token"].as_str().unwrap().into(),
        id: user.id.clone(),
    };
    let (status, out) = user.rpc("list_characters", json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(out["code"], "password_change_required");
    let (status, _) = user
        .post(
            "/api/me/password",
            json!({ "current": "wrong", "password": "a brand new phrase" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, out) = user
        .post(
            "/api/me/password",
            json!({ "current": "temporary words 42", "password": "a brand new phrase" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["mustChangePassword"], false);
    let (status, _) = user.rpc("list_characters", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    // Forcing a change without a new password keeps the old one.
    admin
        .post(
            &format!("/api/admin/accounts/{}/password", user.id),
            json!({}),
        )
        .await;
    let (status, out) = login(router, "waiting", "a brand new phrase").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["user"]["mustChangePassword"], true);
    let user = Client {
        router: router.clone(),
        token: out["token"].as_str().unwrap().into(),
        id: user.id.clone(),
    };
    user.post(
        "/api/me/password",
        json!({ "current": "a brand new phrase", "password": PASSWORD }),
    )
    .await;

    // Signing out everywhere.
    let second = sign_in(router, "waiting").await;
    let (status, out) = admin
        .post(
            &format!("/api/admin/accounts/{}/sign-out", user.id),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(out["ended"].as_u64().unwrap() >= 2, "{out}");
    for client in [&user, &second] {
        let (status, _) = client.get("/api/me").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // Deleting an account takes its sessions, its characters and its content with it.
    let user = sign_in(router, "waiting").await;
    let (status, theirs) = user
        .rpc("create_character", new_character("Soon gone"))
        .await;
    assert_eq!(status, StatusCode::OK, "{theirs}");
    user.rpc("save_character", json!({ "characterId": theirs["id"] }))
        .await;
    let (status, out) = user.rpc("import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let folder = data.join("users").join(&user.id);
    admin.rpc("list_characters", json!({})).await;
    assert!(character_file(data, &user.id, theirs["id"].as_str().unwrap()).is_file());
    let (status, _) = admin
        .delete(&format!("/api/admin/accounts/{}", user.id))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = login(router, "waiting", PASSWORD).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = user.rpc("list_characters", json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!folder.exists(), "{:?}", walk(&folder));
    assert!(data.join("users").join(&admin.id).is_dir(), "others stay");

    // Runtime settings: no approval, then no sign-up, without a restart.
    admin
        .patch("/api/admin/settings", json!({ "requireApproval": false }))
        .await;
    let (_, out) = register(router, "straight-in", "straight-in@example.com").await;
    assert_eq!(out["status"], "active", "{out}");
    let (status, out) = admin
        .patch("/api/admin/settings", json!({ "openRegistration": false }))
        .await;
    assert_eq!(out["openRegistration"], false, "{out}");
    assert_eq!(status, StatusCode::OK);
    let (status, _) = register(router, "too-late", "too-late@example.com").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, info) = send(router, None, "GET", "/api/server", Value::Null).await;
    assert_eq!(info["openRegistration"], false);
    assert_eq!(info["appDownload"], crate::config::DEFAULT_APP_DOWNLOAD);
    admin
        .patch("/api/admin/settings", json!({ "openRegistration": true }))
        .await;

    // Statistics.
    let (_, counted) = admin
        .rpc("create_character", new_character("Counted"))
        .await;
    admin
        .rpc("save_character", json!({ "characterId": counted["id"] }))
        .await;
    let (status, stats) = admin.get("/api/admin/stats").await;
    assert_eq!(status, StatusCode::OK, "{stats}");
    assert_eq!(stats["accounts"]["rejected"], 1, "{stats}");
    assert!(stats["accounts"]["active"].as_i64().unwrap() >= 2);
    assert!(stats["activeSessions"].as_i64().unwrap() >= 1);
    assert!(stats.get("groups").is_none(), "{stats}");
    assert!(stats["diskBytes"].as_u64().unwrap() > 0);
    assert!(stats["characters"].as_u64().unwrap() >= 1, "{stats}");
    assert!(stats["version"].is_string());
    assert!(["sqlite", "postgres"].contains(&stats["database"].as_str().unwrap()));
    assert!(stats["uptimeSeconds"].is_u64());

    // Search, and the activity log in pages.
    let (_, found) = admin.get("/api/admin/accounts?search=STRAIGHT").await;
    assert_eq!(found["total"], 1, "{found}");
    let (_, rejected) = admin.get("/api/admin/accounts?status=rejected").await;
    assert_eq!(
        rejected["items"][0]["username"], "turned-down",
        "{rejected}"
    );
    let (status, page) = admin.get("/api/admin/activity?page=0&perPage=5").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 5);
    assert!(page["total"].as_i64().unwrap() > 5);
    let (_, all) = admin.get("/api/admin/activity?perPage=200").await;
    let kinds: Vec<&str> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    for kind in [
        "sign_up",
        "approve",
        "reject",
        "disable",
        "enable",
        "sign_in",
        "sign_in_failed",
        "admin_granted",
        "admin_removed",
        "password_reset",
        "password_changed",
        "signed_out_everywhere",
        "account_deleted",
        "setting_changed",
    ] {
        assert!(kinds.contains(&kind), "{kind} in {kinds:?}");
    }
    assert!(all["items"][0]["at"].as_i64().unwrap() > 0);
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        out.push(entry.path());
        if entry.path().symlink_metadata().is_ok_and(|m| m.is_dir()) {
            out.extend(walk(&entry.path()));
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn email_is_confirmed_before_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mail = Arc::new(Recorder::default());
    let router = router_with(
        dir.path().to_path_buf(),
        Db::open(&Location::Memory).await.unwrap(),
        Setup {
            require_approval: true,
            mailer: Some(Arc::clone(&mail)),
            ..Setup::default()
        },
    );
    // The first account needs no confirmation: there is nobody to approve it.
    let admin = sign_up(&router, "first").await;
    assert!(mail.0.lock().unwrap().is_empty());
    let (status, out) = register(&router, "mailed", "mailed@example.com").await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["awaitingEmail"], true, "{out}");
    let (status, out) = login(&router, "mailed", PASSWORD).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(out["code"], "email_unconfirmed");
    let link = {
        let sent = mail.0.lock().unwrap();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].0, "mailed@example.com");
        sent[0]
            .2
            .lines()
            .find(|l| l.starts_with("https://party.example.com/api/auth/confirm/"))
            .unwrap()
            .to_string()
    };
    // Unconfirmed, it isn't in the approval queue yet but is listed as waiting for email.
    let row = status_of(&admin, "mailed").await;
    assert_eq!(row["awaitingEmail"], true, "{row}");
    let path = link.trim_start_matches("https://party.example.com");
    let (status, _) = send(&router, None, "GET", path, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &router,
        None,
        "GET",
        "/api/auth/confirm/not-a-token",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "an explanation, not an error page");
    let (status, out) = login(&router, "mailed", PASSWORD).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(out["code"], "account_pending", "{out}");
    // The admin hears about it.
    let mut told = false;
    for _ in 0..50 {
        told = mail
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|(to, subject, _)| to == "first@example.com" && subject.contains("mailed"));
        if told {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(told, "{:?}", mail.0.lock().unwrap());
    let row = status_of(&admin, "mailed").await;
    assert_eq!(row["awaitingEmail"], false);
    assert!(row["emailConfirmedAt"].is_i64(), "{row}");
    admin
        .post(
            &format!("/api/admin/accounts/{}/status", row["id"].as_str().unwrap()),
            json!({ "status": "active" }),
        )
        .await;
    sign_in(&router, "mailed").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_ups_are_rate_limited() {
    let dir = tempfile::tempdir().unwrap();
    let router = router_with(
        dir.path().to_path_buf(),
        Db::open(&Location::Memory).await.unwrap(),
        Setup {
            signups_allowed: crate::auth::SIGNUPS_ALLOWED,
            ..Setup::default()
        },
    );
    for n in 0..crate::auth::SIGNUPS_ALLOWED {
        // Failed attempts count too.
        let (status, _) = register(&router, &format!("u{n}"), "not an address").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, _) = register(&router, "one-more", "one-more@example.com").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test(flavor = "multi_thread")]
async fn sessions_are_required_and_passwords_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let (status, _) = send(&router, None, "GET", "/api/me", Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &router,
        Some("nonsense"),
        "POST",
        "/api/rpc/list_characters",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &router,
        None,
        "POST",
        "/api/auth/register",
        json!({ "username": "weak", "email": "weak@example.com", "password": "password" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    sign_up(&router, "carol").await;
    for _ in 0..10 {
        let (status, _) = login(&router, "carol", "wrong password").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _) = login(&router, "carol", PASSWORD).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_in_use_slides() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    sessions_slide(&router_on(dir.path().to_path_buf(), db.clone()), &db).await;
}

/// A request renews its session's 90 days, once a day at most; an ended session stays ended.
async fn sessions_slide(router: &Router, db: &Db) {
    use crate::auth::{token_hash, RENEW_SECONDS, SESSION_SECONDS};
    use crate::db::now;
    use sqlx::Row;
    let user = sign_up(router, "slider").await;
    let hash = token_hash(&user.token);
    let set = |at: i64| {
        let hash = hash.clone();
        async move {
            sqlx::query("UPDATE sessions SET expires_at = $1 WHERE token_hash = $2")
                .bind(at)
                .bind(hash)
                .execute(db.pool())
                .await
                .unwrap();
        }
    };
    let ends = || async {
        let row = sqlx::query("SELECT expires_at FROM sessions WHERE token_hash = $1")
            .bind(hash.clone())
            .fetch_one(db.pool())
            .await
            .unwrap();
        row.try_get::<i64, _>(0).unwrap()
    };
    let me = || async {
        send(router, Some(&user.token), "GET", "/api/me", Value::Null)
            .await
            .0
    };

    // Renewed an hour ago: not written again.
    let recent = now() + SESSION_SECONDS - 3600;
    set(recent).await;
    assert_eq!(me().await, StatusCode::OK);
    assert_eq!(ends().await, recent);

    // Last used more than a day ago: 90 days from now again, for any signed-in request.
    set(now() + SESSION_SECONDS - RENEW_SECONDS - 60).await;
    let (status, _) = send(
        router,
        Some(&user.token),
        "POST",
        "/api/rpc/list_characters",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ends().await >= now() + SESSION_SECONDS - 5);

    // Nearly over and then used: renewed.
    set(now() + 10).await;
    assert_eq!(me().await, StatusCode::OK);
    assert!(ends().await >= now() + SESSION_SECONDS - 5);

    // Over: refused, and not brought back.
    set(now() - 1).await;
    assert_eq!(me().await, StatusCode::UNAUTHORIZED);
    assert!(ends().await < now());
}

fn content_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../content")
}

/// A desktop data folder whose bundles come from upstream's own exporter, as File > Export to bundle makes them.
struct Desktop {
    app: tauri::AppHandle,
    dir: tempfile::TempDir,
}

impl Desktop {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_path_buf();
        let app = tokio::task::spawn_blocking(move || {
            crate::backend::open_workspace(data, content_root(), |_, _| {}).unwrap()
        })
        .await
        .unwrap();
        Self { app, dir }
    }

    async fn run(&self, command: &str, args: Value) -> Value {
        let app = self.app.clone();
        let command = command.to_string();
        tokio::task::spawn_blocking(move || {
            let Value::Object(args) = args else {
                panic!("{args}")
            };
            crate::backend::invoke(&app, &command, args)
                .unwrap_or_else(|| panic!("{command} is offered"))
                .unwrap_or_else(|e| panic!("{command}: {e}"))
        })
        .await
        .unwrap()
    }

    /// A saved character; its id.
    async fn character(&self, name: &str) -> String {
        let made = self.run("create_character", new_character(name)).await;
        let id = made["id"].as_str().unwrap().to_string();
        self.run("save_character", json!({ "characterId": id }))
            .await;
        id
    }

    /// The bytes of a bundle of `ids` with `packs`, named as desktop Borealis names its own (not a server account).
    async fn export(&self, ids: &[&str], packs: borealis_lib::sync_dto::LinkPackChoice) -> Vec<u8> {
        use borealis_lib::commands_content::TauriSink;
        use borealis_lib::commands_sync::TauriSyncEvents;
        use borealis_lib::device_sync::{DeviceSync, SyncConfig};
        let app = self.app.clone();
        let selection = borealis_lib::sync_dto::LinkSelection {
            character_ids: ids.iter().map(|id| (*id).to_string()).collect(),
            packs,
        };
        tokio::task::spawn_blocking(move || {
            let sync = DeviceSync::new(
                Arc::new(app.clone()),
                Arc::new(TauriSyncEvents(TauriSink(app.clone()))),
                SyncConfig {
                    bind_ip: Some(std::net::Ipv4Addr::LOCALHOST.into()),
                    device_name: "Borealis on Desktop".into(),
                    discovery: false,
                },
                || Err("no device transfer in tests".to_string()),
            );
            let built = sync.build_bundle_file(&selection).unwrap();
            std::fs::read(built.file.path()).unwrap()
        })
        .await
        .unwrap()
    }
}

fn required() -> borealis_lib::sync_dto::LinkPackChoice {
    borealis_lib::sync_dto::LinkPackChoice::Required
}

impl Client {
    async fn stage_bundle(&self, name: &str, bytes: &[u8]) -> (StatusCode, Value) {
        use base64::Engine as _;
        self.rpc(
            "stage_bundle",
            json!({
                "fileName": name,
                "contentBase64": base64::engine::general_purpose::STANDARD.encode(bytes),
            }),
        )
        .await
    }

    /// Stage and preview a bundle; the preview.
    async fn open_bundle(&self, name: &str, bytes: &[u8]) -> Value {
        let (status, staged) = self.stage_bundle(name, bytes).await;
        assert_eq!(status, StatusCode::OK, "{staged}");
        let (status, preview) = self
            .rpc("bundle_open", json!({ "token": staged["token"] }))
            .await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        preview
    }

    async fn accept_bundle(&self, preview: &Value) -> Value {
        let (status, summary) = self
            .rpc("bundle_accept", json!({ "bundleId": preview["bundleId"] }))
            .await;
        assert_eq!(status, StatusCode::OK, "{summary}");
        summary
    }
}

fn incoming(data: &Path, account: &str) -> PathBuf {
    data.join("users")
        .join(account)
        .join("sync")
        .join("incoming")
}

/// Every file under the account's incoming folder.
fn staged_files(data: &Path, account: &str) -> Vec<PathBuf> {
    walk(&incoming(data, account))
        .into_iter()
        .filter(|p| p.is_file())
        .collect()
}

fn actions(preview: &Value) -> Vec<(String, String)> {
    preview["preview"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("{preview}"))
        .iter()
        .map(|i| {
            (
                i["kind"].as_str().unwrap().to_string(),
                i["action"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn one(kind: &str, action: &str) -> Vec<(String, String)> {
    vec![(kind.to_string(), action.to_string())]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bundle_is_imported_into_its_own_account_only() {
    let dir = tempfile::tempdir().unwrap();
    bundle_flow(router(dir.path().to_path_buf()).await, dir.path()).await;
}

/// Bundles from upstream's exporter, staged, previewed and applied in one account, with another account looking on.
async fn bundle_flow(router: Router, data: &Path) {
    let desktop = Desktop::new().await;
    let id = desktop.character("Umtar Rajul").await;
    let bundle = desktop.export(&[&id], required()).await;
    let alice = sign_up(&router, "bundle-alice").await;
    let bob = sign_up(&router, "bundle-bob").await;
    bob.rpc("list_characters", json!({})).await;
    // Its search index is still being written, so look for what a bundle would leave rather than for any change.
    let bob_untouched = || {
        let left: Vec<PathBuf> = walk(&data.join("users").join(&bob.id))
            .into_iter()
            .filter(|p| {
                let name = p.file_name().unwrap().to_string_lossy();
                name.ends_with(".borealis")
                    || name == format!("{id}.json")
                    || name == "party.xml"
                    || name == "sync"
            })
            .collect();
        assert!(left.is_empty(), "{left:?}");
    };

    // Staging answers a token and the name, never the path, and keeps one file in the account's own folder.
    let (status, staged) = alice.stage_bundle("UMTAR RAJUL.borealis", &bundle).await;
    assert_eq!(status, StatusCode::OK, "{staged}");
    assert_eq!(staged["name"], "UMTAR RAJUL.borealis");
    assert!(staged["token"].is_string(), "{staged}");
    assert!(staged.get("path").is_none(), "{staged}");
    let files = staged_files(data, &alice.id);
    assert_eq!(files.len(), 1, "{files:?}");
    let folder = files[0].parent().unwrap();
    assert_eq!(folder.parent().unwrap(), incoming(data, &alice.id));
    assert!(folder
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("open-"));
    let token = json!({ "token": staged["token"] });

    // Another account cannot use the token, and its use leaves the file alone.
    let (status, out) = bob.rpc("bundle_open", token.clone()).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    assert!(
        out["message"]
            .as_str()
            .unwrap()
            .contains("expired or is not valid"),
        "{out}"
    );
    assert_eq!(staged_files(data, &alice.id), files);

    // The preview, once per token.
    let (status, preview) = alice.rpc("bundle_open", token.clone()).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(actions(&preview), one("character", "new"), "{preview}");
    assert_eq!(preview["fileName"], "UMTAR RAJUL.borealis");
    let (status, out) = alice.rpc("bundle_open", token).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    let opened = json!({ "bundleId": preview["bundleId"] });

    // Nor can another account accept or close the preview.
    let (status, out) = bob.rpc("bundle_accept", opened.clone()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
    let (status, out) = bob.rpc("bundle_discard", opened.clone()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(staged_files(data, &alice.id), files);

    // Accept adds the character to this account, and the upload is gone.
    let summary = alice.accept_bundle(&preview).await;
    assert_eq!(summary["added"], 1, "{summary}");
    let (_, list) = alice.rpc("list_characters", json!({})).await;
    assert_eq!(names(&list), ["Umtar Rajul"], "{list}");
    assert!(character_file(data, &alice.id, &id).is_file());
    assert!(staged_files(data, &alice.id).is_empty());
    let (status, out) = alice.rpc("bundle_accept", opened).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "accepted once: {out}");
    let (status, sheet) = alice
        .rpc("load_character", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK, "{sheet}");
    alice
        .rpc("close_character", json!({ "characterId": id }))
        .await;

    // The other account has nothing new.
    let (_, list) = bob.rpc("list_characters", json!({})).await;
    assert!(names(&list).is_empty(), "{list}");
    bob_untouched();

    // The same bundle again is identical; a newer edit replaces the copy and keeps the old one in the sync trash.
    let preview = alice.open_bundle("again.borealis", &bundle).await;
    assert_eq!(actions(&preview), one("character", "identical"));
    assert_eq!(alice.accept_bundle(&preview).await["identical"], 1);
    desktop
        .run("set_xp", json!({ "characterId": id, "xp": 300 }))
        .await;
    desktop
        .run("save_character", json!({ "characterId": id }))
        .await;
    // Edits are compared by file time: make this one clearly later.
    let file = desktop
        .dir
        .path()
        .join("characters")
        .join(format!("{id}.json"));
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    let newer = desktop.export(&[&id], required()).await;
    let preview = alice.open_bundle("newer.borealis", &newer).await;
    assert_eq!(actions(&preview), one("character", "replace"), "{preview}");
    assert_eq!(alice.accept_bundle(&preview).await["replaced"], 1);
    let (_, sheet) = alice
        .rpc("load_character", json!({ "characterId": id }))
        .await;
    assert_eq!(sheet["level"]["xp"], 300, "{}", sheet["level"]);
    let trash = data
        .join("users")
        .join(&alice.id)
        .join("sync")
        .join("trash");
    assert!(
        walk(&trash).iter().any(|p| p.is_file()),
        "the old copy is kept: {:?}",
        walk(&trash)
    );

    // Discarding, or a file that is no bundle, leaves nothing behind.
    let preview = alice.open_bundle("closed.borealis", &bundle).await;
    let (status, out) = alice
        .rpc("bundle_discard", json!({ "bundleId": preview["bundleId"] }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert!(staged_files(data, &alice.id).is_empty());
    let (status, staged) = alice.stage_bundle("fake.borealis", b"not a bundle").await;
    assert_eq!(status, StatusCode::OK, "{staged}");
    let (status, out) = alice
        .rpc("bundle_open", json!({ "token": staged["token"] }))
        .await;
    assert!(!status.is_success(), "{out}");
    assert!(staged_files(data, &alice.id).is_empty());
    let (status, out) = alice
        .rpc(
            "stage_bundle",
            json!({ "fileName": "x.borealis", "contentBase64": "not base64!" }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    assert!(staged_files(data, &alice.id).is_empty());

    // A bundle with a content pack adds the pack to this account's content only.
    desktop.run("import_pack", spell_pack()).await;
    let with_pack = desktop
        .export(&[&id], borealis_lib::sync_dto::LinkPackChoice::All)
        .await;
    let preview = alice.open_bundle("with pack.borealis", &with_pack).await;
    assert!(
        actions(&preview).contains(&("pack".to_string(), "new".to_string())),
        "{preview}"
    );
    alice.accept_bundle(&preview).await;
    let spark = json!({ "elementId": "ID_PARTY_SPARK" });
    let (status, out) = alice.rpc("get_element", spark.clone()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, _) = bob.rpc("get_element", spark).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    bob_untouched();
}

#[tokio::test(flavor = "multi_thread")]
async fn uploaded_bundles_stay_few_and_inside_their_folder() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let router = router(data.to_path_buf()).await;
    let alice = sign_up(&router, "names").await;

    // Whatever name comes, the file is one name inside one fresh folder.
    for (sent, kept) in [
        ("..\\..\\x", "x.borealis"),
        ("a/b.borealis", "b.borealis"),
        ("con", "con.borealis"),
        ("", "bundle.borealis"),
        ("../..", "bundle.borealis"),
        ("Mira: \"the bold\".BOREALIS", "Mira_ _the bold_.borealis"),
    ] {
        let (status, staged) = alice.stage_bundle(sent, b"PK").await;
        assert_eq!(status, StatusCode::OK, "{staged}");
        assert_eq!(staged["name"], kept, "{sent}");
    }
    let files = staged_files(data, &alice.id);
    assert_eq!(files.len(), 6, "{files:?}");
    for file in &files {
        let folder = file.parent().unwrap();
        assert_eq!(
            folder.parent().unwrap(),
            incoming(data, &alice.id),
            "{file:?}"
        );
    }

    // At most eight wait at once: the oldest go.
    let mut last = Value::Null;
    for n in 0..5 {
        let (status, staged) = alice.stage_bundle(&format!("{n}.borealis"), b"PK").await;
        assert_eq!(status, StatusCode::OK, "{staged}");
        last = staged;
    }
    assert_eq!(staged_files(data, &alice.id).len(), 8);
    assert!(
        staged_files(data, &alice.id)
            .iter()
            .any(|p| p.ends_with("4.borealis")),
        "the newest stays: {last}"
    );

    // Uploads from before the workspace opened have no token any more, and are swept when it opens.
    let later = sign_up(&router, "names-later").await;
    let stale = incoming(data, &later.id).join("open-stale");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("a.borealis"), b"PK").unwrap();
    let kept = incoming(data, &later.id).join("other");
    std::fs::create_dir_all(&kept).unwrap();
    let (status, _) = later.rpc("list_characters", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!stale.exists());
    assert!(kept.exists(), "only upload folders are swept");

    // Staging needs a session.
    let (status, _) = send(
        &router,
        None,
        "POST",
        "/api/rpc/stage_bundle",
        json!({ "fileName": "a.borealis", "contentBase64": "UEsD" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bundle_larger_than_the_upload_limit_is_refused_unwritten() {
    let dir = tempfile::tempdir().unwrap();
    let router = router_with(
        dir.path().to_path_buf(),
        Db::open(&Location::Memory).await.unwrap(),
        Setup {
            max_upload: 1024,
            ..Setup::default()
        },
    );
    let alice = sign_up(&router, "big").await;
    let (status, _) = alice.stage_bundle("big.borealis", &[7u8; 4096]).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(staged_files(dir.path(), &alice.id).is_empty());
}

/// A real desktop bundle, when `BOREALIS_TEST_BUNDLE` names one: everything in it is new to a fresh account.
#[tokio::test(flavor = "multi_thread")]
async fn a_real_desktop_bundle_imports() {
    let Ok(file) = std::env::var("BOREALIS_TEST_BUNDLE") else {
        eprintln!("skipped: set BOREALIS_TEST_BUNDLE to a .borealis file");
        return;
    };
    let bytes = std::fs::read(&file).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let alice = sign_up(&router, "real").await;
    let preview = alice.open_bundle("real.borealis", &bytes).await;
    let rows = actions(&preview);
    assert!(rows.iter().all(|(_, action)| action == "new"), "{preview}");
    let characters = rows.iter().filter(|(kind, _)| kind == "character").count();
    let packs = rows.iter().filter(|(kind, _)| kind == "pack").count();
    eprintln!("real bundle: {characters} characters, {packs} packs");
    let summary = alice.accept_bundle(&preview).await;
    assert_eq!(summary["added"], rows.len(), "{summary}");
    let (_, list) = alice.rpc("list_characters", json!({})).await;
    assert_eq!(names(&list).len(), characters, "{list}");
    assert!(staged_files(dir.path(), &alice.id).is_empty());
}

#[test]
fn bundle_names_are_one_component() {
    for (sent, kept) in [
        ("Umtar Rajul.borealis", "Umtar Rajul.borealis"),
        ("C:\\Users\\x\\a.borealis", "a.borealis"),
        ("...", "bundle.borealis"),
        (" . x . ", "x.borealis"),
        // Upstream's own copy_name cases (src-tauri/src/device_sync/inbox.rs).
        ("Mira.BOREALIS", "Mira.borealis"),
        ("a/b\\c.borealis", "c.borealis"),
        ("msf:1234", "msf_1234.borealis"),
        ("x\u{0}y\n.borealis", "x_y_.borealis"),
        (".borealis", "bundle.borealis"),
        ("", "bundle.borealis"),
    ] {
        assert_eq!(crate::backend::bundle_file_name(sent), kept, "{sent:?}");
    }
    assert_eq!(
        crate::backend::bundle_file_name(&"x".repeat(500)).len(),
        120 + ".borealis".len()
    );
    // Linux allows 255 bytes per name: a long name in a script with wide characters still fits.
    for wide in ["角", "😀"] {
        let name = crate::backend::bundle_file_name(&wide.repeat(200));
        assert!(name.len() <= 255, "{} bytes", name.len());
        assert!(name.ends_with(".borealis"));
    }
}

/// File > Export to bundle... with no characters and every pack: the account's library as a download, which another
/// account (or desktop Borealis) opens like any other bundle.
#[tokio::test(flavor = "multi_thread")]
async fn an_account_exports_its_content_as_a_bundle() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let router = router(data.to_path_buf()).await;
    let alice = sign_up(&router, "export-alice").await;
    let bob = sign_up(&router, "export-bob").await;
    let (status, out) = alice.rpc("import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::OK, "{out}");

    // A character must be chosen unless every pack goes.
    let (status, out) = alice
        .rpc(
            "bundle_export",
            json!({ "selection": { "characterIds": [], "packs": "required" } }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{out}");

    let (status, file) = alice
        .rpc(
            "bundle_export",
            json!({ "selection": { "characterIds": [], "packs": "all" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["fileName"], "Borealis content.borealis");
    assert_eq!(file["characters"], 0, "{file}");
    assert_eq!(file["packs"], 1, "{file}");
    assert!(file.get("path").is_none(), "{file}");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(file["dataBase64"].as_str().unwrap())
        .unwrap();
    assert_eq!(bytes.len() as u64, file["bytes"].as_u64().unwrap());
    // Nothing is left behind on the server.
    let left: Vec<PathBuf> = walk(&data.join("users").join(&alice.id))
        .into_iter()
        .filter(|p| p.to_string_lossy().ends_with(".borealis"))
        .collect();
    assert!(left.is_empty(), "{left:?}");

    let preview = bob
        .open_bundle(file["fileName"].as_str().unwrap(), &bytes)
        .await;
    assert_eq!(actions(&preview), one("pack", "new"), "{preview}");
    bob.accept_bundle(&preview).await;
    let (status, out) = bob
        .rpc("get_element", json!({ "elementId": "ID_PARTY_SPARK" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

/// A bundle's packs from an index bring that index along: the account that accepts them follows it, so they keep
/// updating instead of staying a snapshot. Packs the bundle leaves as they were add nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_bundle_brings_the_indexes_its_packs_came_from() {
    use base64::Engine as _;
    const INDEX: &str = "https://example.invalid/homebrew.index";
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path();
    let router = router(data.to_path_buf()).await;
    let alice = sign_up(&router, "index-alice").await;
    let bob = sign_up(&router, "index-bob").await;
    let (status, imported) = alice.rpc("import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    let pack = imported["pack"]["id"].as_str().unwrap().to_string();
    // As an index download writes it (content_index.rs): the pack records the index it came from.
    let meta = data
        .join("users")
        .join(&alice.id)
        .join("library")
        .join(&pack)
        .join("pack-meta.json");
    let mut written: Value =
        serde_json::from_str(&std::fs::read_to_string(&meta).unwrap()).unwrap();
    written["indexUrl"] = json!(INDEX);
    written["sha256"] = json!("00");
    std::fs::write(&meta, written.to_string()).unwrap();

    let (status, file) = alice
        .rpc(
            "bundle_export",
            json!({ "selection": { "characterIds": [], "packs": "all" } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(file["dataBase64"].as_str().unwrap())
        .unwrap();
    let indexes = |list: &Value| -> Vec<String> {
        list.as_array()
            .unwrap_or_else(|| panic!("{list}"))
            .iter()
            .map(|i| i["url"].as_str().unwrap().to_string())
            .collect()
    };
    let (_, list) = bob.rpc("list_indexes", json!({})).await;
    assert!(indexes(&list).is_empty(), "{list}");

    // Accepted: the pack, and the index it came from.
    let preview = bob.open_bundle("content.borealis", &bytes).await;
    assert_eq!(actions(&preview), one("pack", "new"), "{preview}");
    bob.accept_bundle(&preview).await;
    let (status, list) = bob.rpc("list_indexes", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(indexes(&list), [INDEX], "{list}");
    let (status, out) = bob
        .rpc("get_element", json!({ "elementId": "ID_PARTY_SPARK" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");

    // Removed on purpose, it stays removed when the same bundle comes again (its pack is identical).
    let (status, list) = bob.rpc("remove_index", json!({ "url": INDEX })).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert!(indexes(&list).is_empty(), "{list}");
    let preview = bob.open_bundle("again.borealis", &bytes).await;
    assert_eq!(actions(&preview), one("pack", "identical"), "{preview}");
    bob.accept_bundle(&preview).await;
    let (_, list) = bob.rpc("list_indexes", json!({})).await;
    assert!(indexes(&list).is_empty(), "{list}");

    // A homebrew pack (no index) brings none.
    let carol = sign_up(&router, "index-carol").await;
    let (status, plain) = carol.rpc("import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::OK, "{plain}");
    let (_, file) = carol
        .rpc(
            "bundle_export",
            json!({ "selection": { "characterIds": [], "packs": "all" } }),
        )
        .await;
    let plain_bytes = base64::engine::general_purpose::STANDARD
        .decode(file["dataBase64"].as_str().unwrap())
        .unwrap();
    let dave = sign_up(&router, "index-dave").await;
    let preview = dave.open_bundle("plain.borealis", &plain_bytes).await;
    dave.accept_bundle(&preview).await;
    let (_, list) = dave.rpc("list_indexes", json!({})).await;
    assert!(indexes(&list).is_empty(), "{list}");
}

impl Client {
    /// This account's bundle of `ids` with `packs` (`required`, `all`), as File > Export to bundle downloads it.
    async fn export_bundle(&self, ids: &[&str], packs: &str) -> Vec<u8> {
        use base64::Engine as _;
        let (status, file) = self
            .rpc(
                "bundle_export",
                json!({ "selection": { "characterIds": ids, "packs": packs } }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{file}");
        base64::engine::general_purpose::STANDARD
            .decode(file["dataBase64"].as_str().unwrap())
            .unwrap()
    }

    /// Set a character's XP and save it.
    async fn set_xp(&self, id: &str, xp: u32) {
        let character = json!({ "characterId": id });
        let (status, out) = self.rpc("load_character", character.clone()).await;
        assert_eq!(status, StatusCode::OK, "{out}");
        let (status, out) = self
            .rpc("set_xp", json!({ "characterId": id, "xp": xp }))
            .await;
        assert_eq!(status, StatusCode::OK, "{out}");
        let (status, out) = self.rpc("save_character", character.clone()).await;
        assert_eq!(status, StatusCode::OK, "{out}");
        self.rpc("close_character", character).await;
    }

    async fn xp(&self, id: &str) -> Value {
        let character = json!({ "characterId": id });
        let (status, sheet) = self.rpc("load_character", character.clone()).await;
        assert_eq!(status, StatusCode::OK, "{sheet}");
        self.rpc("close_character", character).await;
        sheet["level"]["xp"].clone()
    }
}

/// Make the file's edit clearly later than anything else, as edits are compared by file time.
fn edited_later(file: &Path, secs: u64) {
    std::fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(secs))
        .unwrap();
}

fn labels(preview: &Value) -> Vec<String> {
    preview["preview"]["items"]
        .as_array()
        .unwrap_or_else(|| panic!("{preview}"))
        .iter()
        .map(|i| i["label"].as_str().unwrap().to_string())
        .collect()
}

/// Every file under the account's sync trash: what an accepted bundle replaced.
fn trashed(data: &Path, account: &str) -> Vec<PathBuf> {
    walk(&data.join("users").join(account).join("sync").join("trash"))
        .into_iter()
        .filter(|p| p.is_file())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn another_players_bundle_never_replaces_what_you_have() {
    let dir = tempfile::tempdir().unwrap();
    receive_flow(router(dir.path().to_path_buf()).await, dir.path()).await;
}

/// Bundles from another player: new items come in as the recipient's own, identical ones change nothing, and a pack or
/// character the recipient has with different content is refused, never replaced or renamed.
async fn receive_flow(router: Router, data: &Path) {
    let alice = sign_up(&router, "receive-alice").await;
    let bob = sign_up(&router, "receive-bob").await;
    let (status, hero) = alice.rpc("create_character", new_character("Hero")).await;
    assert_eq!(status, StatusCode::OK, "{hero}");
    let id = hero["id"].as_str().unwrap().to_string();
    alice
        .rpc("save_character", json!({ "characterId": id }))
        .await;
    alice
        .rpc("close_character", json!({ "characterId": id }))
        .await;
    let bundle = alice.export_bundle(&[&id], "required").await;

    // New: it comes in, as the recipient's own copy to edit.
    let preview = bob.open_bundle("hero.borealis", &bundle).await;
    assert_eq!(preview["sourceDevice"], "another Borealis Party account");
    assert_eq!(actions(&preview), one("character", "new"), "{preview}");
    assert_eq!(bob.accept_bundle(&preview).await["added"], 1);
    // The same again is identical.
    let preview = bob.open_bundle("again.borealis", &bundle).await;
    assert_eq!(
        actions(&preview),
        one("character", "identical"),
        "{preview}"
    );
    assert_eq!(labels(&preview), ["Hero"]);
    let summary = bob.accept_bundle(&preview).await;
    assert_eq!(summary["identical"], 1, "{summary}");
    bob.set_xp(&id, 300).await;
    assert_eq!(bob.xp(&id).await, 300);
    let bobs = std::fs::read(character_file(data, &bob.id, &id)).unwrap();

    // Alice edits hers later. Between one account's devices the newer copy would win; from another player it is
    // refused.
    alice.set_xp(&id, 900).await;
    edited_later(&character_file(data, &alice.id, &id), 60);
    let newer = alice.export_bundle(&[&id], "required").await;
    let preview = bob.open_bundle("newer.borealis", &newer).await;
    assert_eq!(
        actions(&preview),
        one("character", "keep_local"),
        "{preview}"
    );
    assert_eq!(
        labels(&preview),
        ["You already have a different copy of Hero; it was not replaced. To take this one, delete yours first."]
    );
    let summary = bob.accept_bundle(&preview).await;
    assert_eq!(
        summary,
        json!({ "added": 0, "replaced": 0, "keptLocal": 1, "identical": 0 })
    );
    assert_eq!(
        std::fs::read(character_file(data, &bob.id, &id)).unwrap(),
        bobs
    );
    assert_eq!(bob.xp(&id).await, 300);
    assert!(
        trashed(data, &bob.id).is_empty(),
        "{:?}",
        trashed(data, &bob.id)
    );
    assert!(staged_files(data, &bob.id).is_empty());

    // A bundle with more: the new character and pack come in, the clashing character stays out.
    let (status, imported) = alice.rpc("import_pack", spell_pack()).await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    let pack = imported["pack"]["id"].as_str().unwrap().to_string();
    let (_, second) = alice.rpc("create_character", new_character("Second")).await;
    let second = second["id"].as_str().unwrap().to_string();
    alice
        .rpc("save_character", json!({ "characterId": second }))
        .await;
    let mixed = alice.export_bundle(&[&id, &second], "all").await;
    let preview = bob.open_bundle("mixed.borealis", &mixed).await;
    let mut rows = actions(&preview);
    rows.sort();
    assert_eq!(
        rows,
        [
            ("character".to_string(), "keep_local".to_string()),
            ("character".to_string(), "new".to_string()),
            ("pack".to_string(), "new".to_string()),
        ],
        "{preview}"
    );
    let summary = bob.accept_bundle(&preview).await;
    assert_eq!(
        summary,
        json!({ "added": 2, "replaced": 0, "keptLocal": 1, "identical": 0 })
    );
    let (_, list) = bob.rpc("list_characters", json!({})).await;
    let mut got = names(&list);
    got.sort_unstable();
    assert_eq!(got, ["Hero", "Second"], "{list}");
    assert_eq!(bob.xp(&id).await, 300);
    let (status, out) = bob
        .rpc("get_element", json!({ "elementId": "ID_PARTY_SPARK" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert!(trashed(data, &bob.id).is_empty());
    assert!(staged_files(data, &bob.id).is_empty());

    // A pack with the same id and different content is refused as well.
    let library = |account: &str| data.join("users").join(account).join("library").join(&pack);
    let input = |account: &str| {
        walk(&library(account))
            .into_iter()
            .find(|p| p.ends_with("input.json"))
            .unwrap_or_else(|| panic!("{:?}", walk(&library(account))))
    };
    let alices = input(&alice.id);
    let changed = std::fs::read_to_string(&alices).unwrap();
    assert!(changed.contains("Instantaneous"), "{changed}");
    let changed = changed.replace("Instantaneous", "1 round");
    std::fs::write(&alices, changed).unwrap();
    edited_later(&alices, 120);
    let bobs_pack = std::fs::read(input(&bob.id)).unwrap();
    let packs = alice.export_bundle(&[], "all").await;
    let preview = bob.open_bundle("packs.borealis", &packs).await;
    assert_eq!(actions(&preview), one("pack", "keep_local"), "{preview}");
    let label = &labels(&preview)[0];
    assert!(
        label.starts_with("You already have a pack called ")
            && label.ends_with("; it was not replaced. Rename one before sharing."),
        "{label}"
    );
    let summary = bob.accept_bundle(&preview).await;
    assert_eq!(summary["keptLocal"], 1, "{summary}");
    assert_eq!(std::fs::read(input(&bob.id)).unwrap(), bobs_pack);
    assert!(trashed(data, &bob.id).is_empty());
    assert!(staged_files(data, &bob.id).is_empty());

    // An account's own bundle is named as such and follows upstream's rule.
    let own = alice.export_bundle(&[&id], "required").await;
    let preview = alice.open_bundle("own.borealis", &own).await;
    assert_eq!(preview["sourceDevice"], "this account");
    assert_eq!(
        actions(&preview),
        one("character", "identical"),
        "{preview}"
    );
    alice.accept_bundle(&preview).await;

    // A bundle from elsewhere that the user says came from another player is checked too.
    let desktop = Desktop::new().await;
    let other = desktop.character("Wanderer").await;
    let from_desktop = desktop.export(&[&other], required()).await;
    let preview = bob.open_bundle("desktop.borealis", &from_desktop).await;
    assert_eq!(preview["sourceDevice"], "Borealis on Desktop");
    assert_eq!(bob.accept_bundle(&preview).await["added"], 1);
    desktop
        .run("set_xp", json!({ "characterId": other, "xp": 300 }))
        .await;
    desktop
        .run("save_character", json!({ "characterId": other }))
        .await;
    edited_later(
        &desktop
            .dir
            .path()
            .join("characters")
            .join(format!("{other}.json")),
        60,
    );
    let later = desktop.export(&[&other], required()).await;
    let before = std::fs::read(character_file(data, &bob.id, &other)).unwrap();
    let preview = bob.open_bundle("desktop again.borealis", &later).await;
    assert_eq!(actions(&preview), one("character", "replace"), "{preview}");
    let (status, summary) = bob
        .rpc(
            "bundle_accept",
            json!({ "bundleId": preview["bundleId"], "fromAnotherPlayer": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(
        summary,
        json!({ "added": 0, "replaced": 0, "keptLocal": 1, "identical": 0 })
    );
    assert_eq!(
        std::fs::read(character_file(data, &bob.id, &other)).unwrap(),
        before
    );
    assert!(trashed(data, &bob.id).is_empty());
    assert!(staged_files(data, &bob.id).is_empty());
}
