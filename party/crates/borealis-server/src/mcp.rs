//! The MCP server: Streamable HTTP at `POST /api/mcp` (MCP 2026-07-28, stateless, plus the legacy `initialize`
//! revisions), over the one tool registry in `party-tools`. See `party/docs/mcp.md`.
//!
//! * **Auth:** part A's [`TokenAuth`]: `Authorization: Bearer` with a personal `bpt_` token or an OAuth `bpa_` access
//!   token issued for this endpoint (never a query string, a cookie or an app session token). It re-reads the account
//!   on every request, so a disabled account or a revoked token counts at once, and a lost DM role does too, because
//!   the tools call the API's own role checks. Its refusal is answered as it is: a 401 with the RFC 9728 challenge
//!   claude.ai and ChatGPT start OAuth from.
//! * **Origin:** a browser request from another site is refused (403), except from this server's own pages and the
//!   AI apps in [`ALLOWED_ORIGINS`] (chatgpt.com, chat.openai.com, claude.ai, claude.com); server-side connectors send
//!   no Origin.
//! * **What tools can do** is the API's: [`crate::tools_backend::ServerTools`] calls the API's handlers. No tool reads
//!   or changes a live combat (D25).
//! * **Metadata:** the protected-resource and authorization-server documents are part A's routes (`tokens.rs`,
//!   `oauth.rs`); the 403 challenge here points at the same URL.
//! * **Kill switch:** `BOREALIS__FEATURES__AI=false` removes these routes (404), D40.

use std::sync::LazyLock;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use party_tools::mcp::Headers;
use party_tools::{Caller, Limits, Registry, Scope, Via};
use serde_json::json;

use crate::tokens::{TokenAuth, TokenRejection};
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

/// The RFC 6750 challenge for a tool the token's scopes don't allow, so a connector can ask the user for it.
fn insufficient_scope(state: &AppState, scope: Scope) -> HeaderValue {
    let mut text = format!(
        "Bearer error=\"insufficient_scope\", scope=\"{}\"",
        scope.as_str()
    );
    if let Some(url) = crate::tokens::resource_metadata_url(state) {
        text.push_str(", resource_metadata=\"");
        text.push_str(&url);
        text.push('"');
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

/// The AI apps whose web pages may call the endpoint from a browser (their connectors mostly call from their
/// servers, which send no Origin). Exact origins: https only, no subdomains, no ports.
pub const ALLOWED_ORIGINS: [&str; 4] = [
    "https://chatgpt.com",
    "https://chat.openai.com",
    "https://claude.ai",
    "https://claude.com",
];

/// An Origin other than the server's own or an allowed AI app's: a web page trying to drive the endpoint from a
/// browser (DNS rebinding, a malicious site). The MCP transport spec answers that with 403.
fn foreign_origin(base: &str, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return false;
    };
    // An Origin that isn't text (or is "null", from a sandboxed frame or a file) is foreign too.
    let Ok(origin) = origin.to_str() else {
        return true;
    };
    let origin = origin.trim_end_matches('/');
    origin != base.trim_end_matches('/')
        && !ALLOWED_ORIGINS
            .iter()
            .any(|allowed| origin.eq_ignore_ascii_case(allowed))
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|h| h.to_str().ok())
}

/// `POST /api/mcp`.
async fn post_mcp(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Result<TokenAuth, TokenRejection>,
    body: Bytes,
) -> Response {
    let base = base_url(&state, &headers);
    if foreign_origin(&base, &headers) {
        return no_store(rpc_error(
            StatusCode::FORBIDDEN,
            -32600,
            "This origin may not use the MCP endpoint.",
        ));
    }
    // Unauthenticated: part A's refusal as it is (401 + the challenge that starts OAuth, or 429, or 404 when off).
    let auth = match auth {
        Ok(auth) => auth,
        Err(refusal) => return no_store(refusal.into_response()),
    };
    if body.len() > MAX_BODY {
        return no_store(rpc_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            -32600,
            "The message is too large.",
        ));
    }
    // The two scope lists are the same names (party/docs/party-tools.md); a name only one side knows grants nothing.
    let caller = Caller {
        account: auth.account.id.clone(),
        token: Some(auth.client.id().to_string()),
        scopes: auth
            .scope_names()
            .into_iter()
            .filter_map(Scope::parse)
            .collect(),
        via: Via::Mcp,
    };
    let backend = ServerTools::new(state.clone(), auth.account).via_client(auth.client.label());
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
        r.headers_mut()
            .insert(header::WWW_AUTHENTICATE, insufficient_scope(&state, scope));
    }
    no_store(r)
}

/// Tests: real personal tokens, written as `POST /api/me/ai/tokens` writes them (without its per-account limits, so a
/// test can mint dozens), and checked by the real [`TokenAuth`].
#[cfg(test)]
pub(crate) mod testing {
    use crate::db::{now, Db};
    use crate::tokens::{new_secret, secret_hash, Scopes, PERSONAL_PREFIX};

    /// A `bpt_` token for the account `user` with `scopes` (space-separated names).
    pub(crate) async fn token(db: &Db, user: &str, scopes: &str) -> String {
        let secret = new_secret(PERSONAL_PREFIX);
        let scopes = Scopes::parse(scopes, &[]).expect("known scopes");
        let at = now();
        sqlx::query(
            "INSERT INTO api_tokens (id, user_id, name, token_hash, hint, scopes, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(user)
        .bind("mcp test")
        .bind(secret_hash(&secret))
        .bind("bpt_test…")
        .bind(scopes.to_text())
        .bind(at)
        .bind(at + 24 * 60 * 60)
        .execute(db.pool())
        .await
        .unwrap();
        secret
    }

    /// Revoke a token, as the Account page does (the row goes).
    pub(crate) async fn revoke(db: &Db, secret: &str) {
        sqlx::query("DELETE FROM api_tokens WHERE token_hash = $1")
            .bind(secret_hash(secret))
            .execute(db.pool())
            .await
            .unwrap();
    }
}
