//! The ChatGPT / claude.ai connector path end to end (0.3.3 part B, parity with BalanceLab): a scripted MCP client
//! that does what those connectors do, from nothing but the MCP URL.
//!
//! 1. `POST /api/mcp` without a token: 401 with `WWW-Authenticate: Bearer resource_metadata="…", scope="…"`.
//! 2. Protected-resource metadata from that URL (RFC 9728), then the authorization server's metadata (RFC 8414, and
//!    the OIDC and path-inserted forms the MCP spec lists).
//! 3. Dynamic client registration (RFC 7591), authorize with PKCE S256 and `resource` (RFC 8707), consent as the
//!    signed-in account, the `iss` check (RFC 9207), the code for tokens.
//! 4. `initialize` (instructions present, also as a resource), `tools/list`, a tool call, in every protocol revision.
//! 5. Refresh (rotation, 90 days), then revoke.
//! 6. A password change, "Sign out everywhere" and the admin's reset and sign-out revoke every token at once; the
//!    connector gets the 401 and connects again.
//!
//! And the refusals: a wrong `resource`, a missing PKCE, an authorization answer from another issuer, a token meant
//! for another server, and browser origins. Runs on SQLite, and on PostgreSQL when `BOREALIS_TEST_POSTGRES_URL` names
//! a database it may wipe.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use base64::Engine as _;
use http_body_util::BodyExt;
use party_tools::mcp::{INSTRUCTIONS, INSTRUCTIONS_URI};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tower::ServiceExt;

use super::{drop_all, send, sign_up, state_with, Client, Setup, POSTGRES};
use crate::db::{now, Db, Location};
use crate::{api_routes, AppState, Config};

const BASE: &str = "https://party.example.com";
const MCP: &str = "https://party.example.com/api/mcp";
const OTHER: &str = "https://other.example.com";
const CHATGPT_CALLBACK: &str = "https://chatgpt.com/connector_platform_oauth_redirect";
const CLAUDE_CALLBACK: &str = "https://claude.ai/api/mcp/auth_callback";

/// Each router comes from its own client address, so one test's per-address limits don't reach another's.
static NEXT_IP: AtomicU8 = AtomicU8::new(1);

fn server(data: &Path, db: Db, public_url: &str) -> (AppState, Router) {
    let mut state = state_with(data.to_path_buf(), db, Setup::default());
    state.config = Arc::new(Config {
        open_registration: true,
        require_approval: false,
        public_url: Some(public_url.to_string()),
        app_download: None,
        trusted_proxies: Vec::new(),
    });
    let ip = NEXT_IP.fetch_add(1, Ordering::Relaxed);
    let router = api_routes(state.clone(), 64 * 1024 * 1024).layer(MockConnectInfo(
        std::net::SocketAddr::from(([10, 88, 0, ip], 9)),
    ));
    (state, router)
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

/// A path on `base` (the test router has no host), checked to be on it.
fn path_on(base: &str, url: &str) -> String {
    let path = url
        .strip_prefix(base)
        .unwrap_or_else(|| panic!("{url} is not on {base}"));
    path.to_string()
}

async fn get(router: &Router, path: &str) -> Raw {
    raw(router, Request::get(path).body(Body::empty()).unwrap()).await
}

async fn form(router: &Router, path: &str, pairs: &[(&str, &str)]) -> Raw {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    raw(
        router,
        Request::post(path)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap(),
    )
    .await
}

/// One MCP message, as a connector's server sends it (no Origin unless given).
async fn mcp(router: &Router, token: Option<&str>, body: &Value, headers: &[(&str, &str)]) -> Raw {
    let mut request = Request::post("/api/mcp")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    raw(router, request.body(Body::from(body.to_string())).unwrap()).await
}

fn initialize(version: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": version,
        "capabilities": {},
        "clientInfo": { "name": "scripted-connector", "version": "1" } } })
}

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// The parameters of a `WWW-Authenticate: Bearer …` challenge.
fn www_authenticate(headers: &HeaderMap) -> HashMap<String, String> {
    let text = headers
        .get(header::WWW_AUTHENTICATE)
        .expect("a WWW-Authenticate challenge")
        .to_str()
        .unwrap();
    let rest = text.strip_prefix("Bearer ").expect("a Bearer challenge");
    let mut out = HashMap::new();
    for part in rest.split("\", ") {
        let (k, v) = part.split_once("=\"").unwrap_or_else(|| panic!("{text}"));
        out.insert(k.trim().to_string(), v.trim_end_matches('"').to_string());
    }
    out
}

fn query_of(url: &str) -> HashMap<String, String> {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn text(v: &Value) -> String {
    v.as_str()
        .unwrap_or_else(|| panic!("not text: {v}"))
        .to_string()
}

fn pkce() -> (String, String) {
    let verifier = crate::tokens::new_secret("v-");
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// What the scripted connector learned by discovery.
struct Discovered {
    issuer: String,
    resource: String,
    scope: String,
    authorize: String,
    token: String,
    register: String,
    revoke: String,
}

/// Steps 1–2: the 401, then the metadata it points at, exactly as a connector follows it.
async fn discover(router: &Router, base: &str) -> Discovered {
    let r = mcp(router, None, &initialize("2025-11-25"), &[]).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.body);
    assert_eq!(r.body["error"], "invalid_token");
    let c = www_authenticate(&r.headers);
    let metadata_url = c["resource_metadata"].clone();
    assert_eq!(
        metadata_url,
        format!("{base}/.well-known/oauth-protected-resource/api/mcp")
    );
    let scope = c["scope"].clone();
    assert!(scope.contains("bestiary:read"), "{scope}");
    assert!(!scope.contains("offline_access"));

    let prm = get(router, &path_on(base, &metadata_url)).await;
    assert_eq!(prm.status, StatusCode::OK);
    let resource = text(&prm.body["resource"]);
    assert_eq!(resource, format!("{base}/api/mcp"));
    assert_eq!(prm.body["bearer_methods_supported"], json!(["header"]));
    let issuer = text(&prm.body["authorization_servers"][0]);
    assert_eq!(issuer, base);
    // Every other place a client may look for it has the same document.
    for path in [
        "/.well-known/oauth-protected-resource",
        "/api/mcp/.well-known/oauth-protected-resource",
    ] {
        assert_eq!(get(router, path).await.body, prm.body, "{path}");
    }

    // RFC 8414 / OIDC discovery for an issuer without a path, in the MCP spec's order, then the forms under the MCP
    // path that some clients try first.
    let asm = get(router, "/.well-known/oauth-authorization-server").await;
    assert_eq!(asm.status, StatusCode::OK);
    for path in [
        "/.well-known/openid-configuration",
        "/.well-known/oauth-authorization-server/api/mcp",
        "/.well-known/openid-configuration/api/mcp",
        "/api/mcp/.well-known/openid-configuration",
        "/api/mcp/.well-known/oauth-authorization-server",
    ] {
        let r = get(router, path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert_eq!(r.body, asm.body, "{path}");
    }
    let m = &asm.body;
    assert_eq!(m["issuer"], issuer);
    // ChatGPT and the MCP spec refuse to go on without S256 listed.
    assert_eq!(m["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(m["authorization_response_iss_parameter_supported"], true);
    assert_eq!(m["response_types_supported"], json!(["code"]));
    assert_eq!(
        m["grant_types_supported"],
        json!(["authorization_code", "refresh_token"])
    );
    assert!(!m["scopes_supported"]
        .as_array()
        .unwrap()
        .contains(&json!("offline_access")));
    assert!(!m["scopes_supported"]
        .as_array()
        .unwrap()
        .contains(&json!("openid")));
    Discovered {
        issuer: issuer.clone(),
        resource,
        scope,
        authorize: path_on(base, &text(&m["authorization_endpoint"])),
        token: path_on(base, &text(&m["token_endpoint"])),
        register: path_on(base, &text(&m["registration_endpoint"])),
        revoke: path_on(base, &text(&m["revocation_endpoint"])),
    }
}

/// DCR as ChatGPT does it: a public client with its stable redirect.
async fn register(router: &Router, d: &Discovered, name: &str, redirect: &str) -> String {
    let r = raw(
        router,
        Request::post(&d.register)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "client_name": name,
                    "redirect_uris": [redirect],
                    "token_endpoint_auth_method": "none",
                    "grant_types": ["authorization_code", "refresh_token"],
                    "response_types": ["code"],
                })
                .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    assert!(r.body.get("client_secret").is_none());
    text(&r.body["client_id"])
}

fn authorize_url(
    d: &Discovered,
    client: &str,
    redirect: &str,
    challenge: Option<&str>,
    resource: Option<&str>,
) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    q.extend_pairs([
        ("response_type", "code"),
        ("client_id", client),
        ("redirect_uri", redirect),
        ("state", "connector-state"),
        ("scope", d.scope.as_str()),
    ]);
    if let Some(c) = challenge {
        q.extend_pairs([("code_challenge", c), ("code_challenge_method", "S256")]);
    }
    if let Some(r) = resource {
        q.append_pair("resource", r);
    }
    format!("{}?{}", d.authorize, q.finish())
}

/// The consent screen, as the signed-in account answers it: the app's redirect with the authorization answer.
async fn consent(router: &Router, base: &str, user: &Client, authorize: &str) -> String {
    let r = get(router, authorize).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let to = r.headers[header::LOCATION].to_str().unwrap().to_string();
    let id = to
        .strip_prefix(&format!("{base}/#/oauth/consent/"))
        .unwrap_or_else(|| panic!("{to}"))
        .to_string();
    let (status, detail) = send(
        router,
        Some(&user.token),
        "GET",
        &format!("/api/oauth/requests/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let all: Vec<Value> = detail["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].clone())
        .collect();
    let (status, out) = send(
        router,
        Some(&user.token),
        "POST",
        &format!("/api/oauth/requests/{id}/approve"),
        json!({ "scopes": all }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    text(&out["redirect"])
}

/// The connector's RFC 9207 check: the answer's `iss` must be, exactly, the issuer it discovered; only then is the
/// code sent anywhere.
fn code_from(answer: &str, redirect: &str, issuer: &str) -> Result<String, String> {
    assert!(answer.starts_with(redirect), "{answer}");
    let q = query_of(answer);
    assert_eq!(q["state"], "connector-state");
    match q.get("iss") {
        Some(iss) if iss == issuer => {}
        Some(iss) => return Err(format!("iss mismatch: {iss} is not {issuer}")),
        None => return Err("no iss".into()),
    }
    q.get("code")
        .cloned()
        .ok_or_else(|| format!("no code: {answer}"))
}

async fn exchange(
    router: &Router,
    d: &Discovered,
    client: &str,
    redirect: &str,
    code: &str,
    verifier: Option<&str>,
    resource: &str,
) -> Raw {
    let mut pairs = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect),
        ("client_id", client),
        ("resource", resource),
    ];
    if let Some(v) = verifier {
        pairs.push(("code_verifier", v));
    }
    form(router, &d.token, &pairs).await
}

async fn refresh(router: &Router, d: &Discovered, client: &str, token: &str) -> Raw {
    form(
        router,
        &d.token,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", token),
            ("client_id", client),
            ("resource", d.resource.as_str()),
        ],
    )
    .await
}

/// Seconds from now until the token with this secret expires.
async fn lifetime(db: &Db, secret: &str) -> i64 {
    let row = sqlx::query("SELECT expires_at FROM oauth_tokens WHERE token_hash = $1")
        .bind(crate::tokens::secret_hash(secret))
        .fetch_one(db.pool())
        .await
        .unwrap();
    row.try_get::<i64, _>(0).unwrap() - now()
}

/// Steps 4: what a connector does once it has a token, in each revision it may speak.
async fn use_tools(router: &Router, token: &str) {
    for version in ["2025-11-25", "2025-06-18", "2025-03-26"] {
        let r = mcp(router, Some(token), &initialize(version), &[]).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let result = &r.body["result"];
        assert_eq!(result["protocolVersion"], version);
        assert_eq!(result["instructions"], INSTRUCTIONS);
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["capabilities"]["resources"].is_object());
        // Plain JSON, no session, no stream.
        assert!(r.headers.get("mcp-session-id").is_none());
        assert!(r.headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/json"));
        // 2025-03-26 clients may send no version header at all.
        let h: Vec<(&str, &str)> = if version == "2025-03-26" {
            vec![]
        } else {
            vec![("mcp-protocol-version", version)]
        };
        let r = mcp(
            router,
            Some(token),
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            &h,
        )
        .await;
        assert_eq!(r.status, StatusCode::ACCEPTED);

        let r = mcp(
            router,
            Some(token),
            &request(1, "tools/list", json!({})),
            &h,
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let names: Vec<&str> = r.body["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"bestiary_search"), "{names:?}");

        let r = mcp(
            router,
            Some(token),
            &request(2, "resources/list", json!({})),
            &h,
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let resources = r.body["result"]["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0]["uri"], INSTRUCTIONS_URI);
        assert_eq!(resources[0]["mimeType"], "text/markdown");
        let r = mcp(
            router,
            Some(token),
            &request(3, "resources/read", json!({ "uri": INSTRUCTIONS_URI })),
            &h,
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        assert_eq!(r.body["result"]["contents"][0]["text"], INSTRUCTIONS);
        assert_eq!(r.body["result"]["contents"][0]["uri"], INSTRUCTIONS_URI);
        let r = mcp(
            router,
            Some(token),
            &request(4, "resources/read", json!({ "uri": "borealis://nope" })),
            &h,
        )
        .await;
        assert_eq!(r.body["error"]["code"], -32002, "{}", r.body);

        let r = mcp(
            router,
            Some(token),
            &request(
                5,
                "tools/call",
                json!({ "name": "bestiary_search", "arguments": { "text": "owlbear" } }),
            ),
            &h,
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        assert_eq!(r.body["result"]["isError"], false, "{}", r.body);
        assert!(
            r.body["result"]["structuredContent"]["data"]
                .to_string()
                .contains("Owlbear"),
            "{}",
            r.body
        );
    }

    // The modern revision: discover carries the instructions too, and the resource is read the same way.
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let h = [("mcp-protocol-version", "2026-07-28")];
    let r = mcp(
        router,
        Some(token),
        &request(6, "server/discover", json!({ "_meta": meta })),
        &h,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["result"]["instructions"], INSTRUCTIONS);
    assert!(r.body["result"]["capabilities"]["resources"].is_object());
    let r = mcp(
        router,
        Some(token),
        &request(
            7,
            "resources/read",
            json!({ "uri": INSTRUCTIONS_URI, "_meta": meta }),
        ),
        &h,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["result"]["contents"][0]["text"], INSTRUCTIONS);
    assert_eq!(r.body["result"]["resultType"], "complete");
}

/// The whole connector path, ChatGPT's way, then the same with claude.ai's callback.
async fn connector_flow(router: &Router, db: &Db) {
    let d = discover(router, BASE).await;
    let user = sign_up(router, "connector-dm").await;
    for (name, callback) in [("ChatGPT", CHATGPT_CALLBACK), ("Claude", CLAUDE_CALLBACK)] {
        let client = register(router, &d, name, callback).await;
        let (verifier, challenge) = pkce();
        let answer = consent(
            router,
            BASE,
            &user,
            &authorize_url(&d, &client, callback, Some(&challenge), Some(&d.resource)),
        )
        .await;
        let code = code_from(&answer, callback, &d.issuer).expect("the right iss");
        let r = exchange(
            router,
            &d,
            &client,
            callback,
            &code,
            Some(&verifier),
            &d.resource,
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        assert_eq!(r.headers[header::CACHE_CONTROL], "no-store");
        assert_eq!(r.body["token_type"], "Bearer");
        assert_eq!(r.body["expires_in"], 3600);
        let access = text(&r.body["access_token"]);
        let refresh_token = text(&r.body["refresh_token"]);
        assert!(access.starts_with("bpa_") && refresh_token.starts_with("bpr_"));
        // Access for an hour, refresh for 90 days (BalanceLab's lifetimes).
        assert!((3590..=3600).contains(&lifetime(db, &access).await));
        let days = lifetime(db, &refresh_token).await;
        assert!((90 * 86400 - 10..=90 * 86400).contains(&days), "{days}");

        use_tools(router, &access).await;

        // Refresh: a new pair, the old refresh token spent; the new access token works.
        let r = refresh(router, &d, &client, &refresh_token).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let access2 = text(&r.body["access_token"]);
        let refresh2 = text(&r.body["refresh_token"]);
        assert_ne!(refresh2, refresh_token);
        let days = lifetime(db, &refresh2).await;
        assert!((90 * 86400 - 10..=90 * 86400).contains(&days), "{days}");
        let r = mcp(router, Some(&access2), &initialize("2025-06-18"), &[]).await;
        assert_eq!(r.status, StatusCode::OK);

        // Revoke (RFC 7009): the refresh token ends the grant, so every token from it stops at once.
        let r = form(
            router,
            &d.revoke,
            &[("token", refresh2.as_str()), ("client_id", client.as_str())],
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        for dead in [&access, &access2] {
            let r = mcp(router, Some(dead), &initialize("2025-11-25"), &[]).await;
            assert_eq!(r.status, StatusCode::UNAUTHORIZED);
            // Still the challenge a connector re-authorizes from.
            assert!(www_authenticate(&r.headers).contains_key("resource_metadata"));
        }
        let r = refresh(router, &d, &client, &refresh2).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        assert_eq!(r.body["error"], "invalid_grant");
    }

    // An expired refresh token is refused like a dead one.
    let client = register(router, &d, "Expiring", CLAUDE_CALLBACK).await;
    let (verifier, challenge) = pkce();
    let answer = consent(
        router,
        BASE,
        &user,
        &authorize_url(&d, &client, CLAUDE_CALLBACK, Some(&challenge), Some(MCP)),
    )
    .await;
    let code = code_from(&answer, CLAUDE_CALLBACK, &d.issuer).unwrap();
    let r = exchange(
        router,
        &d,
        &client,
        CLAUDE_CALLBACK,
        &code,
        Some(&verifier),
        MCP,
    )
    .await;
    let refresh_token = text(&r.body["refresh_token"]);
    sqlx::query("UPDATE oauth_tokens SET expires_at = $1 WHERE token_hash = $2")
        .bind(now() - 1)
        .bind(crate::tokens::secret_hash(&refresh_token))
        .execute(db.pool())
        .await
        .unwrap();
    let r = refresh(router, &d, &client, &refresh_token).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"], "invalid_grant");

    // A personal token (developer tools) still works side by side.
    let (status, made) = send(
        router,
        Some(&user.token),
        "POST",
        "/api/me/ai/tokens",
        json!({ "name": "cli" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let personal = text(&made["token"]);
    assert!(personal.starts_with("bpt_"));
    use_tools(router, &personal).await;
}

/// Wrong `resource`, missing PKCE, and the redirect answers that carry `iss`.
async fn refusals(router: &Router) {
    let d = discover(router, BASE).await;
    let user = sign_up(router, "connector-refusals").await;
    let client = register(router, &d, "ChatGPT", CHATGPT_CALLBACK).await;
    let (verifier, challenge) = pkce();

    // A resource that isn't this server's MCP endpoint: invalid_target, back at the app, with iss.
    for wrong in [
        "https://evil.example/api/mcp",
        "https://party.example.com/api/other",
        "https://party.example.com",
        "http://party.example.com/api/mcp",
    ] {
        let r = get(
            router,
            &authorize_url(&d, &client, CHATGPT_CALLBACK, Some(&challenge), Some(wrong)),
        )
        .await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{wrong}");
        let q = query_of(r.headers[header::LOCATION].to_str().unwrap());
        assert_eq!(q["error"], "invalid_target", "{wrong}");
        assert_eq!(q["iss"], BASE);
        assert_eq!(q["state"], "connector-state");
        assert!(!q.contains_key("code"));
    }

    // No PKCE, or not S256: refused before the consent screen.
    for (c, method) in [(None, None), (Some(challenge.as_str()), Some("plain"))] {
        let mut url = authorize_url(&d, &client, CHATGPT_CALLBACK, None, Some(MCP));
        if let Some(c) = c {
            url.push_str(&format!("&code_challenge={c}"));
        }
        if let Some(m) = method {
            url.push_str(&format!("&code_challenge_method={m}"));
        }
        let r = get(router, &url).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
        let q = query_of(r.headers[header::LOCATION].to_str().unwrap());
        assert_eq!(q["error"], "invalid_request");
        assert_eq!(q["iss"], BASE);
    }

    // A code without its verifier, or with another one, or for another resource: refused, and the code is spent.
    let answer = consent(
        router,
        BASE,
        &user,
        &authorize_url(&d, &client, CHATGPT_CALLBACK, Some(&challenge), Some(MCP)),
    )
    .await;
    let code = code_from(&answer, CHATGPT_CALLBACK, BASE).unwrap();
    let r = exchange(router, &d, &client, CHATGPT_CALLBACK, &code, None, MCP).await;
    assert_eq!(r.body["error"], "invalid_grant", "{}", r.body);
    let r = exchange(
        router,
        &d,
        &client,
        CHATGPT_CALLBACK,
        &code,
        Some(&verifier),
        MCP,
    )
    .await;
    assert_eq!(
        r.body["error"], "invalid_grant",
        "a code is good for one try"
    );
    let answer = consent(
        router,
        BASE,
        &user,
        &authorize_url(&d, &client, CHATGPT_CALLBACK, Some(&challenge), Some(MCP)),
    )
    .await;
    let code = code_from(&answer, CHATGPT_CALLBACK, BASE).unwrap();
    let r = exchange(
        router,
        &d,
        &client,
        CHATGPT_CALLBACK,
        &code,
        Some(&verifier),
        "https://evil.example/api/mcp",
    )
    .await;
    assert_eq!(r.body["error"], "invalid_target", "{}", r.body);

    // Denied consent: access_denied, still with iss.
    let r = get(
        router,
        &authorize_url(&d, &client, CHATGPT_CALLBACK, Some(&challenge), Some(MCP)),
    )
    .await;
    let to = r.headers[header::LOCATION].to_str().unwrap().to_string();
    let id = to.rsplit('/').next().unwrap();
    let (status, out) = send(
        router,
        Some(&user.token),
        "POST",
        &format!("/api/oauth/requests/{id}/deny"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let q = query_of(&text(&out["redirect"]));
    assert_eq!(q["error"], "access_denied");
    assert_eq!(q["iss"], BASE);
}

/// Mix-up (RFC 9207): two servers on one database. The connector expects `BASE`; an authorization answer from
/// `OTHER` carries `iss=OTHER`, so the connector refuses it before sending the code anywhere. And if a client sends
/// it anyway, the token it gets is for OTHER's endpoint, which this server refuses (RFC 8707 audience).
async fn issuer_mixup(ours: &Router, theirs: &Router) {
    let d = discover(ours, BASE).await;
    let other = discover(theirs, OTHER).await;
    assert_eq!(other.issuer, OTHER);
    let user = sign_up(theirs, "connector-mixup").await;
    let client = register(theirs, &other, "ChatGPT", CHATGPT_CALLBACK).await;
    let (verifier, challenge) = pkce();
    let answer = consent(
        theirs,
        OTHER,
        &user,
        &authorize_url(
            &other,
            &client,
            CHATGPT_CALLBACK,
            Some(&challenge),
            Some(&other.resource),
        ),
    )
    .await;
    let refused = code_from(&answer, CHATGPT_CALLBACK, &d.issuer).unwrap_err();
    assert!(refused.contains("iss mismatch"), "{refused}");

    let code = query_of(&answer)["code"].clone();
    // At our token endpoint, asking for our resource: the code was for theirs.
    let r = exchange(
        ours,
        &d,
        &client,
        CHATGPT_CALLBACK,
        &code,
        Some(&verifier),
        MCP,
    )
    .await;
    assert_eq!(r.body["error"], "invalid_target", "{}", r.body);
    // A fresh code, redeemed where it belongs: a token for OTHER's endpoint only.
    let answer = consent(
        theirs,
        OTHER,
        &user,
        &authorize_url(
            &other,
            &client,
            CHATGPT_CALLBACK,
            Some(&challenge),
            Some(&other.resource),
        ),
    )
    .await;
    let code = code_from(&answer, CHATGPT_CALLBACK, OTHER).unwrap();
    let r = exchange(
        theirs,
        &other,
        &client,
        CHATGPT_CALLBACK,
        &code,
        Some(&verifier),
        &other.resource,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let theirs_token = text(&r.body["access_token"]);
    assert_eq!(
        mcp(theirs, Some(&theirs_token), &initialize("2025-11-25"), &[])
            .await
            .status,
        StatusCode::OK
    );
    let r = mcp(ours, Some(&theirs_token), &initialize("2025-11-25"), &[]).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.body);
}

/// Browser origins: this server's own pages and the AI apps' are let through; anything else is 403, before auth.
async fn origins(router: &Router) {
    let user = sign_up(router, "connector-origins").await;
    let (_, made) = send(
        router,
        Some(&user.token),
        "POST",
        "/api/me/ai/tokens",
        json!({ "name": "origins" }),
    )
    .await;
    let token = text(&made["token"]);
    for allowed in [
        BASE,
        "https://chatgpt.com",
        "https://chat.openai.com",
        "https://claude.ai",
        "https://claude.com",
        "https://Claude.AI",
        "https://claude.ai/",
    ] {
        let r = mcp(
            router,
            Some(&token),
            &initialize("2025-11-25"),
            &[("origin", allowed)],
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{allowed}: {}", r.body);
        // Without a token, the same origin gets the 401 challenge, not a 403.
        let r = mcp(
            router,
            None,
            &initialize("2025-11-25"),
            &[("origin", allowed)],
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{allowed}");
    }
    for refused in [
        "https://evil.example",
        "https://claude.ai.evil.example",
        "https://evil-claude.ai",
        "https://sub.claude.ai",
        "http://claude.ai",
        "https://chatgpt.com:8443",
        "https://other.example.com",
        "null",
    ] {
        for token in [Some(token.as_str()), None] {
            let r = mcp(
                router,
                token,
                &initialize("2025-11-25"),
                &[("origin", refused)],
            )
            .await;
            assert_eq!(r.status, StatusCode::FORBIDDEN, "{refused}");
        }
    }
    // No stream and no session to end.
    for method in ["GET", "DELETE"] {
        let r = raw(
            router,
            Request::builder()
                .method(method)
                .uri("/api/mcp")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{method}");
        assert_eq!(r.headers[header::ALLOW], "POST");
    }
}

/// A personal token for `user`, made on the Account page.
async fn personal_token(router: &Router, user: &Client) -> String {
    let (status, made) = send(
        router,
        Some(&user.token),
        "POST",
        "/api/me/ai/tokens",
        json!({ "name": "revocation" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    text(&made["token"])
}

/// The connector path from the 401 to a token pair, for `user`: what ChatGPT does again after a revoke.
async fn connect(router: &Router, user: &Client) -> (String, String, String) {
    let d = discover(router, BASE).await;
    let client = register(router, &d, "ChatGPT", CHATGPT_CALLBACK).await;
    let (verifier, challenge) = pkce();
    let answer = consent(
        router,
        BASE,
        user,
        &authorize_url(&d, &client, CHATGPT_CALLBACK, Some(&challenge), Some(MCP)),
    )
    .await;
    let code = code_from(&answer, CHATGPT_CALLBACK, BASE).unwrap();
    let r = exchange(
        router,
        &d,
        &client,
        CHATGPT_CALLBACK,
        &code,
        Some(&verifier),
        MCP,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    (
        client,
        text(&r.body["access_token"]),
        text(&r.body["refresh_token"]),
    )
}

/// A second sign-in of `name` (another device).
async fn another_device(router: &Router, name: &str, password: &str) -> Client {
    let (status, out) = super::login(router, name, password).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    Client {
        router: router.clone(),
        token: text(&out["token"]),
        id: text(&out["user"]["id"]),
    }
}

async fn mcp_status(router: &Router, token: &str) -> StatusCode {
    let r = mcp(router, Some(token), &initialize("2025-11-25"), &[]).await;
    if r.status == StatusCode::UNAUTHORIZED {
        // Still the challenge a connector starts again from.
        assert!(www_authenticate(&r.headers).contains_key("resource_metadata"));
    }
    r.status
}

/// Everything `user` gave AI apps: a personal token and a connected app. Both work.
struct Access {
    personal: String,
    client: String,
    access: String,
    refresh: String,
}

async fn give_access(router: &Router, user: &Client) -> Access {
    let personal = personal_token(router, user).await;
    let (client, access, refresh) = connect(router, user).await;
    assert_eq!(mcp_status(router, &personal).await, StatusCode::OK);
    assert_eq!(mcp_status(router, &access).await, StatusCode::OK);
    Access {
        personal,
        client,
        access,
        refresh,
    }
}

/// Every token is refused, the refresh token is dead, and the connected app is gone from the Account page.
async fn assert_revoked(router: &Router, user: &Client, a: &Access) {
    assert_eq!(
        mcp_status(router, &a.personal).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        mcp_status(router, &a.access).await,
        StatusCode::UNAUTHORIZED
    );
    let d = discover(router, BASE).await;
    let r = refresh(router, &d, &a.client, &a.refresh).await;
    assert_eq!(r.body["error"], "invalid_grant", "{}", r.body);
    let (status, apps) = send(
        router,
        Some(&user.token),
        "GET",
        "/api/me/ai/apps",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{apps}");
    assert_eq!(apps["apps"], json!([]), "{apps}");
}

/// A password change, "Sign out everywhere", and the admin's reset and sign-out each end every personal token and
/// connected app of the account at once, while the session that asked stays. The connector then has to go through
/// OAuth again, and that works.
async fn revocation(router: &Router) {
    const NEW_PASSWORD: &str = "a brand new passphrase 9";

    // A password change.
    let me = sign_up(router, "revoke-password").await;
    let phone = another_device(router, "revoke-password", super::PASSWORD).await;
    let a = give_access(router, &me).await;
    let (status, out) = send(
        router,
        Some(&me.token),
        "POST",
        "/api/me/password",
        json!({ "current": super::PASSWORD, "password": NEW_PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_revoked(router, &me, &a).await;
    assert_eq!(me.get("/api/me").await.0, StatusCode::OK);
    assert_eq!(phone.get("/api/me").await.0, StatusCode::UNAUTHORIZED);
    // ChatGPT notices the 401 and connects again.
    let (_, access, _) = connect(router, &me).await;
    assert_eq!(mcp_status(router, &access).await, StatusCode::OK);
    use_tools(router, &access).await;

    // "Sign out everywhere" on the Account page.
    let me = sign_up(router, "revoke-everywhere").await;
    let phone = another_device(router, "revoke-everywhere", super::PASSWORD).await;
    let a = give_access(router, &me).await;
    let (status, out) = me.post("/api/me/sign-out-everywhere", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out, json!({ "sessions": 1, "apiTokens": 1, "apps": 1 }));
    assert_revoked(router, &me, &a).await;
    assert_eq!(me.get("/api/me").await.0, StatusCode::OK);
    assert_eq!(phone.get("/api/me").await.0, StatusCode::UNAUTHORIZED);
    let (_, access, _) = connect(router, &me).await;
    assert_eq!(mcp_status(router, &access).await, StatusCode::OK);
    // Nothing else left: the counts are zero.
    let (_, out) = me.post("/api/me/sign-out-everywhere", json!({})).await;
    assert_eq!(out, json!({ "sessions": 0, "apiTokens": 0, "apps": 1 }));
    assert_eq!(mcp_status(router, &access).await, StatusCode::UNAUTHORIZED);
    // Signed out, the route is 401.
    let (status, _) = send(
        router,
        None,
        "POST",
        "/api/me/sign-out-everywhere",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The admin page: sign-out, then a password reset. (The first account on the server is its admin.)
    let admin = another_device(router, "connector-dm", super::PASSWORD).await;
    let target = sign_up(router, "revoke-by-admin").await;
    let a = give_access(router, &target).await;
    let (status, out) = admin
        .post(
            &format!("/api/admin/accounts/{}/sign-out", target.id),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(
        (out["apiTokens"].clone(), out["apps"].clone()),
        (json!(1), json!(1)),
        "{out}"
    );
    let target = another_device(router, "revoke-by-admin", super::PASSWORD).await;
    assert_revoked(router, &target, &a).await;
    let a = give_access(router, &target).await;
    let (status, out) = admin
        .post(
            &format!("/api/admin/accounts/{}/password", target.id),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(
        mcp_status(router, &a.personal).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        mcp_status(router, &a.access).await,
        StatusCode::UNAUTHORIZED
    );
}

async fn everything(dir: &Path, db: &Db) {
    let (_, ours) = server(&dir.join("ours"), db.clone(), BASE);
    let (_, theirs) = server(&dir.join("theirs"), db.clone(), OTHER);
    connector_flow(&ours, db).await;
    refusals(&ours).await;
    issuer_mixup(&ours, &theirs).await;
    origins(&ours).await;
    revocation(&ours).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn chatgpt_and_claude_connect_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    everything(dir.path(), &db).await;
}

/// The same on PostgreSQL. Runs when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.
#[tokio::test(flavor = "multi_thread")]
async fn connectors_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = POSTGRES.lock().await;
    let dir = tempfile::tempdir().unwrap();
    drop_all(&url).await;
    let db = Db::open(&Location::Postgres(url.clone())).await.unwrap();
    everything(dir.path(), &db).await;
    db.pool().close().await;
    drop_all(&url).await;
}
