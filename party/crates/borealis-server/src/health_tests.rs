//! `GET /api/health` (`health.rs`): what an uptime checker sees.

use std::time::Instant;

use axum::routing::get;

use super::*;
use crate::health::{self, Subscriber, Window};

async fn health_of(router: &Router) -> (StatusCode, Option<String>, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let cache = response
        .headers()
        .get("cache-control")
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, cache, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn a_fresh_server_is_ok_without_signing_in_and_tells_no_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let alice = sign_up(&router, "alice").await;
    let (status, cache, out) = health_of(&router).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(cache.as_deref(), Some("no-store"));
    assert_eq!(out["status"], "ok", "{out}");
    assert_eq!(out["version"], crate::release::version());
    assert_eq!(out["database"]["reachable"], true);
    assert_eq!(out["database"]["kind"], "sqlite");
    assert!(out["database"]["latencyMs"].is_u64());
    assert_eq!(out["migrations"]["expected"], crate::db::SCHEMA_VERSION);
    assert_eq!(out["migrations"]["pending"], json!([]));
    assert_eq!(out["migrations"]["ok"], true);
    assert!(out["events"]["subscribers"].is_u64());
    assert_eq!(out["errors"]["windowSeconds"], 300);
    assert!(out["errors"]["answers"].is_u64() && out["errors"]["serverErrors"].is_u64());
    // Nothing about the accounts, the folders or the settings.
    let text = out.to_string();
    for secret in [
        "alice",
        alice.token.as_str(),
        alice.id.as_str(),
        dir.path().to_str().unwrap(),
    ] {
        assert!(!text.contains(secret), "health tells {secret}: {text}");
    }
}

#[tokio::test]
async fn a_database_behind_this_build_is_down_with_503() {
    let dir = tempfile::tempdir().unwrap();
    let last = crate::db::SCHEMA_VERSION;
    let old = Db::open_at(&Location::Memory, last - 1).await.unwrap();
    let router = router_on(dir.path().to_path_buf(), old);
    let (status, _, out) = health_of(&router).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{out}");
    assert_eq!(out["status"], "down");
    assert_eq!(out["database"]["reachable"], true);
    assert_eq!(out["migrations"]["ok"], false);
    assert_eq!(out["migrations"]["pending"], json!([last]));
}

#[tokio::test]
async fn an_open_event_stream_is_counted() {
    let dir = tempfile::tempdir().unwrap();
    let router = router(dir.path().to_path_buf()).await;
    let alice = sign_up(&router, "alice").await;
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/events")
                .header("authorization", format!("Bearer {}", alice.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // Other tests open and close streams too, so only "ours is in it" holds here.
    let (_, _, out) = health_of(&router).await;
    assert!(out["events"]["subscribers"].as_u64().unwrap() >= 1, "{out}");
    drop(response);
}

#[test]
fn the_subscriber_guard_counts_while_it_lives() {
    // The count is shared with tests running meanwhile: with our two guards alive it is at least two.
    let a = Subscriber::new();
    let b = Subscriber::new();
    assert!(health::subscribers() >= 2);
    drop(a);
    drop(b);
}

#[test]
fn the_error_window_keeps_five_minutes() {
    let mut w = Window::new();
    for _ in 0..8 {
        w.record(10, false);
    }
    w.record(10, true);
    w.record(12, true);
    assert_eq!(w.recent(12), (10, 2));
    // Five minutes on, minute 10 still counts; six minutes on it doesn't.
    assert_eq!(w.recent(15), (10, 2));
    assert_eq!(w.recent(16), (1, 1));
    w.record(30, false);
    assert_eq!(w.recent(30), (1, 0), "old minutes are dropped");
}

#[tokio::test]
async fn server_errors_under_api_are_counted() {
    let router: Router = Router::new()
        .route(
            "/api/boom",
            get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        )
        .route("/api/fine", get(|| async { StatusCode::NO_CONTENT }))
        .layer(axum::middleware::from_fn(health::count));
    let (_, errors_before) = health::recent(Instant::now());
    for path in ["/api/boom", "/api/fine", "/api/boom"] {
        router
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
    }
    let (answers, errors) = health::recent(Instant::now());
    assert!(errors >= errors_before + 2, "{errors_before} -> {errors}");
    assert!(answers >= 3);
}

#[test]
fn the_disk_report_has_numbers_where_the_platform_tells() {
    let dir = tempfile::tempdir().unwrap();
    let report = health::disk_report(dir.path());
    if cfg!(unix) {
        let free = report["freeBytes"].as_u64().unwrap();
        assert!(free > 0, "{report}");
        assert!(report["totalBytes"].as_u64().unwrap() >= free);
        assert!(report["ok"].is_boolean());
    } else {
        assert!(report.is_null());
    }
}
