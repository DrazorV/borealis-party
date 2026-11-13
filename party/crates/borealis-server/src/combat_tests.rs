//! Combat end to end (0.3.0): the projection properties (hidden creatures change nothing a player sees, byte for
//! byte; one visible turn per PC and revealed creature; DM-only writes move no `pseq` and send players nothing), the
//! leak suite (every player route with hidden ids, random ids and malformed bodies; whole bodies and headers searched
//! for hidden ids, labels, element ids and stat block text), the role matrix, two writers racing on `seq`, retried
//! requests, damage prompts, export and import, the health band switch (D6), the 90-day sweep, and the upgrade from
//! the 0.2.x schema.
//!
//! Monsters are the bundled SRD 5.2.1 ones. Runs on SQLite, and on PostgreSQL too when `BOREALIS_TEST_POSTGRES_URL`
//! names a database it may wipe.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use party_combat::party_dice::{D20Input, SequenceDie};
use party_combat::{apply, Actor, CombatState, Combatant, CombatantId, Command, Edition, Hp};
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;

use super::{
    drop_all, new_character, router_of, router_on, send, sign_in, sign_up, state_with, Client,
    Setup,
};
use crate::combat::{view_for, view_hash, EncounterInfo, Loaded, Member, Snapshot, Viewer};
use crate::db::{Db, Location};
use crate::hub::Subscription;
use crate::AppState;

const DRAGON: &str = "ID_SRD521_MONSTER_ANCIENT_RED_DRAGON";
const OWLBEAR: &str = "ID_SRD521_MONSTER_OWLBEAR";
const GOBLIN: &str = "ID_SRD521_MONSTER_GOBLIN_WARRIOR";
const NOT_FOUND: &str = "There is no such combat here.";
const PARTY_NOT_FOUND: &str = "There is no such party, invite or character here.";

// Helpers.

async fn character(client: &Client, name: &str) -> String {
    let (status, out) = client.rpc("create_character", new_character(name)).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    let (status, out) = client
        .rpc("save_character", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    id
}

async fn new_party(dm: &Client, name: &str) -> String {
    let (status, out) = dm.post("/api/me/roles/dm", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = dm.post("/api/parties", json!({ "name": name })).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    out["id"].as_str().unwrap().to_string()
}

async fn join(
    dm: &Client,
    party: &str,
    party_name: &str,
    player: &Client,
    username: &str,
    who: &str,
) {
    let c = character(player, who).await;
    let (status, out) = dm
        .post(
            &format!("/api/parties/{party}/invites"),
            json!({ "kind": "username", "username": username }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, invites) = player.get("/api/invites").await;
    let invite = invites["invites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["party"]["name"] == party_name)
        .unwrap_or_else(|| panic!("no invite: {invites}"))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, out) = player
        .post(
            &format!("/api/invites/{invite}/accept"),
            json!({ "characterId": c }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

async fn ok(client: &Client, method: &str, path: &str, body: Value) -> Value {
    let (status, out) = send(&client.router, Some(&client.token), method, path, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {out}");
    out
}

/// A request as it went over the wire: status, headers and body as text.
async fn raw(
    router: &Router,
    token: Option<&str>,
    method: &str,
    path: &str,
    body: &str,
) -> (StatusCode, String, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = format!("{:?}", response.headers());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

fn drain(sub: &mut Subscription) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    while let Ok(m) = sub.rx.try_recv() {
        out.push((m.event.to_string(), m.payload));
    }
    out
}

fn combat_events(events: &[(String, Value)]) -> Vec<Value> {
    events
        .iter()
        .filter(|(e, _)| e == crate::hub::COMBAT_EVENT)
        .map(|(_, p)| p.clone())
        .collect()
}

static REQUESTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn request_id() -> String {
    format!(
        "req-{}",
        REQUESTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    )
}

/// `action` on a combat, with a fresh request id.
async fn act(client: &Client, combat: &str, action: &str, args: Value) -> (StatusCode, Value) {
    client
        .post(
            &format!("/api/combats/{combat}/{action}"),
            json!({ "requestId": request_id(), "args": args }),
        )
        .await
}

async fn act_ok(client: &Client, combat: &str, action: &str, args: Value) -> Value {
    let (status, out) = act(client, combat, action, args.clone()).await;
    assert_eq!(status, StatusCode::OK, "{action} {args}: {out}");
    out
}

fn typed(total: i64) -> Value {
    json!({ "kind": "typed", "total": total })
}

/// A table: a DM with a party of two players' characters, a player of another party, and an outsider.
struct Table {
    state: AppState,
    router: Router,
    dm: Client,
    player: Client,
    member2: Client,
    other: Client,
    outsider: Client,
    party: String,
    other_party: String,
}

async fn table(state: AppState) -> Table {
    let router = router_of(state.clone(), Setup::default().max_upload);
    sign_up(&router, "cbt-admin").await;
    let dm = sign_up(&router, "cbt-dm").await;
    let player = sign_up(&router, "cbt-player").await;
    let member2 = sign_up(&router, "cbt-member2").await;
    let other = sign_up(&router, "cbt-other").await;
    let outsider = sign_up(&router, "cbt-outsider").await;
    let party = new_party(&dm, "Friday Party").await;
    join(
        &dm,
        &party,
        "Friday Party",
        &player,
        "cbt-player",
        "Hero One",
    )
    .await;
    join(
        &dm,
        &party,
        "Friday Party",
        &member2,
        "cbt-member2",
        "Hero Two",
    )
    .await;
    let dm2 = sign_up(&router, "cbt-dm2").await;
    let other_party = new_party(&dm2, "Other Party").await;
    join(
        &dm2,
        &other_party,
        "Other Party",
        &other,
        "cbt-other",
        "Elsewhere",
    )
    .await;
    Table {
        state,
        router,
        dm,
        player,
        member2,
        other,
        outsider,
        party,
        other_party,
    }
}

fn state_in(dir: &Path, db: Db) -> AppState {
    state_with(dir.to_path_buf(), db, Setup::default())
}

async fn memory_table(dir: &Path) -> Table {
    table(state_in(dir, Db::open(&Location::Memory).await.unwrap())).await
}

/// The DM's labels and notes the leak suite looks for.
const SECRETS: [&str; 5] = [
    "SECRET-LABEL-DRAGON",
    "SECRET-PUBLIC-DRAGON",
    "SECRET-LABEL-GOBLIN",
    "SECRET-NOTE",
    "SECRET-ENCOUNTER-NAME",
];

/// A live combat: a hidden dragon and goblin, a revealed owlbear, and the party's two PCs.
struct Fight {
    combat: String,
    encounter: String,
    dragon: String,
    goblin: String,
    owlbear: String,
    /// Encounter creature ids of the hidden ones.
    hidden_sources: Vec<String>,
    pc_player: String,
    pc_member2: String,
    /// Text from the hidden dragon's stat block.
    fragments: Vec<String>,
}

fn combatants_of(view: &Value) -> Vec<Value> {
    view["state"]["combatants"].as_array().unwrap().clone()
}

fn find_by_label(view: &Value, label: &str) -> String {
    combatants_of(view)
        .iter()
        .find(|c| c["label"] == label)
        .unwrap_or_else(|| panic!("no {label}: {view}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn pc_of(view: &Value, owner: &str) -> String {
    let (id, _) = view["characters"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, c)| c["ownerId"] == owner)
        .unwrap_or_else(|| panic!("no PC of {owner}: {view}"));
    id.clone()
}

fn last_creature(v: &Value) -> String {
    v["creatures"].as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn fight(t: &Table) -> Fight {
    let e = ok(
        &t.dm,
        "POST",
        &format!("/api/parties/{}/encounters", t.party),
        json!({ "name": "SECRET-ENCOUNTER-NAME", "publicName": "Cave mouth" }),
    )
    .await;
    let encounter = e["id"].as_str().unwrap().to_string();
    let path = format!("/api/encounters/{encounter}/creatures");
    let v = ok(
        &t.dm,
        "POST",
        &path,
        json!({ "elementId": DRAGON, "dmLabel": "SECRET-LABEL-DRAGON", "publicName": "SECRET-PUBLIC-DRAGON", "hidden": true }),
    )
    .await;
    let dragon_src = last_creature(&v);
    let v = ok(
        &t.dm,
        "POST",
        &path,
        json!({ "elementId": GOBLIN, "dmLabel": "SECRET-LABEL-GOBLIN", "hidden": true }),
    )
    .await;
    let goblin_src = last_creature(&v);
    ok(
        &t.dm,
        "POST",
        &path,
        json!({ "elementId": OWLBEAR, "dmLabel": "Bear", "publicName": "Owlbear", "hidden": false }),
    )
    .await;
    let dm_view = ok(
        &t.dm,
        "POST",
        &format!("/api/encounters/{encounter}/start"),
        json!({ "requestId": request_id() }),
    )
    .await;
    let combat = dm_view["id"].as_str().unwrap().to_string();
    let dragon = find_by_label(&dm_view, "SECRET-LABEL-DRAGON");
    let goblin = find_by_label(&dm_view, "SECRET-LABEL-GOBLIN");
    let owlbear = find_by_label(&dm_view, "Bear");
    let block = &dm_view["creatures"][&dragon]["statBlock"];
    let mut fragments = Vec::new();
    for section in ["actions", "traits"] {
        if let Some(f) = block[section].as_array().and_then(|l| l.first()) {
            let text = f["text"].as_str().unwrap_or_default();
            fragments.push(text.chars().take(40).collect());
        }
    }
    if let Some(formula) = block["hp"]["formula"].as_str() {
        fragments.push(formula.to_string());
    }
    assert!(!fragments.is_empty(), "{block}");
    // Notes on the hidden dragon, for the leak suite.
    act_ok(
        &t.dm,
        &combat,
        "set_notes",
        json!({ "id": dragon, "notes": "SECRET-NOTE" }),
    )
    .await;
    Fight {
        pc_player: pc_of(&dm_view, &t.player.id),
        pc_member2: pc_of(&dm_view, &t.member2.id),
        combat,
        encounter,
        dragon,
        goblin,
        owlbear,
        hidden_sources: vec![dragon_src, goblin_src],
        fragments,
    }
}

/// Initiative for everyone, typed (D43): the dragon first, so a hidden turn comes before the visible ones.
async fn roll_in(t: &Table, f: &Fight) {
    for (id, total) in [(&f.dragon, 25), (&f.owlbear, 18), (&f.goblin, 14)] {
        act_ok(
            &t.dm,
            &f.combat,
            "set_initiative",
            json!({ "id": id, "roll": typed(total) }),
        )
        .await;
    }
    act_ok(
        &t.player,
        &f.combat,
        "set_initiative",
        json!({ "id": f.pc_player, "roll": typed(16) }),
    )
    .await;
    act_ok(
        &t.member2,
        &f.combat,
        "set_initiative",
        json!({ "id": f.pc_member2, "roll": typed(9) }),
    )
    .await;
}

fn leaks(text: &str, f: &Fight) -> Vec<String> {
    let mut found = Vec::new();
    for s in SECRETS {
        if text.contains(s) {
            found.push(s.to_string());
        }
    }
    for id in [&f.dragon, &f.goblin]
        .into_iter()
        .chain(f.hidden_sources.iter())
    {
        if text.contains(id.as_str()) {
            found.push(id.clone());
        }
    }
    for id in [DRAGON, GOBLIN, OWLBEAR, "ID_SRD"] {
        if text.contains(id) {
            found.push(id.to_string());
        }
    }
    for fragment in &f.fragments {
        if !fragment.is_empty() && text.contains(fragment.as_str()) {
            found.push(fragment.clone());
        }
    }
    found
}

// The role matrix.

#[allow(clippy::too_many_lines)]
async fn role_matrix(t: &Table) {
    let f = fight(t).await;
    let c = &f.combat;
    for (who, client, want) in [
        ("dm", &t.dm, StatusCode::OK),
        ("player", &t.player, StatusCode::OK),
        ("member2", &t.member2, StatusCode::OK),
        ("other", &t.other, StatusCode::NOT_FOUND),
        ("outsider", &t.outsider, StatusCode::NOT_FOUND),
    ] {
        let (status, out) = client.get(&format!("/api/combats/{c}")).await;
        assert_eq!(status, want, "{who}: {out}");
        let (status, out) = client
            .get(&format!("/api/parties/{}/combat", t.party))
            .await;
        assert_eq!(status, want, "{who}: {out}");
        if want == StatusCode::OK {
            assert_eq!(out["combat"], c.as_str());
        }
    }
    let (status, _) = send(
        &t.router,
        None,
        "GET",
        &format!("/api/combats/{c}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = t.player.get(&format!("/api/combats/{c}/log")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, log) = t.dm.get(&format!("/api/combats/{c}/log")).await;
    assert_eq!(status, StatusCode::OK, "{log}");
    // DM-only actions: players, other parties' players and outsiders get the 404 and change nothing.
    let dm_only: Vec<(&str, Value)> = vec![
        (
            "damage",
            json!({ "target": f.owlbear, "parts": [{ "amount": typed(5) }] }),
        ),
        ("reveal", json!({ "id": f.dragon })),
        ("hide", json!({ "id": f.owlbear })),
        ("rename", json!({ "id": f.owlbear, "label": "x" })),
        ("start", json!({})),
        ("next_turn", json!({})),
        ("undo", json!({})),
        ("end", json!({})),
        ("export", json!({})),
        ("add_creature", json!({ "copyOf": f.owlbear })),
        ("remove_creature", json!({ "id": f.owlbear })),
        (
            "set_initiative",
            json!({ "id": f.owlbear, "roll": typed(3) }),
        ),
        (
            "death_save",
            json!({ "id": f.pc_member2, "roll": typed(15) }),
        ),
        (
            "set_initiative",
            json!({ "id": f.pc_member2, "roll": typed(3) }),
        ),
        ("add_combatant", json!({})),
        (
            "sync_player_hp",
            json!({ "id": f.pc_player, "current": 1, "max": 1 }),
        ),
    ];
    let (_, before) = t.dm.get(&format!("/api/combats/{c}")).await;
    for (who, client) in [
        ("player", &t.player),
        ("other", &t.other),
        ("outsider", &t.outsider),
    ] {
        for (action, args) in &dm_only {
            let (status, out) = act(client, c, action, args.clone()).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{who} {action}: {out}");
            assert_eq!(out["message"], NOT_FOUND, "{who} {action}: {out}");
        }
    }
    let (_, after) = t.dm.get(&format!("/api/combats/{c}")).await;
    assert_eq!(before["seq"], after["seq"], "nobody else wrote");
    // The DM's own server-only commands aren't offered either.
    for action in ["add_combatant", "sync_player_hp"] {
        let (status, _) = act(&t.dm, c, action, json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{action}");
    }
    // The player's own PC: allowed, and answered with the projection.
    let v = act_ok(
        &t.player,
        c,
        "set_initiative",
        json!({ "id": f.pc_player, "roll": typed(12) }),
    )
    .await;
    assert!(
        v.get("seq").is_none() && v.get("state").is_none(),
        "a player gets the projection: {v}"
    );
    assert_eq!(v["me"][0]["id"], f.pc_player.as_str());
    assert_eq!(v["me"][0]["prompts"], json!([]));
    let (_, log) = t.dm.get(&format!("/api/combats/{c}/log")).await;
    assert!(!log["entries"].as_array().unwrap().is_empty(), "{log}");
    // A creature the player's PC summons: a server id, the player acts for it, nobody else does.
    let v = act_ok(
        &t.player,
        c,
        "summon",
        json!({ "owner": f.pc_player, "combatant": { "id": "my-wolf", "kind": "creature", "label": "Wolf", "public_name": "Wolf", "hp": { "current": 11, "max": 11 } } }),
    )
    .await;
    let wolf = v["me"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["kind"] == "creature")
        .unwrap_or_else(|| panic!("the summon is mine: {v}"))["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(wolf, "my-wolf");
    assert!(uuid::Uuid::parse_str(&wolf).is_ok());
    let v = act_ok(
        &t.player,
        c,
        "damage",
        json!({ "target": wolf, "parts": [{ "amount": typed(3) }] }),
    )
    .await;
    let mine = v["me"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == wolf.as_str())
        .unwrap()
        .clone();
    assert_eq!(mine["hp"]["current"], 8, "{mine}");
    let (status, _) = act(
        &t.member2,
        c,
        "damage",
        json!({ "target": wolf, "parts": [{ "amount": typed(3) }] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another player's creature");
    let (status, _) = act(
        &t.player,
        c,
        "damage",
        json!({ "target": f.owlbear, "parts": [{ "amount": typed(3) }] }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a DM creature");
    // Starting: only the DM, and only once while live.
    let (status, _) = t
        .player
        .post(
            &format!("/api/encounters/{}/start", f.encounter),
            json!({ "requestId": request_id() }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, out) =
        t.dm.post(
            &format!("/api/encounters/{}/start", f.encounter),
            json!({ "requestId": request_id() }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{out}");
    // Import: the DM of the party only.
    let export = act_ok(&t.dm, c, "export", json!({})).await;
    for client in [&t.player, &t.other, &t.outsider] {
        let (status, _) = client
            .post(
                &format!("/api/parties/{}/combats/import", t.party),
                json!({ "requestId": request_id(), "file": export }),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    // Another party's DM is an outsider here.
    let (status, _) =
        t.dm.get(&format!("/api/parties/{}/combat", t.other_party))
            .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_dm_drives_creatures_and_players_their_own_pc() {
    let dir = tempfile::tempdir().unwrap();
    role_matrix(&memory_table(dir.path()).await).await;
}

// The leak suite.

const FIXED: [&str; 7] = [
    "Not making death saves.",
    "Dead: set HP to revive.",
    "No concentration check is waiting.",
    "The combat isn't running.",
    "The combat has ended.",
    "That roll isn't valid.",
    "Too many attempts. Wait a few minutes and try again.",
];

#[allow(clippy::too_many_lines)]
async fn leak_suite(t: &Table) {
    let f = fight(t).await;
    roll_in(t, &f).await;
    act_ok(&t.dm, &f.combat, "start", json!({})).await;
    act_ok(
        &t.dm,
        &f.combat,
        "damage",
        json!({ "target": f.dragon, "parts": [{ "amount": typed(30), "damage_type": "fire" }] }),
    )
    .await;
    act_ok(
        &t.dm,
        &f.combat,
        "condition_add",
        json!({ "effect": { "name": "SECRET-NOTE", "target": f.goblin, "conditions": ["prone"] } }),
    )
    .await;
    let c = &f.combat;
    let random = uuid::Uuid::new_v4().to_string();
    let ids: Vec<&str> = vec![
        f.dragon.as_str(),
        f.goblin.as_str(),
        f.hidden_sources[0].as_str(),
        random.as_str(),
        f.pc_member2.as_str(),
        "../../etc",
        "%00",
        "",
    ];
    let mut routes: Vec<(String, String, String)> = vec![
        ("GET".into(), format!("/api/combats/{c}"), String::new()),
        (
            "GET".into(),
            format!("/api/parties/{}/combat", t.party),
            String::new(),
        ),
        ("GET".into(), format!("/api/combats/{c}/log"), String::new()),
        (
            "GET".into(),
            format!("/api/combats/{random}"),
            String::new(),
        ),
        (
            "GET".into(),
            format!("/api/combats/{}", f.dragon),
            String::new(),
        ),
        (
            "POST".into(),
            format!("/api/combats/{c}/export"),
            "{}".into(),
        ),
    ];
    let actions = [
        "set_initiative",
        "death_save",
        "set_concentration",
        "end_concentration",
        "resolve_concentration",
        "resolve_prompt",
        "damage",
        "reveal",
        "undo",
        "no_such_action",
        "Death_Save",
    ];
    for action in actions {
        for id in &ids {
            for args in [
                json!({ "id": id, "roll": typed(10) }),
                json!({ "id": id, "name": "Bless" }),
                json!({ "promptId": id, "applied": true }),
                json!({ "target": id, "parts": [{ "amount": typed(1) }] }),
            ] {
                let body = json!({ "requestId": request_id(), "args": args }).to_string();
                routes.push(("POST".into(), format!("/api/combats/{c}/{action}"), body));
            }
        }
        for body in [
            "",
            "not json",
            "[]",
            "{\"args\":{}}",
            "{\"requestId\":\"\\u0000\",\"args\":{}}",
            "{\"requestId\":\"a\",\"args\":\"x\"}",
            "{\"requestId\":\"a\",\"args\":{\"id\":42}}",
        ] {
            routes.push((
                "POST".into(),
                format!("/api/combats/{c}/{action}"),
                body.to_string(),
            ));
        }
    }
    routes.push((
        "POST".into(),
        format!("/api/combats/{}/death_save", f.dragon),
        "{}".into(),
    ));
    // Every route for every caller (the write limit is reset between requests here; it has its own test).
    for (who, client) in [
        ("player", &t.player),
        ("member2", &t.member2),
        ("other", &t.other),
        ("outsider", &t.outsider),
    ] {
        for (method, path, body) in &routes {
            crate::combat::test_forget_writes(&client.id);
            let (status, headers, text) =
                raw(&t.router, Some(&client.token), method, path, body).await;
            let found = leaks(&text, &f);
            assert!(
                found.is_empty(),
                "{who} {method} {path} {body}: {found:?} in {text}"
            );
            let found = leaks(&headers, &f);
            assert!(
                found.is_empty(),
                "{who} {method} {path}: {found:?} in headers {headers}"
            );
            if status == StatusCode::NOT_FOUND {
                let v: Value = serde_json::from_str(&text).unwrap();
                assert!(
                    v["message"] == NOT_FOUND || v["message"] == PARTY_NOT_FOUND,
                    "{who} {path}: {text}"
                );
            } else if status != StatusCode::OK {
                let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                assert!(
                    FIXED.iter().any(|m| v["message"] == *m),
                    "{who} {method} {path} {body}: {status} {text}"
                );
            }
        }
    }
    // The player's own projection: hidden creatures absent, no count, no slot.
    let (_, view) = t.player.get(&format!("/api/combats/{c}")).await;
    let order = view["order"].as_array().unwrap();
    assert_eq!(order.len(), 3, "two PCs and the owlbear: {view}");
    let owlbear = order.iter().find(|o| o["kind"] == "creature").unwrap();
    assert_eq!(owlbear["name"], "Owlbear");
    assert!(
        owlbear["statBlock"].is_object(),
        "D20 b: the full stat block once revealed"
    );
    assert!(owlbear["statBlock"].get("xp").is_none());
    assert!(owlbear.get("initiative").is_none());
    for key in ["seq", "state", "creatures", "characters", "undo"] {
        assert!(view.get(key).is_none(), "{key} in {view}");
    }
    let text = view.to_string();
    for key in [
        "\"label\"",
        "\"notes\"",
        "tiebreak",
        "next_effect",
        "\"seq\"",
    ] {
        assert!(!text.contains(key), "{key} in {text}");
    }
    // D5: the dragon acted first, unseen; the pointer is on a visible turn or waiting, never on it.
    assert_ne!(view["turn"]["id"], f.dragon.as_str());
    // Another member's PC: a band, no HP numbers (D6).
    let theirs = order
        .iter()
        .find(|o| o["kind"] == "character" && o["mine"] == false)
        .unwrap();
    assert!(theirs.get("hp").is_none(), "{theirs}");
    assert!(theirs.get("band").is_some(), "{theirs}");
    // Reveal: the dragon appears, with its stat block (D20 b).
    act_ok(&t.dm, c, "reveal", json!({ "id": f.dragon })).await;
    let (_, view) = t.player.get(&format!("/api/combats/{c}")).await;
    let dragon = view["order"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == f.dragon.as_str())
        .cloned()
        .unwrap_or_else(|| panic!("revealed: {view}"));
    assert_eq!(dragon["name"], "SECRET-PUBLIC-DRAGON");
    assert!(dragon["statBlock"].is_object());
    assert!(!view.to_string().contains("SECRET-LABEL-DRAGON"));
    // Hidden again: gone again.
    act_ok(&t.dm, c, "hide", json!({ "id": f.dragon })).await;
    let (_, view) = t.player.get(&format!("/api/combats/{c}")).await;
    assert!(leaks(&view.to_string(), &f).is_empty(), "{view}");
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_hidden_or_dm_only_reaches_a_player() {
    let dir = tempfile::tempdir().unwrap();
    leak_suite(&memory_table(dir.path()).await).await;
}

// Player events and pseq.

async fn player_events(t: &Table) {
    let f = fight(t).await;
    roll_in(t, &f).await;
    let mut dm_sub = t.state.hub.subscribe(&t.dm.id, "s-dm", None);
    let mut sub = t.state.hub.subscribe(&t.player.id, "s-player", None);
    let (_, view) = t.player.get(&format!("/api/combats/{}", f.combat)).await;
    let pseq = view["pseq"].as_i64().unwrap();
    assert!(pseq >= 1, "{view}");
    // DM-only writes: hidden creatures, labels, notes, initiative of the hidden.
    let dm_only = [
        (
            "damage",
            json!({ "target": f.dragon, "parts": [{ "amount": typed(12) }] }),
        ),
        ("rename", json!({ "id": f.goblin, "label": "Goblin boss" })),
        ("rename", json!({ "id": f.owlbear, "label": "Big bear" })),
        ("set_notes", json!({ "id": f.owlbear, "notes": "mean" })),
        (
            "set_initiative",
            json!({ "id": f.goblin, "roll": typed(2) }),
        ),
        (
            "condition_add",
            json!({ "effect": { "name": "Down", "target": f.goblin, "conditions": ["prone"] } }),
        ),
        ("heal", json!({ "target": f.dragon, "amount": typed(5) })),
    ];
    for (action, args) in &dm_only {
        act_ok(&t.dm, &f.combat, action, args.clone()).await;
        let got = drain(&mut sub);
        assert!(
            combat_events(&got).is_empty(),
            "{action}: players heard {got:?}"
        );
        let dm_got = drain(&mut dm_sub);
        assert_eq!(
            combat_events(&dm_got).len(),
            1,
            "{action}: the DM hears every write: {dm_got:?}"
        );
    }
    let (_, view) = t.player.get(&format!("/api/combats/{}", f.combat)).await;
    assert_eq!(
        view["pseq"].as_i64().unwrap(),
        pseq,
        "DM-only writes move no pseq"
    );
    // A reveal reaches the player at once, with only their own pseq.
    act_ok(&t.dm, &f.combat, "reveal", json!({ "id": f.goblin })).await;
    let got = drain(&mut sub);
    let events = combat_events(&got);
    assert_eq!(events.len(), 1, "{got:?}");
    assert_eq!(events[0]["pseq"].as_i64().unwrap(), pseq + 1);
    assert!(events[0].get("seq").is_none(), "{:?}", events[0]);
    let (_, view) = t.player.get(&format!("/api/combats/{}", f.combat)).await;
    assert!(view["order"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["id"] == f.goblin.as_str()));
    // A prompt for one player moves only that player's view.
    let mut sub2 = t.state.hub.subscribe(&t.member2.id, "s-m2", None);
    act_ok(
        &t.dm,
        &f.combat,
        "damage",
        json!({ "target": f.pc_player, "parts": [{ "amount": typed(1), "damage_type": "slashing" }] }),
    )
    .await;
    assert_eq!(combat_events(&drain(&mut sub)).len(), 1);
    assert!(
        combat_events(&drain(&mut sub2)).is_empty(),
        "member2's view didn't change"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn players_hear_of_a_combat_only_when_their_view_changes() {
    let dir = tempfile::tempdir().unwrap();
    player_events(&memory_table(dir.path()).await).await;
}

// Damage prompts (D7 a), the health band (D6) and ending.

#[allow(clippy::too_many_lines)]
async fn prompts_and_band(t: &Table) {
    assert!(!crate::parties::in_combat(&t.party));
    let (_, roster) = t
        .player
        .get(&format!("/api/parties/{}/roster", t.party))
        .await;
    assert!(
        !roster.to_string().contains("\"band\""),
        "no band outside combat: {roster}"
    );
    let f = fight(t).await;
    assert!(
        crate::parties::in_combat(&t.party),
        "D6: on while a combat is live"
    );
    let (_, roster) = t
        .player
        .get(&format!("/api/parties/{}/roster", t.party))
        .await;
    assert!(
        roster.to_string().contains("\"band\""),
        "the band during combat: {roster}"
    );
    roll_in(t, &f).await;
    act_ok(&t.dm, &f.combat, "start", json!({})).await;
    let (_, before) = t.player.get(&format!("/api/combats/{}", f.combat)).await;
    let hp = before["me"][0]["hp"].clone();
    act_ok(
        &t.dm,
        &f.combat,
        "damage",
        json!({ "target": f.pc_player, "parts": [{ "amount": typed(3), "damage_type": "slashing" }] }),
    )
    .await;
    let (_, view) = t.player.get(&format!("/api/combats/{}", f.combat)).await;
    assert_eq!(
        view["me"][0]["hp"], hp,
        "the DM never writes a PC's HP (D7 a)"
    );
    let prompts = view["me"][0]["prompts"].as_array().unwrap().clone();
    assert_eq!(prompts.len(), 1, "{view}");
    assert_eq!(prompts[0]["amount"], 3);
    assert_eq!(prompts[0]["kind"], "damage");
    let (_, other) = t.member2.get(&format!("/api/combats/{}", f.combat)).await;
    assert!(other["me"][0]["prompts"].as_array().unwrap().is_empty());
    let (_, dm_view) = t.dm.get(&format!("/api/combats/{}", f.combat)).await;
    assert_eq!(dm_view["prompts"].as_array().unwrap().len(), 1);
    let prompt = prompts[0]["id"].as_str().unwrap();
    let (status, _) = act(
        &t.member2,
        &f.combat,
        "resolve_prompt",
        json!({ "promptId": prompt, "applied": true }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "only its owner answers it");
    let max = hp["max"].as_i64().unwrap().max(3);
    let now = (hp["current"].as_i64().unwrap() - 3).max(0);
    let v = act_ok(
        &t.player,
        &f.combat,
        "resolve_prompt",
        json!({ "promptId": prompt, "applied": true, "hp": { "current": now, "max": max, "temp": 0 } }),
    )
    .await;
    assert_eq!(v["me"][0]["hp"]["current"], now, "{v}");
    assert!(v["me"][0]["prompts"].as_array().unwrap().is_empty());
    let (status, _) = act(
        &t.player,
        &f.combat,
        "resolve_prompt",
        json!({ "promptId": prompt, "applied": true }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "answered once");
    // End: off again, and the encounter is ended.
    act_ok(&t.dm, &f.combat, "end", json!({})).await;
    assert!(!crate::parties::in_combat(&t.party));
    let (_, live) = t
        .player
        .get(&format!("/api/parties/{}/combat", t.party))
        .await;
    assert_eq!(live["combat"], Value::Null);
    let (_, e) = t.dm.get(&format!("/api/encounters/{}", f.encounter)).await;
    assert_eq!(e["status"], "ended", "{e}");
    let (status, out) = act(&t.dm, &f.combat, "next_turn", json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{out}");
    let (status, out) = act(
        &t.player,
        &f.combat,
        "death_save",
        json!({ "id": f.pc_player, "roll": typed(10) }),
    )
    .await;
    assert_eq!(
        (status, out["message"].as_str()),
        (StatusCode::CONFLICT, Some("The combat has ended."))
    );
    let (status, _) = t.player.get(&format!("/api/combats/{}", f.combat)).await;
    assert_eq!(status, StatusCode::OK, "an ended combat stays readable");
}

#[tokio::test(flavor = "multi_thread")]
async fn damage_on_a_pc_is_a_prompt_and_the_band_shows_only_in_combat() {
    let dir = tempfile::tempdir().unwrap();
    prompts_and_band(&memory_table(dir.path()).await).await;
}

// Retries, CAS and undo.

fn hp_of(v: &Value, id: &str) -> i64 {
    combatants_of(v).iter().find(|c| c["id"] == id).unwrap()["hp"]["current"]
        .as_i64()
        .unwrap()
}

#[allow(clippy::too_many_lines)]
async fn retries_and_cas(t: &Table) {
    let f = fight(t).await;
    let path = format!("/api/combats/{}/damage", f.combat);
    let body = json!({ "requestId": "same-request-1", "args": { "target": f.owlbear, "parts": [{ "amount": typed(4) }] } });
    let (status, first) = t.dm.post(&path, body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let (status, again) = t.dm.post(&path, body).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["replayed"], true);
    assert_eq!(again["seq"], first["seq"], "done once");
    assert_eq!(hp_of(&again, &f.owlbear), hp_of(&first, &f.owlbear));
    // The same id from the player is the player's own.
    let pbody =
        json!({ "requestId": "same-request-1", "args": { "id": f.pc_player, "roll": typed(11) } });
    let (status, v) = t
        .player
        .post(&format!("/api/combats/{}/set_initiative", f.combat), pbody)
        .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(v.get("replayed").is_none());
    // expectSeq: a stale one is refused.
    let seq = t.dm.get(&format!("/api/combats/{}", f.combat)).await.1["seq"]
        .as_i64()
        .unwrap();
    let (status, out) = t
        .dm
        .post(
            &path,
            json!({ "requestId": request_id(), "expectSeq": seq - 1, "args": { "target": f.owlbear, "parts": [{ "amount": typed(1) }] } }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{out}");
    assert_eq!(out["code"], "stale");
    assert_eq!(out["seq"], seq);
    let (status, out) = t
        .dm
        .post(
            &path,
            json!({ "requestId": request_id(), "expectSeq": seq, "args": { "target": f.owlbear, "parts": [{ "amount": typed(1) }] } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    // Two writers at once, without the in-process lock: compare-and-set keeps every write, once.
    let before = t.dm.get(&format!("/api/combats/{}", f.combat)).await.1;
    let start_hp = hp_of(&before, &f.owlbear);
    let start_seq = before["seq"].as_i64().unwrap();
    let mut tasks = Vec::new();
    for _ in 0..6 {
        let state = t.state.clone();
        let combat = f.combat.clone();
        let owlbear = f.owlbear.clone();
        let account = t.dm.id.clone();
        tasks.push(tokio::spawn(async move {
            crate::combat::test_write_damage(&state, &combat, &account, &owlbear, 1).await
        }));
    }
    let mut seqs = BTreeSet::new();
    for task in tasks {
        seqs.insert(task.await.unwrap().unwrap());
    }
    assert_eq!(seqs.len(), 6, "each write got its own seq: {seqs:?}");
    let after = t.dm.get(&format!("/api/combats/{}", f.combat)).await.1;
    assert_eq!(after["seq"].as_i64().unwrap(), start_seq + 6, "{after}");
    assert_eq!(hp_of(&after, &f.owlbear), start_hp - 6);
    // Undo walks back one write at a time.
    let v = act_ok(&t.dm, &f.combat, "undo", json!({})).await;
    assert_eq!(hp_of(&v, &f.owlbear), start_hp - 5);
    assert_eq!(v["undo"][0], "damage");
}

#[tokio::test(flavor = "multi_thread")]
async fn retries_do_a_write_once_and_two_writers_both_land() {
    let dir = tempfile::tempdir().unwrap();
    retries_and_cas(&memory_table(dir.path()).await).await;
}

// Export and import (D45).

#[allow(clippy::too_many_lines)]
async fn export_import(t: &Table) {
    let f = fight(t).await;
    roll_in(t, &f).await;
    act_ok(&t.dm, &f.combat, "start", json!({})).await;
    act_ok(
        &t.dm,
        &f.combat,
        "damage",
        json!({ "target": f.owlbear, "parts": [{ "amount": typed(7) }] }),
    )
    .await;
    let file = act_ok(&t.dm, &f.combat, "export", json!({})).await;
    assert_eq!(file["format"], "borealis-party-combat");
    assert_eq!(file["schema_version"], 1);
    let import = format!("/api/parties/{}/combats/import", t.party);
    let (status, out) =
        t.dm.post(&import, json!({ "requestId": request_id(), "file": file }))
            .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "one live combat per party: {out}"
    );
    act_ok(&t.dm, &f.combat, "end", json!({})).await;
    let request = request_id();
    let v = ok(
        &t.dm,
        "POST",
        &import,
        json!({ "requestId": request, "file": file.to_string() }),
    )
    .await;
    let new = v["id"].as_str().unwrap().to_string();
    assert_ne!(new, f.combat, "always a new combat");
    assert_eq!(v["seq"], 0);
    let old: Vec<String> = combatants_of(&file)
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    for c in combatants_of(&v) {
        let id = c["id"].as_str().unwrap().to_string();
        assert!(!old.contains(&id), "new ids: {c}");
        assert!(uuid::Uuid::parse_str(&id).is_ok());
    }
    let pick = |v: &Value, label: &str| {
        combatants_of(v)
            .into_iter()
            .find(|c| c["label"] == label)
            .unwrap()
    };
    assert_eq!(pick(&v, "Bear")["hp"], pick(&file, "Bear")["hp"]);
    assert_eq!(v["state"]["round"], file["state"]["round"]);
    let bear = pick(&v, "Bear")["id"].as_str().unwrap().to_string();
    assert!(v["creatures"][&bear]["statBlock"].is_object());
    let characters = v["characters"].as_object().unwrap();
    assert_eq!(characters.len(), 2);
    assert!(characters.values().all(|c| c["member"] == true));
    let (status, seen) = t.player.get(&format!("/api/combats/{new}")).await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!(seen["me"].as_array().unwrap().len(), 1);
    // Retried: the same combat.
    let again = ok(
        &t.dm,
        "POST",
        &import,
        json!({ "requestId": request, "file": file }),
    )
    .await;
    assert_eq!(again["id"], new.as_str());
    assert_eq!(again["replayed"], true);
    // Refused: another format, a newer schema.
    act_ok(&t.dm, &new, "end", json!({})).await;
    let mut bad = file.clone();
    bad["format"] = json!("something-else");
    let (status, _) =
        t.dm.post(&import, json!({ "requestId": request_id(), "file": bad }))
            .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let mut newer = file.clone();
    newer["schema_version"] = json!(99);
    let (status, _) =
        t.dm.post(&import, json!({ "requestId": request_id(), "file": newer }))
            .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_export_imports_as_a_new_combat() {
    let dir = tempfile::tempdir().unwrap();
    export_import(&memory_table(dir.path()).await).await;
}

// Creatures added mid-fight, the log sweep and the write limit.

async fn count_rows(db: &Db, table: &str, combat: &str) -> i64 {
    sqlx::query(&format!(
        "SELECT COUNT(*) FROM {table} WHERE combat_id = $1"
    ))
    .bind(combat)
    .fetch_one(db.pool())
    .await
    .unwrap()
    .try_get::<i64, _>(0)
    .unwrap()
}

async fn add_sweep_and_limits(t: &Table) {
    let f = fight(t).await;
    let v = act_ok(
        &t.dm,
        &f.combat,
        "add_creature",
        json!({ "copyOf": f.owlbear, "count": 2 }),
    )
    .await;
    let bears: Vec<Value> = combatants_of(&v)
        .into_iter()
        .filter(|c| c["public_name"] == "Owlbear")
        .collect();
    assert_eq!(bears.len(), 3, "{v}");
    assert!(bears.iter().any(|b| b["label"] == "Bear 2"));
    assert!(bears.iter().any(|b| b["label"] == "Bear 3"));
    for b in &bears {
        let id = b["id"].as_str().unwrap();
        assert!(uuid::Uuid::parse_str(id).is_ok(), "random UUIDs: {id}");
        assert!(v["creatures"][id]["statBlock"].is_object());
    }
    let v = act_ok(
        &t.dm,
        &f.combat,
        "add_creature",
        json!({ "encounterCreatureId": f.hidden_sources[1], "hidden": true }),
    )
    .await;
    assert_eq!(
        combatants_of(&v)
            .iter()
            .filter(|c| c["visible"] == false)
            .count(),
        3,
        "{}",
        v["state"]["combatants"]
    );
    // The turn order lists those with an initiative: the copies rolled theirs, the first owlbear gets one now.
    act_ok(
        &t.dm,
        &f.combat,
        "set_initiative",
        json!({ "id": f.owlbear, "roll": typed(11) }),
    )
    .await;
    let (_, seen) = t.player.get(&format!("/api/combats/{}", f.combat)).await;
    assert_eq!(
        seen["order"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|o| o["kind"] == "creature")
            .count(),
        3
    );
    // One undo step for the whole add (after the initiative's own).
    act_ok(&t.dm, &f.combat, "undo", json!({})).await;
    let v = act_ok(&t.dm, &f.combat, "undo", json!({})).await;
    assert_eq!(
        combatants_of(&v)
            .iter()
            .filter(|c| c["visible"] == false)
            .count(),
        2
    );
    // The sweep: an ended combat's log goes 90 days later, not before.
    act_ok(&t.dm, &f.combat, "end", json!({})).await;
    let db = &t.state.db;
    assert!(count_rows(db, "combat_log", &f.combat).await > 0);
    crate::combat::sweep(db, crate::db::now() + 89 * 86_400)
        .await
        .unwrap();
    assert!(
        count_rows(db, "combat_log", &f.combat).await > 0,
        "kept for 90 days"
    );
    crate::combat::sweep(db, crate::db::now() + 91 * 86_400)
        .await
        .unwrap();
    assert_eq!(count_rows(db, "combat_log", &f.combat).await, 0);
    assert_eq!(count_rows(db, "combat_journal", &f.combat).await, 0);
    let (status, _) = t.dm.get(&format!("/api/combats/{}", f.combat)).await;
    assert_eq!(status, StatusCode::OK, "the combat itself stays");
    // The write limit: 240 a minute per account.
    let mut limited = false;
    for _ in 0..260 {
        let (status, _) = act(&t.outsider, &f.combat, "next_turn", json!({})).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
    }
    assert!(limited, "writes are limited per account");
}

#[tokio::test(flavor = "multi_thread")]
async fn creatures_join_mid_fight_logs_are_swept_and_writes_limited() {
    let dir = tempfile::tempdir().unwrap();
    add_sweep_and_limits(&memory_table(dir.path()).await).await;
}

// Properties of the projection, without a database.

/// A small reproducible random source (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn snapshot_of(state: CombatState, roster: BTreeMap<String, Member>) -> Snapshot {
    Snapshot {
        combat: Loaded {
            id: "combat-1".into(),
            party: "party-1".into(),
            encounter: Some("enc-1".into()),
            status: "live".into(),
            seq: 7,
            state,
            die: party_combat::party_dice::SeededDie::new(1),
            started_at: 1,
            ended_at: None,
        },
        roster,
        prompts: vec![],
        rolls: vec![],
        encounter: Some(EncounterInfo {
            id: "enc-1".into(),
            name: "DM name".into(),
            public_name: "Public".into(),
        }),
    }
}

fn run(state: &CombatState, cmd: &Command) -> CombatState {
    let mut die = SequenceDie::new(vec![10; 64]);
    apply(state, &Actor::Dm, cmd, &mut die)
        .unwrap_or_else(|e| panic!("{cmd:?}: {e}"))
        .state
}

fn creature_member(text: &str) -> Member {
    let words = if text.starts_with("hidden-") {
        "SECRET bite"
    } else {
        "A bite"
    };
    Member {
        kind: "creature".into(),
        encounter_creature: Some(format!("src-{text}")),
        owner: None,
        character: None,
        block: Some(json!({ "actions": [{ "name": "Bite", "text": format!("{words} {text}") }] })),
    }
}

fn pc_member(i: u64) -> Member {
    Member {
        kind: "character".into(),
        encounter_creature: None,
        owner: Some(format!("acct-{i}")),
        character: Some(format!("c_{i}")),
        block: None,
    }
}

fn add_to(state: &CombatState, c: Combatant) -> CombatState {
    run(
        state,
        &Command::AddCombatant {
            combatant: Box::new(c),
            roll_initiative: false,
        },
    )
}

fn project(state: &CombatState, roster: &BTreeMap<String, Member>) -> Vec<u8> {
    let chars: BTreeSet<String> = ["c_0".to_string()].into();
    let v = view_for(
        &Viewer::Player {
            account: "acct-0",
            characters: &chars,
        },
        &snapshot_of(state.clone(), roster.clone()),
    );
    serde_json::to_vec(&v).unwrap()
}

fn on_visible(s: &CombatState) -> bool {
    s.current
        .as_ref()
        .is_some_and(|id| !id.0.starts_with("hidden-"))
}

/// Two combats: `visible` has the PCs and revealed creatures; `hidden` has the same plus hidden creatures at random
/// places in the order. Each visible step runs on both (the second also runs its hidden turns, and the DM renames
/// and annotates hidden creatures). After each step the player projections must be byte-identical; while hidden
/// creatures act, nothing a player sees moves; and each round gives each visible combatant exactly one turn.
#[test]
#[allow(clippy::too_many_lines)]
fn hidden_creatures_change_nothing_a_player_sees() {
    for case in 0..150_u64 {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (case + 1));
        let edition = if case % 2 == 0 {
            Edition::Srd521
        } else {
            Edition::Srd51
        };
        let mut visible = CombatState::new(edition);
        let mut roster = BTreeMap::new();
        let pcs = 1 + rng.below(4);
        let revealed = rng.below(4);
        let hidden_count = 1 + rng.below(5);
        let mut visible_ids = Vec::new();
        for i in 0..pcs {
            let id = format!("pc-{i}");
            let hp = Hp {
                current: 20,
                max: 20,
                temp: 0,
            };
            visible = add_to(
                &visible,
                Combatant::character(&id, format!("acct-{i}"), format!("Hero {i}"), hp, 15, 2),
            );
            roster.insert(id.clone(), pc_member(i));
            visible_ids.push(id);
        }
        for i in 0..revealed {
            let id = format!("seen-{i}");
            let mut c = Combatant::creature(&id, format!("Label {i}"), 30, 13, 1);
            c.public_name = format!("Beast {i}");
            visible = add_to(&visible, c);
            visible = run(
                &visible,
                &Command::Reveal {
                    id: CombatantId::new(id.clone()),
                },
            );
            roster.insert(id.clone(), creature_member(&id));
            visible_ids.push(id);
        }
        let mut hidden = visible.clone();
        let mut hidden_roster = roster.clone();
        let mut hidden_ids = Vec::new();
        for i in 0..hidden_count {
            let id = format!("hidden-{i}-{}", rng.next());
            let mut c = Combatant::creature(&id, format!("SECRET LABEL {i}"), 50, 18, 3);
            c.public_name = format!("SECRET PUBLIC {i}");
            hidden = add_to(&hidden, c);
            hidden_roster.insert(id.clone(), creature_member(&id));
            hidden_ids.push(id);
        }
        // Initiative: the same for the visible ones in both (no ties among them); random for the hidden.
        let mut totals: Vec<i32> = (1..=40).collect();
        for id in &visible_ids {
            let pick = usize::try_from(rng.below(totals.len() as u64)).unwrap();
            let total = totals.remove(pick);
            let cmd = Command::SetInitiative {
                id: CombatantId::new(id.clone()),
                roll: D20Input::Typed {
                    total,
                    natural: None,
                },
            };
            visible = run(&visible, &cmd);
            hidden = run(&hidden, &cmd);
        }
        for id in &hidden_ids {
            let total = i32::try_from(1 + rng.below(40)).unwrap();
            hidden = run(
                &hidden,
                &Command::SetInitiative {
                    id: CombatantId::new(id.clone()),
                    roll: D20Input::Typed {
                        total,
                        natural: None,
                    },
                },
            );
        }
        assert_eq!(
            project(&visible, &roster),
            project(&hidden, &hidden_roster),
            "case {case}: setup"
        );
        visible = run(&visible, &Command::Start);
        hidden = run(&hidden, &Command::Start);
        let mut guard = 0;
        while !on_visible(&hidden) {
            hidden = run(&hidden, &Command::NextTurn);
            guard += 1;
            assert!(guard < 50, "case {case}");
        }
        assert!(on_visible(&visible), "case {case}");
        let steps = usize::try_from(3 * (pcs + revealed)).unwrap();
        for step in 0..steps {
            if let Some(id) = hidden_ids.get(usize::try_from(rng.below(hidden_count)).unwrap()) {
                let cid = CombatantId::new(id.clone());
                hidden = run(
                    &hidden,
                    &Command::SetNotes {
                        id: cid.clone(),
                        notes: "SECRET".into(),
                    },
                );
                if rng.below(2) == 0 {
                    hidden = run(
                        &hidden,
                        &Command::Rename {
                            id: cid,
                            label: format!("SECRET {step}"),
                        },
                    );
                }
            }
            let a = project(&visible, &roster);
            let b = project(&hidden, &hidden_roster);
            assert_eq!(
                a,
                b,
                "case {case} step {step}:\n{}\n{}",
                String::from_utf8_lossy(&a),
                String::from_utf8_lossy(&b)
            );
            let text = String::from_utf8_lossy(&b);
            assert!(
                !text.contains("SECRET") && !text.contains("hidden-"),
                "case {case}: {text}"
            );
            visible = run(&visible, &Command::NextTurn);
            hidden = run(&hidden, &Command::NextTurn);
            let mut guard = 0;
            while !on_visible(&hidden) {
                // While a hidden creature acts, the players' view is still the last visible turn's.
                let seen: Value =
                    serde_json::from_slice(&project(&hidden, &hidden_roster)).unwrap();
                let was: Value = serde_json::from_slice(&b).unwrap();
                assert_eq!(
                    seen["turn"], was["turn"],
                    "case {case}: a hidden turn showed"
                );
                assert_eq!(seen["order"], was["order"], "case {case}");
                hidden = run(&hidden, &Command::NextTurn);
                guard += 1;
                assert!(guard < 50, "case {case}");
            }
        }
        // The visible order has the PCs and revealed creatures only; each gets one turn a round.
        let view: Value = serde_json::from_slice(&project(&hidden, &hidden_roster)).unwrap();
        assert_eq!(
            view["order"].as_array().unwrap().len() as u64,
            pcs + revealed,
            "case {case}"
        );
        let mut s = hidden.clone();
        let mut pointers = Vec::new();
        for _ in 0..s.combatants.len() {
            let v: Value = serde_json::from_slice(&project(&s, &hidden_roster)).unwrap();
            pointers.push(v["turn"]["id"].clone());
            s = run(&s, &Command::NextTurn);
        }
        pointers.dedup();
        let distinct: BTreeSet<String> = pointers
            .iter()
            .filter_map(|p| p.as_str().map(str::to_string))
            .collect();
        assert_eq!(
            distinct.len() as u64,
            pcs + revealed,
            "case {case}: {pointers:?}"
        );
        assert_eq!(
            pointers.len() as u64,
            pcs + revealed,
            "case {case}: each visible turn once: {pointers:?}"
        );
    }
}

/// A DM-only write leaves every player's projection, and so its hash, unchanged.
#[test]
fn dm_only_writes_leave_the_player_hash_alone() {
    let mut state = CombatState::new(Edition::Srd521);
    let mut roster = BTreeMap::new();
    let hp = Hp {
        current: 20,
        max: 20,
        temp: 0,
    };
    state = add_to(
        &state,
        Combatant::character("pc-0", "acct-0", "Hero", hp, 15, 2),
    );
    roster.insert("pc-0".to_string(), pc_member(0));
    state = add_to(
        &state,
        Combatant::creature("hidden-ghost", "SECRET", 40, 12, 1),
    );
    roster.insert("hidden-ghost".to_string(), creature_member("hidden-ghost"));
    let chars: BTreeSet<String> = ["c_0".to_string()].into();
    let hash = |s: &CombatState, r: &BTreeMap<String, Member>| {
        view_hash(&view_for(
            &Viewer::Player {
                account: "acct-0",
                characters: &chars,
            },
            &snapshot_of(s.clone(), r.clone()),
        ))
    };
    let base = hash(&state, &roster);
    let ghost = CombatantId::new("hidden-ghost");
    for cmd in [
        Command::SetNotes {
            id: ghost.clone(),
            notes: "x".into(),
        },
        Command::Rename {
            id: ghost.clone(),
            label: "y".into(),
        },
        Command::SetInitiative {
            id: ghost.clone(),
            roll: D20Input::Typed {
                total: 30,
                natural: None,
            },
        },
        Command::SetHp {
            id: ghost.clone(),
            current: 5,
            max: None,
            temp: None,
        },
        Command::RemoveCombatant { id: ghost.clone() },
    ] {
        let next = run(&state, &cmd);
        assert_eq!(hash(&next, &roster), base, "{cmd:?}");
        state = next;
    }
    // A hidden creature's stat block changing is DM-only too.
    let mut edited = roster.clone();
    edited.insert(
        "hidden-ghost".to_string(),
        creature_member("hidden-another"),
    );
    assert_eq!(hash(&state, &edited), base);
    // And a player-visible change does move it.
    let moved = run(
        &state,
        &Command::SetInitiative {
            id: CombatantId::new("pc-0"),
            roll: D20Input::Typed {
                total: 4,
                natural: None,
            },
        },
    );
    assert_ne!(base, hash(&moved, &roster));
}

// The upgrade from 0.2.x and the schema.

async fn seed_0_2(location: &Location, data: &Path) {
    let old = Db::open_at(location, 3).await.unwrap();
    let router = router_on(data.to_path_buf(), old.clone());
    sign_up(&router, "up-admin").await;
    let dm = sign_up(&router, "up-dm").await;
    let player = sign_up(&router, "up-player").await;
    let party = new_party(&dm, "Before 0.3").await;
    join(&dm, &party, "Before 0.3", &player, "up-player", "Old Hand").await;
    old.pool().close().await;
}

async fn check_upgraded(location: &Location, data: &Path) {
    let db = Db::open(location).await.unwrap();
    let version: i64 = sqlx::query("SELECT MAX(version) FROM schema_version")
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(version, i64::try_from(Db::schema_version()).unwrap());
    let router = router_on(data.to_path_buf(), db.clone());
    let dm = sign_in(&router, "up-dm").await;
    let player = sign_in(&router, "up-player").await;
    let (_, list) = dm.get("/api/parties").await;
    let party = list["parties"][0]["id"].as_str().unwrap().to_string();
    let e = ok(
        &dm,
        "POST",
        &format!("/api/parties/{party}/encounters"),
        json!({ "name": "After" }),
    )
    .await;
    let e = e["id"].as_str().unwrap().to_string();
    ok(
        &dm,
        "POST",
        &format!("/api/encounters/{e}/creatures"),
        json!({ "elementId": GOBLIN, "hidden": false }),
    )
    .await;
    let v = ok(
        &dm,
        "POST",
        &format!("/api/encounters/{e}/start"),
        json!({ "requestId": "up-1" }),
    )
    .await;
    let combat = v["id"].as_str().unwrap().to_string();
    assert_eq!(
        v["characters"].as_object().unwrap().len(),
        1,
        "the old member is in the fight"
    );
    let (status, seen) = player.get(&format!("/api/combats/{combat}")).await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    act_ok(&dm, &combat, "end", json!({})).await;
    // A database that says the step ran but has no combats table gets it again.
    for table in [
        "combat_requests",
        "combat_prompts",
        "combat_views",
        "combat_log",
        "combat_journal",
        "combatants",
        "combats",
    ] {
        sqlx::query(&format!("DROP TABLE {table}"))
            .execute(db.pool())
            .await
            .unwrap();
    }
    let again = Db::open(location).await.unwrap();
    let router = router_on(data.to_path_buf(), again);
    let dm = sign_in(&router, "up-dm").await;
    let (status, out) = dm.get(&format!("/api/parties/{party}/combat")).await;
    assert_eq!(
        (status, out["combat"].clone()),
        (StatusCode::OK, Value::Null),
        "{out}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn combats_arrive_on_a_0_2_database() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("party.db");
    let data = dir.path().join("data");
    let location = Location::Sqlite(file.clone());
    seed_0_2(&location, &data).await;
    check_upgraded(&location, &data).await;
    // The instance still moves to another database, combats and all.
    let db = Db::open(&location).await.unwrap();
    let router = router_on(data.clone(), db.clone());
    let dm = sign_in(&router, "up-dm").await;
    let (_, list) = dm.get("/api/parties").await;
    let party = list["parties"][0]["id"].as_str().unwrap().to_string();
    let e = ok(
        &dm,
        "POST",
        &format!("/api/parties/{party}/encounters"),
        json!({ "name": "Moving" }),
    )
    .await;
    let e = e["id"].as_str().unwrap().to_string();
    ok(
        &dm,
        "POST",
        &format!("/api/encounters/{e}/creatures"),
        json!({ "elementId": OWLBEAR }),
    )
    .await;
    let v = ok(
        &dm,
        "POST",
        &format!("/api/encounters/{e}/start"),
        json!({ "requestId": "move-1" }),
    )
    .await;
    let combat = v["id"].as_str().unwrap().to_string();
    let first = combatants_of(&v)[0]["id"].clone();
    act_ok(
        &dm,
        &combat,
        "set_initiative",
        json!({ "id": first, "roll": typed(5) }),
    )
    .await;
    db.pool().close().await;
    let moved = Db::open(&Location::Memory).await.unwrap();
    let report = crate::migrate::adopt_sqlite(&moved, &file)
        .await
        .unwrap()
        .unwrap();
    let text = crate::migrate::describe(&report);
    for want in [
        "combats 1",
        "combatants 2",
        "combat_journal 1",
        "combat_requests 2",
    ] {
        assert!(text.contains(want), "{text}");
    }
}

// PostgreSQL.

async fn postgres_table(url: &str, dir: &Path) -> Table {
    drop_all(url).await;
    let db = Db::open(&Location::Postgres(url.to_string()))
        .await
        .unwrap();
    table(state_in(dir, db)).await
}

/// Everything above on PostgreSQL (READ COMMITTED: the two-writer race is real there). Runs when
/// `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.
#[tokio::test(flavor = "multi_thread")]
async fn combats_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = super::POSTGRES.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let location = Location::Postgres(url.clone());
    drop_all(&url).await;
    seed_0_2(&location, &dir.path().join("up")).await;
    check_upgraded(&location, &dir.path().join("up")).await;
    role_matrix(&postgres_table(&url, &dir.path().join("roles")).await).await;
    leak_suite(&postgres_table(&url, &dir.path().join("leaks")).await).await;
    player_events(&postgres_table(&url, &dir.path().join("events")).await).await;
    prompts_and_band(&postgres_table(&url, &dir.path().join("prompts")).await).await;
    retries_and_cas(&postgres_table(&url, &dir.path().join("cas")).await).await;
    export_import(&postgres_table(&url, &dir.path().join("export")).await).await;
    add_sweep_and_limits(&postgres_table(&url, &dir.path().join("add")).await).await;
    drop_all(&url).await;
}
