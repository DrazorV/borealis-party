//! The OAuth 2.1 authorization server for remote MCP clients such as ChatGPT connectors and claude.ai custom
//! connectors (plan 4.5a part A, D24). It follows the MCP authorization spec (2025-11-25):
//!
//! - authorization-server metadata (RFC 8414) at `/.well-known/oauth-authorization-server` and
//!   `/.well-known/openid-configuration` (each also under the MCP path), and protected-resource metadata (RFC 9728)
//!   for `/api/mcp` at `/.well-known/oauth-protected-resource/api/mcp` (and at the root);
//! - dynamic client registration (RFC 7591) at `/api/oauth/register`;
//! - the authorization-code grant with PKCE S256 required, the `iss` response parameter (RFC 9207), resource
//!   indicators (RFC 8707: tokens are for `<public_url>/api/mcp` only), and exact redirect-URI matching (loopback
//!   redirects may use any port, RFC 8252);
//! - a consent screen in the party app (`#/oauth/consent/<id>`), approved with the account's session (a header, never
//!   a cookie: no other site can approve for the user);
//! - access tokens for an hour and refresh tokens for 90 days that rotate on every use; a reused refresh token or code revokes
//!   the whole grant;
//! - revocation (RFC 7009) and the Account page's list of connected apps.
//!
//! Codes, tokens, client secrets and consent ids are stored as peppered hashes (`tokens::secret_hash`). The issuer is
//! `server.public_url`; without it these routes answer 404 (personal API tokens still work).

use std::collections::HashMap;

use axum::extract::{DefaultBodyLimit, FromRequest, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::{Form, Json};
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
use url::Url;

use crate::admin::record;
use crate::auth::{ClientIp, User};
use crate::db::{now, Activity};
use crate::error::{ApiError, ApiResult};
use crate::tokens::{
    mcp_resource, new_secret, same, secret_hash, take, Scopes, ACCESS_PREFIX, REFRESH_PREFIX,
};
use crate::AppState;

/// Seconds an access token lasts.
pub const ACCESS_SECONDS: i64 = 60 * 60;
/// Seconds a refresh token lasts; each refresh brings a new one, so a connection in use stays.
pub const REFRESH_SECONDS: i64 = 90 * 24 * 60 * 60;
/// Seconds an authorization code lasts.
pub const CODE_SECONDS: i64 = 5 * 60;
/// Seconds the consent screen waits for an answer.
pub const REQUEST_SECONDS: i64 = 10 * 60;
/// A registered client that never got a grant is removed after this many seconds.
const UNUSED_CLIENT_SECONDS: i64 = 30 * 24 * 60 * 60;
/// Registrations per client address per hour.
pub const REGISTRATIONS_PER_HOUR: usize = 20;
/// Token requests per client address per minute.
pub const TOKEN_CALLS_PER_MINUTE: usize = 60;
/// Authorization requests per client address per ten minutes.
const AUTHORIZE_PER_TEN_MINUTES: usize = 60;
/// Body limit of the form and JSON endpoints.
const SMALL_BODY: usize = 32 * 1024;
/// Prefix of an authorization code.
const CODE_PREFIX: &str = "bpc_";
/// Prefix of a client secret.
const CLIENT_SECRET_PREFIX: &str = "bps_";
/// Scope names clients add that mean nothing here (refresh tokens always come).
const IGNORED_SCOPES: [&str; 1] = ["offline_access"];
/// The token endpoint's ways of telling who the client is.
const AUTH_METHODS: [&str; 3] = ["none", "client_secret_post", "client_secret_basic"];
/// Schemes a redirect may never use: the consent page navigates to it.
const BAD_SCHEMES: [&str; 13] = [
    "javascript",
    "data",
    "file",
    "vbscript",
    "about",
    "blob",
    "ws",
    "wss",
    "ftp",
    "view-source",
    "mailto",
    "tel",
    "filesystem",
];

/// The routes, unless `BOREALIS__FEATURES__AI=false`.
pub fn routes() -> axum::Router<AppState> {
    routes_if(crate::config::features().ai)
}

/// The routes when `on`; none otherwise (404).
pub(crate) fn routes_if(on: bool) -> axum::Router<AppState> {
    use axum::routing::{delete, get, post};
    if !on {
        return axum::Router::new();
    }
    let machine = axum::Router::new()
        .route("/api/oauth/register", post(register))
        .route("/api/oauth/token", post(token))
        .route("/api/oauth/revoke", post(revoke))
        .route("/api/oauth/requests/{request}/approve", post(approve))
        .route("/api/oauth/requests/{request}/deny", post(deny))
        .layer(DefaultBodyLimit::max(SMALL_BODY));
    axum::Router::new()
        // Every place a client may look (MCP authorization 2025-11-25 "Authorization Server Metadata Discovery",
        // RFC 8414 3.1, RFC 9728 3.1, OIDC Discovery 4): the root forms, the path-inserted forms for the MCP endpoint
        // and the older suffix forms some clients still try. The same two documents everywhere.
        .route(
            "/.well-known/oauth-authorization-server",
            get(server_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server/api/mcp",
            get(server_metadata),
        )
        .route(
            "/api/mcp/.well-known/oauth-authorization-server",
            get(server_metadata),
        )
        .route("/.well-known/openid-configuration", get(server_metadata))
        .route(
            "/.well-known/openid-configuration/api/mcp",
            get(server_metadata),
        )
        .route(
            "/api/mcp/.well-known/openid-configuration",
            get(server_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(resource_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/api/mcp",
            get(resource_metadata),
        )
        .route(
            "/api/mcp/.well-known/oauth-protected-resource",
            get(resource_metadata),
        )
        .route("/api/oauth/authorize", get(authorize))
        .route("/api/oauth/requests/{request}", get(request_detail))
        .route("/api/me/ai/apps", get(apps))
        .route("/api/me/ai/apps/{grant}", delete(disconnect))
        .merge(machine)
}

fn not_offered() -> ApiError {
    ApiError::NotFound(
        "This server has no public address (server.public_url), so apps can't connect with OAuth. Use an API token."
            .into(),
    )
}

fn issuer(state: &AppState) -> ApiResult<&str> {
    state.config.public_url.as_deref().ok_or_else(not_offered)
}

// Metadata.

/// `GET /.well-known/oauth-authorization-server` (RFC 8414), also served as `/.well-known/openid-configuration`.
///
/// The OIDC form is the same OAuth document: no ID tokens are issued (there is no `openid` scope), but a client that
/// tries OIDC discovery finds the endpoints and `code_challenge_methods_supported`, which the MCP spec requires there.
pub async fn server_metadata(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let base = issuer(&state)?;
    Ok(Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/api/oauth/authorize"),
        "token_endpoint": format!("{base}/api/oauth/token"),
        "registration_endpoint": format!("{base}/api/oauth/register"),
        "revocation_endpoint": format!("{base}/api/oauth/revoke"),
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": AUTH_METHODS,
        "revocation_endpoint_auth_methods_supported": AUTH_METHODS,
        "scopes_supported": Scopes::all().names(),
        "authorization_response_iss_parameter_supported": true,
        "client_id_metadata_document_supported": false,
        "subject_types_supported": ["public"],
    })))
}

/// `GET /.well-known/oauth-protected-resource[/api/mcp]` (RFC 9728): the MCP endpoint and who issues its tokens.
pub async fn resource_metadata(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let base = issuer(&state)?;
    Ok(Json(json!({
        "resource": format!("{base}/api/mcp"),
        "resource_name": "Borealis Connect",
        "authorization_servers": [base],
        "scopes_supported": Scopes::all().names(),
        "bearer_methods_supported": ["header"],
    })))
}

// Errors in OAuth's own shape.

/// An OAuth error answer (RFC 6749 5.2): `{ error, error_description }`.
#[derive(Debug)]
pub struct OAuthError {
    status: StatusCode,
    error: &'static str,
    description: String,
    /// Ask for HTTP Basic again (a client that sent it got it wrong).
    basic: bool,
}

impl OAuthError {
    fn new(error: &'static str, description: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error,
            description: description.into(),
            basic: false,
        }
    }

    fn client(basic: bool) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            basic,
            ..Self::new(
                "invalid_client",
                "The client is unknown or its secret is wrong.",
            )
        }
    }

    fn grant(description: &str) -> Self {
        Self::new("invalid_grant", description)
    }

    fn too_many() -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            ..Self::new(
                "slow_down",
                "Too many requests. Wait a minute and try again.",
            )
        }
    }
}

impl From<ApiError> for OAuthError {
    fn from(e: ApiError) -> Self {
        let status = e.into_response().status();
        Self {
            status,
            ..Self::new("server_error", "Something went wrong on the server.")
        }
    }
}

impl From<sqlx::Error> for OAuthError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::from(e).into()
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(json!({ "error": self.error, "error_description": self.description })),
        )
            .into_response();
        let headers = response.headers_mut();
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if self.basic {
            headers.insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Basic realm=\"borealis-party\""),
            );
        }
        response
    }
}

type OAuthResult<T> = Result<T, OAuthError>;

fn no_store<T: IntoResponse>(body: T) -> Response {
    let mut response = body.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

/// Form or query parameters, each at most once (RFC 6749 3.1: a repeated parameter is an error).
fn single(pairs: Vec<(String, String)>) -> OAuthResult<HashMap<String, String>> {
    let mut map = HashMap::new();
    for (key, value) in pairs {
        if map.insert(key.clone(), value).is_some() {
            return Err(OAuthError::new(
                "invalid_request",
                format!("{key} is given more than once."),
            ));
        }
    }
    Ok(map)
}

// Redirect URIs.

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// A redirect URI a client may register: https anywhere, http on this machine only (loopback, RFC 8252), or an app's
/// own scheme (`cursor://…`). Never a fragment, credentials, or a scheme a browser would run.
fn redirect_ok(text: &str) -> Result<Url, String> {
    if text.len() > 1000 {
        return Err("A redirect URI is at most 1000 characters.".into());
    }
    let url = Url::parse(text).map_err(|_| format!("{text:?} is not an absolute URL."))?;
    if url.fragment().is_some() || !url.username().is_empty() || url.password().is_some() {
        return Err("A redirect URI has no fragment or credentials.".into());
    }
    match url.scheme() {
        "https" if url.host_str().is_some() => Ok(url),
        "http" if url.host_str().is_some_and(is_loopback) => Ok(url),
        "http" | "https" => Err("Use https, or http on 127.0.0.1 or localhost only.".into()),
        scheme if BAD_SCHEMES.contains(&scheme) => {
            Err(format!("The {scheme}: scheme can't be a redirect."))
        }
        _ => Ok(url),
    }
}

/// Whether `given` is one of `registered`: exactly, or for a loopback http redirect, the same but for the port.
fn redirect_matches(registered: &[String], given: &str) -> bool {
    if registered.iter().any(|r| r == given) {
        return true;
    }
    let Ok(g) = Url::parse(given) else {
        return false;
    };
    if g.scheme() != "http" || !g.host_str().is_some_and(is_loopback) {
        return false;
    }
    registered.iter().any(|r| {
        Url::parse(r).is_ok_and(|r| {
            r.scheme() == "http"
                && r.host_str() == g.host_str()
                && r.path() == g.path()
                && r.query() == g.query()
                && r.fragment().is_none()
        })
    })
}

/// `redirect` with the answer's parameters added, and `iss` (RFC 9207).
fn redirect_with(
    redirect: &str,
    params: &[(&str, &str)],
    state: Option<&str>,
    iss: &str,
) -> String {
    let Ok(mut url) = Url::parse(redirect) else {
        return redirect.to_string();
    };
    {
        let mut q = url.query_pairs_mut();
        for (k, v) in params {
            q.append_pair(k, v);
        }
        if let Some(state) = state {
            q.append_pair("state", state);
        }
        q.append_pair("iss", iss);
    }
    url.to_string()
}

fn host_of(redirect: &str) -> String {
    Url::parse(redirect)
        .ok()
        .map(|u| match u.host_str() {
            Some(host) => format!("{}://{host}", u.scheme()),
            None => format!("{}:", u.scheme()),
        })
        .unwrap_or_default()
}

// Clients.

struct Client {
    id: String,
    secret_hash: Option<String>,
    name: String,
    client_uri: Option<String>,
    redirect_uris: Vec<String>,
}

async fn client(state: &AppState, id: &str) -> ApiResult<Option<Client>> {
    if id.is_empty() || id.len() > 100 {
        return Ok(None);
    }
    let row = sqlx::query(
        "SELECT id, secret_hash, name, client_uri, redirect_uris_json FROM oauth_clients WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(state.db.pool())
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Client {
        id: row.try_get(0)?,
        secret_hash: row.try_get(1)?,
        name: row.try_get(2)?,
        client_uri: row.try_get(3)?,
        redirect_uris: serde_json::from_str(&row.try_get::<String, _>(4)?).unwrap_or_default(),
    }))
}

fn text_field(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(str::to_string)
}

fn clean_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).take(100).collect();
    let name = name.trim().to_string();
    if name.is_empty() {
        "Unnamed app".into()
    } else {
        name
    }
}

fn metadata_error(description: impl Into<String>) -> OAuthError {
    OAuthError::new("invalid_client_metadata", description)
}

/// `POST /api/oauth/register` (RFC 7591): anyone may register an app; it can do nothing until an account consents.
pub async fn register(
    State(state): State<AppState>,
    ip: ClientIp,
    request: Request,
) -> OAuthResult<Response> {
    issuer(&state)?;
    if !take(
        &format!("oauth-register:{}", ip.limit_key()),
        REGISTRATIONS_PER_HOUR,
        60 * 60,
    ) {
        return Err(OAuthError::too_many());
    }
    let Json(body) = Json::<Value>::from_request(request, &state)
        .await
        .map_err(|_| metadata_error("Send the client metadata as a JSON object."))?;
    if !body.is_object() {
        return Err(metadata_error("Send the client metadata as a JSON object."));
    }
    let uris: Vec<String> = match body.get("redirect_uris") {
        Some(Value::Array(list)) => list
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect::<Option<_>>()
            .ok_or_else(|| {
                OAuthError::new("invalid_redirect_uri", "redirect_uris holds strings only.")
            })?,
        _ => {
            return Err(OAuthError::new(
                "invalid_redirect_uri",
                "redirect_uris is required.",
            ))
        }
    };
    if uris.is_empty() || uris.len() > 10 {
        return Err(OAuthError::new(
            "invalid_redirect_uri",
            "Register 1 to 10 redirect URIs.",
        ));
    }
    for uri in &uris {
        redirect_ok(uri).map_err(|m| OAuthError::new("invalid_redirect_uri", m))?;
    }
    // RFC 7591 2: client_secret_basic when not said.
    let method = text_field(&body, "token_endpoint_auth_method")
        .unwrap_or_else(|| "client_secret_basic".into());
    if !AUTH_METHODS.contains(&method.as_str()) {
        return Err(metadata_error(format!(
            "token_endpoint_auth_method must be one of {}.",
            AUTH_METHODS.join(", ")
        )));
    }
    let grant_types: Vec<String> = match body.get("grant_types") {
        None | Some(Value::Null) => vec!["authorization_code".into(), "refresh_token".into()],
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|_| metadata_error("grant_types holds strings only."))?,
    };
    if grant_types
        .iter()
        .any(|g| g != "authorization_code" && g != "refresh_token")
    {
        return Err(metadata_error(
            "Only the authorization_code and refresh_token grants are offered.",
        ));
    }
    if let Some(v) = body.get("response_types").filter(|v| !v.is_null()) {
        let types: Vec<String> = serde_json::from_value(v.clone())
            .map_err(|_| metadata_error("response_types holds strings only."))?;
        if types.iter().any(|t| t != "code") {
            return Err(metadata_error("Only the code response type is offered."));
        }
    }
    let name = clean_name(&text_field(&body, "client_name").unwrap_or_default());
    let client_uri = text_field(&body, "client_uri")
        .filter(|u| u.len() <= 500)
        .filter(|u| Url::parse(u).is_ok_and(|u| u.scheme() == "https"));
    let at = now();
    // Registrations nobody ever used.
    sqlx::query(
        "DELETE FROM oauth_clients WHERE created_at < $1
         AND NOT EXISTS (SELECT 1 FROM oauth_grants g WHERE g.client_id = oauth_clients.id)",
    )
    .bind(at - UNUSED_CLIENT_SECONDS)
    .execute(state.db.pool())
    .await?;
    let id = uuid::Uuid::new_v4().to_string();
    let secret = (method != "none").then(|| new_secret(CLIENT_SECRET_PREFIX));
    sqlx::query(
        "INSERT INTO oauth_clients (id, secret_hash, name, client_uri, redirect_uris_json, auth_method, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&id)
    .bind(secret.as_deref().map(secret_hash))
    .bind(&name)
    .bind(&client_uri)
    .bind(serde_json::to_string(&uris).unwrap_or_else(|_| "[]".into()))
    .bind(&method)
    .bind(at)
    .execute(state.db.pool())
    .await?;
    let mut out = json!({
        "client_id": id,
        "client_id_issued_at": at,
        "client_name": name,
        "redirect_uris": uris,
        "token_endpoint_auth_method": method,
        "grant_types": grant_types,
        "response_types": ["code"],
    });
    if let Some(uri) = client_uri {
        out["client_uri"] = json!(uri);
    }
    if let Some(secret) = secret {
        out["client_secret"] = json!(secret);
        out["client_secret_expires_at"] = json!(0);
    }
    Ok(no_store((StatusCode::CREATED, Json(out))))
}

// Authorization.

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// A page for an authorization request that can't be sent back to the app (unknown app or redirect).
fn error_page(text: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" \
         content=\"width=device-width, initial-scale=1\"><title>Can't connect the app</title></head>\
         <body style=\"font-family: system-ui, sans-serif; max-width: 32rem; margin: 4rem auto; padding: 0 16px; \
         line-height: 1.5\"><h1 style=\"font-size: 1.4rem\">Can't connect the app</h1><p>{}</p>\
         <p><a href=\"/\">Open Borealis Connect</a></p></body></html>",
        escape(text)
    );
    no_store((StatusCode::BAD_REQUEST, Html(html)))
}

fn pkce_challenge_ok(challenge: &str) -> bool {
    challenge.len() == 43
        && challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn verifier_ok(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
}

/// BASE64URL(SHA256(verifier)) == challenge (RFC 7636 S256), compared in constant time.
fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    let made = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier));
    same(made.as_bytes(), challenge.as_bytes())
}

/// `resource` as given, which must be this server's MCP endpoint (RFC 8707), or that endpoint when absent.
fn resource_arg(state: &AppState, given: Option<&String>) -> Result<String, String> {
    let ours = mcp_resource(state).ok_or_else(|| "no public address".to_string())?;
    match given.map(|r| r.trim_end_matches('/')) {
        None => Ok(ours),
        Some(r) if r == ours => Ok(ours),
        Some(_) => Err(format!("This server issues tokens for {ours} only.")),
    }
}

/// `GET /api/oauth/authorize`: check the request, keep it, and open the consent screen in the app.
pub async fn authorize(
    State(state): State<AppState>,
    ip: ClientIp,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Response {
    let Some(base) = state.config.public_url.clone() else {
        return not_offered().into_response();
    };
    if !take(
        &format!("oauth-authorize:{}", ip.limit_key()),
        AUTHORIZE_PER_TEN_MINUTES,
        10 * 60,
    ) {
        return error_page("Too many requests. Wait a few minutes and try again.");
    }
    let params = match single(pairs) {
        Ok(p) => p,
        Err(e) => return error_page(&e.description),
    };
    let found = match client(&state, params.get("client_id").map_or("", String::as_str)).await {
        Ok(found) => found,
        Err(e) => return e.into_response(),
    };
    let Some(app) = found else {
        return error_page(
            "This app isn't registered with this server. Add the connector again in the app.",
        );
    };
    let redirect = match params.get("redirect_uri") {
        Some(r) if redirect_matches(&app.redirect_uris, r) => r.clone(),
        Some(_) => {
            return error_page("The app asked to come back to an address it did not register.")
        }
        None if app.redirect_uris.len() == 1 => app.redirect_uris[0].clone(),
        None => return error_page("The app did not say where to come back to (redirect_uri)."),
    };
    let client_state = params.get("state").cloned();
    if client_state.as_ref().is_some_and(|s| s.len() > 1024) {
        return error_page("The app's state parameter is too long.");
    }
    let back = |error: &str, description: &str| {
        Redirect::to(&redirect_with(
            &redirect,
            &[("error", error), ("error_description", description)],
            client_state.as_deref(),
            &base,
        ))
        .into_response()
    };
    if params.get("response_type").map(String::as_str) != Some("code") {
        return back(
            "unsupported_response_type",
            "Only response_type=code is offered.",
        );
    }
    let challenge = params.get("code_challenge").cloned().unwrap_or_default();
    if params.get("code_challenge_method").map(String::as_str) != Some("S256")
        || !pkce_challenge_ok(&challenge)
    {
        return back(
            "invalid_request",
            "PKCE with code_challenge_method=S256 is required.",
        );
    }
    let scopes = match params.get("scope") {
        None => Scopes::defaults(),
        Some(text) => match Scopes::parse(text, &IGNORED_SCOPES) {
            Ok(s) if s.is_empty() => Scopes::defaults(),
            Ok(s) => s,
            Err(word) => return back("invalid_scope", &format!("There is no scope {word:?}.")),
        },
    };
    let resource = match resource_arg(&state, params.get("resource")) {
        Ok(r) => r,
        Err(m) => return back("invalid_target", &m),
    };
    let at = now();
    let id = new_secret("");
    let saved = async {
        sqlx::query("DELETE FROM oauth_requests WHERE expires_at < $1")
            .bind(at)
            .execute(state.db.pool())
            .await?;
        sqlx::query(
            "INSERT INTO oauth_requests (id_hash, client_id, redirect_uri, scopes, state, code_challenge, resource,
             created_at, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(secret_hash(&id))
        .bind(&app.id)
        .bind(&redirect)
        .bind(scopes.to_text())
        .bind(&client_state)
        .bind(&challenge)
        .bind(&resource)
        .bind(at)
        .bind(at + REQUEST_SECONDS)
        .execute(state.db.pool())
        .await
    };
    if let Err(e) = saved.await {
        return ApiError::from(e).into_response();
    }
    no_store(Redirect::to(&format!("{base}/#/oauth/consent/{id}")))
}

struct Pending {
    client: Client,
    redirect_uri: String,
    scopes: Scopes,
    state: Option<String>,
    code_challenge: String,
    resource: String,
    expires_at: i64,
}

fn request_gone() -> ApiError {
    ApiError::NotFound(
        "That connection request has expired or was answered already. Start again in the app."
            .into(),
    )
}

async fn pending(state: &AppState, id: &str) -> ApiResult<(String, Pending)> {
    issuer(state)?;
    let hash = secret_hash(id);
    let row = sqlx::query(
        "SELECT client_id, redirect_uri, scopes, state, code_challenge, resource, expires_at FROM oauth_requests
         WHERE id_hash = $1 AND expires_at > $2",
    )
    .bind(&hash)
    .bind(now())
    .fetch_optional(state.db.pool())
    .await?
    .ok_or_else(request_gone)?;
    let app = client(state, &row.try_get::<String, _>(0)?)
        .await?
        .ok_or_else(request_gone)?;
    Ok((
        hash,
        Pending {
            client: app,
            redirect_uri: row.try_get(1)?,
            scopes: Scopes::stored(&row.try_get::<String, _>(2)?),
            state: row.try_get(3)?,
            code_challenge: row.try_get(4)?,
            resource: row.try_get(5)?,
            expires_at: row.try_get(6)?,
        },
    ))
}

fn scope_list(scopes: &Scopes) -> Vec<Value> {
    scopes
        .iter()
        .map(|s| json!({ "id": s.as_str(), "label": s.label(), "default": s.default_on() }))
        .collect()
}

/// `GET /api/oauth/requests/{request}`: what the consent screen shows.
pub async fn request_detail(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let (_, p) = pending(&state, &id).await?;
    Ok(no_store(Json(json!({
        "client": {
            "name": p.client.name,
            "uri": p.client.client_uri,
            "redirectUri": p.redirect_uri,
            "redirectHost": host_of(&p.redirect_uri),
        },
        "scopes": scope_list(&p.scopes),
        "account": { "username": user.username },
        "expiresAt": p.expires_at,
    }))))
}

/// Take the request out (once only): `None` when it was answered meanwhile.
async fn take_request(state: &AppState, hash: &str) -> ApiResult<bool> {
    let done = sqlx::query("DELETE FROM oauth_requests WHERE id_hash = $1")
        .bind(hash)
        .execute(state.db.pool())
        .await?;
    Ok(done.rows_affected() == 1)
}

/// The consent form.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConsentForm {
    /// The scopes the account allows: some of those asked for.
    scopes: Vec<String>,
}

/// `POST /api/oauth/requests/{request}/approve`: the account allows the app; the answer is where to send the browser.
pub async fn approve(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    Json(form): Json<ConsentForm>,
) -> ApiResult<Response> {
    let base = issuer(&state)?.to_string();
    let (hash, p) = pending(&state, &id).await?;
    let scopes = Scopes::from_names(&form.scopes)?;
    if scopes.is_empty() {
        return Err(ApiError::Invalid(
            "Allow at least one thing, or choose Deny.".into(),
        ));
    }
    if !scopes.within(&p.scopes) {
        return Err(ApiError::Invalid(
            "You can only allow what the app asked for.".into(),
        ));
    }
    if !take_request(&state, &hash).await? {
        return Err(request_gone());
    }
    let code = new_secret(CODE_PREFIX);
    let at = now();
    sqlx::query(
        "INSERT INTO oauth_codes (code_hash, client_id, user_id, redirect_uri, scopes, code_challenge, resource,
         created_at, expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(secret_hash(&code))
    .bind(&p.client.id)
    .bind(&user.id)
    .bind(&p.redirect_uri)
    .bind(scopes.to_text())
    .bind(&p.code_challenge)
    .bind(&p.resource)
    .bind(at)
    .bind(at + CODE_SECONDS)
    .execute(state.db.pool())
    .await?;
    record(
        &state,
        Activity::new("ai_app_approved")
            .by(&user)
            .detail(scopes.to_text()),
    )
    .await;
    let redirect = redirect_with(
        &p.redirect_uri,
        &[("code", &code)],
        p.state.as_deref(),
        &base,
    );
    Ok(no_store(Json(json!({ "redirect": redirect }))))
}

/// `POST /api/oauth/requests/{request}/deny`: the account refuses; the app is told `access_denied`.
pub async fn deny(
    State(state): State<AppState>,
    _user: User,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let base = issuer(&state)?.to_string();
    let (hash, p) = pending(&state, &id).await?;
    if !take_request(&state, &hash).await? {
        return Err(request_gone());
    }
    let redirect = redirect_with(
        &p.redirect_uri,
        &[
            ("error", "access_denied"),
            ("error_description", "The account did not allow the app."),
        ],
        p.state.as_deref(),
        &base,
    );
    Ok(no_store(Json(json!({ "redirect": redirect }))))
}

// The token endpoint.

/// The client a token or revocation request is from: HTTP Basic (RFC 6749 2.3.1), or `client_id` and
/// `client_secret` in the form.
async fn authenticate_client(
    state: &AppState,
    headers: &HeaderMap,
    params: &HashMap<String, String>,
) -> OAuthResult<Client> {
    let basic = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "));
    let (id, secret, by_header) = if let Some(b64) = basic {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or_else(|| OAuthError::client(true))?;
        let (id, secret) = decoded
            .split_once(':')
            .ok_or_else(|| OAuthError::client(true))?;
        let decode = |s: &str| {
            url::form_urlencoded::parse(format!("x={s}").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default()
        };
        if params.contains_key("client_secret") {
            return Err(OAuthError::new(
                "invalid_request",
                "Send the client secret one way only.",
            ));
        }
        (decode(id), Some(decode(secret)), true)
    } else {
        (
            params.get("client_id").cloned().unwrap_or_default(),
            params.get("client_secret").cloned(),
            false,
        )
    };
    if let (true, Some(form_id)) = (by_header, params.get("client_id")) {
        if *form_id != id {
            return Err(OAuthError::client(true));
        }
    }
    let app = client(state, &id)
        .await?
        .ok_or_else(|| OAuthError::client(by_header))?;
    match (&app.secret_hash, secret) {
        (None, None) => Ok(app),
        (Some(stored), Some(given)) if same(stored.as_bytes(), secret_hash(&given).as_bytes()) => {
            Ok(app)
        }
        _ => Err(OAuthError::client(by_header)),
    }
}

/// Revoke a grant and everything issued under it.
async fn revoke_grant(state: &AppState, grant: &str) -> ApiResult<()> {
    sqlx::query("DELETE FROM oauth_grants WHERE id = $1")
        .bind(grant)
        .execute(state.db.pool())
        .await?;
    Ok(())
}

async fn user_active(state: &AppState, user: &str) -> ApiResult<bool> {
    Ok(
        sqlx::query("SELECT 1 FROM users WHERE id = $1 AND status = 'active'")
            .bind(user)
            .fetch_optional(state.db.pool())
            .await?
            .is_some(),
    )
}

/// A new access token, and a new refresh token, under `grant`.
async fn issue(
    state: &AppState,
    grant: &str,
    access: &Scopes,
    refresh: &Scopes,
) -> ApiResult<Value> {
    let at = now();
    let access_token = new_secret(ACCESS_PREFIX);
    let refresh_token = new_secret(REFRESH_PREFIX);
    let mut tx = state.db.pool().begin().await?;
    // Spent and expired ones of this grant are only clutter (a spent refresh token is kept until it expires, so
    // a second use is still recognised).
    sqlx::query("DELETE FROM oauth_tokens WHERE grant_id = $1 AND expires_at <= $2")
        .bind(grant)
        .bind(at)
        .execute(&mut *tx)
        .await?;
    for (token, kind, scopes, seconds) in [
        (&access_token, "access", access, ACCESS_SECONDS),
        (&refresh_token, "refresh", refresh, REFRESH_SECONDS),
    ] {
        sqlx::query(
            "INSERT INTO oauth_tokens (token_hash, grant_id, kind, scopes, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(secret_hash(token))
        .bind(grant)
        .bind(kind)
        .bind(scopes.to_text())
        .bind(at)
        .bind(at + seconds)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": ACCESS_SECONDS,
        "refresh_token": refresh_token,
        "scope": access.to_text(),
    }))
}

async fn code_grant(
    state: &AppState,
    app: &Client,
    params: &HashMap<String, String>,
) -> OAuthResult<Value> {
    let code = params
        .get("code")
        .ok_or_else(|| OAuthError::new("invalid_request", "code is required."))?;
    let hash = secret_hash(code);
    let at = now();
    let row = sqlx::query(
        "SELECT client_id, user_id, redirect_uri, scopes, code_challenge, resource, expires_at, used_at, grant_id
         FROM oauth_codes WHERE code_hash = $1",
    )
    .bind(&hash)
    .fetch_optional(state.db.pool())
    .await?
    .ok_or_else(|| OAuthError::grant("The code is unknown, used or expired."))?;
    let client_id: String = row.try_get(0)?;
    if client_id != app.id {
        return Err(OAuthError::grant("The code is unknown, used or expired."));
    }
    // One use only. A second one means it leaked: whatever the first gave is revoked (OAuth 2.1 4.1.3).
    let first =
        sqlx::query("UPDATE oauth_codes SET used_at = $1 WHERE code_hash = $2 AND used_at IS NULL")
            .bind(at)
            .bind(&hash)
            .execute(state.db.pool())
            .await?
            .rows_affected()
            == 1;
    if !first {
        if let Some(grant) = row.try_get::<Option<String>, _>(8)? {
            revoke_grant(state, &grant).await?;
        }
        return Err(OAuthError::grant("The code is unknown, used or expired."));
    }
    let user: String = row.try_get(1)?;
    let redirect: String = row.try_get(2)?;
    let scopes = Scopes::stored(&row.try_get::<String, _>(3)?);
    let challenge: String = row.try_get(4)?;
    let resource: String = row.try_get(5)?;
    let expires_at: i64 = row.try_get(6)?;
    if expires_at <= at {
        return Err(OAuthError::grant("The code is unknown, used or expired."));
    }
    if params.get("redirect_uri").is_some_and(|r| *r != redirect) {
        return Err(OAuthError::grant(
            "redirect_uri is not the one the code was given for.",
        ));
    }
    let verifier = params.get("code_verifier").map_or("", String::as_str);
    if !verifier_ok(verifier) || !pkce_matches(verifier, &challenge) {
        return Err(OAuthError::grant(
            "The code_verifier does not match the code_challenge.",
        ));
    }
    if let Some(given) = params.get("resource") {
        if given.trim_end_matches('/') != resource {
            return Err(OAuthError::new(
                "invalid_target",
                "resource is not the one the code was given for.",
            ));
        }
    }
    if !user_active(state, &user).await? {
        return Err(OAuthError::grant("The account can't be used now."));
    }
    let grant = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO oauth_grants (id, user_id, client_id, scopes, resource, created_at) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&grant)
    .bind(&user)
    .bind(&app.id)
    .bind(scopes.to_text())
    .bind(&resource)
    .bind(at)
    .execute(state.db.pool())
    .await?;
    sqlx::query("UPDATE oauth_codes SET grant_id = $1 WHERE code_hash = $2")
        .bind(&grant)
        .bind(&hash)
        .execute(state.db.pool())
        .await?;
    sqlx::query("DELETE FROM oauth_codes WHERE expires_at < $1")
        .bind(at - 60 * 60)
        .execute(state.db.pool())
        .await?;
    Ok(issue(state, &grant, &scopes, &scopes).await?)
}

async fn refresh_grant(
    state: &AppState,
    app: &Client,
    params: &HashMap<String, String>,
) -> OAuthResult<Value> {
    let token = params
        .get("refresh_token")
        .ok_or_else(|| OAuthError::new("invalid_request", "refresh_token is required."))?;
    let dead = || OAuthError::grant("The refresh token is unknown, used, expired or revoked.");
    if !token.starts_with(REFRESH_PREFIX) {
        return Err(dead());
    }
    let hash = secret_hash(token);
    let at = now();
    let row = sqlx::query(
        "SELECT g.id, g.client_id, g.user_id, g.scopes, g.resource, k.expires_at, k.used_at
         FROM oauth_tokens k JOIN oauth_grants g ON g.id = k.grant_id
         WHERE k.token_hash = $1 AND k.kind = 'refresh'",
    )
    .bind(&hash)
    .fetch_optional(state.db.pool())
    .await?
    .ok_or_else(dead)?;
    let grant: String = row.try_get(0)?;
    if row.try_get::<String, _>(1)? != app.id {
        return Err(dead());
    }
    // Rotation: each refresh token works once. A second use means it leaked: the grant goes (OAuth 2.1 4.3.1).
    let first = sqlx::query(
        "UPDATE oauth_tokens SET used_at = $1 WHERE token_hash = $2 AND used_at IS NULL",
    )
    .bind(at)
    .bind(&hash)
    .execute(state.db.pool())
    .await?
    .rows_affected()
        == 1;
    if !first {
        revoke_grant(state, &grant).await?;
        return Err(dead());
    }
    if row.try_get::<i64, _>(5)? <= at {
        return Err(dead());
    }
    let user: String = row.try_get(2)?;
    let granted = Scopes::stored(&row.try_get::<String, _>(3)?);
    let resource: String = row.try_get(4)?;
    if let Some(given) = params.get("resource") {
        if given.trim_end_matches('/') != resource {
            return Err(OAuthError::new(
                "invalid_target",
                "resource is not the one the app was allowed.",
            ));
        }
    }
    let access = match params.get("scope") {
        None => granted.clone(),
        Some(text) => {
            let asked = Scopes::parse(text, &IGNORED_SCOPES).map_err(|w| {
                OAuthError::new("invalid_scope", format!("There is no scope {w:?}."))
            })?;
            if !asked.within(&granted) {
                return Err(OAuthError::new(
                    "invalid_scope",
                    "That is more than the account allowed.",
                ));
            }
            if asked.is_empty() {
                granted.clone()
            } else {
                asked
            }
        }
    };
    if !user_active(state, &user).await? {
        return Err(dead());
    }
    Ok(issue(state, &grant, &access, &granted).await?)
}

/// `POST /api/oauth/token` (form): `authorization_code` with PKCE, or `refresh_token`.
pub async fn token(
    State(state): State<AppState>,
    ip: ClientIp,
    headers: HeaderMap,
    Form(pairs): Form<Vec<(String, String)>>,
) -> OAuthResult<Response> {
    issuer(&state)?;
    if !take(
        &format!("oauth-token:{}", ip.limit_key()),
        TOKEN_CALLS_PER_MINUTE,
        60,
    ) {
        return Err(OAuthError::too_many());
    }
    let params = single(pairs)?;
    let app = authenticate_client(&state, &headers, &params).await?;
    let out = match params.get("grant_type").map(String::as_str) {
        Some("authorization_code") => code_grant(&state, &app, &params).await?,
        Some("refresh_token") => refresh_grant(&state, &app, &params).await?,
        _ => {
            return Err(OAuthError::new(
                "unsupported_grant_type",
                "grant_type is authorization_code or refresh_token.",
            ))
        }
    };
    Ok(no_store(Json(out)))
}

/// `POST /api/oauth/revoke` (RFC 7009): a refresh token ends the whole grant, an access token only itself. Always
/// 200, whatever the token was.
pub async fn revoke(
    State(state): State<AppState>,
    ip: ClientIp,
    headers: HeaderMap,
    Form(pairs): Form<Vec<(String, String)>>,
) -> OAuthResult<Response> {
    issuer(&state)?;
    if !take(
        &format!("oauth-token:{}", ip.limit_key()),
        TOKEN_CALLS_PER_MINUTE,
        60,
    ) {
        return Err(OAuthError::too_many());
    }
    let params = single(pairs)?;
    let app = authenticate_client(&state, &headers, &params).await?;
    let token = params
        .get("token")
        .ok_or_else(|| OAuthError::new("invalid_request", "token is required."))?;
    let row = sqlx::query(
        "SELECT k.kind, g.id FROM oauth_tokens k JOIN oauth_grants g ON g.id = k.grant_id
         WHERE k.token_hash = $1 AND g.client_id = $2",
    )
    .bind(secret_hash(token))
    .bind(&app.id)
    .fetch_optional(state.db.pool())
    .await?;
    if let Some(row) = row {
        let kind: String = row.try_get(0)?;
        let grant: String = row.try_get(1)?;
        if kind == "refresh" {
            revoke_grant(&state, &grant).await?;
        } else {
            sqlx::query("DELETE FROM oauth_tokens WHERE token_hash = $1")
                .bind(secret_hash(token))
                .execute(state.db.pool())
                .await?;
        }
    }
    Ok(no_store(Json(json!({}))))
}

// Connected apps on the Account page.

/// `GET /api/me/ai/apps`: the apps the account connected that can still be used.
pub async fn apps(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    let at = now();
    // Apps whose every token expired can't be used any more.
    sqlx::query(
        "DELETE FROM oauth_grants WHERE user_id = $1
         AND NOT EXISTS (SELECT 1 FROM oauth_tokens k WHERE k.grant_id = oauth_grants.id AND k.expires_at > $2)",
    )
    .bind(&user.id)
    .bind(at)
    .execute(state.db.pool())
    .await?;
    let rows = sqlx::query(
        "SELECT g.id, c.name, c.client_uri, c.redirect_uris_json, g.scopes, g.created_at, g.last_used_at
         FROM oauth_grants g JOIN oauth_clients c ON c.id = g.client_id
         WHERE g.user_id = $1 ORDER BY g.created_at DESC, g.id",
    )
    .bind(&user.id)
    .fetch_all(state.db.pool())
    .await?;
    let mut list = Vec::new();
    for row in &rows {
        let uris: Vec<String> =
            serde_json::from_str(&row.try_get::<String, _>(3)?).unwrap_or_default();
        list.push(json!({
            "id": row.try_get::<String, _>(0)?,
            "name": row.try_get::<String, _>(1)?,
            "uri": row.try_get::<Option<String>, _>(2)?,
            "redirectHost": uris.first().map(|u| host_of(u)).unwrap_or_default(),
            "scopes": Scopes::stored(&row.try_get::<String, _>(4)?).names(),
            "connectedAt": row.try_get::<i64, _>(5)?,
            "lastUsedAt": row.try_get::<Option<i64>, _>(6)?,
        }));
    }
    Ok(Json(json!({ "apps": list })))
}

/// `DELETE /api/me/ai/apps/{grant}`: disconnect an app; its tokens stop working at once.
pub async fn disconnect(
    State(state): State<AppState>,
    user: User,
    Path(grant): Path<String>,
) -> ApiResult<Json<Value>> {
    let done = sqlx::query("DELETE FROM oauth_grants WHERE id = $1 AND user_id = $2")
        .bind(&grant)
        .bind(&user.id)
        .execute(state.db.pool())
        .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::NotFound("There is no such app here.".into()));
    }
    record(&state, Activity::new("ai_app_disconnected").by(&user)).await;
    Ok(Json(json!({ "revoked": true })))
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn redirects_are_checked_and_matched() {
        assert!(redirect_ok("https://claude.ai/api/mcp/auth_callback").is_ok());
        assert!(redirect_ok("https://chatgpt.com/connector_platform_oauth_redirect").is_ok());
        assert!(redirect_ok("http://127.0.0.1:3118/callback").is_ok());
        assert!(redirect_ok("http://localhost/callback").is_ok());
        assert!(redirect_ok("cursor://anysphere.cursor-mcp/oauth/callback").is_ok());
        for bad in [
            "http://example.com/cb",
            "javascript:alert(1)",
            "data:text/html,hi",
            "https://a.example/cb#frag",
            "https://user:pw@a.example/cb",
            "/relative",
        ] {
            assert!(redirect_ok(bad).is_err(), "{bad}");
        }
        let reg = vec![
            "http://127.0.0.1:3118/callback".to_string(),
            "https://claude.ai/api/mcp/auth_callback".to_string(),
        ];
        assert!(redirect_matches(&reg, "http://127.0.0.1:50000/callback"));
        assert!(redirect_matches(
            &reg,
            "https://claude.ai/api/mcp/auth_callback"
        ));
        assert!(!redirect_matches(
            &reg,
            "https://claude.ai/api/mcp/auth_callback/"
        ));
        assert!(!redirect_matches(&reg, "http://127.0.0.1:3118/other"));
        assert!(!redirect_matches(&reg, "http://localhost:3118/callback"));
    }

    #[test]
    fn pkce_s256_rfc_7636_appendix_b() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(verifier_ok(verifier) && pkce_challenge_ok(challenge));
        assert!(pkce_matches(verifier, challenge));
        assert!(!pkce_matches(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXX",
            challenge
        ));
    }

    #[test]
    fn answers_carry_state_and_iss() {
        let url = redirect_with(
            "https://app.example/cb?x=1",
            &[("code", "bpc_a")],
            Some("s t"),
            "https://party.example.com",
        );
        assert_eq!(
            url,
            "https://app.example/cb?x=1&code=bpc_a&state=s+t&iss=https%3A%2F%2Fparty.example.com"
        );
    }
}
