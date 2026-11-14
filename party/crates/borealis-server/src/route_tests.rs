//! Every route the server offered before the routes moved into per-feature `routes()` functions still answers.
//! A route that went missing would fall through to `/api/{*rest}` and answer "The server does not offer that."

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use crate::auth::RateLimiter;
use crate::db::{Db, Location};
use crate::workspaces::Workspaces;
use crate::{api_routes, AppState, Config};

/// (method, path) of each route, with a made-up value for each path parameter.
const ROUTES: &[(&str, &str)] = &[
    ("GET", "/api/server"),
    ("GET", "/api/health"),
    ("POST", "/api/auth/register"),
    ("POST", "/api/auth/login"),
    ("POST", "/api/auth/logout"),
    ("GET", "/api/auth/confirm/t"),
    ("GET", "/api/me"),
    ("PATCH", "/api/me"),
    ("POST", "/api/me/password"),
    ("GET", "/api/admin/stats"),
    ("GET", "/api/admin/settings"),
    ("PATCH", "/api/admin/settings"),
    ("GET", "/api/admin/proxies"),
    ("POST", "/api/admin/proxies"),
    ("GET", "/api/admin/activity"),
    ("GET", "/api/admin/accounts"),
    ("DELETE", "/api/admin/accounts/x"),
    ("POST", "/api/admin/accounts/x/status"),
    ("POST", "/api/admin/accounts/x/admin"),
    ("POST", "/api/admin/accounts/x/password"),
    ("POST", "/api/admin/accounts/x/sign-out"),
    ("POST", "/api/rpc/list_characters"),
    ("GET", "/api/events"),
];

async fn answer(router: &axum::Router, method: &str, path: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn not_offered(body: &Value) -> bool {
    body["message"] == "The server does not offer that."
}

#[tokio::test]
async fn every_route_is_still_registered() {
    let dir = tempfile::tempdir().unwrap();
    let content = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../content");
    let state = AppState {
        db: Db::open(&Location::Memory).await.unwrap(),
        workspaces: Arc::new(Workspaces::new(dir.path().to_path_buf(), content)),
        config: Arc::new(Config {
            open_registration: true,
            require_approval: false,
            public_url: None,
            app_download: None,
            trusted_proxies: Vec::new(),
        }),
        login_limiter: RateLimiter::logins(),
        signup_limiter: RateLimiter::signups(crate::auth::SIGNUPS_ALLOWED),
        mailer: None,
        accounts: Arc::new(tokio::sync::Mutex::new(())),
        started: std::time::Instant::now(),
        hub: Arc::new(crate::hub::PartyHub::new(dir.path().join("party-cards"))),
    };
    let router = api_routes(state, 1024 * 1024).layer(MockConnectInfo(std::net::SocketAddr::from(
        ([127, 0, 0, 1], 9),
    )));

    for (method, path) in ROUTES {
        let (status, body) = answer(&router, method, path).await;
        assert!(
            status != StatusCode::METHOD_NOT_ALLOWED && !not_offered(&body),
            "{method} {path} is no longer routed: {status} {body}"
        );
    }
    let (status, body) = answer(&router, "GET", "/api/no-such-thing").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(not_offered(&body), "{body}");
}
