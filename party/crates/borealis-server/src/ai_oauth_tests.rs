//! Browser sign-in for AI providers (0.3.2): "Connect OpenRouter" against a mock OpenRouter on 127.0.0.1 (no real
//! call is ever made). The state and the PKCE verifier, a replayed or expired state, a session signed out in between,
//! the key sealed at rest and never in an answer or the log, removal, and the note people see without a server key.
//!
//! Tests here run on the current thread: the test key, the mock ports and the mock OpenRouter address are per thread.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;

use super::ai_providers_tests::{captured_logs, mock, Mock};
use super::{router_of, sign_up, state_with, Client, Setup};
use crate::ai_assistant::{set_test_settings, TEST_SECRET};
use crate::ai_oauth::{age_sign_ins, challenge, set_test_openrouter, OPENROUTER_MODEL, TTL};
use crate::ai_providers::KEYS_OFF_NOTE;
use crate::config::AiSettings;
use crate::db::{Db, Location};
use crate::{AppState, Config};

const PUBLIC: &str = "https://party.example.com";
const OR_KEY: &str = "sk-or-v1-PLAINTEXT-OPENROUTER-KEY-0123456789-abcd";
const CODE: &str = "or-code-THE-AUTH-CODE-42";

fn keyed(on: bool) {
    set_test_settings(AiSettings {
        secret_key: on.then(|| TEST_SECRET.into()),
        ..AiSettings::default()
    });
}

/// A server with `server.public_url` set (or not), on a fresh database.
async fn server(public: bool) -> (AppState, Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut state = state_with(
        dir.path().to_path_buf(),
        Db::open(&Location::Memory).await.unwrap(),
        Setup::default(),
    );
    state.config = Arc::new(Config {
        open_registration: true,
        require_approval: false,
        public_url: public.then(|| PUBLIC.to_string()),
        app_download: None,
        trusted_proxies: Vec::new(),
    });
    let router = router_of(state.clone(), 64 * 1024 * 1024);
    (state, router, dir)
}

/// OpenRouter's key exchange, answering `reply`.
async fn openrouter(reply: (u16, Value)) -> Mock {
    let m = mock(vec![reply]).await;
    set_test_openrouter(Some(m.url.clone()));
    m
}

async fn start(me: &Client, body: Value) -> (StatusCode, Value) {
    me.post("/api/ai/oauth/openrouter/start", body).await
}

/// The `state` and `code_challenge` of the address `start` answered, after checking the rest of it.
fn parts(url: &str) -> (String, String) {
    let u = url::Url::parse(url).unwrap();
    assert_eq!(u.scheme(), "https");
    assert_eq!(u.host_str(), Some("openrouter.ai"));
    assert_eq!(u.path(), "/auth");
    let q: std::collections::HashMap<String, String> = u.query_pairs().into_owned().collect();
    assert_eq!(
        q["callback_url"],
        format!("{PUBLIC}/api/ai/oauth/openrouter/callback")
    );
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["code_challenge"].len(), 43);
    assert_eq!(q["state"].len(), 43);
    assert!(!q.contains_key("code_verifier"));
    (q["state"].clone(), q["code_challenge"].clone())
}

/// The browser coming back: status and page.
async fn back(router: &Router, query: &str) -> (StatusCode, String, Option<String>) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/ai/oauth/openrouter/callback?{query}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let referrer = response
        .headers()
        .get("referrer-policy")
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into(), referrer)
}

#[tokio::test(flavor = "current_thread")]
async fn connect_openrouter_saves_a_sealed_key_once_and_it_can_be_removed() {
    keyed(true);
    let logs = captured_logs();
    let (state, router, _dir) = server(true).await;
    let _admin = sign_up(&router, "or-admin").await;
    let me = sign_up(&router, "or-user").await;
    let m = openrouter((200, json!({ "key": OR_KEY, "user_id": "user_123" }))).await;

    let (_, status) = me.get("/api/ai").await;
    assert_eq!(status["signIn"]["openrouter"], true, "{status}");

    let (code, out) = start(&me, json!({ "consent": true, "autoApply": true })).await;
    assert_eq!(code, StatusCode::OK, "{out}");
    let (state_param, sent_challenge) = parts(out["url"].as_str().unwrap());

    let (code, page, referrer) = back(&router, &format!("code={CODE}&state={state_param}")).await;
    assert_eq!(code, StatusCode::OK, "{page}");
    assert!(page.contains("OpenRouter is connected"), "{page}");
    assert!(!page.contains(OR_KEY) && !page.contains(CODE));
    assert_eq!(referrer.as_deref(), Some("no-referrer"));

    // The exchange: the code, and the verifier whose S256 challenge went to the browser.
    let seen = m.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/auth/keys");
    assert_eq!(seen[0].body["code"], CODE);
    assert_eq!(seen[0].body["code_challenge_method"], "S256");
    let verifier = seen[0].body["code_verifier"].as_str().unwrap().to_string();
    assert_eq!(challenge(&verifier), sent_challenge);
    assert!(seen[0].headers.get("authorization").is_none());

    // Saved like a pasted key: OpenRouter's address, the model to start with, only the last four shown.
    let (_, status) = me.get("/api/ai").await;
    let p = &status["provider"];
    assert_eq!(p["provider"], "openai", "{status}");
    assert_eq!(p["mode"], "server");
    assert_eq!(p["baseUrl"], "https://openrouter.ai/api/v1");
    assert_eq!(p["model"], OPENROUTER_MODEL);
    assert_eq!(p["hasKey"], true);
    assert_eq!(p["keyReadable"], true);
    assert_eq!(p["keyLast4"], "abcd");
    assert_eq!(p["autoApply"], true);
    assert!(!status.to_string().contains(OR_KEY));

    // Sealed at rest.
    let row = sqlx::query("SELECT key_sealed FROM ai_providers WHERE user_id = $1")
        .bind(&me.id)
        .fetch_one(state.db.pool())
        .await
        .unwrap();
    let sealed: String = row.try_get(0).unwrap();
    assert!(!sealed.contains(OR_KEY) && !sealed.contains("PLAINTEXT"));

    // The same state again: refused, and OpenRouter is not asked again.
    let (code, page, _) = back(&router, &format!("code={CODE}&state={state_param}")).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("expired or was already used"), "{page}");
    assert_eq!(m.seen().len(), 1);

    // Removable like any key.
    let (code, out) = me.delete("/api/ai/key").await;
    assert_eq!(code, StatusCode::OK, "{out}");
    assert_eq!(out["provider"]["hasKey"], false, "{out}");

    if let Some(logs) = logs {
        let logged = String::from_utf8_lossy(&logs.0.lock().unwrap()).to_string();
        assert!(
            logged.contains(&format!("ai: account {} connected OpenRouter", me.id)),
            "the log was captured"
        );
        for secret in [
            OR_KEY,
            "PLAINTEXT",
            CODE,
            verifier.as_str(),
            state_param.as_str(),
        ] {
            assert!(!logged.contains(secret), "{secret} was logged");
        }
    } else {
        eprintln!("not checked: another log subscriber was set first");
    }
    set_test_openrouter(None);
}

#[tokio::test(flavor = "current_thread")]
async fn an_unknown_expired_or_signed_out_sign_in_is_refused_without_asking_openrouter() {
    keyed(true);
    let (_state, router, _dir) = server(true).await;
    let me = sign_up(&router, "or-refused").await;
    let m = openrouter((200, json!({ "key": OR_KEY }))).await;

    // No state, a made-up one.
    for query in [
        format!("code={CODE}"),
        format!("code={CODE}&state=made-up-state"),
    ] {
        let (code, page, _) = back(&router, &query).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{page}");
    }

    // Expired.
    let (_, out) = start(&me, json!({ "consent": true })).await;
    let (expired, _) = parts(out["url"].as_str().unwrap());
    age_sign_ins(TTL + 1);
    let (code, page, _) = back(&router, &format!("code={CODE}&state={expired}")).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("expired"), "{page}");

    // Cancelled at OpenRouter.
    let (_, out) = start(&me, json!({ "consent": true })).await;
    let (cancelled, _) = parts(out["url"].as_str().unwrap());
    let (code, _, _) = back(&router, &format!("error=access_denied&state={cancelled}")).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);

    // Signed out between the start and the callback.
    let (_, out) = start(&me, json!({ "consent": true })).await;
    let (signed_out, _) = parts(out["url"].as_str().unwrap());
    let (code, out) = me.post("/api/auth/logout", Value::Null).await;
    assert!(code.is_success(), "{out}");
    let (code, page, _) = back(&router, &format!("code={CODE}&state={signed_out}")).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("signed out"), "{page}");

    assert!(m.seen().is_empty(), "OpenRouter was asked");
    set_test_openrouter(None);
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_exchange_or_an_answer_without_a_key_saves_nothing() {
    keyed(true);
    let (_state, router, _dir) = server(true).await;
    let me = sign_up(&router, "or-bad-answer").await;
    for reply in [
        (
            403,
            json!({ "error": { "message": format!("bad code {CODE}") } }),
        ),
        (200, json!({ "nothing": true })),
    ] {
        let _m = openrouter(reply).await;
        let (_, out) = start(&me, json!({ "consent": true })).await;
        let (s, _) = parts(out["url"].as_str().unwrap());
        let (code, page, _) = back(&router, &format!("code={CODE}&state={s}")).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{page}");
        assert!(!page.contains(CODE), "{page}");
        let (_, status) = me.get("/api/ai").await;
        assert!(status["provider"].is_null(), "{status}");
    }
    set_test_openrouter(None);
}

#[tokio::test(flavor = "current_thread")]
async fn starting_needs_consent_a_model_name_and_a_public_address() {
    keyed(true);
    let (_state, router, _dir) = server(true).await;
    let me = sign_up(&router, "or-start").await;
    let (code, out) = start(&me, json!({})).await;
    assert_eq!(out["code"], "ai_consent_required", "{code} {out}");
    let (code, out) = start(&me, json!({ "consent": true, "model": "bad model!" })).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{out}");
    let (code, _) = super::send(
        &router,
        None,
        "POST",
        "/api/ai/oauth/openrouter/start",
        json!({ "consent": true }),
    )
    .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);

    let (_state, router, _dir) = server(false).await;
    let me = sign_up(&router, "or-no-public").await;
    let (_, status) = me.get("/api/ai").await;
    assert_eq!(status["signIn"]["openrouter"], false, "{status}");
    let (_, out) = start(&me, json!({ "consent": true })).await;
    assert_eq!(out["code"], "ai_sign_in_unavailable", "{out}");
}

#[tokio::test(flavor = "current_thread")]
async fn without_a_server_key_people_get_a_plain_note_and_only_admins_the_instructions() {
    keyed(false);
    let (_state, router, _dir) = server(true).await;
    let admin = sign_up(&router, "nokey-admin").await;
    let me = sign_up(&router, "nokey-user").await;

    let (_, mine) = me.get("/api/ai").await;
    assert_eq!(mine["keys"]["canStore"], false, "{mine}");
    assert_eq!(mine["keys"]["why"], KEYS_OFF_NOTE);
    assert!(mine["keys"].get("adminHelp").is_none(), "{mine}");
    assert!(
        !mine.to_string().contains("BOREALIS__AI__SECRET_KEY"),
        "{mine}"
    );

    let (_, theirs) = admin.get("/api/ai").await;
    assert_eq!(theirs["keys"]["why"], KEYS_OFF_NOTE);
    assert!(theirs["keys"]["adminHelp"]
        .as_str()
        .unwrap()
        .contains("BOREALIS__AI__SECRET_KEY"));

    // The sign-in is refused with the same note, before anything is opened.
    let (code, out) = start(&me, json!({ "consent": true })).await;
    assert_eq!(code, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(out["code"], "ai_keys_unavailable");
    assert!(
        !out.to_string().contains("BOREALIS__AI__SECRET_KEY"),
        "{out}"
    );
    keyed(true);
}
