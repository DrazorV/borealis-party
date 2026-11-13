//! The MCP server: Streamable HTTP at `POST /api/mcp` (MCP 2026-07-28, stateless, plus the legacy `initialize`
//! revisions), over the one tool registry in `party-tools`. See `party/docs/mcp.md`.
//!
//! * **Auth:** `Authorization: Bearer <token>` only (never a query string or a cookie): a personal `bpt_` token or
//!   an OAuth access token, both from part A. Session tokens of the app are refused here (and `bpt_` tokens are
//!   refused everywhere else, by part A). Every request re-reads the account, so a disabled account, a revoked token
//!   or a lost DM role counts at once. A refusal carries the RFC 9728 challenge (`resource_metadata`, `scope`).
//! * **Origin:** a browser request from another site is refused (403); server-side connectors send no Origin.
//! * **What tools can do** is the API's: [`crate::tools_backend::ServerTools`] calls the API's handlers. No tool reads
//!   or changes a live combat (D25).
//! * **Metadata:** the protected-resource and authorization-server documents are part A's routes; the challenges
//!   here point at the same URL.
//! * **Kill switch:** `BOREALIS__FEATURES__AI=false` removes these routes (404), D40.

use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock, OnceLock};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use party_tools::mcp::Headers;
use party_tools::{BoxFuture, Caller, Limits, Registry, Scope, Via};
use serde_json::json;

use crate::auth::{Status, User};
use crate::tools_backend::ServerTools;
use crate::AppState;

/// The endpoint's path.
pub const PATH: &str = "/api/mcp";
/// Largest message accepted.
const MAX_BODY: usize = 1024 * 1024;

/// The registry shared by the MCP server and the in-app assistant (its limits count per token or per account).
#[must_use]
pub fn registry() -> &'static Registry {
    static REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
        Registry::new(Limits::default()).with_tools(crate::tools_backend::external_specs())
    });
    &REGISTRY
}

/// What a valid token grants.
#[derive(Debug, Clone)]
pub struct Grant {
    /// The account, as read from the database for this request.
    pub user: User,
    /// The token's (or OAuth grant's) id, for limits and the activity log.
    pub token_id: String,
    pub scopes: BTreeSet<Scope>,
}

/// Checks a bearer token for `/api/mcp`. Part A's `TokenAuth` implements it (tokens and OAuth access tokens, with
/// the audience check); until then the server installs nothing and every token is refused.
pub trait TokenVerifier: Send + Sync {
    /// The grant, or `None` for a token that isn't valid for this resource (unknown, expired, revoked, another
    /// audience, a disabled account).
    fn verify<'a>(&'a self, state: &'a AppState, token: &'a str) -> BoxFuture<'a, Option<Grant>>;
}

static VERIFIER: OnceLock<Arc<dyn TokenVerifier>> = OnceLock::new();

/// Install the token verifier, once, at start-up (part A) or in tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn install_verifier(v: Arc<dyn TokenVerifier>) {
    let _ = VERIFIER.set(v);
}

/// The account behind `user_id` when it may use tools now: active, not waiting on a new password.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn active_user(state: &AppState, user_id: &str) -> Option<User> {
    let row = state.db.account(user_id).await.ok()??;
    (row.status == Status::Active && !row.user.must_change_password).then_some(row.user)
}

/// The routes, unless `BOREALIS__FEATURES__AI=false`.
pub fn routes() -> axum::Router<AppState> {
    routes_if(crate::config::features().ai)
}

/// The routes when `on`; none otherwise.
pub(crate) fn routes_if(on: bool) -> axum::Router<AppState> {
    use axum::routing::post;
    if !on {
        return axum::Router::new();
    }
    axum::Router::new()
        // The RFC 9728 metadata (`/.well-known/oauth-protected-resource[/api/mcp]`) is part A's (`tokens.rs`).
        .route(PATH, post(post_mcp).get(no_stream).delete(no_stream))
}

/// Where the server is reached: `server.public_url`, else the request's own host.
fn base_url(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(url) = &state.config.public_url {
        return url.clone();
    }
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| {
            h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".:-[]".contains(&b))
        })
        .unwrap_or("localhost");
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
        .filter(|p| *p == "https")
        .unwrap_or("http");
    format!("{scheme}://{host}")
}

fn metadata_url(base: &str) -> String {
    format!("{base}/.well-known/oauth-protected-resource{PATH}")
}

/// This server opens no stream (MCP 2026-07-28 has none; a legacy client falls back to POST answers).
async fn no_stream() -> Response {
    let mut r = StatusCode::METHOD_NOT_ALLOWED.into_response();
    r.headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST"));
    no_store(r)
}

fn no_store(mut r: Response) -> Response {
    r.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, private"),
    );
    r
}

fn challenge(base: &str, error: Option<&str>, scope: &str) -> HeaderValue {
    let mut text = format!("Bearer resource_metadata=\"{}\"", metadata_url(base));
    if let Some(e) = error {
        text.push_str(&format!(", error=\"{e}\""));
    }
    if !scope.is_empty() {
        text.push_str(&format!(", scope=\"{scope}\""));
    }
    HeaderValue::from_str(&text).unwrap_or_else(|_| HeaderValue::from_static("Bearer"))
}

fn rpc_error(status: StatusCode, code: i64, message: &str) -> Response {
    (
        status,
        Json(
            json!({ "jsonrpc": "2.0", "id": null, "error": { "code": code, "message": message } }),
        ),
    )
        .into_response()
}

fn unauthorized(base: &str, message: &str) -> Response {
    let all: Vec<&str> = Scope::ALL.iter().map(|s| s.as_str()).collect();
    let mut r = rpc_error(StatusCode::UNAUTHORIZED, -32001, message);
    r.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        challenge(base, Some("invalid_token"), &all.join(" ")),
    );
    no_store(r)
}

/// An Origin other than the server's own: a web page trying to drive the endpoint from a browser.
fn foreign_origin(base: &str, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) else {
        return false;
    };
    origin.trim_end_matches('/') != base.trim_end_matches('/')
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|h| h.to_str().ok())
}

/// `POST /api/mcp`.
async fn post_mcp(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let base = base_url(&state, &headers);
    if foreign_origin(&base, &headers) {
        return no_store(rpc_error(
            StatusCode::FORBIDDEN,
            -32600,
            "This origin may not use the MCP endpoint.",
        ));
    }
    let Some(token) = crate::auth::bearer(&headers) else {
        return unauthorized(&base, "Send Authorization: Bearer <token>.");
    };
    let Some(verifier) = VERIFIER.get() else {
        return unauthorized(&base, "This server does not accept MCP tokens yet.");
    };
    let Some(grant) = verifier.verify(&state, token).await else {
        return unauthorized(
            &base,
            "The token isn't valid (expired, revoked or not for this server).",
        );
    };
    if body.len() > MAX_BODY {
        return no_store(rpc_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            -32600,
            "The message is too large.",
        ));
    }
    let caller = Caller {
        account: grant.user.id.clone(),
        token: Some(grant.token_id),
        scopes: grant.scopes,
        via: Via::Mcp,
    };
    let backend = ServerTools::new(state, grant.user);
    let reply = party_tools::mcp::handle(
        registry(),
        &backend,
        &caller,
        Headers {
            protocol_version: header_str(&headers, "mcp-protocol-version"),
            method: header_str(&headers, "mcp-method"),
            name: header_str(&headers, "mcp-name"),
        },
        &body,
    )
    .await;
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::OK);
    let mut r = match reply.body {
        Some(b) => (status, Json(b)).into_response(),
        None => status.into_response(),
    };
    if let Some(scope) = reply.insufficient_scope {
        r.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            challenge(&base, Some("insufficient_scope"), scope.as_str()),
        );
    }
    no_store(r)
}

/// Tests: a verifier over tokens the test hands out.
#[cfg(test)]
pub(crate) mod testing {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    /// Token -> (account, scopes).
    type Tokens = Mutex<HashMap<String, (String, BTreeSet<Scope>)>>;

    static TOKENS: LazyLock<Tokens> = LazyLock::new(|| Mutex::new(HashMap::new()));

    struct Mock;

    impl TokenVerifier for Mock {
        fn verify<'a>(
            &'a self,
            state: &'a AppState,
            token: &'a str,
        ) -> BoxFuture<'a, Option<Grant>> {
            Box::pin(async move {
                let (user_id, scopes) = TOKENS.lock().ok()?.get(token).cloned()?;
                let user = active_user(state, &user_id).await?;
                Some(Grant {
                    user,
                    token_id: format!("t-{}", &token[token.len().saturating_sub(6)..]),
                    scopes,
                })
            })
        }
    }

    /// A `bpt_` token for `user` with `scopes`.
    pub(crate) fn token(user: &str, scopes: &str) -> String {
        install_verifier(Arc::new(Mock));
        let t = format!("bpt_test_{}", uuid::Uuid::new_v4().simple());
        TOKENS.lock().unwrap().insert(
            t.clone(),
            (user.to_string(), Scope::parse_list(scopes).unwrap()),
        );
        t
    }

    /// Forget a token (revocation).
    pub(crate) fn revoke(token: &str) {
        TOKENS.lock().unwrap().remove(token);
    }
}
