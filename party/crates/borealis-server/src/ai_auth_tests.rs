//! AI app auth end to end (0.3.1 part A, `tokens.rs` and `oauth.rs`): personal tokens (create, use, expiry, revoke,
//! hashed at rest), the OAuth flow with PKCE and its failures (wrong verifier, reused code, redirect mismatch, revoked
//! or reused refresh), dynamic client registration, the metadata, scopes over the role matrix (a player's token never
//! sees hidden monsters or DM data), rate limits, and the upgrade from the 0.3.0 schema. Runs on SQLite, and on
//! PostgreSQL too when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::extract::{Path as UrlPath, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::Engine as _;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tower::ServiceExt;

use super::{drop_all, new_character, send, sign_up, state_with, Client, Setup, POSTGRES};
use crate::db::{Db, Location};
use crate::tokens::{Scope, TokenAuth};
use crate::{api_routes, AppState, Config};

const BASE: &str = "https://party.example.com";
const MCP: &str = "https://party.example.com/api/mcp";
const CALLBACK: &str = "https://claude.ai/api/mcp/auth_callback";
const OWLBEAR: &str = "ID_SRD521_MONSTER_OWLBEAR";
const GOBLIN: &str = "ID_SRD521_MONSTER_GOBLIN_WARRIOR";

/// Each router comes from its own client address, so the per-address limits of one test don't reach another's.
static NEXT_IP: AtomicU8 = AtomicU8::new(1);

// A stand-in for the MCP server (part B): routes that take `TokenAuth`, check a scope, and call the session
// handlers with the token's account.

async fn whoami(auth: TokenAuth) -> Response {
    axum::Json(json!({
        "username": auth.account.username,
        "scopes": auth.scope_names(),
        "id": auth.client.id(),
        "client": auth.client.label(),
    }))
    .into_response()
}

async fn encounter(
    State(state): State<AppState>,
    auth: TokenAuth,
    UrlPath(id): UrlPath<String>,
) -> Response {
    if let Err(refusal) = auth.require(Scope::EncountersRead) {
        return refusal.into_response();
    }
    crate::encounters::read(State(state), auth.account, UrlPath(id))
        .await
        .into_response()
}

async fn encounters(
    State(state): State<AppState>,
    auth: TokenAuth,
    UrlPath(party): UrlPath<String>,
) -> Response {
    if let Err(refusal) = auth.require(Scope::EncountersRead) {
        return refusal.into_response();
    }
    crate::encounters::list(State(state), auth.account, UrlPath(party))
        .await
        .into_response()
}

fn ai_state(data: PathBuf, db: Db) -> AppState {
    let mut state = state_with(data, db, Setup::default());
    state.config = Arc::new(Config {
        open_registration: true,
        require_approval: false,
        public_url: Some(BASE.to_string()),
        app_download: None,
        trusted_proxies: Vec::new(),
    });
    state
}

fn ai_router(state: AppState) -> Router {
    let ip = NEXT_IP.fetch_add(1, Ordering::Relaxed);
    let mcp = Router::new()
        .route("/mcp-test/whoami", get(whoami))
        .route("/mcp-test/encounters/{id}", get(encounter))
        .route("/mcp-test/parties/{party}/encounters", get(encounters))
        .with_state(state.clone());
    api_routes(state, 64 * 1024 * 1024)
        .merge(mcp)
        .layer(MockConnectInfo(std::net::SocketAddr::from((
            [10, 77, 0, ip],
            9,
        ))))
}

struct Raw {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

async fn raw(router: &Router, request: Request<Body>) -> Raw {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Raw {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

async fn bearer_get(router: &Router, token: &str, path: &str) -> Raw {
    raw(
        router,
        Request::get(path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn who(router: &Router, token: &str) -> StatusCode {
    bearer_get(router, token, "/mcp-test/whoami").await.status
}

async fn get_plain(router: &Router, path: &str) -> Raw {
    raw(router, Request::get(path).body(Body::empty()).unwrap()).await
}

async fn form(
    router: &Router,
    path: &str,
    pairs: &[(&str, &str)],
    basic: Option<(&str, &str)>,
) -> Raw {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    let mut request =
        Request::post(path).header("content-type", "application/x-www-form-urlencoded");
    if let Some((id, secret)) = basic {
        let b64 = base64::engine::general_purpose::STANDARD.encode(format!("{id}:{secret}"));
        request = request.header("authorization", format!("Basic {b64}"));
    }
    raw(router, request.body(Body::from(body)).unwrap()).await
}

async fn post_json(router: &Router, path: &str, body: Value) -> Raw {
    raw(
        router,
        Request::post(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

fn location(r: &Raw) -> String {
    r.headers
        .get(header::LOCATION)
        .unwrap_or_else(|| panic!("no Location: {} {}", r.status, r.body))
        .to_str()
        .unwrap()
        .to_string()
}

fn query_of(url: &str) -> std::collections::HashMap<String, String> {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn pkce() -> (String, String) {
    let verifier = crate::tokens::new_secret("v-");
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn text(v: &Value) -> String {
    v.as_str()
        .unwrap_or_else(|| panic!("not text: {v}"))
        .to_string()
}

/// Every text value in the AI tables; none may hold a secret.
async fn stored_text(db: &Db) -> Vec<String> {
    let mut out = Vec::new();
    for (table, columns) in crate::migrate::tables() {
        if !(table.starts_with("oauth_") || table == "api_tokens") {
            continue;
        }
        for (column, kind) in columns {
            if kind != "text" {
                continue;
            }
            let rows = sqlx::query(&format!("SELECT {column} FROM {table}"))
                .fetch_all(db.pool())
                .await
                .unwrap();
            for row in rows {
                if let Some(v) = row.try_get::<Option<String>, _>(0).unwrap() {
                    out.push(v);
                }
            }
        }
    }
    out
}

fn assert_not_stored(stored: &[String], secret: &str) {
    assert!(
        stored.iter().all(|v| !v.contains(secret)),
        "a secret is stored in plain text"
    );
}

// Party helpers (as in encounters_tests).

async fn character(client: &Client, name: &str) -> String {
    let (status, out) = client.rpc("create_character", new_character(name)).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let id = text(&out["id"]);
    let (status, out) = client
        .rpc("save_character", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    id
}

async fn ok(client: &Client, method: &str, path: &str, body: Value) -> Value {
    let (status, out) = send(&client.router, Some(&client.token), method, path, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {out}");
    out
}

struct Table {
    dm: Client,
    player: Client,
    party: String,
    shown: String,
    draft: String,
}

/// A DM with a party, a player in it, an encounter shown to players with a hidden goblin (DM label and notes
/// secret) and a revealed owlbear, and a draft.
async fn table(router: &Router, tag: &str) -> Table {
    let dm = sign_up(router, &format!("dm-{tag}")).await;
    let player = sign_up(router, &format!("pl-{tag}")).await;
    ok(&dm, "POST", "/api/me/roles/dm", json!({})).await;
    let party = text(&ok(&dm, "POST", "/api/parties", json!({ "name": "Tuesday" })).await["id"]);
    let c = character(&player, "Hero").await;
    ok(
        &dm,
        "POST",
        &format!("/api/parties/{party}/invites"),
        json!({ "kind": "username", "username": format!("pl-{tag}") }),
    )
    .await;
    let invites = ok(&player, "GET", "/api/invites", Value::Null).await;
    let invite = text(&invites["invites"][0]["id"]);
    ok(
        &player,
        "POST",
        &format!("/api/invites/{invite}/accept"),
        json!({ "characterId": c }),
    )
    .await;
    let draft = text(
        &ok(
            &dm,
            "POST",
            &format!("/api/parties/{party}/encounters"),
            json!({ "name": "SECRET-DRAFT" }),
        )
        .await["id"],
    );
    let shown = text(
        &ok(
            &dm,
            "POST",
            &format!("/api/parties/{party}/encounters"),
            json!({ "name": "SECRET-NAME", "publicName": "Swamp ambush", "dmNotes": "SECRET-NOTES" }),
        )
        .await["id"],
    );
    ok(
        &dm,
        "POST",
        &format!("/api/encounters/{shown}/creatures"),
        json!({ "elementId": GOBLIN, "dmLabel": "SECRET-LABEL boss" }),
    )
    .await;
    ok(
        &dm,
        "POST",
        &format!("/api/encounters/{shown}/creatures"),
        json!({ "elementId": OWLBEAR, "publicName": "Big beast", "hidden": false }),
    )
    .await;
    ok(
        &dm,
        "PATCH",
        &format!("/api/encounters/{shown}"),
        json!({ "status": "ready", "playerVisible": true }),
    )
    .await;
    Table {
        dm,
        player,
        party,
        shown,
        draft,
    }
}

async fn new_token(client: &Client, body: Value) -> Value {
    ok(client, "POST", "/api/me/ai/tokens", body).await
}

async fn new_secret_token(client: &Client, body: Value) -> String {
    text(&new_token(client, body).await["token"])
}

async fn set(db: &Db, sql: &str, value: Option<&str>) {
    let mut q = sqlx::query(sql);
    q = match value {
        Some(v) => q.bind(v.to_string()),
        None => q.bind(crate::db::now() - 1),
    };
    q.execute(db.pool()).await.unwrap();
}

// Personal tokens.

async fn personal_tokens(router: &Router, db: &Db) {
    let me = sign_up(router, "token-owner").await;
    let info = ok(&me, "GET", "/api/me/ai", Value::Null).await;
    assert_eq!(info["mcpUrl"], MCP);
    assert_eq!(info["scopes"].as_array().unwrap().len(), 7);

    // Defaults: every scope (there is none for live combat or publishing: D25, D47); 90 days.
    let made = new_token(&me, json!({ "name": "Claude Code" })).await;
    let secret = text(&made["token"]);
    assert!(secret.starts_with("bpt_") && secret.len() >= 4 + 43);
    let scopes: Vec<&str> = made["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        scopes.contains(&"encounters:simulate") && !scopes.iter().any(|s| s.starts_with("combat:"))
    );
    assert_eq!(scopes.len(), 7);
    let days = (made["expiresAt"].as_i64().unwrap() - made["createdAt"].as_i64().unwrap()) / 86_400;
    assert_eq!(days, 90);

    // Shown once: the list never has it.
    let list = ok(&me, "GET", "/api/me/ai/tokens", Value::Null).await;
    assert_eq!(list["tokens"].as_array().unwrap().len(), 1);
    assert!(!list.to_string().contains(&secret));
    assert_eq!(list["tokens"][0]["lastUsedAt"], Value::Null);

    // Use.
    let r = bearer_get(router, &secret, "/mcp-test/whoami").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["username"], "token-owner");
    let list = ok(&me, "GET", "/api/me/ai/tokens", Value::Null).await;
    assert!(list["tokens"][0]["lastUsedAt"].is_i64());

    // A token is not a session, and a session is not a token.
    for (method, path) in [
        ("GET", "/api/me"),
        ("POST", "/api/rpc/list_characters"),
        ("GET", "/api/parties"),
        ("GET", "/api/me/ai/tokens"),
    ] {
        let (status, _) = send(router, Some(&secret), method, path, json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
    let r = bearer_get(router, &me.token, "/mcp-test/whoami").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let www = r.headers[header::WWW_AUTHENTICATE].to_str().unwrap();
    assert!(
        www.contains("resource_metadata=\"https://party.example.com/.well-known/oauth-protected-resource/api/mcp\""),
        "{www}"
    );
    assert_eq!(
        get_plain(router, "/mcp-test/whoami").await.status,
        StatusCode::UNAUTHORIZED
    );

    // Hashed at rest.
    let stored = stored_text(db).await;
    assert_not_stored(&stored, &secret);
    assert_not_stored(&stored, &secret[4..]);

    // Chosen scopes and expiry; bad ones refused.
    let narrow = new_token(
        &me,
        json!({ "name": "reader", "scopes": ["bestiary:read"], "expiresInDays": 7 }),
    )
    .await;
    assert_eq!(narrow["scopes"], json!(["bestiary:read"]));
    for bad in [
        json!({ "name": "x", "scopes": ["admin"] }),
        json!({ "name": "x", "scopes": [] }),
        json!({ "name": "x", "expiresInDays": 366 }),
        json!({ "name": "x", "expiresInDays": 0 }),
        json!({ "name": "  " }),
    ] {
        let (status, out) = me.post("/api/me/ai/tokens", bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {out}");
    }

    // Expiry.
    let short = new_token(&me, json!({ "name": "short" })).await;
    let short_secret = text(&short["token"]);
    assert_eq!(who(router, &short_secret).await, StatusCode::OK);
    sqlx::query("UPDATE api_tokens SET expires_at = $1 WHERE id = $2")
        .bind(crate::db::now() - 1)
        .bind(text(&short["id"]))
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(who(router, &short_secret).await, StatusCode::UNAUTHORIZED);
    let list = ok(&me, "GET", "/api/me/ai/tokens", Value::Null).await;
    assert_eq!(
        list["tokens"].as_array().unwrap().len(),
        2,
        "expired tokens are not listed"
    );

    // A disabled account's tokens stop at once.
    for (status, want) in [
        ("disabled", StatusCode::UNAUTHORIZED),
        ("active", StatusCode::OK),
    ] {
        sqlx::query("UPDATE users SET status = $1 WHERE id = $2")
            .bind(status)
            .bind(&me.id)
            .execute(db.pool())
            .await
            .unwrap();
        assert_eq!(who(router, &secret).await, want, "{status}");
    }

    // Another account can't revoke it; its owner can, at once.
    let other = sign_up(router, "token-other").await;
    let id = text(&made["id"]);
    let (status, _) = other.delete(&format!("/api/me/ai/tokens/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(who(router, &secret).await, StatusCode::OK);
    ok(
        &me,
        "DELETE",
        &format!("/api/me/ai/tokens/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(who(router, &secret).await, StatusCode::UNAUTHORIZED);
}

// OAuth.

async fn metadata(router: &Router) {
    let r = get_plain(router, "/.well-known/oauth-authorization-server").await;
    assert_eq!(r.status, StatusCode::OK);
    let m = &r.body;
    assert_eq!(m["issuer"], BASE);
    assert_eq!(
        m["authorization_endpoint"],
        "https://party.example.com/api/oauth/authorize"
    );
    assert_eq!(
        m["token_endpoint"],
        "https://party.example.com/api/oauth/token"
    );
    assert_eq!(
        m["registration_endpoint"],
        "https://party.example.com/api/oauth/register"
    );
    assert_eq!(m["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(m["authorization_response_iss_parameter_supported"], true);
    assert!(m["token_endpoint_auth_methods_supported"]
        .as_array()
        .unwrap()
        .contains(&json!("none")));
    for path in [
        "/.well-known/oauth-protected-resource/api/mcp",
        "/.well-known/oauth-protected-resource",
    ] {
        let r = get_plain(router, path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert_eq!(r.body["resource"], MCP);
        assert_eq!(r.body["authorization_servers"], json!([BASE]));
    }
}

struct App {
    id: String,
    secret: Option<String>,
}

async fn register(router: &Router, body: Value) -> App {
    let r = post_json(router, "/api/oauth/register", body).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    App {
        id: text(&r.body["client_id"]),
        secret: r.body["client_secret"].as_str().map(str::to_string),
    }
}

async fn public_app(router: &Router, name: &str, redirect: &str) -> App {
    register(
        router,
        json!({ "client_name": name, "redirect_uris": [redirect], "token_endpoint_auth_method": "none" }),
    )
    .await
}

async fn dcr(router: &Router) {
    // A public client, as Claude registers.
    let r = post_json(
        router,
        "/api/oauth/register",
        json!({ "client_name": "Claude", "redirect_uris": [CALLBACK], "token_endpoint_auth_method": "none",
                "grant_types": ["authorization_code", "refresh_token"], "response_types": ["code"] }),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert!(r.body.get("client_secret").is_none());
    // The RFC default is a confidential client with a secret.
    let r = post_json(
        router,
        "/api/oauth/register",
        json!({ "client_name": "ChatGPT", "redirect_uris": ["https://chatgpt.com/connector_platform_oauth_redirect"] }),
    )
    .await;
    assert_eq!(r.body["token_endpoint_auth_method"], "client_secret_basic");
    assert!(text(&r.body["client_secret"]).starts_with("bps_"));
    for (bad, error) in [
        (json!({ "client_name": "x" }), "invalid_redirect_uri"),
        (json!({ "redirect_uris": [] }), "invalid_redirect_uri"),
        (
            json!({ "redirect_uris": ["javascript:alert(1)"] }),
            "invalid_redirect_uri",
        ),
        (
            json!({ "redirect_uris": ["http://evil.example/cb"] }),
            "invalid_redirect_uri",
        ),
        (
            json!({ "redirect_uris": ["https://a.example/cb#x"] }),
            "invalid_redirect_uri",
        ),
        (
            json!({ "redirect_uris": [CALLBACK], "token_endpoint_auth_method": "private_key_jwt" }),
            "invalid_client_metadata",
        ),
        (
            json!({ "redirect_uris": [CALLBACK], "grant_types": ["client_credentials"] }),
            "invalid_client_metadata",
        ),
        (
            json!({ "redirect_uris": [CALLBACK], "response_types": ["token"] }),
            "invalid_client_metadata",
        ),
        (json!([1, 2]), "invalid_client_metadata"),
    ] {
        let r = post_json(router, "/api/oauth/register", bad.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(r.body["error"], error, "{bad}: {}", r.body);
    }
}

fn authorize_url(client: &str, redirect: &str, challenge: &str, extra: &[(&str, &str)]) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    q.extend_pairs([
        ("response_type", "code"),
        ("client_id", client),
        ("redirect_uri", redirect),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", "xyz-state"),
        ("resource", MCP),
    ]);
    q.extend_pairs(extra);
    format!("/api/oauth/authorize?{}", q.finish())
}

/// Authorize; the consent request id from the redirect to the app.
async fn start(router: &Router, url: &str) -> String {
    let r = get_plain(router, url).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let to = location(&r);
    to.strip_prefix("https://party.example.com/#/oauth/consent/")
        .unwrap_or_else(|| panic!("{to}"))
        .to_string()
}

/// Authorize, then consent as `user` with `allow` (all asked for when `None`); the code from the redirect.
async fn consent(router: &Router, user: &Client, url: &str, allow: Option<Value>) -> String {
    let id = start(router, url).await;
    let detail = ok(
        user,
        "GET",
        &format!("/api/oauth/requests/{id}"),
        Value::Null,
    )
    .await;
    let me = ok(user, "GET", "/api/me", Value::Null).await;
    assert_eq!(detail["account"]["username"], me["username"]);
    let redirect_host = text(&detail["client"]["redirectHost"]);
    assert!(
        redirect_host == "https://claude.ai" || redirect_host == "http://127.0.0.1",
        "{redirect_host}"
    );
    let allow = allow.unwrap_or_else(|| {
        json!(detail["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].clone())
            .collect::<Vec<_>>())
    });
    let out = ok(
        user,
        "POST",
        &format!("/api/oauth/requests/{id}/approve"),
        json!({ "scopes": allow }),
    )
    .await;
    let redirect = text(&out["redirect"]);
    let q = query_of(&redirect);
    assert_eq!(q["state"], "xyz-state");
    assert_eq!(q["iss"], BASE);
    // Answered once only.
    let (status, _) = user
        .post(
            &format!("/api/oauth/requests/{id}/approve"),
            json!({ "scopes": ["bestiary:read"] }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    q["code"].clone()
}

async fn exchange(router: &Router, app: &App, code: &str, verifier: &str) -> Raw {
    form(
        router,
        "/api/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", CALLBACK),
            ("client_id", &app.id),
            ("code_verifier", verifier),
            ("resource", MCP),
        ],
        None,
    )
    .await
}

async fn refresh(router: &Router, app: &App, token: &str) -> Raw {
    form(
        router,
        "/api/oauth/token",
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", token),
            ("client_id", &app.id),
            ("resource", MCP),
        ],
        None,
    )
    .await
}

/// A fresh grant for `user`: its access and refresh tokens.
async fn connect(router: &Router, app: &App, user: &Client) -> (String, String) {
    let (verifier, challenge) = pkce();
    let code = consent(
        router,
        user,
        &authorize_url(&app.id, CALLBACK, &challenge, &[]),
        None,
    )
    .await;
    let r = exchange(router, app, &code, &verifier).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    (
        text(&r.body["access_token"]),
        text(&r.body["refresh_token"]),
    )
}

async fn oauth_flow(router: &Router, db: &Db) {
    let user = sign_up(router, "oauth-user").await;
    let app = public_app(router, "Claude", CALLBACK).await;

    // The whole flow, with a narrowed consent.
    let (verifier, challenge) = pkce();
    let url = authorize_url(
        &app.id,
        CALLBACK,
        &challenge,
        &[("scope", "bestiary:read encounters:read offline_access")],
    );
    let code = consent(router, &user, &url, Some(json!(["encounters:read"]))).await;
    assert!(code.starts_with("bpc_"));
    let r = exchange(router, &app, &code, &verifier).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(r.body["token_type"], "Bearer");
    assert_eq!(r.body["scope"], "encounters:read");
    assert_eq!(r.body["expires_in"], 3600);
    let access = text(&r.body["access_token"]);
    let refresh1 = text(&r.body["refresh_token"]);
    assert!(access.starts_with("bpa_") && refresh1.starts_with("bpr_"));
    let me = bearer_get(router, &access, "/mcp-test/whoami").await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["username"], "oauth-user");
    assert_eq!(me.body["scopes"], json!(["encounters:read"]));
    // An access token is no session either.
    let (status, _) = send(router, Some(&access), "GET", "/api/me", Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A reused code fails, and revokes what the first use gave.
    let r = exchange(router, &app, &code, &verifier).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"], "invalid_grant");
    assert_eq!(who(router, &access).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        refresh(router, &app, &refresh1).await.body["error"],
        "invalid_grant"
    );

    // A wrong verifier fails, and burns the code.
    let (verifier, challenge) = pkce();
    let code = consent(
        router,
        &user,
        &authorize_url(&app.id, CALLBACK, &challenge, &[]),
        None,
    )
    .await;
    let (other_verifier, _) = pkce();
    let r = exchange(router, &app, &code, &other_verifier).await;
    assert_eq!(r.body["error"], "invalid_grant", "{}", r.body);
    assert_eq!(
        exchange(router, &app, &code, &verifier).await.body["error"],
        "invalid_grant"
    );
    // No verifier at all.
    let (_, challenge) = pkce();
    let code = consent(
        router,
        &user,
        &authorize_url(&app.id, CALLBACK, &challenge, &[]),
        None,
    )
    .await;
    let r = form(
        router,
        "/api/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", &app.id),
        ],
        None,
    )
    .await;
    assert_eq!(r.body["error"], "invalid_grant");

    // A redirect mismatch: at the token endpoint ...
    let (verifier, challenge) = pkce();
    let code = consent(
        router,
        &user,
        &authorize_url(&app.id, CALLBACK, &challenge, &[]),
        None,
    )
    .await;
    let r = form(
        router,
        "/api/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", "https://claude.ai/api/mcp/other"),
            ("client_id", &app.id),
            ("code_verifier", &verifier),
        ],
        None,
    )
    .await;
    assert_eq!(r.body["error"], "invalid_grant");
    // ... and at authorize, where nothing is sent to the unregistered address.
    let (_, challenge) = pkce();
    for redirect in [
        "https://evil.example/cb",
        "https://claude.ai/api/mcp/auth_callback/",
    ] {
        let r = get_plain(router, &authorize_url(&app.id, redirect, &challenge, &[])).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{redirect}");
        assert!(r.headers.get(header::LOCATION).is_none());
    }
    // Unknown client: a page, no redirect.
    let r = get_plain(router, &authorize_url("nobody", CALLBACK, &challenge, &[])).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.headers.get(header::LOCATION).is_none());
    // Errors that can go back go back, with state and iss.
    for (key, value, error) in [
        ("code_challenge_method", "plain", "invalid_request"),
        ("scope", "admin:everything", "invalid_scope"),
        ("resource", "https://other.example/mcp", "invalid_target"),
        ("response_type", "token", "unsupported_response_type"),
    ] {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in [
            ("response_type", "code"),
            ("client_id", app.id.as_str()),
            ("redirect_uri", CALLBACK),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", "s1"),
        ] {
            if k != key {
                q.append_pair(k, v);
            }
        }
        q.append_pair(key, value);
        let r = get_plain(router, &format!("/api/oauth/authorize?{}", q.finish())).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{error}");
        let back = query_of(&location(&r));
        assert_eq!(back["error"], error);
        assert_eq!(back["state"], "s1");
        assert_eq!(back["iss"], BASE);
    }
    // No PKCE at all.
    let callback: String = url::form_urlencoded::byte_serialize(CALLBACK.as_bytes()).collect();
    let r = get_plain(
        router,
        &format!(
            "/api/oauth/authorize?response_type=code&client_id={}&redirect_uri={callback}&state=s",
            app.id
        ),
    )
    .await;
    assert_eq!(query_of(&location(&r))["error"], "invalid_request");
    // A repeated parameter.
    let url = format!(
        "{}&state=again",
        authorize_url(&app.id, CALLBACK, &challenge, &[])
    );
    assert_eq!(
        get_plain(router, &url).await.status,
        StatusCode::BAD_REQUEST
    );

    // Deny.
    let id = start(router, &authorize_url(&app.id, CALLBACK, &challenge, &[])).await;
    let out = ok(
        &user,
        "POST",
        &format!("/api/oauth/requests/{id}/deny"),
        json!({}),
    )
    .await;
    let q = query_of(&text(&out["redirect"]));
    assert_eq!(q["error"], "access_denied");
    assert_eq!(q["state"], "xyz-state");
    assert_eq!(q["iss"], BASE);
    // Consent can't add scopes the app didn't ask for, and needs a session.
    let id = start(
        router,
        &authorize_url(&app.id, CALLBACK, &challenge, &[("scope", "bestiary:read")]),
    )
    .await;
    let (status, _) = user
        .post(
            &format!("/api/oauth/requests/{id}/approve"),
            json!({ "scopes": ["homebrew:write"] }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(
        router,
        None,
        "POST",
        &format!("/api/oauth/requests/{id}/approve"),
        json!({ "scopes": ["bestiary:read"] }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // With no scope asked for: the defaults.
    let id = start(router, &authorize_url(&app.id, CALLBACK, &challenge, &[])).await;
    let detail = ok(
        &user,
        "GET",
        &format!("/api/oauth/requests/{id}"),
        Value::Null,
    )
    .await;
    let asked: Vec<&str> = detail["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(asked.len(), 7);
    assert!(asked.contains(&"encounters:simulate") && !asked.contains(&"index:publish"));

    // Refresh rotates; the old one, used again, revokes the grant.
    let (access1, refresh1) = connect(router, &app, &user).await;
    let r = refresh(router, &app, &refresh1).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let access2 = text(&r.body["access_token"]);
    let refresh2 = text(&r.body["refresh_token"]);
    assert_ne!(refresh1, refresh2);
    assert_eq!(who(router, &access2).await, StatusCode::OK);
    // Narrowing on refresh, never widening.
    let r = form(
        router,
        "/api/oauth/token",
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh2),
            ("client_id", &app.id),
            ("scope", "bestiary:read"),
        ],
        None,
    )
    .await;
    assert_eq!(r.body["scope"], "bestiary:read", "{}", r.body);
    let refresh3 = text(&r.body["refresh_token"]);
    let r = form(
        router,
        "/api/oauth/token",
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh3),
            ("client_id", &app.id),
            ("scope", "combat:run"),
        ],
        None,
    )
    .await;
    assert_eq!(
        r.body["error"], "invalid_scope",
        "a scope that does not exist"
    );
    let stored = stored_text(db).await;
    for secret in [&access1, &refresh1, &access2, &refresh2, &refresh3, &code] {
        assert_not_stored(&stored, secret);
    }
    // Reuse of refresh1: every token of the grant stops.
    assert_eq!(
        refresh(router, &app, &refresh1).await.body["error"],
        "invalid_grant"
    );
    assert_eq!(who(router, &access2).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        refresh(router, &app, &refresh3).await.body["error"],
        "invalid_grant"
    );

    // Revocation (RFC 7009) of a refresh token ends the grant; unknown tokens still get 200.
    let (access, refresh_token) = connect(router, &app, &user).await;
    let r = form(
        router,
        "/api/oauth/revoke",
        &[("token", &refresh_token), ("client_id", &app.id)],
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        refresh(router, &app, &refresh_token).await.body["error"],
        "invalid_grant"
    );
    assert_eq!(who(router, &access).await, StatusCode::UNAUTHORIZED);
    let r = form(
        router,
        "/api/oauth/revoke",
        &[("token", "bpr_nothing"), ("client_id", &app.id)],
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    // Revoking an access token ends only it.
    let (access, refresh_token) = connect(router, &app, &user).await;
    form(
        router,
        "/api/oauth/revoke",
        &[("token", &access), ("client_id", &app.id)],
        None,
    )
    .await;
    assert_eq!(who(router, &access).await, StatusCode::UNAUTHORIZED);
    let r = refresh(router, &app, &refresh_token).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let refresh_token = text(&r.body["refresh_token"]);
    let access = text(&r.body["access_token"]);

    // Connected apps on the Account page: listed, and disconnecting stops the app at once.
    let apps = ok(&user, "GET", "/api/me/ai/apps", Value::Null).await;
    let list = apps["apps"].as_array().unwrap();
    assert_eq!(list.len(), 1, "revoked grants are gone: {apps}");
    assert_eq!(list[0]["name"], "Claude");
    assert_eq!(list[0]["redirectHost"], "https://claude.ai");
    let grant = text(&list[0]["id"]);
    let stranger = sign_up(router, "oauth-stranger").await;
    let (status, _) = stranger.delete(&format!("/api/me/ai/apps/{grant}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(who(router, &access).await, StatusCode::OK);
    ok(
        &user,
        "DELETE",
        &format!("/api/me/ai/apps/{grant}"),
        Value::Null,
    )
    .await;
    assert_eq!(who(router, &access).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        refresh(router, &app, &refresh_token).await.body["error"],
        "invalid_grant"
    );

    // A token issued for another resource, and an expired one, are refused.
    let (access, _) = connect(router, &app, &user).await;
    set(
        db,
        "UPDATE oauth_grants SET resource = $1",
        Some("https://other.example/api/mcp"),
    )
    .await;
    assert_eq!(who(router, &access).await, StatusCode::UNAUTHORIZED);
    set(db, "UPDATE oauth_grants SET resource = $1", Some(MCP)).await;
    assert_eq!(who(router, &access).await, StatusCode::OK);
    set(
        db,
        "UPDATE oauth_tokens SET expires_at = $1 WHERE kind = 'access'",
        None,
    )
    .await;
    assert_eq!(who(router, &access).await, StatusCode::UNAUTHORIZED);

    // An expired code.
    let (verifier, challenge) = pkce();
    let code = consent(
        router,
        &user,
        &authorize_url(&app.id, CALLBACK, &challenge, &[]),
        None,
    )
    .await;
    set(
        db,
        "UPDATE oauth_codes SET expires_at = $1 WHERE used_at IS NULL",
        None,
    )
    .await;
    assert_eq!(
        exchange(router, &app, &code, &verifier).await.body["error"],
        "invalid_grant"
    );
}

async fn confidential_client(router: &Router) {
    let user = sign_up(router, "oauth-confidential").await;
    let app = register(
        router,
        json!({ "client_name": "ChatGPT", "redirect_uris": [CALLBACK] }),
    )
    .await;
    let secret = app.secret.clone().unwrap();
    let (verifier, challenge) = pkce();
    let code = consent(
        router,
        &user,
        &authorize_url(&app.id, CALLBACK, &challenge, &[]),
        None,
    )
    .await;
    let pairs = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", CALLBACK),
        ("code_verifier", verifier.as_str()),
    ];
    // Without its secret, or with a wrong one: invalid_client, and the code is untouched.
    let mut with_id = pairs.to_vec();
    with_id.push(("client_id", &app.id));
    let r = form(router, "/api/oauth/token", &with_id, None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.body["error"], "invalid_client");
    let r = form(
        router,
        "/api/oauth/token",
        &pairs,
        Some((&app.id, "bps_wrong")),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.headers.get(header::WWW_AUTHENTICATE).is_some());
    let r = form(router, "/api/oauth/token", &pairs, Some((&app.id, &secret))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let refresh_token = text(&r.body["refresh_token"]);
    // Another client can't use its refresh token; client_secret_post works too.
    let other = public_app(router, "Other", CALLBACK).await;
    assert_eq!(
        refresh(router, &other, &refresh_token).await.body["error"],
        "invalid_grant"
    );
    let r = form(
        router,
        "/api/oauth/token",
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh_token),
            ("client_id", &app.id),
            ("client_secret", &secret),
        ],
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let r = form(
        router,
        "/api/oauth/token",
        &[("grant_type", "password"), ("client_id", &other.id)],
        None,
    )
    .await;
    assert_eq!(r.body["error"], "unsupported_grant_type");
}

/// Loopback redirects (Claude Code, CLIs) may use any port, as RFC 8252 asks.
async fn loopback(router: &Router) {
    let user = sign_up(router, "oauth-cli").await;
    let app = public_app(router, "CLI", "http://127.0.0.1:3118/callback").await;
    let (verifier, challenge) = pkce();
    let id = start(
        router,
        &authorize_url(&app.id, "http://127.0.0.1:50123/callback", &challenge, &[]),
    )
    .await;
    let out = ok(
        &user,
        "POST",
        &format!("/api/oauth/requests/{id}/approve"),
        json!({ "scopes": ["bestiary:read"] }),
    )
    .await;
    let redirect = text(&out["redirect"]);
    assert!(
        redirect.starts_with("http://127.0.0.1:50123/callback?"),
        "{redirect}"
    );
    let code = query_of(&redirect)["code"].clone();
    let r = form(
        router,
        "/api/oauth/token",
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", "http://127.0.0.1:50123/callback"),
            ("client_id", &app.id),
            ("code_verifier", &verifier),
        ],
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    // The grant is bestiary:read only: a refresh can't widen it.
    let r = form(
        router,
        "/api/oauth/token",
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &text(&r.body["refresh_token"])),
            ("client_id", &app.id),
            ("scope", "encounters:read"),
        ],
        None,
    )
    .await;
    assert_eq!(r.body["error"], "invalid_scope", "{}", r.body);
}

// Scopes over the role matrix.

async fn role_matrix(router: &Router) {
    let t = table(router, "matrix").await;
    let every: Vec<&str> = Scope::ALL.iter().map(|s| s.as_str()).collect();
    let dm_token = new_secret_token(&t.dm, json!({ "name": "all", "scopes": every })).await;
    let player_token = new_secret_token(&t.player, json!({ "name": "all", "scopes": every })).await;
    let narrow = new_secret_token(
        &t.dm,
        json!({ "name": "bestiary only", "scopes": ["bestiary:read"] }),
    )
    .await;
    let outsider = sign_up(router, "outsider-matrix").await;
    let outsider_token = new_secret_token(&outsider, json!({ "name": "x" })).await;
    let shown = format!("/mcp-test/encounters/{}", t.shown);
    let listing = format!("/mcp-test/parties/{}/encounters", t.party);

    // The DM's token: the DM view, exactly what the account's session gets.
    let r = bearer_get(router, &dm_token, &shown).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let body = r.body.to_string();
    assert!(
        body.contains("SECRET-LABEL") && body.contains("SECRET-NOTES"),
        "{body}"
    );
    let session = ok(
        &t.dm,
        "GET",
        &format!("/api/encounters/{}", t.shown),
        Value::Null,
    )
    .await;
    assert_eq!(session, r.body);
    assert_eq!(
        bearer_get(router, &dm_token, &listing).await.body["role"],
        "dm"
    );

    // The player's token, with every scope: only the projection. No hidden goblin, label, notes or real name.
    let r = bearer_get(router, &player_token, &shown).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let body = r.body.to_string();
    for secret in ["SECRET", "Goblin", "GOBLIN"] {
        assert!(
            !body.contains(secret),
            "{secret} reached a player's token: {body}"
        );
    }
    assert!(body.contains("Big beast"), "{body}");
    let session = ok(
        &t.player,
        "GET",
        &format!("/api/encounters/{}", t.shown),
        Value::Null,
    )
    .await;
    assert_eq!(session, r.body);
    let r = bearer_get(router, &player_token, &listing).await;
    assert_eq!(r.body["role"], "player");
    assert!(!r.body.to_string().contains("SECRET"), "{}", r.body);
    // The draft, and an outsider: the same generic 404 as a session gets.
    let draft = format!("/mcp-test/encounters/{}", t.draft);
    assert_eq!(
        bearer_get(router, &player_token, &draft).await.status,
        StatusCode::NOT_FOUND
    );
    for path in [&shown, &listing] {
        assert_eq!(
            bearer_get(router, &outsider_token, path).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }

    // encounters:write includes encounters:read.
    let writer = new_secret_token(
        &t.dm,
        json!({ "name": "writer", "scopes": ["encounters:write"] }),
    )
    .await;
    assert_eq!(
        bearer_get(router, &writer, &shown).await.status,
        StatusCode::OK
    );
    // Without the scope: 403 insufficient_scope, whatever the role.
    let r = bearer_get(router, &narrow, &shown).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.body["error"], "insufficient_scope");
    let www = r.headers[header::WWW_AUTHENTICATE].to_str().unwrap();
    assert!(
        www.contains("error=\"insufficient_scope\"") && www.contains("scope=\"encounters:read\""),
        "{www}"
    );

    // The role is re-checked on every call: once the party is gone, so is the DM's access.
    ok(
        &t.dm,
        "DELETE",
        &format!("/api/parties/{}", t.party),
        Value::Null,
    )
    .await;
    for token in [&dm_token, &player_token] {
        assert_eq!(
            bearer_get(router, token, &shown).await.status,
            StatusCode::NOT_FOUND
        );
    }
}

// Limits.

async fn rate_limits(router: &Router) {
    let me = sign_up(router, "limits").await;
    let secret = new_secret_token(&me, json!({ "name": "busy" })).await;
    for i in 0..crate::tokens::CALLS_PER_MINUTE {
        assert_eq!(who(router, &secret).await, StatusCode::OK, "call {i}");
    }
    let r = bearer_get(router, &secret, "/mcp-test/whoami").await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    // Per token: another token of the same account still works.
    let other = new_secret_token(&me, json!({ "name": "quiet" })).await;
    assert_eq!(who(router, &other).await, StatusCode::OK);

    // Registrations per client address.
    let mut last = StatusCode::CREATED;
    for _ in 0..=crate::oauth::REGISTRATIONS_PER_HOUR {
        last = post_json(
            router,
            "/api/oauth/register",
            json!({ "redirect_uris": [CALLBACK] }),
        )
        .await
        .status;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    // Token requests per client address.
    let mut last = StatusCode::OK;
    for _ in 0..=crate::oauth::TOKEN_CALLS_PER_MINUTE {
        last = form(
            router,
            "/api/oauth/token",
            &[("grant_type", "refresh_token")],
            None,
        )
        .await
        .status;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

async fn everything(router: &Router, db: &Db) {
    metadata(router).await;
    personal_tokens(router, db).await;
    dcr(router).await;
    oauth_flow(router, db).await;
    confidential_client(router).await;
    loopback(router).await;
    role_matrix(router).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ai_auth_flows() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    let router = ai_router(ai_state(dir.path().to_path_buf(), db.clone()));
    everything(&router, &db).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ai_auth_rate_limits() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    rate_limits(&ai_router(ai_state(dir.path().to_path_buf(), db))).await;
}

#[tokio::test]
async fn without_a_public_url_oauth_is_not_offered_but_tokens_work() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    let router = ai_router(state_with(dir.path().to_path_buf(), db, Setup::default()));
    assert_eq!(
        get_plain(&router, "/.well-known/oauth-authorization-server")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let r = post_json(
        &router,
        "/api/oauth/register",
        json!({ "redirect_uris": [CALLBACK] }),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let me = sign_up(&router, "no-public-url").await;
    let secret = new_secret_token(&me, json!({ "name": "cli" })).await;
    assert_eq!(who(&router, &secret).await, StatusCode::OK);
}

#[tokio::test]
async fn the_switch_removes_every_route() {
    use axum::routing::any;
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    let off = crate::tokens::routes_if(false)
        .merge(crate::oauth::routes_if(false))
        .route("/{*rest}", any(|| async { StatusCode::IM_A_TEAPOT }))
        .with_state(ai_state(dir.path().to_path_buf(), db));
    for path in [
        "/api/me/ai/tokens",
        "/api/me/ai/apps",
        "/.well-known/oauth-authorization-server",
        "/api/oauth/authorize",
    ] {
        assert_eq!(
            get_plain(&off, path).await.status,
            StatusCode::IM_A_TEAPOT,
            "{path} is still routed"
        );
    }
}

// The upgrade from 0.3.0.

/// Schema 6 (0.3.0) with accounts, a party and an encounter; HEAD adds step 7 (and the later ones) and every flow works over it. A
/// database that says it is at 7 but has no `api_tokens` gets the step again.
async fn upgrade_from_0_3_0(location: &Location, data: &Path, url: Option<&str>) {
    let old = Db::open_at(location, 6).await.unwrap();
    {
        let router = ai_router(ai_state(data.to_path_buf(), old.clone()));
        table(&router, "up").await;
    }
    old.pool().close().await;

    let db = Db::open(location).await.unwrap();
    let version: i64 = sqlx::query("SELECT MAX(version) FROM schema_version")
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(version, i64::try_from(Db::schema_version()).unwrap());
    let router = ai_router(ai_state(data.to_path_buf(), db.clone()));
    let dm = super::sign_in(&router, "dm-up").await;
    let secret = new_secret_token(&dm, json!({ "name": "after upgrade" })).await;
    assert_eq!(who(&router, &secret).await, StatusCode::OK);
    everything(&router, &db).await;
    db.pool().close().await;

    if let Some(url) = url {
        sqlx::any::install_default_drivers();
        let pool = sqlx::AnyPool::connect(url).await.unwrap();
        for t in [
            "oauth_tokens",
            "oauth_grants",
            "oauth_codes",
            "oauth_requests",
            "oauth_clients",
            "api_tokens",
        ] {
            sqlx::query(&format!("DROP TABLE {t}"))
                .execute(&pool)
                .await
                .unwrap();
        }
        pool.close().await;
        let db = Db::open(location).await.unwrap();
        let rows: i64 = sqlx::query("SELECT COUNT(*) FROM api_tokens")
            .fetch_one(db.pool())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        assert_eq!(rows, 0);
        db.pool().close().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_0_3_0_database_upgrades_to_ai_auth() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("party.sqlite");
    let url = format!("sqlite://{}", file.display());
    upgrade_from_0_3_0(
        &Location::Sqlite(file),
        &dir.path().join("data"),
        Some(&url),
    )
    .await;
}

/// Everything above on PostgreSQL. Runs when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.
#[tokio::test(flavor = "multi_thread")]
async fn ai_auth_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = POSTGRES.lock().await;
    let dir = tempfile::tempdir().unwrap();
    drop_all(&url).await;
    upgrade_from_0_3_0(
        &Location::Postgres(url.clone()),
        &dir.path().join("up"),
        Some(&url),
    )
    .await;
    drop_all(&url).await;
    let db = Db::open(&Location::Postgres(url.clone())).await.unwrap();
    rate_limits(&ai_router(ai_state(dir.path().join("limits"), db.clone()))).await;
    db.pool().close().await;
    drop_all(&url).await;
}
