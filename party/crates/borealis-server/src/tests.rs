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
use crate::db::{Db, Location, OpenError};
use crate::mail::{Mailer, Recorder};
use crate::workspaces::Workspaces;
use crate::{api_routes, AppState, Config};

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
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            require_approval: false,
            mailer: None,
            signups_allowed: 1000,
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
        }),
        login_limiter: RateLimiter::logins(),
        signup_limiter: RateLimiter::new(setup.signups_allowed, Duration::from_secs(3600)),
        mailer,
        accounts: Arc::new(tokio::sync::Mutex::new(())),
        started: std::time::Instant::now(),
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

fn refused(opened: Result<Db, OpenError>) {
    match opened {
        Err(error @ OpenError::OldLayout) => {
            assert!(error.to_string().contains("fresh config folder"), "{error}")
        }
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("an old database was opened"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_old_database_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("old.db");
    make_old(&format!("sqlite://{}?mode=rwc", file.display())).await;
    refused(Db::open(&Location::Sqlite(file.clone())).await);
    // Nor is it copied into a new PostgreSQL database.
    let new = Db::open(&Location::Memory).await.unwrap();
    assert!(matches!(
        crate::migrate::adopt_sqlite(&new, &file).await,
        Err(crate::migrate::MigrateError::Source(OpenError::OldLayout))
    ));
    assert_eq!(new.user_count().await.unwrap(), 0);
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
    make_old(&url).await;
    refused(Db::open(&Location::Postgres(url.clone())).await);
    drop_all(&url).await;
    let db = Db::open(&Location::Postgres(url.clone())).await.unwrap();
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
