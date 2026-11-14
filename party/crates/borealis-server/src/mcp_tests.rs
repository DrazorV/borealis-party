//! The MCP server end to end (0.3.1 part B): protocol conformance over HTTP (modern 2026-07-28 and legacy
//! `initialize`, errors, auth challenges, Origin, the kill switch), the scope matrix, the leak suite (a player-owned
//! token calling every tool with hidden ids, random ids and malformed arguments gets nothing hidden or DM-only),
//! no tool for live combats (D25) and simulations that write nothing, and idempotent writes.
//!
//! Tokens are real personal tokens checked by part A's `TokenAuth` (`mcp::testing` writes them as the Account page
//! does, without its per-account limits; one flow mints its token through `POST /api/me/ai/tokens` itself). Monsters are the bundled SRD 5.2.1 ones. Runs on
//! SQLite, and on PostgreSQL too when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.

use std::path::Path;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use party_tools::Scope;
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;

use super::{drop_all, new_character, router_of, send, sign_up, state_with, Client, Setup};
use crate::db::{Db, Location};
use crate::mcp::testing;
use crate::AppState;

const DRAGON: &str = "ID_SRD521_MONSTER_ANCIENT_RED_DRAGON";
const OWLBEAR: &str = "ID_SRD521_MONSTER_OWLBEAR";
const GOBLIN: &str = "ID_SRD521_MONSTER_GOBLIN_WARRIOR";
const ALL_SCOPES: &str =
    "bestiary:read encounters:write encounters:simulate characters:read homebrew:write";
/// A spell the hidden dragon precasts; its key never reaches a player.
const SPELL_KEY: &str = "ID_2024_SPELL_FIREBALL";
const SECRETS: [&str; 5] = [
    "SECRET-LABEL-DRAGON",
    "SECRET-PUBLIC-DRAGON",
    "SECRET-LABEL-GOBLIN",
    "SECRET-NOTE",
    "SECRET-ENCOUNTER-NAME",
];

fn request_id() -> String {
    format!("req-{}", uuid::Uuid::new_v4().simple())
}

async fn ok(client: &Client, method: &str, path: &str, body: Value) -> Value {
    let (status, out) = send(&client.router, Some(&client.token), method, path, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {out}");
    out
}

async fn character(client: &Client, name: &str) -> String {
    let out = ok(
        client,
        "POST",
        "/api/rpc/create_character",
        new_character(name),
    )
    .await;
    let id = out["id"].as_str().unwrap().to_string();
    ok(
        client,
        "POST",
        "/api/rpc/save_character",
        json!({ "characterId": id }),
    )
    .await;
    id
}

/// A table: a DM with a party, one player, and an outsider; a player-visible encounter with a hidden dragon and goblin
/// and a revealed owlbear; and a draft encounter.
struct Table {
    state: AppState,
    router: Router,
    dm: Client,
    player: Client,
    outsider: Client,
    party: String,
    encounter: String,
    /// Ids that must never reach the player: the hidden creatures and the draft.
    hidden_ids: Vec<String>,
    player_character: String,
    dm_character: String,
}

async fn table(state: AppState) -> Table {
    let router = router_of(state.clone(), Setup::default().max_upload);
    sign_up(&router, "mcp-admin").await;
    let dm = sign_up(&router, "mcp-dm").await;
    let player = sign_up(&router, "mcp-player").await;
    let outsider = sign_up(&router, "mcp-outsider").await;
    ok(&dm, "POST", "/api/me/roles/dm", json!({})).await;
    let party = ok(
        &dm,
        "POST",
        "/api/parties",
        json!({ "name": "Tuesday Party" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let player_character = character(&player, "Hero One").await;
    let dm_character = character(&dm, "DM Own Hero").await;
    ok(
        &dm,
        "POST",
        &format!("/api/parties/{party}/invites"),
        json!({ "kind": "username", "username": "mcp-player" }),
    )
    .await;
    let invites = ok(&player, "GET", "/api/invites", Value::Null).await;
    let invite = invites["invites"][0]["id"].as_str().unwrap().to_string();
    ok(
        &player,
        "POST",
        &format!("/api/invites/{invite}/accept"),
        json!({ "characterId": player_character }),
    )
    .await;
    let encounter = ok(
        &dm,
        "POST",
        &format!("/api/parties/{party}/encounters"),
        json!({ "name": "SECRET-ENCOUNTER-NAME", "publicName": "Cave mouth", "dmNotes": "SECRET-NOTE" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/encounters/{encounter}/creatures");
    let last = |v: &Value| {
        v["creatures"].as_array().unwrap().last().unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let v = ok(&dm, "POST", &path, json!({ "elementId": DRAGON, "dmLabel": "SECRET-LABEL-DRAGON", "publicName": "SECRET-PUBLIC-DRAGON", "hidden": true })).await;
    let dragon_src = last(&v);
    // A precast spell of the hidden dragon and a spell rule of the DM (0.3.2): DM-only, like the labels.
    ok(
        &dm,
        "POST",
        &format!("/api/encounters/{encounter}/casts"),
        json!({ "casterId": dragon_src, "spellKey": SPELL_KEY, "slotLevel": 3 }),
    )
    .await;
    ok(
        &dm,
        "PUT",
        "/api/spell-mechanics",
        json!({ "kind": "alias", "spellKey": SPELL_KEY, "ruleset": "dnd2024", "aliasName": "SECRET Spell Alias" }),
    )
    .await;
    let v = ok(
        &dm,
        "POST",
        &path,
        json!({ "elementId": GOBLIN, "dmLabel": "SECRET-LABEL-GOBLIN", "hidden": true }),
    )
    .await;
    let goblin_src = last(&v);
    ok(&dm, "POST", &path, json!({ "elementId": OWLBEAR, "dmLabel": "Bear", "publicName": "Owlbear", "hidden": false })).await;
    ok(
        &dm,
        "PATCH",
        &format!("/api/encounters/{encounter}"),
        json!({ "status": "ready", "playerVisible": true }),
    )
    .await;
    let draft = ok(
        &dm,
        "POST",
        &format!("/api/parties/{party}/encounters"),
        json!({ "name": "SECRET-ENCOUNTER-NAME 2" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let hidden_ids = vec![dragon_src, goblin_src, draft];
    Table {
        state,
        router,
        dm,
        player,
        outsider,
        party,
        encounter,
        hidden_ids,
        player_character,
        dm_character,
    }
}

/// The server state with a public URL, as a real server has: the 401 challenge names the metadata URL from it.
fn mcp_state(dir: &Path, db: Db) -> AppState {
    let mut state = state_with(dir.to_path_buf(), db, Setup::default());
    state.config = std::sync::Arc::new(crate::Config {
        open_registration: true,
        require_approval: false,
        public_url: Some("https://party.example.com".to_string()),
        app_download: None,
        trusted_proxies: Vec::new(),
    });
    state
}

async fn memory_table(dir: &Path) -> Table {
    table(mcp_state(dir, Db::open(&Location::Memory).await.unwrap())).await
}

/// One POST to `/api/mcp`: status, headers and the body (`Null` when empty).
async fn post(
    router: &Router,
    token: Option<&str>,
    body: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/mcp")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = router
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn modern(id: i64, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
    });
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

async fn rpc(router: &Router, token: &str, msg: &Value) -> (StatusCode, HeaderMap, Value) {
    let method = msg["method"].as_str().unwrap().to_string();
    post(
        router,
        Some(token),
        &msg.to_string(),
        &[
            ("mcp-protocol-version", "2026-07-28"),
            ("mcp-method", &method),
        ],
    )
    .await
}

/// `tools/call`: the HTTP status and the JSON-RPC body.
async fn call(router: &Router, token: &str, tool: &str, args: Value) -> (StatusCode, Value) {
    let msg = modern(7, "tools/call", json!({ "name": tool, "arguments": args }));
    let (s, _, b) = rpc(router, token, &msg).await;
    (s, b)
}

/// A successful tool result's data.
async fn data(router: &Router, token: &str, tool: &str, args: Value) -> Value {
    let (s, b) = call(router, token, tool, args.clone()).await;
    assert_eq!(s, StatusCode::OK, "{tool} {args}: {b}");
    assert_eq!(b["result"]["isError"], false, "{tool} {args}: {b}");
    assert_eq!(b["result"]["structuredContent"]["untrusted"], true);
    b["result"]["structuredContent"]["data"].clone()
}

async fn count(db: &Db, table: &str) -> i64 {
    sqlx::query(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get::<i64, _>(0)
        .unwrap()
}

// Flows (run on SQLite and PostgreSQL).

async fn conformance(t: &Table) {
    let r = &t.router;
    let tok = testing::token(&t.state.db, &t.dm.id, ALL_SCOPES).await;

    // Auth: none, an app session token, a revoked token: 401 with the RFC 9728 challenge.
    for token in [None, Some(t.dm.token.as_str())] {
        let (s, h, b) = post(
            r,
            token,
            &modern(1, "server/discover", json!({})).to_string(),
            &[],
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{b}");
        let www = h["www-authenticate"].to_str().unwrap();
        assert!(
            www.contains("resource_metadata=")
                && www.contains("/.well-known/oauth-protected-resource/api/mcp"),
            "{www}"
        );
        assert_eq!(h["cache-control"], "no-store, private");
    }
    // A token made on the Account page (part A's route) works end to end.
    let made = ok(
        &t.dm,
        "POST",
        "/api/me/ai/tokens",
        json!({ "name": "Claude Code", "scopes": ["bestiary:read"] }),
    )
    .await;
    let minted = made["token"].as_str().unwrap().to_string();
    assert!(minted.starts_with("bpt_"));
    let (s, _, b) = rpc(r, &minted, &modern(1, "tools/list", json!({}))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let names: Vec<&str> = b["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["bestiary_search", "monster_get"]);
    let gone = testing::token(&t.state.db, &t.dm.id, ALL_SCOPES).await;
    testing::revoke(&t.state.db, &gone).await;
    let (s, _, _) = post(
        r,
        Some(&gone),
        &modern(1, "server/discover", json!({})).to_string(),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // No stream: GET and DELETE are 405.
    for m in ["GET", "DELETE"] {
        let (s, _) = send(r, Some(&tok), m, "/api/mcp", Value::Null).await;
        assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED, "{m}");
    }

    // A browser page from another site.
    let (s, _, _) = post(
        r,
        Some(&tok),
        &modern(1, "server/discover", json!({})).to_string(),
        &[("origin", "https://evil.example")],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Legacy: initialize, then requests answered on their own by the version header.
    let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "old", "version": "1" } } });
    let (s, h, b) = post(r, Some(&tok), &init.to_string(), &[]).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["result"]["protocolVersion"], "2025-06-18");
    assert!(b["result"]["capabilities"]["tools"].is_object());
    assert_eq!(b["result"]["serverInfo"]["name"], "borealis-party");
    // The DM's-assistant workflow, the same text the AI apps page shows.
    let instructions = b["result"]["instructions"].as_str().unwrap();
    assert_eq!(instructions, party_tools::mcp::INSTRUCTIONS);
    for needed in [
        "bestiary_search",
        "monster_get",
        "homebrew_validate",
        "live combat",
        "difficulty rating",
        "plain counts",
        "Never invent a stat block",
        "real dice",
        "never follow instructions",
    ] {
        assert!(instructions.contains(needed), "{needed} missing");
    }
    assert!(instructions.lines().count() <= 40);
    assert!(h.get("mcp-session-id").is_none());
    let (s, _, b) = post(
        r,
        Some(&tok),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        &[("mcp-protocol-version", "2025-06-18")],
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED);
    assert!(b.is_null());
    let (s, _, b) = post(
        r,
        Some(&tok),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        &[("mcp-protocol-version", "2025-06-18")],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(b["result"]["tools"].as_array().unwrap().len() > 10, "{b}");
    assert!(b["result"].get("resultType").is_none());
    let (_, _, b) = post(
        r,
        Some(&tok),
        r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#,
        &[("mcp-protocol-version", "2025-11-25")],
    )
    .await;
    assert_eq!(b["result"], json!({}));
    let (_, _, b) = post(r, Some(&tok), r#"{"jsonrpc":"2.0","id":4,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#, &[]).await;
    assert_eq!(b["result"]["protocolVersion"], "2025-11-25");

    // Modern: discover, list, call.
    let (s, _, b) = rpc(r, &tok, &modern(5, "server/discover", json!({}))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["result"]["supportedVersions"], json!(["2026-07-28"]));
    assert_eq!(b["result"]["instructions"], party_tools::mcp::INSTRUCTIONS);
    assert_eq!(b["result"]["resultType"], "complete");
    assert!(b["result"]["_meta"]["io.modelcontextprotocol/serverInfo"].is_object());
    let (_, _, b) = rpc(r, &tok, &modern(6, "tools/list", json!({}))).await;
    let tools = b["result"]["tools"].as_array().unwrap();
    assert_eq!(b["result"]["resultType"], "complete");
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        assert!(tool["annotations"]["readOnlyHint"].is_boolean());
    }
    let found = data(r, &tok, "bestiary_search", json!({ "text": "owlbear" })).await;
    assert!(found.to_string().contains("Owlbear"), "{found}");
    assert!(
        !found.to_string().contains("\"xp\""),
        "XP left the server: {found}"
    );
    let monster = data(r, &tok, "monster_get", json!({ "id": OWLBEAR })).await;
    assert!(!monster.to_string().contains("\"xp\""), "{monster}");

    // Errors.
    let (s, _, b) = post(r, Some(&tok), "{not json", &[]).await;
    assert_eq!(
        (s, b["error"]["code"].clone()),
        (StatusCode::BAD_REQUEST, json!(-32700))
    );
    let (s, _, b) = post(
        r,
        Some(&tok),
        &json!([modern(1, "tools/list", json!({}))]).to_string(),
        &[],
    )
    .await;
    assert_eq!(
        (s, b["error"]["code"].clone()),
        (StatusCode::BAD_REQUEST, json!(-32600))
    );
    // Resources exist since 0.3.3 (the instructions); prompts don't.
    let (s, _, b) = rpc(r, &tok, &modern(1, "prompts/list", json!({}))).await;
    assert_eq!(
        (s, b["error"]["code"].clone()),
        (StatusCode::NOT_FOUND, json!(-32601))
    );
    let mut old = modern(1, "tools/list", json!({}));
    old["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("2030-01-01");
    let (s, _, b) = post(r, Some(&tok), &old.to_string(), &[]).await;
    assert_eq!(
        (s, b["error"]["code"].clone()),
        (StatusCode::BAD_REQUEST, json!(-32022))
    );
    assert_eq!(b["error"]["data"]["supported"], json!(["2026-07-28"]));
    let (s, _, b) = post(
        r,
        Some(&tok),
        &modern(1, "tools/list", json!({})).to_string(),
        &[("mcp-protocol-version", "2025-06-18")],
    )
    .await;
    assert_eq!(
        (s, b["error"]["code"].clone()),
        (StatusCode::BAD_REQUEST, json!(-32020))
    );
    let (s, _, b) = post(
        r,
        Some(&tok),
        &modern(1, "tools/list", json!({})).to_string(),
        &[("mcp-method", "tools/call")],
    )
    .await;
    assert_eq!(
        (s, b["error"]["code"].clone()),
        (StatusCode::BAD_REQUEST, json!(-32020))
    );
    let mut nocaps = modern(1, "tools/list", json!({}));
    nocaps["params"]["_meta"]
        .as_object_mut()
        .unwrap()
        .remove("io.modelcontextprotocol/clientCapabilities");
    let (s, _, b) = post(r, Some(&tok), &nocaps.to_string(), &[]).await;
    assert_eq!(
        (s, b["error"]["code"].clone()),
        (StatusCode::BAD_REQUEST, json!(-32602))
    );
    let (s, _, b) = post(
        r,
        Some(&tok),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        &[("mcp-protocol-version", "1999-01-01")],
    )
    .await;
    assert_eq!(
        (s, b["error"]["code"].clone()),
        (StatusCode::BAD_REQUEST, json!(-32022))
    );
    let (s, b) = call(r, &tok, "no_such_tool", json!({})).await;
    assert_eq!(
        (s, b["error"]["code"].clone()),
        (StatusCode::BAD_REQUEST, json!(-32602))
    );
    let (s, b) = call(
        r,
        &tok,
        "monster_get",
        json!({ "id": OWLBEAR, "path": "/etc" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["result"]["isError"], true, "{b}");
    assert!(b["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("invalid_argument"));
    // Part D's tools are registered with their own schemas and answer through its module.
    let (_, b) = call(r, &tok, "homebrew_list", json!({})).await;
    assert_eq!(b["result"]["isError"], false, "{b}");
    let (_, _, list) = rpc(r, &tok, &modern(1, "tools/list", json!({}))).await;
    let create = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "homebrew_create")
        .cloned()
        .unwrap_or_else(|| panic!("no homebrew_create: {list}"));
    assert!(
        create["inputSchema"]["properties"]["xml"].is_object(),
        "{create}"
    );
    assert!(create["inputSchema"]["required"]
        .as_array()
        .unwrap()
        .contains(&json!("requestId")));
    // A refused element comes back with the engine's report as details, so the model can fix it and retry.
    let bad =
        json!({ "requestId": request_id(), "xml": "<element type=\"Spell\" id=\"\"></element>" });
    let (_, b) = call(r, &tok, "homebrew_create", bad).await;
    assert_eq!(b["result"]["isError"], true, "{b}");
    let err: Value =
        serde_json::from_str(b["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(err["code"], "invalid_homebrew", "{err}");
    assert!(err["details"]["report"].is_object(), "no report: {err}");

    // The activity log has the calls by name and status, never their arguments.
    let rows = sqlx::query("SELECT detail FROM activity WHERE kind = 'ai_tool'")
        .fetch_all(t.state.db.pool())
        .await
        .unwrap();
    let details: Vec<String> = rows
        .iter()
        .map(|r| r.try_get::<String, _>(0).unwrap())
        .collect();
    assert!(
        details
            .iter()
            .any(|d| d == "mcp bestiary_search bestiary:read ok"),
        "{details:?}"
    );
    assert!(
        details
            .iter()
            .all(|d| !d.contains("owlbear") && !d.contains("/etc")),
        "{details:?}"
    );

    // The kill switch: no routes at all.
    let off: Router = Router::new()
        .merge(crate::mcp::routes_if(false))
        .with_state(t.state.clone());
    let resp = off
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/mcp")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

async fn scope_matrix(t: &Table) {
    let r = &t.router;
    let args = |tool: &str| -> Value {
        match tool {
            "bestiary_search" => json!({}),
            "monster_get" => json!({ "id": OWLBEAR }),
            "encounter_list" => json!({ "partyId": t.party }),
            "encounter_get" => json!({ "encounterId": t.encounter }),
            "sim_start" | "sim_batch" => json!({ "encounterId": t.encounter }),
            _ => json!({ "requestId": request_id() }),
        }
    };
    for spec in party_tools::spec::all() {
        // Every scope that doesn't grant this tool.
        let without: Vec<&str> = Scope::ALL
            .iter()
            .filter(|s| !party_tools::scope::grants(&[**s].into_iter().collect(), spec.scope))
            .map(|s| s.as_str())
            .collect();
        let tok = testing::token(&t.state.db, &t.dm.id, &without.join(" ")).await;
        let (_, _, list) = rpc(r, &tok, &modern(1, "tools/list", json!({}))).await;
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["name"].as_str().unwrap())
            .collect();
        assert!(
            !names.contains(&spec.name),
            "{} listed without {}",
            spec.name,
            spec.scope
        );
        let msg = modern(
            2,
            "tools/call",
            json!({ "name": spec.name, "arguments": args(spec.name) }),
        );
        let (s, h, b) = rpc(r, &tok, &msg).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{}: {b}", spec.name);
        let www = h["www-authenticate"].to_str().unwrap();
        assert!(
            www.contains("insufficient_scope") && www.contains(spec.scope.as_str()),
            "{www}"
        );
        // With only its scope: allowed past the scope check (whatever the handler then says).
        let tok = testing::token(&t.state.db, &t.dm.id, spec.scope.as_str()).await;
        let (s, b) = call(r, &tok, spec.name, args(spec.name)).await;
        assert_eq!(s, StatusCode::OK, "{}: {b}", spec.name);
        assert!(
            !b.to_string().contains("insufficient_scope"),
            "{}: {b}",
            spec.name
        );
    }
    // Implications: write implies read; simulate implies nothing else.
    let tok = testing::token(&t.state.db, &t.dm.id, "encounters:write").await;
    data(
        r,
        &tok,
        "encounter_get",
        json!({ "encounterId": t.encounter }),
    )
    .await;
    let tok = testing::token(&t.state.db, &t.dm.id, "encounters:simulate").await;
    let (s, _) = call(
        r,
        &tok,
        "encounter_get",
        json!({ "encounterId": t.encounter }),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

/// Every tool the player-owned token can name, with hidden, random and malformed arguments.
async fn leak_suite(t: &Table) {
    let r = &t.router;
    let mut tok = testing::token(&t.state.db, &t.player.id, ALL_SCOPES).await;
    let random = uuid::Uuid::new_v4().to_string();
    let mut ids: Vec<String> = t.hidden_ids.clone();
    ids.push(random.clone());
    ids.push(t.dm_character.clone());
    let mut calls: Vec<(&str, Value)> = vec![
        ("encounter_list", json!({ "partyId": t.party })),
        ("encounter_get", json!({ "encounterId": t.encounter })),
        ("character_list", json!({})),
        (
            "character_get",
            json!({ "characterId": t.player_character }),
        ),
        ("sim_start", json!({ "encounterId": t.encounter })),
        (
            "sim_batch",
            json!({ "encounterId": t.encounter, "runs": 3 }),
        ),
        (
            "encounter_update",
            json!({ "requestId": request_id(), "encounterId": t.encounter, "name": "x" }),
        ),
        (
            "encounter_create",
            json!({ "requestId": request_id(), "partyId": t.party, "name": "x" }),
        ),
    ];
    for id in &ids {
        calls.extend([
            ("encounter_get", json!({ "encounterId": id })),
            ("encounter_list", json!({ "partyId": id })),
            ("sim_start", json!({ "encounterId": id })),
            ("sim_state", json!({ "simId": id })),
            ("encounter_update", json!({ "requestId": request_id(), "encounterId": id, "status": "ready" })),
            ("encounter_add_creature", json!({ "requestId": request_id(), "encounterId": id, "elementId": GOBLIN })),
            ("encounter_add_creature", json!({ "requestId": request_id(), "encounterId": t.encounter, "elementId": GOBLIN, "ownerCreatureId": id })),
            ("encounter_remove_creature", json!({ "requestId": request_id(), "encounterId": t.encounter, "creatureId": id })),
            ("encounter_remove_creature", json!({ "requestId": request_id(), "encounterId": id, "creatureId": id })),
            ("character_get", json!({ "characterId": id })),
            ("character_get", json!({ "characterId": id, "partyId": t.party, "ownerId": t.dm.id })),
        ]);
    }
    // Malformed: wrong types, unknown keys, huge strings, arrays, nulls.
    let huge = "x".repeat(100_000);
    for tool in party_tools::spec::all().iter().map(|s| s.name) {
        for bad in [
            json!(null),
            json!([1, 2]),
            json!("text"),
            json!(42),
            json!({ "dmId": t.dm.id }),
            json!({ "encounterId": [t.encounter.clone()] }),
            json!({ "encounterId": huge }),
            json!({ "combatId": { "$ne": 1 } }),
            json!({ "partyId": t.party, "path": "../../etc/passwd" }),
        ] {
            calls.push((tool, bad));
        }
    }
    let mut not_found = 0;
    for (n, (tool, args)) in calls.iter().enumerate() {
        if n % 50 == 49 {
            // A fresh token now and then, so the per-token rate never hides a call.
            tok = testing::token(&t.state.db, &t.player.id, ALL_SCOPES).await;
        }
        let msg = modern(9, "tools/call", json!({ "name": tool, "arguments": args }));
        let (s, h, b) = rpc(r, &tok, &msg).await;
        let text = format!("{b} {h:?}");
        for secret in SECRETS {
            assert!(
                !text.contains(secret),
                "{tool} {args}: {secret} leaked: {text}"
            );
        }
        for spell in [SPELL_KEY, "\"casts\"", "spell alias"] {
            assert!(
                !text.to_lowercase().contains(&spell.to_lowercase()),
                "{tool} {args}: the DM's spell data ({spell}) leaked: {text}"
            );
        }
        for id in &t.hidden_ids {
            // The id may be echoed only as the argument the player sent? No: never, not even in a message.
            assert!(
                !text.contains(id.as_str()),
                "{tool} {args}: hidden id {id} leaked: {text}"
            );
        }
        assert!(
            s == StatusCode::OK || s == StatusCode::BAD_REQUEST,
            "{tool} {args}: {s} {b}"
        );
        if b.to_string().contains("not_found") {
            not_found += 1;
        }
    }
    // Each hidden or random id reaches at least five handlers, which answer the one 404.
    assert!(
        not_found >= ids.len() * 5,
        "only {not_found} of {} calls were 404s",
        calls.len()
    );
    // What the player may see they do see: the encounter's projection, with the revealed owlbear only.
    let view = data(
        r,
        &tok,
        "encounter_get",
        json!({ "encounterId": t.encounter }),
    )
    .await;
    assert!(view.to_string().contains("Owlbear"), "{view}");
    assert_eq!(
        view["creatures"].as_array().map(Vec::len),
        Some(1),
        "{view}"
    );
    // The outsider sees the party as not found, whatever the scopes.
    let out = testing::token(&t.state.db, &t.outsider.id, ALL_SCOPES).await;
    let (_, b) = call(r, &out, "encounter_list", json!({ "partyId": t.party })).await;
    assert!(b.to_string().contains("not_found"), "{b}");
    let (_, b) = call(
        r,
        &out,
        "encounter_get",
        json!({ "encounterId": t.encounter }),
    )
    .await;
    assert!(b.to_string().contains("not_found"), "{b}");
}

async fn rows_now(db: &Db) -> Vec<i64> {
    let mut out = Vec::new();
    for table in [
        "combats",
        "combatants",
        "combat_journal",
        "combat_log",
        "combat_requests",
        "encounters",
        "encounter_creatures",
    ] {
        out.push(count(db, table).await);
    }
    out
}

/// D25: no tool reads or changes a live combat; the simulator's tools run on copies and write nothing.
async fn no_combat_tools(t: &Table) {
    let r = &t.router;
    let dm = testing::token(&t.state.db, &t.dm.id, ALL_SCOPES).await;
    let started = ok(
        &t.dm,
        "POST",
        &format!("/api/encounters/{}/start", t.encounter),
        json!({ "requestId": request_id() }),
    )
    .await;
    let combat = started["id"].as_str().unwrap().to_string();
    let (_, _, list) = rpc(r, &dm, &modern(1, "tools/list", json!({}))).await;
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert!(names.iter().all(|n| !n.starts_with("combat_")), "{names:?}");
    assert!(
        names
            .iter()
            .all(|n| !n.starts_with("index_") || *n == "index_build"),
        "{names:?}"
    );
    for tool in [
        "combat_get",
        "combat_action",
        "combat_simulate",
        "index_publish",
    ] {
        let (s, b) = call(r, &dm, tool, json!({ "combatId": combat })).await;
        assert_eq!(
            (s, b["error"]["code"].clone()),
            (StatusCode::BAD_REQUEST, json!(-32602)),
            "{tool}: {b}"
        );
    }
    let before = rows_now(&t.state.db).await;
    let (_, before_view) = send(
        r,
        Some(&t.dm.token),
        "GET",
        &format!("/api/combats/{combat}"),
        Value::Null,
    )
    .await;
    // The simulator runs on a copy of the encounter while the combat is live, and writes nothing (D25, D26).
    for (tool, args, fails) in [
        ("sim_start", json!({ "encounterId": t.encounter }), false),
        (
            "sim_batch",
            json!({ "encounterId": t.encounter, "runs": 5 }),
            false,
        ),
        ("sim_start", json!({ "combatId": combat }), true),
        ("sim_start", json!({ "encounterId": combat }), true),
    ] {
        let (_, b) = call(r, &dm, tool, args.clone()).await;
        assert_eq!(b["result"]["isError"], fails, "{tool} {args}: {b}");
    }
    // A player of the party can't simulate the DM's encounter.
    let player = testing::token(&t.state.db, &t.player.id, ALL_SCOPES).await;
    let (_, b) = call(
        r,
        &player,
        "sim_start",
        json!({ "encounterId": t.encounter }),
    )
    .await;
    assert!(b.to_string().contains("not_found"), "{b}");
    assert_eq!(rows_now(&t.state.db).await, before);
    let (_, after_view) = send(
        r,
        Some(&t.dm.token),
        "GET",
        &format!("/api/combats/{combat}"),
        Value::Null,
    )
    .await;
    assert_eq!(before_view, after_view);
}

async fn idempotency(t: &Table) {
    let r = &t.router;
    let tok = testing::token(&t.state.db, &t.dm.id, "encounters:write").await;
    let rid = request_id();
    let before = count(&t.state.db, "encounters").await;
    let args = json!({ "requestId": rid, "partyId": t.party, "name": "Swamp ambush" });
    let (_, first) = call(r, &tok, "encounter_create", args.clone()).await;
    let (_, again) = call(r, &tok, "encounter_create", args.clone()).await;
    let one = &first["result"]["structuredContent"];
    let two = &again["result"]["structuredContent"];
    assert_eq!(one["data"]["id"], two["data"]["id"], "{first} {again}");
    assert_eq!(
        (one["replayed"].clone(), two["replayed"].clone()),
        (json!(false), json!(true))
    );
    assert_eq!(count(&t.state.db, "encounters").await, before + 1);
    let (_, other) = call(
        r,
        &tok,
        "encounter_create",
        json!({ "requestId": rid, "partyId": t.party, "name": "Other" }),
    )
    .await;
    assert_eq!(other["result"]["isError"], true, "{other}");
    assert_eq!(count(&t.state.db, "encounters").await, before + 1);
    // A write without a request id is refused before anything runs.
    let (_, b) = call(
        r,
        &tok,
        "encounter_create",
        json!({ "partyId": t.party, "name": "No id" }),
    )
    .await;
    assert_eq!(b["result"]["isError"], true);
    let enc = one["data"]["id"].as_str().unwrap().to_string();
    let add =
        json!({ "requestId": request_id(), "encounterId": enc, "elementId": GOBLIN, "count": 2 });
    let (_, a) = call(r, &tok, "encounter_add_creature", add.clone()).await;
    let (_, b) = call(r, &tok, "encounter_add_creature", add).await;
    assert_eq!(a["result"]["isError"], false, "{a}");
    assert_eq!(b["result"]["structuredContent"]["replayed"], true, "{b}");
    assert_eq!(count(&t.state.db, "encounter_creatures").await, 3 + 2);
    // The per-token rate limit.
    let limited = testing::token(&t.state.db, &t.dm.id, "bestiary:read").await;
    let mut refused = false;
    for _ in 0..70 {
        let (_, b) = call(r, &limited, "monster_get", json!({ "id": OWLBEAR })).await;
        refused |= b.to_string().contains("too_many_requests");
    }
    assert!(refused, "no rate limit after 70 calls");
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_protocol_conformance() {
    let dir = tempfile::tempdir().unwrap();
    conformance(&memory_table(dir.path()).await).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn every_tool_needs_its_scope() {
    let dir = tempfile::tempdir().unwrap();
    scope_matrix(&memory_table(dir.path()).await).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_player_token_gets_nothing_hidden() {
    let dir = tempfile::tempdir().unwrap();
    leak_suite(&memory_table(dir.path()).await).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_tool_touches_a_live_combat() {
    let dir = tempfile::tempdir().unwrap();
    no_combat_tools(&memory_table(dir.path()).await).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_are_idempotent_and_rate_limited() {
    let dir = tempfile::tempdir().unwrap();
    idempotency(&memory_table(dir.path()).await).await;
}

/// Everything above on PostgreSQL. Runs when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.
#[tokio::test(flavor = "multi_thread")]
async fn mcp_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = super::POSTGRES.lock().await;
    let dir = tempfile::tempdir().unwrap();
    for (i, flow) in ["conformance", "scopes", "leaks", "combats", "idempotency"]
        .iter()
        .enumerate()
    {
        drop_all(&url).await;
        let db = Db::open(&Location::Postgres(url.clone())).await.unwrap();
        let t = table(mcp_state(&dir.path().join(format!("{i}")), db)).await;
        match *flow {
            "conformance" => conformance(&t).await,
            "scopes" => scope_matrix(&t).await,
            "leaks" => leak_suite(&t).await,
            "combats" => no_combat_tools(&t).await,
            _ => idempotency(&t).await,
        }
    }
}
