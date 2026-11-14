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
    assert!(
        v["me"][0]["characterId"]
            .as_str()
            .is_some_and(|c| c.starts_with("c_")),
        "the open character is known: {v}"
    );
    // The last write was the player's: the DM's undo says so with its own code.
    let (status, out) = act(&t.dm, c, "undo", json!({})).await;
    assert_eq!(
        (status, out["code"].as_str()),
        (StatusCode::CONFLICT, Some("undo_player")),
        "{out}"
    );
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
        // Spells (0.3.2): the DM's casts, and the one a player may answer (their own PC's row).
        "cast_spell",
        "resolve_cast_row",
        "apply_cast",
        "cancel_cast",
        "apply_zone",
        "perform_statblock",
        "standard_action",
        "bind_to_concentration",
    ];
    for action in actions {
        for id in &ids {
            for args in [
                json!({ "id": id, "roll": typed(10) }),
                json!({ "id": id, "name": "Bless" }),
                json!({ "promptId": id, "applied": true }),
                json!({ "target": id, "parts": [{ "amount": typed(1) }] }),
                json!({ "caster": id, "spellKey": "ID_2024_SPELL_FIREBALL", "slot": 3, "targets": [id] }),
                json!({ "cast": id, "target": id, "roll": typed(10) }),
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
    // Later writes don't change what a retry answers: the first try's answer.
    act_ok(
        &t.dm,
        &f.combat,
        "set_notes",
        json!({ "id": f.owlbear, "notes": "later" }),
    )
    .await;
    let (_, late) = t
        .dm
        .post(
            &path,
            json!({ "requestId": "same-request-1", "args": { "target": f.owlbear, "parts": [{ "amount": typed(4) }] } }),
        )
        .await;
    assert_eq!(late["seq"], first["seq"], "the original answer: {late}");
    assert_eq!(late["replayed"], true);
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
    // A hand-edited file's XP never comes back, in the DM's view or an export (D16).
    let mut with_xp = file.clone();
    for c in with_xp["server"]["creatures"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        if let Some(block) = c["statBlock"].as_object_mut() {
            block.insert("xp".into(), json!(1800));
            block.insert("xp_lair".into(), json!(2000));
        }
    }
    let v = ok(
        &t.dm,
        "POST",
        &import,
        json!({ "requestId": request_id(), "file": with_xp }),
    )
    .await;
    let edited = v["id"].as_str().unwrap().to_string();
    let exported = act_ok(&t.dm, &edited, "export", json!({})).await;
    for (what, creatures) in [
        ("view", &v["creatures"]),
        ("export", &exported["server"]["creatures"]),
    ] {
        let creatures = creatures.as_object().unwrap();
        assert!(!creatures.is_empty(), "{what}");
        for c in creatures.values() {
            assert!(
                c["statBlock"].get("xp").is_none() && c["statBlock"].get("xp_lair").is_none(),
                "no XP in the {what}: {c}"
            );
        }
    }
    act_ok(&t.dm, &edited, "end", json!({})).await;
    // An encounter pet comes into the fight with its owner, acting right after it.
    let e = ok(
        &t.dm,
        "POST",
        &format!("/api/parties/{}/encounters", t.party),
        json!({ "name": "Pets" }),
    )
    .await;
    let e = e["id"].as_str().unwrap().to_string();
    let path = format!("/api/encounters/{e}/creatures");
    let v = ok(
        &t.dm,
        "POST",
        &path,
        json!({ "elementId": OWLBEAR, "dmLabel": "Rider", "hidden": false }),
    )
    .await;
    let rider = last_creature(&v);
    ok(
        &t.dm,
        "POST",
        &path,
        json!({ "elementId": GOBLIN, "dmLabel": "Pet", "hidden": false, "ownerId": rider }),
    )
    .await;
    let v = ok(
        &t.dm,
        "POST",
        &format!("/api/encounters/{e}/start"),
        json!({ "requestId": request_id() }),
    )
    .await;
    let owner = find_by_label(&v, "Rider");
    let pet = combatants_of(&v)
        .into_iter()
        .find(|c| c["label"] == "Pet")
        .unwrap();
    assert_eq!(pet["owner_id"], owner.as_str(), "{pet}");
    assert_eq!(pet["initiative_mode"], "after_owner");
    let combat = v["id"].as_str().unwrap().to_string();
    act_ok(&t.dm, &combat, "end", json!({})).await;
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
        offers: vec![],
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
        ..Member::default()
    }
}

fn pc_member(i: u64) -> Member {
    Member {
        kind: "character".into(),
        encounter_creature: None,
        owner: Some(format!("acct-{i}")),
        character: Some(format!("c_{i}")),
        block: None,
        ..Member::default()
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
            // A villain's pet, hidden like its owner, acting right after it.
            if rng.below(2) == 0 {
                let pet = format!("hidden-pet-{i}-{}", rng.next());
                let mut p = Combatant::creature(&pet, format!("SECRET PET {i}"), 10, 12, 1);
                p.public_name = format!("SECRET PET PUBLIC {i}");
                p.owner_id = Some(CombatantId::new(id.clone()));
                hidden = add_to(&hidden, p);
                hidden_roster.insert(pet.clone(), creature_member(&pet));
            }
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
    summons_and_companions(&postgres_table(&url, &dir.path().join("summons")).await).await;
    live_and_unblocked(&postgres_table(&url, &dir.path().join("live")).await).await;
    companions_from_an_encounter(&postgres_table(&url, &dir.path().join("companions")).await).await;
    spells_in_combat(&postgres_table(&url, &dir.path().join("spells")).await).await;
    drop_all(&url).await;
}

// Summons and companions (controlled creatures).

async fn summons_and_companions(t: &Table) {
    let f = fight(t).await;
    let (_, dm_view) = t.dm.get(&format!("/api/combats/{}", f.combat)).await;
    let character = dm_view["characters"][&f.pc_player]["characterId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(dm_view["creatures"][&f.dragon]["origin"], "encounter");
    assert!(dm_view["creatures"][&f.dragon]["name"]
        .as_str()
        .is_some_and(|n| !n.is_empty()));
    // A companion on the player's sheet is offered, to the DM and to its owner only.
    let mut card = t
        .state
        .hub
        .cards
        .get(&t.player.id, &character)
        .unwrap_or_default();
    card.companions = vec![crate::hub::CompanionCard {
        key: "granted".into(),
        name: "Owl".into(),
        element_id: None,
        hp: 1,
        ac: 11,
        initiative: 3,
        speed: 5,
    }];
    t.state.hub.cards.put(&t.player.id, &character, card, true);
    let (_, mine) = t.player.get(&format!("/api/combats/{}", f.combat)).await;
    let offers = mine["me"][0]["offers"].as_array().unwrap().clone();
    assert_eq!(offers.len(), 1, "{mine}");
    assert_eq!(offers[0]["name"], "Owl");
    let (_, theirs) = t.member2.get(&format!("/api/combats/{}", f.combat)).await;
    assert!(
        !theirs.to_string().contains("Owl"),
        "another player sees no offer: {theirs}"
    );
    // The player brings it in: a server id, controlled by their PC, shown to the party.
    let v = act_ok(
        &t.player,
        &f.combat,
        "summon",
        json!({ "owner": f.pc_player, "companion": "granted" }),
    )
    .await;
    let owl = v["me"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["kind"] == "creature")
        .unwrap_or_else(|| panic!("the owl is mine: {v}"))["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(uuid::Uuid::parse_str(&owl).is_ok());
    assert!(
        v["me"][0]["offers"].as_array().unwrap().is_empty(),
        "offered once: {v}"
    );
    let (_, dm_view) = t.dm.get(&format!("/api/combats/{}", f.combat)).await;
    assert_eq!(dm_view["creatures"][&owl]["origin"], "companion");
    act_ok(
        &t.dm,
        &f.combat,
        "set_initiative",
        json!({ "id": f.pc_player, "roll": typed(14) }),
    )
    .await;
    let (_, seen) = t.member2.get(&format!("/api/combats/{}", f.combat)).await;
    let row = seen["order"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == owl.as_str())
        .cloned()
        .unwrap_or_else(|| panic!("a PC's summon is seen by the party: {seen}"));
    assert_eq!(
        row["ownerId"],
        f.pc_player.as_str(),
        "it acts after its owner"
    );
    assert_eq!(row["mine"], false);
    // A creature from the player's own content.
    let v = act_ok(
        &t.player,
        &f.combat,
        "summon",
        json!({ "owner": f.pc_player, "elementId": GOBLIN }),
    )
    .await;
    assert_eq!(
        v["me"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["kind"] == "creature")
            .count(),
        2,
        "{v}"
    );
    // Not for another player's PC, and copyOf is the DM's.
    let (status, _) = act(
        &t.member2,
        &f.combat,
        "summon",
        json!({ "owner": f.pc_player, "elementId": GOBLIN }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = act(
        &t.player,
        &f.combat,
        "summon",
        json!({ "owner": f.pc_player, "copyOf": f.owlbear }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A villain's hidden pet: summoned by the DM to a hidden creature, it leaves no trace for players.
    let (_, before) = t.player.get(&format!("/api/combats/{}", f.combat)).await;
    let v = act_ok(
        &t.dm,
        &f.combat,
        "summon",
        json!({ "owner": f.dragon, "copyOf": f.goblin, "name": "SECRET-PET" }),
    )
    .await;
    let pet = combatants_of(&v)
        .into_iter()
        .find(|c| c["label"] == "SECRET-PET")
        .unwrap_or_else(|| panic!("{v}"));
    assert_eq!(pet["visible"], false, "a hidden owner's pet starts hidden");
    let (_, after) = t.player.get(&format!("/api/combats/{}", f.combat)).await;
    assert_eq!(before, after, "byte for byte, pseq included");
    assert!(!after.to_string().contains(pet["id"].as_str().unwrap()));
    // The player may act for their owl, not for the DM's creatures.
    let v = act_ok(
        &t.player,
        &f.combat,
        "damage",
        json!({ "target": owl, "parts": [{ "amount": typed(1) }] }),
    )
    .await;
    let me_owl = v["me"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == owl.as_str())
        .unwrap()
        .clone();
    assert_eq!(me_owl["hp"]["current"], 0, "{me_owl}");
    act_ok(&t.player, &f.combat, "dismiss", json!({ "id": owl })).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn players_bring_in_companions_and_summons_and_hidden_pets_stay_hidden() {
    let dir = tempfile::tempdir().unwrap();
    summons_and_companions(&memory_table(dir.path()).await).await;
}

// Live: push latency, the heartbeat, and writes that never wait for a workspace.

async fn next_combat_event(sub: &mut Subscription, within: std::time::Duration) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let left = deadline.checked_duration_since(tokio::time::Instant::now())?;
        match tokio::time::timeout(left, sub.rx.recv()).await {
            Ok(Some(m)) if m.event == crate::hub::COMBAT_EVENT => return Some(m.payload),
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return None,
        }
    }
}

#[allow(clippy::cast_precision_loss)]
async fn live_and_unblocked(t: &Table) {
    let f = fight(t).await;
    roll_in(t, &f).await;
    let mut dm_sub = t.state.hub.subscribe(&t.dm.id, "lat-dm", None);
    let mut sub = t.state.hub.subscribe(&t.player.id, "lat-player", None);
    // Push latency: write to event, for the DM and a player, 40 reveals and hides.
    let mut times = Vec::new();
    let mut last_seq = None;
    let mut last_pseq = None;
    for i in 0..40 {
        let action = if i % 2 == 0 { "reveal" } else { "hide" };
        let start = std::time::Instant::now();
        act_ok(&t.dm, &f.combat, action, json!({ "id": f.goblin })).await;
        let dm_event = next_combat_event(&mut dm_sub, std::time::Duration::from_secs(2))
            .await
            .expect("the DM hears every write");
        let event = next_combat_event(&mut sub, std::time::Duration::from_secs(2))
            .await
            .expect("the player hears a reveal and a hide");
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        // Gapless: the DM's seq and the player's pseq go up by one each time.
        let seq = dm_event["seq"].as_i64().unwrap();
        let pseq = event["pseq"].as_i64().unwrap();
        if let (Some(a), Some(b)) = (last_seq, last_pseq) {
            assert_eq!((seq, pseq), (a + 1, b + 1), "no gaps");
        }
        last_seq = Some(seq);
        last_pseq = Some(pseq);
    }
    times.sort_by(f64::total_cmp);
    let p95 = times[(times.len() * 95 / 100).min(times.len() - 1)];
    eprintln!(
        "combat push latency ({}): p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        t.state.db.kind(),
        times[times.len() / 2],
        p95,
        times[times.len() - 1]
    );
    assert!(p95 < 300.0, "p95 {p95} ms");
    // No blocking: a slow job in the DM's and the player's workspaces doesn't hold up combat writes, and neither do
    // the account's combat job slots being taken.
    let dm_ws = t.state.workspaces.get(&t.dm.id).await.unwrap();
    let player_ws = t.state.workspaces.get(&t.player.id).await.unwrap();
    let slow = |ws: std::sync::Arc<crate::workspaces::Workspace>| {
        tokio::spawn(async move {
            ws.blocking(|_| {
                std::thread::sleep(std::time::Duration::from_secs(3));
                Ok(())
            })
            .await
        })
    };
    let jobs = [slow(dm_ws), slow(player_ws)];
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    for _ in 0..5 {
        let start = std::time::Instant::now();
        act_ok(
            &t.dm,
            &f.combat,
            "damage",
            json!({ "target": f.owlbear, "parts": [{ "amount": typed(1) }] }),
        )
        .await;
        act_ok(
            &t.player,
            &f.combat,
            "set_initiative",
            json!({ "id": f.pc_player, "roll": typed(15) }),
        )
        .await;
        let (status, _) = t.player.get(&format!("/api/combats/{}", f.combat)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            start.elapsed() < std::time::Duration::from_millis(1500),
            "{:?}",
            start.elapsed()
        );
    }
    assert!(
        jobs.iter().all(|j| !j.is_finished()),
        "the slow jobs were still running"
    );
    for j in jobs {
        j.await.unwrap().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn changes_reach_everyone_within_300_ms_and_never_wait_for_a_workspace() {
    let dir = tempfile::tempdir().unwrap();
    live_and_unblocked(&memory_table(dir.path()).await).await;
}

/// The account's event stream pings at least every 20 s (Cloudflare drops an idle stream at about 100 s).
#[tokio::test(flavor = "multi_thread")]
async fn the_event_stream_pings() {
    assert!(crate::hub::PING_EVERY <= std::time::Duration::from_secs(20));
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path(), Db::open(&Location::Memory).await.unwrap());
    let router = router_of(state, Setup::default().max_upload);
    sign_up(&router, "ping-admin").await;
    let client = sign_up(&router, "ping-user").await;
    let request = Request::builder()
        .uri("/api/events")
        .header("authorization", format!("Bearer {}", client.token))
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let start = std::time::Instant::now();
    let mut seen = String::new();
    while start.elapsed() < std::time::Duration::from_secs(25) {
        let Ok(Some(Ok(frame))) =
            tokio::time::timeout(std::time::Duration::from_secs(25), body.frame()).await
        else {
            break;
        };
        if let Some(bytes) = frame.data_ref() {
            seen.push_str(&String::from_utf8_lossy(bytes));
        }
        if seen.contains(crate::hub::PING_EVENT) {
            break;
        }
    }
    assert!(
        seen.contains(crate::hub::PING_EVENT),
        "no ping in {:?}: {seen}",
        start.elapsed()
    );
    assert!(start.elapsed() <= std::time::Duration::from_secs(21));
}

/// A hidden creature's use of a legendary pool it shares with a revealed one leaves the player projection
/// byte-identical: the counts are DM only (user, 2026-11-13).
#[test]
fn a_hidden_pool_member_changes_no_player_byte() {
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
    state = add_to(&state, Combatant::creature("seen-0", "Seen", 90, 18, 2));
    state = run(
        &state,
        &Command::Reveal {
            id: CombatantId::new("seen-0"),
        },
    );
    roster.insert("seen-0".to_string(), creature_member("seen-0"));
    state = add_to(&state, Combatant::creature("hidden-0", "SECRET", 90, 18, 2));
    roster.insert("hidden-0".to_string(), creature_member("hidden-0"));
    state = run(
        &state,
        &Command::SetLegendaryPool {
            pool: "pair".into(),
            value: party_combat::LegendaryPool {
                max: 3,
                remaining: 3,
                members: vec![CombatantId::new("hidden-0"), CombatantId::new("seen-0")],
                guessed: false,
            },
        },
    );
    let before = project(&state, &roster);
    for delta in [-1, -1, 2, -3] {
        state = run(
            &state,
            &Command::AdjustLegendary {
                pool: "pair".into(),
                delta,
            },
        );
        assert_eq!(project(&state, &roster), before, "delta {delta}");
    }
    let text = String::from_utf8(before).unwrap();
    assert!(
        !text.contains("legendary_actions") && !text.contains("\"remaining\""),
        "{text}"
    );
}

// Legendary actions per creature (C16, user 2026-11-13).

fn pools_of(v: &Value) -> BTreeMap<String, (u64, u64, Vec<String>)> {
    v["state"]["legendary_pools"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, p)| {
            let members = p["members"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m.as_str().unwrap().to_string())
                .collect();
            (
                k.clone(),
                (
                    p["max"].as_u64().unwrap(),
                    p["remaining"].as_u64().unwrap(),
                    members,
                ),
            )
        })
        .collect()
}

/// Each legendary creature has its own pool, a copy added mid-fight too; a combat 0.3.0 saved with one pool for
/// two creatures loads as one pool each; players never see a count.
async fn legendary_per_creature(t: &Table) {
    let f = fight(t).await;
    let v = act_ok(
        &t.dm,
        &f.combat,
        "add_creature",
        json!({ "copyOf": f.dragon, "count": 1 }),
    )
    .await;
    let copy = combatants_of(&v)
        .into_iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .find(|id| id != &f.dragon && v["creatures"][id]["statBlock"]["legendary"].is_object())
        .expect("the dragon's copy");
    // Sizes come from each block: its own count, else its preamble's "can take N", else 3.
    let block = v["creatures"][&f.dragon]["statBlock"].clone();
    let variant = |per_round: Option<u8>, preamble: &str| {
        let mut b = block.clone();
        b["legendary"]["preamble"] = json!(preamble);
        match per_round {
            Some(n) => b["legendary"]["per_round"] = json!(n),
            None => {
                b["legendary"].as_object_mut().unwrap().remove("per_round");
            }
        }
        Member {
            kind: "creature".into(),
            block: Some(b),
            ..Member::default()
        }
    };
    let roster = BTreeMap::from([
        (
            "a".to_string(),
            variant(None, "It can take 2 legendary actions."),
        ),
        (
            "b".to_string(),
            variant(Some(1), "It can take 3 legendary actions."),
        ),
        ("c".to_string(), variant(None, "")),
        ("pc".to_string(), pc_member(0)),
    ]);
    assert_eq!(
        crate::combat::legendary_sizes(&roster),
        BTreeMap::from([
            ("a".to_string(), (2, false)),
            ("b".to_string(), (1, false)),
            ("c".to_string(), (3, true)),
        ])
    );
    let pools = pools_of(&v);
    assert_eq!(
        pools[&f.dragon],
        (3, 3, vec![f.dragon.clone()]),
        "{pools:?}"
    );
    assert_eq!(pools[&copy], (3, 3, vec![copy.clone()]), "{pools:?}");
    assert_eq!(pools.len(), 2, "{pools:?}");

    // As 0.3.0 might have saved it: one pool for both, no per-creature mark.
    let row = sqlx::query("SELECT state_json FROM combats WHERE id = $1")
        .bind(&f.combat)
        .fetch_one(t.state.db.pool())
        .await
        .unwrap();
    let mut old: Value = serde_json::from_str(&row.try_get::<String, _>(0).unwrap()).unwrap();
    old.as_object_mut()
        .unwrap()
        .remove("legendary_per_creature");
    old["legendary_pools"] = json!({
        "shared": { "max": 3, "remaining": 1, "members": [f.dragon, copy] }
    });
    sqlx::query("UPDATE combats SET state_json = $1 WHERE id = $2")
        .bind(old.to_string())
        .bind(&f.combat)
        .execute(t.state.db.pool())
        .await
        .unwrap();
    let v = ok(
        &t.dm,
        "GET",
        &format!("/api/combats/{}", f.combat),
        json!(null),
    )
    .await;
    let pools = pools_of(&v);
    assert_eq!(
        pools[&f.dragon],
        (3, 1, vec![f.dragon.clone()]),
        "{pools:?}"
    );
    assert_eq!(pools[&copy], (3, 3, vec![copy.clone()]), "{pools:?}");
    assert_eq!(pools.len(), 2, "{pools:?}");
    // It writes on as usual and stays split.
    act_ok(
        &t.dm,
        &f.combat,
        "adjust_legendary",
        json!({ "pool": copy, "delta": -2 }),
    )
    .await;
    let v = ok(
        &t.dm,
        "GET",
        &format!("/api/combats/{}", f.combat),
        json!(null),
    )
    .await;
    let pools = pools_of(&v);
    assert_eq!((pools[&f.dragon].1, pools[&copy].1), (1, 1), "{pools:?}");

    // D20 b: reveal both; players still see no live count (the printed traits' text and keys may show).
    for id in [&f.dragon, &copy] {
        act_ok(&t.dm, &f.combat, "reveal", json!({ "id": id })).await;
    }
    let (status, seen) = t.player.get(&format!("/api/combats/{}", f.combat)).await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    let text = seen.to_string();
    for dm_only in [
        "\"legendary_pools\":",
        "\"legendary_resistance\":",
        "\"legendary_per_creature\":",
        "\"guessed\":",
    ] {
        assert!(!text.contains(dm_only), "{dm_only} in {text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn legendary_actions_are_per_creature_and_old_shared_pools_split() {
    let dir = tempfile::tempdir().unwrap();
    legendary_per_creature(&memory_table(dir.path()).await).await;
}

// Companions from an encounter (D17 b): their printed HP and AC, and rolled HP from their formula.

/// An Aurora pack with two made-up Companion elements (Aurora's loader needs every one of these setters).
fn companions_pack() -> Value {
    use base64::Engine as _;
    let xml = r#"<?xml version="1.0" encoding="utf-8"?><elements>
      <info><update version="0.0.1"><file name="combat-companions.xml" url="https://example.invalid/combat-companions.xml"/></update></info>
      <element name="Hound" type="Companion" source="Test Companions" id="ID_TEST_COMPANION_HOUND">
        <setters><set name="ac">13</set><set name="hp">9 (2d8)</set><set name="speed">40 ft.</set>
          <set name="strength">13</set><set name="dexterity">14</set><set name="constitution">12</set>
          <set name="intelligence">3</set><set name="wisdom">12</set><set name="charisma">6</set>
          <set name="type">Beast</set><set name="size">Medium</set><set name="challenge">1/4</set><set name="alignment">unaligned</set></setters>
      </element>
      <element name="Wisp" type="Companion" source="Test Companions" id="ID_TEST_COMPANION_WISP">
        <setters><set name="ac">12</set><set name="hp">4 (1d8)</set><set name="speed">0 ft., fly 30 ft.</set>
          <set name="strength">1</set><set name="dexterity">16</set><set name="constitution">10</set>
          <set name="intelligence">6</set><set name="wisdom">12</set><set name="charisma">10</set>
          <set name="type">Fey</set><set name="size">Tiny</set><set name="challenge">0</set><set name="alignment">neutral</set></setters>
      </element></elements>"#;
    json!({ "request": {
        "fileName": "combat-companions.xml",
        "contentBase64": base64::engine::general_purpose::STANDARD.encode(xml),
    }})
}

async fn companions_from_an_encounter(t: &Table) {
    let (status, out) = t.dm.rpc("import_pack", companions_pack()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let hp_of_label = |v: &Value, label: &str| {
        combatants_of(v)
            .into_iter()
            .find(|c| c["label"] == label)
            .unwrap_or_else(|| panic!("no {label}: {v}"))
    };
    for (mode, rolled) in [("average", false), ("rolled", true)] {
        let e = ok(
            &t.dm,
            "POST",
            &format!("/api/parties/{}/encounters", t.party),
            json!({ "name": format!("Companions {mode}"), "hpMode": mode }),
        )
        .await;
        let e = e["id"].as_str().unwrap().to_string();
        let path = format!("/api/encounters/{e}/creatures");
        ok(
            &t.dm,
            "POST",
            &path,
            json!({ "elementId": "ID_TEST_COMPANION_HOUND", "dmLabel": "Hound", "hidden": false }),
        )
        .await;
        ok(
            &t.dm,
            "POST",
            &path,
            json!({ "elementId": "ID_TEST_COMPANION_WISP", "dmLabel": "Wisp", "hidden": false }),
        )
        .await;
        let v = ok(
            &t.dm,
            "POST",
            &format!("/api/encounters/{e}/start"),
            json!({ "requestId": request_id() }),
        )
        .await;
        let hound = hp_of_label(&v, "Hound");
        let wisp = hp_of_label(&v, "Wisp");
        let hp = hound["hp"]["max"].as_i64().unwrap();
        if rolled {
            assert!((2..=16).contains(&hp), "2d8 rolled: {hp}");
        } else {
            assert_eq!(hp, 9, "the printed average, not a fallback: {hound}");
            assert_eq!(wisp["hp"]["max"], 4);
        }
        assert_eq!(hound["ac"], 13, "{hound}");
        assert_eq!(wisp["ac"], 12, "{wisp}");
        let id = hound["id"].as_str().unwrap();
        assert_eq!(v["creatures"][id]["origin"], "encounter");
        assert_eq!(v["creatures"][id]["name"], "Hound");
        let combat = v["id"].as_str().unwrap().to_string();
        act_ok(&t.dm, &combat, "end", json!({})).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn encounter_companions_start_with_their_printed_or_rolled_numbers() {
    let dir = tempfile::tempdir().unwrap();
    companions_from_an_encounter(&memory_table(dir.path()).await).await;
}

// Spells in combat (0.3.2): precast spells with their real effect and slot, the legacy custom effect's slot, a live
// cast with a player's own save row (the caster hidden), a summon spell's notice and a creature bound to its
// caster's concentration, and what players never get: the casting numbers, definitions, spell keys and the hidden
// caster.

const MAGE_2014: &str = "ID_SRD51_MONSTER_MAGE";
const OWLBEAR_2014: &str = "ID_SRD51_MONSTER_OWLBEAR";
const MAGE_ARMOR_2014: &str = "ID_PHB_SPELL_MAGE_ARMOR";
const FIREBALL_2014: &str = "ID_PHB_SPELL_FIREBALL";
const CONJURE_ANIMALS_2014: &str = "ID_PHB_SPELL_CONJURE_ANIMALS";

fn combatant_in<'a>(view: &'a Value, id: &str) -> &'a Value {
    view["state"]["combatants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap_or_else(|| panic!("no {id}"))
}

fn state_of(view: &Value) -> CombatState {
    serde_json::from_value(view["state"].clone()).unwrap()
}

fn slots_left(view: &Value, id: &str, level: u8) -> u64 {
    combatant_in(view, id)["spell_slots"][level.to_string()]["remaining"]
        .as_u64()
        .unwrap_or_else(|| panic!("no level {level} slots: {}", combatant_in(view, id)))
}

/// What a player must never see of the spells in this fight.
fn spell_leaks(text: &str, mage: &str, mage_src: &str) -> Vec<String> {
    let mut found = Vec::new();
    for s in [
        "SECRET-MAGE",
        mage,
        mage_src,
        MAGE_2014,
        MAGE_ARMOR_2014,
        FIREBALL_2014,
        CONJURE_ANIMALS_2014,
        "\"mech\"",
        "\"casting\"",
        "\"spec\"",
        "\"stats\"",
        "Fireball",
        "Mage Armor",
    ] {
        if !s.is_empty() && text.contains(s) {
            found.push(s.to_string());
        }
    }
    found
}

#[allow(clippy::too_many_lines)]
async fn spells_in_combat(t: &Table) {
    // A 2014 party with the player's character, and an encounter with a hidden SRD 5.1 Mage and a revealed owlbear.
    let (status, party) =
        t.dm.post(
            "/api/parties",
            json!({ "name": "Spell Party", "ruleset": "dnd2014" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{party}");
    let party = party["id"].as_str().unwrap().to_string();
    join(
        &t.dm,
        &party,
        "Spell Party",
        &t.player,
        "cbt-player",
        "Hero Three",
    )
    .await;
    let e = ok(
        &t.dm,
        "POST",
        &format!("/api/parties/{party}/encounters"),
        json!({ "name": "Tower", "publicName": "Tower" }),
    )
    .await;
    let enc = e["id"].as_str().unwrap().to_string();
    let path = format!("/api/encounters/{enc}/creatures");
    let v = ok(
        &t.dm,
        "POST",
        &path,
        json!({ "elementId": MAGE_2014, "dmLabel": "SECRET-MAGE", "hidden": true }),
    )
    .await;
    let mage_src = last_creature(&v);
    let v = ok(
        &t.dm,
        "POST",
        &path,
        json!({ "elementId": OWLBEAR_2014, "dmLabel": "Bear", "publicName": "Owlbear", "hidden": false }),
    )
    .await;
    let bear_src = last_creature(&v);

    // Precast spells: checked against the encounter and the DM's own content.
    let casts = format!("/api/encounters/{enc}/casts");
    for (bad, why) in [
        (
            json!({ "casterId": "nobody", "spellKey": MAGE_ARMOR_2014, "slotLevel": 1 }),
            "not a creature here",
        ),
        (
            json!({ "casterId": mage_src, "spellKey": "ID_NO_SUCH_SPELL", "slotLevel": 1 }),
            "not installed",
        ),
        (
            json!({ "casterId": mage_src, "spellKey": FIREBALL_2014, "slotLevel": 2 }),
            "below the spell's level",
        ),
        (
            json!({ "casterId": mage_src, "spellKey": FIREBALL_2014, "slotLevel": 10 }),
            "no such slot",
        ),
        (
            json!({ "casterId": mage_src, "spellKey": "ID_2024_SPELL_MAGE_ARMOR", "slotLevel": 1 }),
            "the other edition",
        ),
        (
            json!({ "casterId": mage_src, "spellKey": MAGE_ARMOR_2014, "slotLevel": 1, "targets": ["elsewhere"] }),
            "a target not here",
        ),
        (
            json!({ "casterId": mage_src, "spellKey": MAGE_ARMOR_2014, "choices": { "script": 1 } }),
            "choices that don't fit",
        ),
        (
            json!({ "casterId": mage_src, "spellKey": MAGE_ARMOR_2014, "extra": 1 }),
            "an unknown field",
        ),
    ] {
        let (status, out) = t.dm.post(&casts, bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {out}");
    }
    let v = ok(
        &t.dm,
        "POST",
        &casts,
        json!({ "casterId": mage_src, "spellKey": MAGE_ARMOR_2014, "slotLevel": 1, "targets": ["self"] }),
    )
    .await;
    assert_eq!(v["casts"][0]["name"], "Mage Armor", "{v}");
    assert!(v.get("castWarning").is_none());
    let cast_id = v["casts"][0]["id"].as_str().unwrap().to_string();
    let v = ok(
        &t.dm,
        "PATCH",
        &format!("{casts}/{cast_id}"),
        json!({ "slotLevel": 1 }),
    )
    .await;
    assert_eq!(v["casts"].as_array().unwrap().len(), 1);
    // The legacy "Custom effect" with a slot level spends that slot too.
    ok(
        &t.dm,
        "POST",
        &format!("/api/encounters/{enc}/effects"),
        json!({ "creatureId": mage_src, "name": "Old ward", "slotLevel": 1 }),
    )
    .await;
    // Players see no precast spells, and can't write them.
    ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{enc}"),
        json!({ "status": "ready", "playerVisible": true }),
    )
    .await;
    let (status, seen) = t.player.get(&format!("/api/encounters/{enc}")).await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert!(seen.get("casts").is_none(), "{seen}");
    let text = seen.to_string();
    assert!(
        spell_leaks(&text, "", &mage_src).is_empty(),
        "{:?} in {text}",
        spell_leaks(&text, "", &mage_src)
    );
    let (status, _) = t
        .player
        .post(
            &casts,
            json!({ "casterId": bear_src, "spellKey": MAGE_ARMOR_2014 }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Start: Mage Armor is on the mage with its real effect (12 -> 15), and two level 1 slots are spent.
    let dm_view = ok(
        &t.dm,
        "POST",
        &format!("/api/encounters/{enc}/start"),
        json!({ "requestId": request_id() }),
    )
    .await;
    let combat = dm_view["id"].as_str().unwrap().to_string();
    let mage = find_by_label(&dm_view, "SECRET-MAGE");
    let bear = find_by_label(&dm_view, "Bear");
    let pc = pc_of(&dm_view, &t.player.id);
    assert_eq!(slots_left(&dm_view, &mage, 1), 2, "precast + custom effect");
    assert_eq!(slots_left(&dm_view, &mage, 3), 3);
    let st = state_of(&dm_view);
    let m = st.combatant(&CombatantId::new(mage.clone())).unwrap();
    assert_eq!(
        party_combat::derive::effective_ac(&st, m).total,
        Some(15),
        "Mage Armor: 13 + DEX 2"
    );
    assert!(
        st.effects.iter().any(|e| e
            .spec
            .spell
            .as_ref()
            .is_some_and(|s| s.key == MAGE_ARMOR_2014)),
        "{:?}",
        st.effects
    );

    // Casting is the DM's: a player gets the one 404, whatever they send.
    for action in [
        "cast_spell",
        "apply_cast",
        "cancel_cast",
        "apply_zone",
        "perform_statblock",
        "standard_action",
        "bind_to_concentration",
    ] {
        let (status, out) = act(&t.player, &combat, action, json!({ "caster": pc })).await;
        assert_eq!(
            (status, out["message"].as_str()),
            (StatusCode::NOT_FOUND, Some(NOT_FOUND)),
            "{action}"
        );
    }
    let (status, out) = act(
        &t.dm,
        &combat,
        "cast_spell",
        json!({ "spellKey": FIREBALL_2014 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no caster: {out}");
    let (status, out) = act(
        &t.dm,
        &combat,
        "cast_spell",
        json!({ "caster": mage, "spellKey": "ID_NO_SUCH_SPELL", "slot": 3 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "not installed: {out}");
    let (status, out) = act(
        &t.dm,
        &combat,
        "cast_spell",
        json!({ "caster": mage, "spellKey": FIREBALL_2014, "slot": 3, "precast": true,
                "targets": [bear], "force": true }),
    )
    .await;
    // A client's `precast` is dropped: this is a live cast, which spends a slot like any other.
    assert_eq!(status, StatusCode::OK, "{out}");
    act_ok(
        &t.dm,
        &combat,
        "cancel_cast",
        json!({ "cast": out["state"]["casts"][0]["id"], "refund": true }),
    )
    .await;

    // A live Fireball (compiled on the server from the curated SRD row): the DM types the owlbear's save, the
    // player's PC waits for its player.
    let v = act_ok(
        &t.dm,
        &combat,
        "cast_spell",
        json!({ "caster": mage, "spellKey": FIREBALL_2014, "slot": 3, "targets": [bear, pc], "force": true,
                "rows": [{ "target": bear, "roll": typed(4) }] }),
    )
    .await;
    assert_eq!(slots_left(&v, &mage, 3), 2);
    let pending = &v["state"]["casts"][0];
    let cast = pending["id"].as_str().unwrap().to_string();
    assert!(cast.len() > 20, "a random id, not a count: {cast}");
    assert_eq!(pending["spec"]["dc"], 14, "the mage's DC: {pending}");
    assert_eq!(pending["spec"]["origin"]["kind"], "srd", "{pending}");
    // The player sees only their own row, the caster unnamed while hidden.
    let (_, pv) = t.player.get(&format!("/api/combats/{combat}")).await;
    let rows = pv["casts"].as_array().unwrap_or_else(|| panic!("{pv}"));
    assert_eq!(rows.len(), 1, "{pv}");
    assert_eq!(rows[0]["target"], pc.as_str());
    assert_eq!(rows[0]["ability"], "dex");
    assert_eq!(rows[0]["dc"], 14);
    assert!(
        rows[0].get("from").is_none() && rows[0].get("spell").is_none(),
        "{pv}"
    );
    let text = pv.to_string();
    assert!(
        spell_leaks(&text, &mage, &mage_src).is_empty(),
        "{:?} in {text}",
        spell_leaks(&text, &mage, &mage_src)
    );
    // Another player of the party sees no row; they can't answer the PC's row or the owlbear's.
    let (_, other) = t.member2.get(&format!("/api/parties/{party}/combat")).await;
    assert!(
        other.get("casts").is_none_or(|c| c == &json!([])),
        "{other}"
    );
    let (status, _) = act(
        &t.player,
        &combat,
        "resolve_cast_row",
        json!({ "cast": cast, "target": bear, "roll": typed(20) }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // The player answers their own row with their dice.
    act_ok(
        &t.player,
        &combat,
        "resolve_cast_row",
        json!({ "cast": cast, "target": pc, "roll": typed(18) }),
    )
    .await;
    let bear_hp = combatant_in(&v, &bear)["hp"]["current"].as_i64().unwrap();
    let v = act_ok(
        &t.dm,
        &combat,
        "apply_cast",
        json!({ "cast": cast, "amounts": [{ "kind": "typed", "total": 20 }] }),
    )
    .await;
    assert_eq!(
        combatant_in(&v, &bear)["hp"]["current"].as_i64().unwrap(),
        bear_hp - 20,
        "the failed save takes it all"
    );
    assert!(
        v["state"]["casts"].as_array().is_none_or(Vec::is_empty),
        "applied: {}",
        v["state"]["casts"]
    );

    // Conjure Animals: no spawn, a notice for the toast; the DM adds the beasts, controlled by and bound to the mage.
    let v = act_ok(
        &t.dm,
        &combat,
        "cast_spell",
        json!({ "caster": mage, "spellKey": CONJURE_ANIMALS_2014, "slot": 3, "force": true }),
    )
    .await;
    let notice = v["notices"]
        .as_array()
        .and_then(|n| n.iter().find(|e| e["event"] == "summon_options"))
        .unwrap_or_else(|| panic!("no summon notice: {}", v["notices"]));
    assert_eq!(notice["caster"], mage.as_str());
    assert_eq!(notice["concentration"], true);
    assert_eq!(notice["info"]["creature_types"][0], "beast", "{notice}");
    let before = v["state"]["combatants"].as_array().unwrap().len();
    assert_eq!(before, combatants_of(&v).len());
    let (status, out) = act(
        &t.dm,
        &combat,
        "add_creature",
        json!({ "copyOf": bear, "controlledBy": bear, "bindConcentration": true }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the owlbear concentrates on nothing: {out}"
    );
    let v = act_ok(
        &t.dm,
        &combat,
        "add_creature",
        json!({ "copyOf": bear, "count": 2, "controlledBy": mage, "bindConcentration": true }),
    )
    .await;
    let beasts: Vec<String> = combatants_of(&v)
        .iter()
        .filter(|c| c["owner_id"] == mage.as_str())
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(beasts.len(), 2, "{}", v["state"]);
    // The concentration ends: the bound beasts go with it.
    let v = act_ok(&t.dm, &combat, "end_concentration", json!({ "id": mage })).await;
    for b in &beasts {
        assert_eq!(combatant_in(&v, b)["removed"], true, "{b}");
    }
    // A player still sees nothing of the casting or the mage.
    let (_, pv) = t.player.get(&format!("/api/combats/{combat}")).await;
    let text = pv.to_string();
    assert!(
        spell_leaks(&text, &mage, &mage_src).is_empty(),
        "{:?} in {text}",
        spell_leaks(&text, &mage, &mage_src)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn spells_cast_in_combat_with_their_slots_and_nothing_reaches_players() {
    let dir = tempfile::tempdir().unwrap();
    spells_in_combat(&memory_table(dir.path()).await).await;
}
