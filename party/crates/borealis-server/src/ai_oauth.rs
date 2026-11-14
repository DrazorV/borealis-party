//! Browser sign-in for AI providers (0.3.2): "Connect OpenRouter" on the Account page's "Your AI".
//!
//! OpenRouter's OAuth PKCE flow (<https://openrouter.ai/docs/guides/overview/auth/oauth>) gives the person a key of
//! their own, which they control on openrouter.ai; there is no client id or client secret.
//!
//! 1. `POST /api/ai/oauth/openrouter/start` (signed in, after the person agreed that their requests go to
//!    OpenRouter): the server makes a random `state` and a PKCE verifier, keeps them for [`TTL`] bound to the account
//!    and the session that asked, and answers the `https://openrouter.ai/auth` address the app opens in the browser
//!    (the system browser in the apps).
//! 2. OpenRouter sends the browser back to `GET /api/ai/oauth/openrouter/callback?code=&state=` on the server's public
//!    address (`server.public_url`), so it works the same from the web, the desktop and the phone apps. The `state` is
//!    taken out at once (a second use, or one after [`TTL`], is refused), the session must still be signed in, and
//!    the code is exchanged with the verifier at OpenRouter's fixed address (`/api/v1/auth/keys`), through the same
//!    checked, pinned, no-redirect request as every provider call (`ai_providers::post_json`).
//! 3. The key is saved exactly like a pasted key (`ai_assistant::store_signed_in`): sealed with the server key, shown
//!    as its last four characters, removable. The app sees it the next time it asks `GET /api/ai`.
//!
//! The code, the verifier and the key are never logged or put in an answer; the callback page sends no referrer.
//!
//! **Sign in with ChatGPT** is not here: OpenAI offers it to selected partners only (a waitlist), and its open-source
//! track allows only loopback redirects, so a server at its own address doesn't qualify (`party/docs/ai-assistant.md`).
//! **Claude** has no sign-in for other apps; the Account page points to "AI apps" (MCP) instead.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use base64::Engine as _;
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::ai_providers::{self, Secret};
use crate::auth::User;
use crate::db::now;
use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// How long a started sign-in may take, in seconds (OpenRouter's code lasts 10 minutes too).
pub const TTL: i64 = 10 * 60;
/// Sign-ins one account may have open at once; a new one replaces the oldest.
const PER_ACCOUNT: usize = 3;
/// Sign-ins open on the whole server.
const MAX_OPEN: usize = 10_000;

const OPENROUTER_AUTH: &str = "https://openrouter.ai/auth";
const OPENROUTER_API: &str = "https://openrouter.ai/api/v1";
/// The model a connected OpenRouter key starts with; the person can change it.
pub const OPENROUTER_MODEL: &str = "openrouter/auto";

/// A sign-in that was started and not yet finished.
struct Open {
    user: String,
    /// The hash of the session that started it (`auth::token_hash`).
    session: String,
    verifier: Secret,
    model: String,
    auto_apply: bool,
    consented_at: i64,
    started: i64,
}

fn open_sign_ins() -> &'static Mutex<HashMap<String, Open>> {
    static OPEN: OnceLock<Mutex<HashMap<String, Open>>> = OnceLock::new();
    OPEN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The key for a `state`: its hash, so the map never holds the state itself.
fn slot(state: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(state.as_bytes()))
}

/// 32 random bytes, base64url without padding (43 characters).
fn random() -> ApiResult<String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| ApiError::Internal("no randomness".into()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// The PKCE S256 challenge of `verifier`.
#[must_use]
pub fn challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// OpenRouter's API address: fixed, or in tests this thread's mock.
fn openrouter_api() -> String {
    #[cfg(test)]
    {
        if let Some(url) = TEST_OPENROUTER.with(|u| u.borrow().clone()) {
            return url;
        }
    }
    OPENROUTER_API.to_string()
}

#[cfg(test)]
thread_local! {
    static TEST_OPENROUTER: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Tests: send this thread's code exchanges to a mock (`{url}/auth/keys`).
#[cfg(test)]
pub fn set_test_openrouter(url: Option<String>) {
    TEST_OPENROUTER.with(|u| *u.borrow_mut() = url);
}

/// Tests: make every open sign-in `seconds` older.
#[cfg(test)]
pub fn age_sign_ins(seconds: i64) {
    for open in open_sign_ins()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .values_mut()
    {
        open.started -= seconds;
    }
}

/// Which browser sign-ins this server offers, for `GET /api/ai`. They need the public address (the callback lands
/// there); without a server key they are shown but can't be used (the page says why, as for a pasted key).
pub(crate) fn offered(state: &AppState) -> Value {
    json!({ "openrouter": state.config.public_url.is_some() })
}

/// `POST /api/ai/oauth/openrouter/start`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartForm {
    /// The person agreed that their requests go to OpenRouter.
    #[serde(default)]
    consent: bool,
    /// The model to start with; OpenRouter's automatic choice when absent.
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    auto_apply: bool,
}

async fn start(
    State(state): State<AppState>,
    user: User,
    headers: HeaderMap,
    Json(form): Json<StartForm>,
) -> ApiResult<Json<Value>> {
    if !form.consent {
        return Err(ApiError::Refused(
            "ai_consent_required",
            "Agree first that what you ask the assistant, and what its tools read for it, goes to OpenRouter.".into(),
        ));
    }
    if !crate::ai_assistant::can_store_keys() {
        return Err(ApiError::Refused(
            "ai_keys_unavailable",
            ai_providers::KEYS_OFF_NOTE.into(),
        ));
    }
    let Some(public) = state.config.public_url.clone() else {
        return Err(ApiError::Refused(
            "ai_sign_in_unavailable",
            "This server has no public address, so OpenRouter can't send you back to it. Paste a key instead."
                .into(),
        ));
    };
    let model = match form.model.as_deref().map(str::trim) {
        None | Some("") => OPENROUTER_MODEL.to_string(),
        Some(m) => crate::ai_assistant::check_model(m)?,
    };
    let session = crate::auth::bearer(&headers)
        .map(crate::auth::token_hash)
        .ok_or_else(|| ApiError::Unauthorized("Please sign in.".into()))?;
    let state_param = random()?;
    let verifier = random()?;
    let callback = format!("{public}/api/ai/oauth/openrouter/callback");
    let mut url =
        url::Url::parse(OPENROUTER_AUTH).map_err(|e| ApiError::Internal(e.to_string()))?;
    url.query_pairs_mut()
        .append_pair("callback_url", &callback)
        .append_pair("code_challenge", &challenge(&verifier))
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state_param)
        .append_pair("key_label", "Borealis Party");
    {
        let mut open = open_sign_ins()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let cutoff = now() - TTL;
        open.retain(|_, o| o.started > cutoff);
        let mut mine: Vec<(String, i64)> = open
            .iter()
            .filter(|(_, o)| o.user == user.id)
            .map(|(k, o)| (k.clone(), o.started))
            .collect();
        mine.sort_by_key(|(_, started)| *started);
        while mine.len() >= PER_ACCOUNT {
            open.remove(&mine.remove(0).0);
        }
        if open.len() >= MAX_OPEN {
            return Err(ApiError::Refused(
                "ai_rate_limited",
                "Too many sign-ins are open on this server. Wait a few minutes and try again."
                    .into(),
            ));
        }
        open.insert(
            slot(&state_param),
            Open {
                user: user.id.clone(),
                session,
                verifier: Secret::new(verifier),
                model,
                auto_apply: form.auto_apply,
                consented_at: now(),
                started: now(),
            },
        );
    }
    tracing::info!("ai: account {} started connecting OpenRouter", user.id);
    Ok(Json(json!({ "url": url.as_str() })))
}

/// `GET /api/ai/oauth/openrouter/callback`.
#[derive(Deserialize)]
struct CallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// The page the browser lands on: no scripts, no store, no referrer (the address held the code).
fn page(status: StatusCode, title: &str, text: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" \
         content=\"width=device-width, initial-scale=1\"><title>{t}</title></head>\
         <body style=\"font-family: system-ui, sans-serif; max-width: 32rem; margin: 4rem auto; padding: 0 16px; \
         line-height: 1.5\"><h1 style=\"font-size: 1.4rem\">{t}</h1><p>{x}</p>\
         <p><a href=\"/#/account\">Open Borealis Party</a></p></body></html>",
        t = escape(title),
        x = escape(text)
    );
    let mut response = (status, Html(html)).into_response();
    let h = response.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // The server's own CSP (main.rs) applies on top.
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn failed(text: &str) -> Response {
    page(StatusCode::BAD_REQUEST, "OpenRouter isn't connected", text)
}

const START_AGAIN: &str =
    "This sign-in link has expired or was already used. Start again from Account, Your AI, Connect OpenRouter.";

/// A code as OpenRouter sends it: short, URL-safe text.
fn code_ok(code: &str) -> bool {
    (1..=512).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
}

async fn callback(State(state): State<AppState>, Query(q): Query<CallbackQuery>) -> Response {
    // The state is taken out first, whatever follows: it is good for one try.
    let Some(open) = q.state.as_deref().and_then(|s| {
        open_sign_ins()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&slot(s))
    }) else {
        return failed(START_AGAIN);
    };
    if open.started <= now() - TTL {
        return failed(START_AGAIN);
    }
    if q.error.is_some() {
        return failed(
            "OpenRouter didn't connect (the sign-in was cancelled or refused). You can try again.",
        );
    }
    let Some(code) = q.code.as_deref().filter(|c| code_ok(c)) else {
        return failed("OpenRouter sent no usable code. Start again from Account, Your AI.");
    };
    // The session that started it must still be signed in, as the same account.
    match state.db.session_user(&open.session).await {
        Ok(Some((user, _))) if user.id == open.user => {}
        Ok(_) => {
            return failed(
                "You were signed out of Borealis Party since you started. Sign in and start again.",
            )
        }
        Err(e) => {
            tracing::warn!("ai: checking a session for a sign-in: {e:?}");
            return failed("Something went wrong on the server. Try again.");
        }
    }
    let body = json!({
        "code": code,
        "code_verifier": open.verifier.expose(),
        "code_challenge_method": "S256",
    });
    let url = format!("{}/auth/keys", openrouter_api());
    let key = match ai_providers::post_json(
        &url,
        &body,
        Duration::from_secs(20),
        Some(&open.verifier),
        |r| r,
    )
    .await
    {
        Ok((_, reply)) => reply["key"].as_str().map(str::trim).map(Secret::new),
        Err(e) => {
            // Only the status: the message could echo what was sent.
            tracing::warn!(
                "ai: account {}: OpenRouter refused the code exchange ({:?})",
                open.user,
                e.status
            );
            return failed(
                "OpenRouter didn't accept the sign-in. Start again from Account, Your AI.",
            );
        }
    };
    let Some(key) = key.filter(|k| {
        let t = k.expose();
        (8..=512).contains(&t.len()) && !t.chars().any(|c| c.is_whitespace() || c.is_control())
    }) else {
        tracing::warn!("ai: account {}: OpenRouter's answer had no key", open.user);
        return failed("OpenRouter's answer had no key. Start again from Account, Your AI.");
    };
    if let Err(e) = crate::ai_assistant::store_signed_in(
        &state,
        &open.user,
        OPENROUTER_API,
        &open.model,
        &key,
        open.auto_apply,
        open.consented_at,
    )
    .await
    {
        tracing::warn!(
            "ai: account {}: saving the OpenRouter key: {e:?}",
            open.user
        );
        return failed("The server couldn't save the key. Try again, or paste a key instead.");
    }
    tracing::info!("ai: account {} connected OpenRouter", open.user);
    page(
        StatusCode::OK,
        "OpenRouter is connected",
        "Your OpenRouter key is saved. You can close this tab and go back to Borealis Party.",
    )
}

/// The sign-in routes (inside the AI routes, so `[features] ai = false` removes them too).
pub(crate) fn routes() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/ai/oauth/openrouter/start", post(start))
        .route("/api/ai/oauth/openrouter/callback", get(callback))
}
