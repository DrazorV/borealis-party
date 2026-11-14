//! Encounters end to end (0.3.0): the role matrix, the leak suite (nothing hidden or DM-only reaches a player, in
//! bodies or headers, whatever id or body the player sends), player events only when the projection changes, the
//! party's edition and its warnings, companions and controlled creatures, overrides and refresh conflicts, starting
//! effects and rules, and the upgrade from the 0.2.x schema. There is no difficulty rating (the user's D16).
//!
//! Monsters are the bundled SRD 5.2.1 ones. Runs on SQLite, and on PostgreSQL too when `BOREALIS_TEST_POSTGRES_URL`
//! names a database it may wipe. `BOREALIS_TEST_5ETOOLS_DIR` (a local 5etools checkout) adds a realistic run that
//! checks counts only.

use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::Row;
use tower::ServiceExt;

use super::{
    drop_all, new_character, router_of, router_on, send, sign_in, sign_up, state_with, Client,
    Setup,
};
use crate::db::{Db, Location};
use crate::hub::{Audience, Subscription};
use crate::AppState;

const GOBLIN: &str = "ID_SRD521_MONSTER_GOBLIN_WARRIOR";
const OWLBEAR: &str = "ID_SRD521_MONSTER_OWLBEAR";
const GOBLIN_2014: &str = "ID_SRD51_MONSTER_GOBLIN";
const WOLF: &str = "ID_SRD521_MONSTER_WOLF";
const NOT_FOUND: &str = "There is no such encounter here.";

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

/// `player` (signed up as `username`) joins `party` with a new character named `who`.
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

async fn new_encounter(dm: &Client, party: &str, body: Value) -> String {
    let v = ok(
        dm,
        "POST",
        &format!("/api/parties/{party}/encounters"),
        body,
    )
    .await;
    v["id"].as_str().unwrap().to_string()
}

async fn add(dm: &Client, enc: &str, body: Value) -> Value {
    ok(
        dm,
        "POST",
        &format!("/api/encounters/{enc}/creatures"),
        body,
    )
    .await
}

fn creature_ids(view: &Value) -> Vec<String> {
    view["creatures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

fn has_warning(view: &Value, kind: &str) -> bool {
    view["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["kind"] == kind)
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

/// Events waiting on a subscription.
fn drain(sub: &mut Subscription) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(m) = sub.rx.try_recv() {
        out.push(m.payload);
    }
    out
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
    // The first account is the server's admin: keep it out of the roles under test.
    sign_up(&router, "enc-admin").await;
    let dm = sign_up(&router, "enc-dm").await;
    let player = sign_up(&router, "enc-player").await;
    let member2 = sign_up(&router, "enc-member2").await;
    let other = sign_up(&router, "enc-other").await;
    let outsider = sign_up(&router, "enc-outsider").await;
    let party = new_party(&dm, "Tuesday Party").await;
    join(
        &dm,
        &party,
        "Tuesday Party",
        &player,
        "enc-player",
        "Hero One",
    )
    .await;
    join(
        &dm,
        &party,
        "Tuesday Party",
        &member2,
        "enc-member2",
        "Hero Two",
    )
    .await;
    let dm2 = sign_up(&router, "enc-dm2").await;
    let other_party = new_party(&dm2, "Other Party").await;
    join(
        &dm2,
        &other_party,
        "Other Party",
        &other,
        "enc-other",
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

/// What the DM has built, with the secrets the leak suite looks for.
struct Built {
    draft: String,
    shown: String,
    hidden: String,
    /// The hidden villain's hidden pet, and its pet the DM revealed on its own.
    hidden_pet: String,
    loose_pet: String,
    revealed: String,
    effect: String,
    rule: String,
    /// A precast spell of the hidden creature (0.3.2).
    cast: String,
}

const SECRETS: [&str; 6] = [
    "DM-SECRET-NAME",
    "SECRET-LABEL",
    "SECRET-NOTE",
    "SECRET-DM-NOTES",
    "SECRET-RULE",
    "SECRET-EFFECT",
];

async fn build(t: &Table) -> Built {
    let draft = new_encounter(&t.dm, &t.party, json!({ "name": "Draft fight" })).await;
    add(&t.dm, &draft, json!({ "elementId": GOBLIN })).await;
    let shown = new_encounter(
        &t.dm,
        &t.party,
        json!({ "name": "DM-SECRET-NAME", "publicName": "Goblin ambush", "dmNotes": "SECRET-DM-NOTES" }),
    )
    .await;
    let v = add(
        &t.dm,
        &shown,
        json!({ "elementId": GOBLIN, "dmLabel": "SECRET-LABEL boss" }),
    )
    .await;
    let hidden = creature_ids(&v)[0].clone();
    let v = add(
        &t.dm,
        &shown,
        json!({ "elementId": OWLBEAR, "publicName": "Big beast", "hidden": false }),
    )
    .await;
    let revealed = creature_ids(&v)[1].clone();
    // The hidden villain's pets: one hidden with it, one the DM revealed on its own.
    let v = add(
        &t.dm,
        &shown,
        json!({ "elementId": WOLF, "ownerId": hidden, "dmLabel": "SECRET-LABEL pet" }),
    )
    .await;
    let hidden_pet = v["creatures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["dmLabel"] == "SECRET-LABEL pet")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let v = add(
        &t.dm,
        &shown,
        json!({ "elementId": WOLF, "ownerId": hidden, "hidden": false, "publicName": "Loose wolf", "initMode": "own" }),
    )
    .await;
    let loose_pet = v["creatures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["publicName"] == "Loose wolf")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{shown}/creatures/{hidden}"),
        json!({ "overrides": { "notes": "SECRET-NOTE", "hp": { "average": 30 } } }),
    )
    .await;
    let v = ok(
        &t.dm,
        "POST",
        &format!("/api/encounters/{shown}/effects"),
        json!({ "creatureId": hidden, "name": "SECRET-EFFECT", "concentration": true, "slotLevel": 1 }),
    )
    .await;
    let effect = v["effects"][0]["id"].as_str().unwrap().to_string();
    let v = ok(
        &t.dm,
        "POST",
        &format!("/api/encounters/{shown}/rules"),
        json!({ "name": "SECRET-RULE", "creatureId": hidden,
                "flags": [{ "creatureId": revealed, "ability": "Multiattack", "whileActive": "off" }] }),
    )
    .await;
    let rule = v["rules"][0]["id"].as_str().unwrap().to_string();
    let v = ok(
        &t.dm,
        "POST",
        &format!("/api/encounters/{shown}/casts"),
        json!({ "casterId": hidden, "spellKey": "ID_2024_SPELL_FIREBALL", "slotLevel": 3, "targets": [revealed] }),
    )
    .await;
    let cast = v["casts"][0]["id"].as_str().unwrap().to_string();
    let v = ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{shown}"),
        json!({ "status": "ready", "playerVisible": true }),
    )
    .await;
    assert_eq!(v["visibleToPlayers"], true, "{v}");
    Built {
        draft,
        shown,
        hidden,
        hidden_pet,
        loose_pet,
        revealed,
        effect,
        rule,
        cast,
    }
}

// The role matrix.

/// Every route, against the DM, a player of the party, a player of another party, an outsider and no session.
async fn role_matrix(t: &Table) {
    let b = build(t).await;
    let e = &b.shown;
    let routes: Vec<(&str, String, Value)> = vec![
        (
            "GET",
            format!("/api/parties/{}/encounters", t.party),
            Value::Null,
        ),
        (
            "POST",
            format!("/api/parties/{}/encounters", t.party),
            json!({ "name": "New" }),
        ),
        ("GET", format!("/api/encounters/{e}"), Value::Null),
        ("GET", format!("/api/encounters/{}", b.draft), Value::Null),
        (
            "PATCH",
            format!("/api/encounters/{e}"),
            json!({ "name": "Renamed" }),
        ),
        (
            "POST",
            format!("/api/encounters/{e}/creatures"),
            json!({ "elementId": GOBLIN }),
        ),
        (
            "PATCH",
            format!("/api/encounters/{e}/creatures/{}", b.revealed),
            json!({ "publicName": "Huge beast" }),
        ),
        (
            "POST",
            format!("/api/encounters/{e}/creatures/{}/refresh", b.revealed),
            json!({}),
        ),
        (
            "POST",
            format!("/api/encounters/{e}/creatures/{}/swap", b.revealed),
            json!({}),
        ),
        (
            "POST",
            format!("/api/encounters/{e}/effects"),
            json!({ "creatureId": b.revealed, "name": "Bless" }),
        ),
        (
            "PATCH",
            format!("/api/encounters/{e}/effects/{}", b.effect),
            json!({ "detail": "x" }),
        ),
        (
            "POST",
            format!("/api/encounters/{e}/rules"),
            json!({ "name": "Rule", "endsWhen": "manual" }),
        ),
        (
            "PATCH",
            format!("/api/encounters/{e}/rules/{}", b.rule),
            json!({ "active": false }),
        ),
        (
            "DELETE",
            format!("/api/encounters/{e}/effects/{}", b.effect),
            Value::Null,
        ),
        (
            "DELETE",
            format!("/api/encounters/{e}/rules/{}", b.rule),
            Value::Null,
        ),
        (
            "DELETE",
            format!("/api/encounters/{e}/creatures/{}", b.hidden),
            Value::Null,
        ),
        (
            "DELETE",
            format!("/api/encounters/{}", b.draft),
            Value::Null,
        ),
    ];
    for (method, path, body) in &routes {
        let (status, out) = send(&t.router, None, method, path, body.clone()).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "signed out {method} {path}: {out}"
        );
    }
    for (who, client) in [("other", &t.other), ("outsider", &t.outsider)] {
        for (method, path, body) in &routes {
            let (status, out) =
                send(&t.router, Some(&client.token), method, path, body.clone()).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "{who} {method} {path}: {out}"
            );
        }
    }
    // A player of the party: the list and the shown encounter's projection, nothing else.
    for client in [&t.player, &t.member2] {
        for (method, path, body) in &routes {
            let (status, out) =
                send(&t.router, Some(&client.token), method, path, body.clone()).await;
            let readable =
                *method == "GET" && (path.ends_with("/encounters") || path.ends_with(e.as_str()));
            let want = if readable {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            };
            assert_eq!(status, want, "player {method} {path}: {out}");
        }
    }
    let (_, list) = t
        .player
        .get(&format!("/api/parties/{}/encounters", t.party))
        .await;
    assert_eq!(list["role"], "player");
    assert_eq!(
        list["encounters"].as_array().unwrap().len(),
        1,
        "drafts aren't listed: {list}"
    );
    assert_eq!(list["encounters"][0]["publicName"], "Goblin ambush");
    // Nothing the others tried changed anything.
    let (_, v) = t.dm.get(&format!("/api/encounters/{e}")).await;
    assert_eq!(v["name"], "DM-SECRET-NAME");
    assert_eq!(v["creatures"].as_array().unwrap().len(), 4);
    assert_eq!(v["effects"].as_array().unwrap().len(), 1);
    for (method, path, body) in &routes {
        let (status, out) = send(&t.router, Some(&t.dm.token), method, path, body.clone()).await;
        assert_eq!(status, StatusCode::OK, "DM {method} {path}: {out}");
    }
    // DM tools on make no one the DM of someone else's party.
    let (status, _) = t.player.post("/api/me/roles/dm", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = t
        .player
        .post(
            &format!("/api/parties/{}/encounters", t.party),
            json!({ "name": "Mine" }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_dm_builds_and_players_see_the_projection() {
    let dir = tempfile::tempdir().unwrap();
    role_matrix(&memory_table(dir.path()).await).await;
}

// The leak suite.

async fn leak_suite(t: &Table) {
    let b = build(t).await;
    let random = uuid::Uuid::new_v4().to_string();
    let ids = [
        b.shown.clone(),
        b.draft.clone(),
        b.hidden.clone(),
        b.hidden_pet.clone(),
        b.effect.clone(),
        random.clone(),
        "not%20an%20id".to_string(),
        "x".repeat(65),
        "%00".to_string(),
    ];
    let bodies = [
        "{}",
        "not json",
        r#"{"elementId":"ID_SRD521_MONSTER_GOBLIN_WARRIOR","hidden":false,"name":"x"}"#,
    ];
    let mut seen: Vec<String> = Vec::new();
    let mut refusals: Vec<String> = Vec::new();
    for client in [&t.player, &t.member2, &t.outsider, &t.other] {
        let reader = std::ptr::eq(client, &t.player) || std::ptr::eq(client, &t.member2);
        for id in &ids {
            let mut paths: Vec<(&str, String)> = vec![
                ("GET", format!("/api/encounters/{id}")),
                ("PATCH", format!("/api/encounters/{id}")),
                ("DELETE", format!("/api/encounters/{id}")),
                ("POST", format!("/api/encounters/{id}/creatures")),
                ("POST", format!("/api/encounters/{id}/effects")),
                ("POST", format!("/api/encounters/{id}/rules")),
                ("POST", format!("/api/encounters/{id}/casts")),
            ];
            for sub in [
                &b.cast,
                &b.hidden,
                &b.hidden_pet,
                &b.revealed,
                &random,
                &b.effect,
                &b.rule,
            ] {
                for (m, tail) in [
                    ("PATCH", "creatures"),
                    ("DELETE", "creatures"),
                    ("PATCH", "effects"),
                    ("DELETE", "effects"),
                    ("PATCH", "rules"),
                    ("DELETE", "rules"),
                    ("PATCH", "casts"),
                    ("DELETE", "casts"),
                ] {
                    paths.push((m, format!("/api/encounters/{id}/{tail}/{sub}")));
                }
                paths.push((
                    "POST",
                    format!("/api/encounters/{id}/creatures/{sub}/refresh"),
                ));
                paths.push(("POST", format!("/api/encounters/{id}/creatures/{sub}/swap")));
            }
            for (method, path) in paths {
                for body in bodies {
                    let (status, headers, text) =
                        raw(&t.router, Some(&client.token), method, &path, body).await;
                    assert!(
                        headers.contains("no-store, private"),
                        "{method} {path}: {headers}"
                    );
                    seen.push(format!("{headers}\n{text}"));
                    if status == StatusCode::OK {
                        assert!(
                            reader && method == "GET" && *id == b.shown,
                            "{method} {path} answered 200: {text}"
                        );
                    } else {
                        assert_eq!(
                            status,
                            StatusCode::NOT_FOUND,
                            "{method} {path} {body}: {text}"
                        );
                        refusals.push(text);
                    }
                    if method == "GET" || method == "DELETE" {
                        break;
                    }
                }
            }
        }
        for party in [&t.party, &t.other_party, &random] {
            let (_, headers, text) = raw(
                &t.router,
                Some(&client.token),
                "GET",
                &format!("/api/parties/{party}/encounters"),
                "",
            )
            .await;
            seen.push(format!("{headers}\n{text}"));
            let (status, headers, text) = raw(
                &t.router,
                Some(&client.token),
                "POST",
                &format!("/api/parties/{party}/encounters"),
                "{\"name\":1}",
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{text}");
            seen.push(format!("{headers}\n{text}"));
        }
    }
    let one = json!({ "code": "not_found", "message": NOT_FOUND }).to_string();
    for text in &refusals {
        assert_eq!(text, &one, "every refusal is word for word the same");
    }
    let all = seen.join("\n");
    for secret in SECRETS {
        assert!(!all.contains(secret), "{secret} reached a player");
    }
    for leak in [
        b.hidden.as_str(),
        b.hidden_pet.as_str(),
        b.effect.as_str(),
        b.rule.as_str(),
        b.draft.as_str(),
        b.cast.as_str(),
        "ID_2024_SPELL",
        "spellKey",
        "\"casts\"",
        "Fireball",
        "ID_SRD521",
        "elementId",
        "statblock_json",
        "\"xp\"",
        "packRev",
        "Goblin Warrior",
        "dmLabel",
        "dmNotes",
        "overrides",
        "initMode",
        "totalXp",
        "updatedAt",
    ] {
        assert!(!all.contains(leak), "{leak} reached a player");
    }
    // The projection: the revealed creatures with their whole stat blocks (D20 b), nothing of the DM's.
    let (_, _, text) = raw(
        &t.router,
        Some(&t.player.token),
        "GET",
        &format!("/api/encounters/{}", b.shown),
        "",
    )
    .await;
    let v: Value = serde_json::from_str(&text).unwrap();
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["creatures", "id", "partyId", "publicName", "status"],
        "{v}"
    );
    let shown: Vec<&str> = v["creatures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    // Pets sit right after their owner, so the loose wolf comes before the Owlbear.
    assert_eq!(shown, [b.loose_pet.as_str(), b.revealed.as_str()], "{v}");
    let owlbear = &v["creatures"][1];
    assert_eq!(
        (owlbear["publicName"].clone(), owlbear["type"].clone()),
        (json!("Big beast"), json!("Monstrosity"))
    );
    assert_eq!(owlbear["hp"]["max"], 59, "{v}");
    assert!(owlbear["statBlock"]["abilities"].is_object());
    // The loose pet never names its hidden owner.
    assert!(v["creatures"][0].get("ownerId").is_none(), "{v}");
    assert!(
        !text.contains("\"xp\"") && !text.contains(&b.hidden) && !text.contains("SECRET"),
        "{text}"
    );
    // Revealing the owner doesn't reveal its hidden pet; then the loose pet names it.
    ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{}/creatures/{}", b.shown, b.hidden),
        json!({ "hidden": false }),
    )
    .await;
    let (_, _, text) = raw(
        &t.router,
        Some(&t.player.token),
        "GET",
        &format!("/api/encounters/{}", b.shown),
        "",
    )
    .await;
    assert!(
        !text.contains(&b.hidden_pet) && !text.contains("SECRET"),
        "{text}"
    );
    let v: Value = serde_json::from_str(&text).unwrap();
    let pet = v["creatures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == b.loose_pet.as_str())
        .unwrap();
    assert_eq!(pet["ownerId"], b.hidden.as_str());
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_hidden_or_dm_only_reaches_a_player() {
    let dir = tempfile::tempdir().unwrap();
    leak_suite(&memory_table(dir.path()).await).await;
}

// Events.

async fn player_events(t: &Table) {
    let mut player = t.state.hub.subscribe(&t.player.id, "player-test", None);
    let mut dm = t.state.hub.subscribe(&t.dm.id, "dm-test", None);
    let seq = |t: &Table| t.state.hub.seq(&t.party, Audience::Players);
    let players_before = seq(t);
    // A draft: created, renamed, given creatures, overrides, effects and rules. Players hear nothing.
    let d = new_encounter(&t.dm, &t.party, json!({ "name": "Draft" })).await;
    let v = add(&t.dm, &d, json!({ "elementId": GOBLIN, "count": 3 })).await;
    let c = creature_ids(&v)[0].clone();
    let v = ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{d}"),
        json!({ "name": "Draft 2", "playerVisible": true }),
    )
    .await;
    assert_eq!(v["playerVisible"], false, "a draft is never shown");
    ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{d}/creatures/{c}"),
        json!({ "hidden": false, "overrides": { "ac": { "value": 17 } } }),
    )
    .await;
    ok(
        &t.dm,
        "POST",
        &format!("/api/encounters/{d}/effects"),
        json!({ "creatureId": c, "name": "Shield of Faith" }),
    )
    .await;
    ok(
        &t.dm,
        "POST",
        &format!("/api/encounters/{d}/rules"),
        json!({ "name": "Rally", "creatureId": c }),
    )
    .await;
    assert!(
        drain(&mut player).is_empty(),
        "a draft's edits reach no player"
    );
    assert_eq!(seq(t), players_before, "nor move their sequence");
    let dm_events = drain(&mut dm);
    assert!(
        dm_events.len() >= 6,
        "the DM hears each write: {dm_events:?}"
    );
    assert!(dm_events
        .iter()
        .all(|e| e["kind"] == "encounters" && e["party"] == t.party.as_str()));

    // Shown to players: they hear it once.
    ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{d}"),
        json!({ "status": "ready", "playerVisible": true }),
    )
    .await;
    let got = drain(&mut player);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["id"], d.as_str());
    assert_eq!(seq(t), players_before + 1);
    let path = format!("/api/encounters/{d}");
    let shown = raw(&t.router, Some(&t.player.token), "GET", &path, "")
        .await
        .2;
    // Writes players can't see.
    let v = add(&t.dm, &d, json!({ "elementId": OWLBEAR })).await;
    let hidden = creature_ids(&v).last().unwrap().clone();
    for (method, p, body) in [
        (
            "PATCH",
            format!("/api/encounters/{d}/creatures/{hidden}"),
            json!({ "dmLabel": "Lurker", "overrides": { "hp": { "average": 90 } } }),
        ),
        (
            "PATCH",
            format!("/api/encounters/{d}/creatures/{c}"),
            json!({ "dmLabel": "Renamed by the DM" }),
        ),
        (
            "PATCH",
            format!("/api/encounters/{d}"),
            json!({ "name": "Only the DM sees this", "dmNotes": "x", "hpMode": "rolled" }),
        ),
        (
            "POST",
            format!("/api/encounters/{d}/effects"),
            json!({ "creatureId": hidden, "name": "Mage Armor" }),
        ),
        (
            "POST",
            format!("/api/encounters/{d}/creatures/{hidden}/refresh"),
            json!({}),
        ),
    ] {
        ok(&t.dm, method, &p, body).await;
    }
    assert!(
        drain(&mut player).is_empty(),
        "writes players can't see send them nothing"
    );
    assert_eq!(
        raw(&t.router, Some(&t.player.token), "GET", &path, "")
            .await
            .2,
        shown,
        "and their projection is byte-identical"
    );
    // A reveal: one event, and the creature appears.
    ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{d}/creatures/{hidden}"),
        json!({ "hidden": false }),
    )
    .await;
    assert_eq!(drain(&mut player).len(), 1);
    assert!(raw(&t.router, Some(&t.player.token), "GET", &path, "")
        .await
        .2
        .contains(&hidden));
    // Hidden again: it drops out of the projection again.
    ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{d}/creatures/{hidden}"),
        json!({ "hidden": true }),
    )
    .await;
    assert_eq!(drain(&mut player).len(), 1);
    assert_eq!(
        raw(&t.router, Some(&t.player.token), "GET", &path, "")
            .await
            .2,
        shown
    );
    // Back to draft: players hear it, then get the same 404 as for anything else.
    ok(&t.dm, "PATCH", &path, json!({ "status": "draft" })).await;
    assert_eq!(drain(&mut player).len(), 1);
    let (status, _) = t.player.get(&path).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A deleted draft: nothing.
    ok(&t.dm, "DELETE", &path, Value::Null).await;
    assert!(drain(&mut player).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn players_hear_of_an_encounter_only_when_their_view_changes() {
    let dir = tempfile::tempdir().unwrap();
    player_events(&memory_table(dir.path()).await).await;
}

// Editions (D23 b), companions (D17 b) and controlled creatures.

/// An Aurora pack with one Companion element (an owl familiar, made up for the test).
fn companion_pack() -> Value {
    use base64::Engine as _;
    let xml = r#"<?xml version="1.0" encoding="utf-8"?><elements>
      <info><update version="0.0.1"><file name="companions.xml" url="https://example.invalid/companions.xml"/></update></info>
      <element name="Owl" type="Companion" source="Test Companions" id="ID_TEST_COMPANION_OWL">
        <setters><set name="ac">11</set><set name="hp">1 (1d4-1)</set><set name="speed">5 ft., fly 60 ft.</set>
          <set name="strength">3</set><set name="dexterity">13</set><set name="constitution">8</set>
          <set name="intelligence">2</set><set name="wisdom">12</set><set name="charisma">7</set>
          <set name="type">Beast</set><set name="size">Tiny</set><set name="challenge">0</set><set name="alignment">unaligned</set></setters>
      </element></elements>"#;
    json!({ "request": {
        "fileName": "companions.xml",
        "contentBase64": base64::engine::general_purpose::STANDARD.encode(xml),
    }})
}

fn find<'a>(view: &'a Value, key: &str, value: &str) -> &'a Value {
    view["creatures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c[key] == value)
        .unwrap_or_else(|| panic!("no creature with {key} {value}: {view}"))
}

async fn editions_and_controlled(t: &Table) {
    // The encounter uses the party's edition; a block of the other one is flagged with its counterpart.
    let e = new_encounter(&t.dm, &t.party, json!({ "name": "Mixed" })).await;
    let v = add(&t.dm, &e, json!({ "elementId": GOBLIN_2014 })).await;
    assert_eq!(v["ruleset"], "dnd2024");
    let edition = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["kind"] == "edition")
        .unwrap();
    assert_eq!(edition["creatures"][0]["ruleset"], "dnd2014");
    let goblin = creature_ids(&v)[0].clone();
    let v = ok(
        &t.dm,
        "POST",
        &format!("/api/encounters/{e}/creatures/{goblin}/swap"),
        json!({ "elementId": GOBLIN }),
    )
    .await;
    assert_eq!(
        (
            v["creatures"][0]["elementId"].clone(),
            v["creatures"][0]["ruleset"].clone()
        ),
        (json!(GOBLIN), json!("dnd2024"))
    );
    assert!(!has_warning(&v, "edition"));
    // No per-encounter edition and no difficulty (D16, D23 b).
    let (status, _) =
        t.dm.patch(
            &format!("/api/encounters/{e}"),
            json!({ "ruleset": "dnd2014" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) =
        t.dm.post(&format!("/api/encounters/{e}/evaluate"), json!({}))
            .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for key in ["evaluation", "rulesetSet"] {
        assert!(v.get(key).is_none(), "{key}");
    }
    assert!(
        v["creatures"][0].get("xp").is_none() && v["creatures"][0]["statBlock"].get("xp").is_none()
    );
    assert_eq!(v["creatures"][0]["cr"], "1/4", "CR stays a printed stat");
    // The party switches to 2014: the 2024 block and the 2024 characters are flagged.
    let (status, out) =
        t.dm.patch(
            &format!("/api/parties/{}", t.party),
            json!({ "ruleset": "dnd2014" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, v) = t.dm.get(&format!("/api/encounters/{e}")).await;
    assert_eq!(v["ruleset"], "dnd2014");
    assert!(
        has_warning(&v, "edition") && has_warning(&v, "characters_edition"),
        "{v}"
    );
    let (_, list) =
        t.dm.get(&format!("/api/parties/{}/encounters", t.party))
            .await;
    let row = &list["encounters"][0];
    assert!(
        row.get("xp").is_none() && row.get("band").is_none(),
        "{row}"
    );

    // A companion (D17 b) as the goblin's pet: what it prints, nothing filled in.
    let (status, out) = t.dm.rpc("import_pack", companion_pack()).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let v = add(
        &t.dm,
        &e,
        json!({ "elementId": "ID_TEST_COMPANION_OWL", "ownerId": goblin }),
    )
    .await;
    let owl = find(&v, "kind", "companion").clone();
    let owl_id = owl["id"].as_str().unwrap().to_string();
    assert_eq!(
        (
            owl["ownerId"].clone(),
            owl["initMode"].clone(),
            owl["hidden"].clone()
        ),
        (json!(goblin), json!("after_owner"), json!(true))
    );
    assert_eq!(
        (
            owl["type"].clone(),
            owl["size"].clone(),
            owl["publicName"].clone()
        ),
        (json!("Beast"), json!(["tiny"]), json!("Beast"))
    );
    assert_eq!(
        (
            owl["statBlock"]["ac"].clone(),
            owl["statBlock"]["hp"].clone(),
            owl["statBlock"]["hpFormula"].clone()
        ),
        (json!(11), json!(1), json!("1d4-1"))
    );
    assert_eq!(
        owl["statBlock"]["languages"],
        Value::Null,
        "missing fields stay null"
    );
    assert_eq!(owl["printed"]["saves"]["dex"], 1);
    assert_eq!(
        creature_ids(&v)[1],
        owl_id,
        "a pet sits right after its owner"
    );
    // Overrides work on it too.
    let v = ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{e}/creatures/{owl_id}"),
        json!({ "overrides": { "hp": { "average": 3 } }, "initMode": "own" }),
    )
    .await;
    let owl = find(&v, "id", &owl_id);
    assert_eq!(
        (
            owl["statBlock"]["hp"].clone(),
            owl["initMode"].clone(),
            owl["modified"].clone()
        ),
        (json!(3), json!("own"), json!(true))
    );
    // A pet can't own pets; an owner must be in the encounter; an owner with pets can't become a pet.
    let v = add(&t.dm, &e, json!({ "elementId": WOLF })).await;
    let wolf = creature_ids(&v).last().unwrap().clone();
    let path = format!("/api/encounters/{e}/creatures");
    for body in [
        json!({ "elementId": WOLF, "ownerId": owl_id }),
        json!({ "elementId": WOLF, "ownerId": uuid::Uuid::new_v4().to_string() }),
        json!({ "elementId": WOLF, "ownerId": goblin, "initMode": "whenever" }),
    ] {
        let (status, out) = t.dm.post(&path, body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {out}");
    }
    let (status, _) =
        t.dm.patch(&format!("{path}/{wolf}"), json!({ "ownerId": wolf }))
            .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "not its own owner");
    let (status, _) =
        t.dm.patch(&format!("{path}/{goblin}"), json!({ "ownerId": wolf }))
            .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an owner can't become a pet"
    );
    let (status, out) =
        t.dm.patch(&format!("{path}/{wolf}"), json!({ "ownerId": goblin }))
            .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    // Players: the revealed owl alone, a companion with its block, its hidden owner never named.
    ok(
        &t.dm,
        "PATCH",
        &format!("{path}/{owl_id}"),
        json!({ "hidden": false }),
    )
    .await;
    ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{e}"),
        json!({ "status": "ready", "playerVisible": true }),
    )
    .await;
    let (_, _, text) = raw(
        &t.router,
        Some(&t.player.token),
        "GET",
        &format!("/api/encounters/{e}"),
        "",
    )
    .await;
    let seen: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(seen["creatures"].as_array().unwrap().len(), 1, "{seen}");
    assert_eq!(
        (
            seen["creatures"][0]["kind"].clone(),
            seen["creatures"][0]["statBlock"]["hp"].clone()
        ),
        (json!("companion"), json!(3))
    );
    assert!(seen["creatures"][0].get("ownerId").is_none());
    assert!(
        !text.contains(&goblin) && !text.contains(&wolf) && !text.contains("ID_TEST"),
        "{text}"
    );
    // Removing the owner removes its controlled creatures.
    let v = ok(&t.dm, "DELETE", &format!("{path}/{goblin}"), Value::Null).await;
    assert!(v["creatures"].as_array().unwrap().is_empty(), "{v}");
}

#[tokio::test(flavor = "multi_thread")]
async fn editions_companions_and_controlled_creatures() {
    let dir = tempfile::tempdir().unwrap();
    editions_and_controlled(&memory_table(dir.path()).await).await;
}

// Overrides and refresh.

async fn overrides_and_refresh(t: &Table) {
    let e = new_encounter(&t.dm, &t.party, json!({ "name": "Tweaked" })).await;
    let v = add(&t.dm, &e, json!({ "elementId": OWLBEAR, "count": 2 })).await;
    let ids = creature_ids(&v);
    assert_eq!(v["creatures"][0]["dmLabel"], "Owlbear 1");
    assert_eq!(v["creatures"][1]["dmLabel"], "Owlbear 2");
    assert_eq!(
        v["creatures"][0]["publicName"], "Monstrosity",
        "no number on the public name"
    );
    assert_eq!(
        v["creatures"][0]["hidden"], true,
        "new creatures start hidden"
    );
    let c = &ids[0];
    let path = format!("/api/encounters/{e}/creatures/{c}");
    let v = ok(
        &t.dm,
        "PATCH",
        &path,
        json!({ "overrides": { "hp": { "average": 80, "formula": "9d10+31" }, "ac": { "value": 15 }, "saves": { "dex": 4 },
                "traits": [{ "name": "Scarred", "text": "It can't be frightened." }], "notes": "From the sidebar." } }),
    )
    .await;
    let mine = &v["creatures"][0];
    assert_eq!(
        (
            mine["hp"]["average"].clone(),
            mine["ac"].clone(),
            mine["modified"].clone()
        ),
        (json!(80), json!(15), json!(true))
    );
    assert_eq!(mine["printed"]["hp"]["average"], 59);
    assert_eq!(
        mine["overrides"]["hp"]["base"]["average"], 59,
        "the source value is kept"
    );
    assert_eq!(
        mine["statBlock"]["traits"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["name"],
        "Scarred"
    );
    for bad in [
        json!({ "overrides": { "hp": { "value": { "average": 1 }, "base": null } } }),
        json!({ "overrides": { "ac": { "value": 99 } } }),
        json!({ "overrides": { "hp": { "average": 10, "formula": "rm -rf" } } }),
        json!({ "revealLevel": "full" }),
        json!({ "hpMode": "fixed" }),
        json!({ "hpMax": 0, "hpMode": "fixed" }),
        json!({ "dmLabel": "" }),
        json!({ "nonsense": 1 }),
    ] {
        let (status, out) = t.dm.patch(&path, bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {out}");
    }
    // A typed HP (the DM rolled real dice, D43).
    let v = ok(
        &t.dm,
        "PATCH",
        &path,
        json!({ "hpMode": "fixed", "hpMax": 64 }),
    )
    .await;
    assert_eq!(v["creatures"][0]["hpMax"], 64);
    // Refresh with an unchanged source: nothing to decide.
    let r = ok(&t.dm, "POST", &format!("{path}/refresh"), json!({})).await;
    assert_eq!(
        (r["changed"].clone(), r["applied"].clone()),
        (json!(false), json!(true)),
        "{r}"
    );
    // Make the copy older than the source: as if the source's HP and AC changed since the override.
    let row =
        sqlx::query("SELECT statblock_json, overrides_json FROM encounter_creatures WHERE id = $1")
            .bind(c)
            .fetch_one(t.state.db.pool())
            .await
            .unwrap();
    let mut block: Value = serde_json::from_str(&row.try_get::<String, _>(0).unwrap()).unwrap();
    let mut o: Value = serde_json::from_str(&row.try_get::<String, _>(1).unwrap()).unwrap();
    block["data"]["hp"]["average"] = json!(50);
    block["data"]["ac"][0]["value"] = json!(11);
    o["hp"]["base"]["average"] = json!(50);
    o["ac"]["base"]["value"] = json!(11);
    sqlx::query("UPDATE encounter_creatures SET statblock_json = $1, overrides_json = $2, pack_rev = 'old' WHERE id = $3")
        .bind(block.to_string())
        .bind(o.to_string())
        .bind(c)
        .execute(t.state.db.pool())
        .await
        .unwrap();
    let r = ok(&t.dm, "POST", &format!("{path}/refresh"), json!({})).await;
    assert_eq!(
        (r["changed"].clone(), r["applied"].clone()),
        (json!(true), json!(false)),
        "{r}"
    );
    let fields: Vec<&str> = r["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["hp", "ac"]);
    assert_eq!(
        (
            r["conflicts"][0]["was"]["average"].clone(),
            r["conflicts"][0]["source"]["average"].clone()
        ),
        (json!(50), json!(59))
    );
    let (_, v) = t.dm.get(&format!("/api/encounters/{e}")).await;
    assert_eq!(v["creatures"][0]["packRev"], "old", "nothing changed yet");
    // Keep the DM's HP, take the source's AC.
    let r = ok(
        &t.dm,
        "POST",
        &format!("{path}/refresh"),
        json!({ "choices": { "hp": "keep", "ac": "source" } }),
    )
    .await;
    assert_eq!(r["applied"], true);
    let mine = &r["encounter"]["creatures"][0];
    assert_eq!(
        (
            mine["hp"]["average"].clone(),
            mine["ac"].clone(),
            mine["printed"]["hp"]["average"].clone()
        ),
        (json!(80), json!(13), json!(59))
    );
    assert!(mine["overrides"].get("ac").is_none());
    assert_ne!(mine["packRev"], "old");
    // A monster gone from the DM's content: a conflict that says so.
    sqlx::query("UPDATE encounter_creatures SET element_id = 'ID_NOT_THERE' WHERE id = $1")
        .bind(&ids[1])
        .execute(t.state.db.pool())
        .await
        .unwrap();
    let (status, out) =
        t.dm.post(
            &format!("/api/encounters/{e}/creatures/{}/refresh", ids[1]),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{out}");
    // Only typed Monster elements (D17), and only ones the DM has.
    for (element, words) in [
        ("ID_SRD521_MONSTER_NO_SUCH", "isn't in your content"),
        ("ID_LANGUAGE_GOBLIN", "Only monsters"),
    ] {
        let (status, out) =
            t.dm.post(
                &format!("/api/encounters/{e}/creatures"),
                json!({ "elementId": element }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
        assert!(out["message"].as_str().unwrap().contains(words), "{out}");
    }
    for bad in [
        json!({ "elementId": GOBLIN, "count": 0 }),
        json!({ "elementId": GOBLIN, "count": 21 }),
    ] {
        let (status, _) =
            t.dm.post(&format!("/api/encounters/{e}/creatures"), bad)
                .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    // Reorder: the second first.
    let v = ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{e}/creatures/{}", ids[1]),
        json!({ "sort": 0 }),
    )
    .await;
    assert_eq!(creature_ids(&v), [ids[1].clone(), ids[0].clone()]);
    // A 2014 block in a 2024 encounter is flagged, not refused.
    let v = add(&t.dm, &e, json!({ "elementId": GOBLIN_2014 })).await;
    assert!(has_warning(&v, "edition"));
    // The copies round-trip through the stat block model.
    let rows =
        sqlx::query("SELECT statblock_json FROM encounter_creatures WHERE encounter_id = $1")
            .bind(&e)
            .fetch_all(t.state.db.pool())
            .await
            .unwrap();
    for row in rows {
        let snap: party_encounter::creature::Snapshot =
            serde_json::from_str(&row.try_get::<String, _>(0).unwrap()).unwrap();
        let data = snap.data.expect("a monster copy");
        assert!(data.validate().is_empty(), "{:?}", data.validate());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn overrides_keep_their_source_and_refresh_shows_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    overrides_and_refresh(&memory_table(dir.path()).await).await;
}

// Starting effects and rules.

async fn effects_and_rules(t: &Table) {
    let e = new_encounter(&t.dm, &t.party, json!({ "name": "Precast" })).await;
    let v = add(&t.dm, &e, json!({ "elementId": GOBLIN, "count": 2 })).await;
    let ids = creature_ids(&v);
    let other = new_encounter(&t.dm, &t.party, json!({ "name": "Elsewhere" })).await;
    let v = add(&t.dm, &other, json!({ "elementId": GOBLIN })).await;
    let foreign = creature_ids(&v)[0].clone();
    let effects = format!("/api/encounters/{e}/effects");
    let v = ok(
        &t.dm,
        "POST",
        &effects,
        json!({ "creatureId": ids[0], "name": "Mage Armor", "detail": "AC 13 + Dex", "sourceCreatureId": ids[1],
                "durationTurns": 10, "concentration": false, "slotLevel": 1, "modifiers": { "ac": 2, "speedAdd": 10 } }),
    )
    .await;
    let effect = &v["effects"][0];
    assert_eq!(effect["modifiers"], json!({ "ac": 2, "speedAdd": 10 }));
    assert_eq!(effect["sourceCreatureId"], ids[1].as_str());
    let eid = effect["id"].as_str().unwrap().to_string();
    for bad in [
        json!({ "creatureId": foreign, "name": "X" }),
        json!({ "creatureId": ids[0], "name": "" }),
        json!({ "creatureId": ids[0], "name": "X", "slotLevel": 10 }),
        json!({ "creatureId": ids[0], "name": "X", "modifiers": { "ac": 11 } }),
        json!({ "creatureId": ids[0], "name": "X", "modifiers": { "fly": 1 } }),
        json!({ "name": "No creature" }),
    ] {
        let (status, out) = t.dm.post(&effects, bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {out}");
    }
    let v = ok(
        &t.dm,
        "PATCH",
        &format!("{effects}/{eid}"),
        json!({ "durationTurns": null, "charges": 3, "concentration": true }),
    )
    .await;
    assert_eq!(
        (
            v["effects"][0]["durationTurns"].clone(),
            v["effects"][0]["charges"].clone()
        ),
        (Value::Null, json!(3))
    );
    assert_eq!(
        v["effects"][0]["name"], "Mage Armor",
        "what the PATCH left out stays"
    );
    assert_eq!(v["effects"][0]["slotLevel"], 1);
    let rules = format!("/api/encounters/{e}/rules");
    let v = ok(
        &t.dm,
        "POST",
        &rules,
        json!({ "name": "While the leader stands", "text": "The others get a third attack.", "creatureId": ids[0],
                "flags": [{ "creatureId": ids[1], "ability": "Third Attack", "whileActive": "on" }] }),
    )
    .await;
    let rule = &v["rules"][0];
    assert_eq!(
        (rule["endsWhen"].clone(), rule["active"].clone()),
        (json!("creature_down"), json!(true))
    );
    let rid = rule["id"].as_str().unwrap().to_string();
    for bad in [
        json!({ "name": "No creature to fall" }),
        json!({ "name": "X", "endsWhen": "never" }),
        json!({ "name": "X", "endsWhen": "manual", "flags": [{ "creatureId": foreign, "ability": "A", "whileActive": "on" }] }),
        json!({ "name": "X", "endsWhen": "manual", "flags": [{ "creatureId": ids[0], "ability": "A", "whileActive": "maybe" }] }),
        json!({ "name": "X", "endsWhen": "manual", "script": "1" }),
    ] {
        let (status, out) = t.dm.post(&rules, bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {out}");
    }
    let v = ok(
        &t.dm,
        "POST",
        &rules,
        json!({ "name": "By hand", "endsWhen": "manual" }),
    )
    .await;
    assert_eq!(v["rules"][1]["creatureId"], Value::Null);
    let v = ok(
        &t.dm,
        "PATCH",
        &format!("{rules}/{rid}"),
        json!({ "active": false }),
    )
    .await;
    assert_eq!(
        (
            v["rules"][0]["active"].clone(),
            v["rules"][0]["name"].clone()
        ),
        (json!(false), json!("While the leader stands"))
    );
    // Deleting the creatures: their effects go; the rule keeps its text and loses its link and flags.
    ok(
        &t.dm,
        "DELETE",
        &format!("/api/encounters/{e}/creatures/{}", ids[1]),
        Value::Null,
    )
    .await;
    let v = ok(
        &t.dm,
        "DELETE",
        &format!("/api/encounters/{e}/creatures/{}", ids[0]),
        Value::Null,
    )
    .await;
    assert!(v["effects"].as_array().unwrap().is_empty(), "{v}");
    let rule = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == rid.as_str())
        .unwrap();
    assert_eq!(
        (rule["creatureId"].clone(), rule["flags"].clone()),
        (Value::Null, json!([]))
    );
    assert_eq!(rule["text"], "The others get a third attack.");
    // Another encounter's effect id is the same 404.
    let (status, out) =
        t.dm.patch(
            &format!("/api/encounters/{other}/effects/{eid}"),
            json!({ "name": "x" }),
        )
        .await;
    assert_eq!(
        (status, out["message"].as_str()),
        (StatusCode::NOT_FOUND, Some(NOT_FOUND))
    );
    for bad in [
        json!({ "status": "live" }),
        json!({ "status": "won" }),
        json!({ "ruleset": "dnd5" }),
        json!({ "hpMode": "max" }),
        json!({ "name": "" }),
        json!({ "publicName": "x".repeat(61) }),
        json!({ "dmNotes": "x".repeat(4001) }),
    ] {
        let (status, out) =
            t.dm.patch(&format!("/api/encounters/{e}"), bad.clone())
                .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {out}");
    }
    let (status, _) =
        t.dm.post(
            &format!("/api/parties/{}/encounters", t.party),
            json!({ "name": "Shown", "playerVisible": true }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a new encounter is a draft"
    );
    ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{e}"),
        json!({ "status": "ready", "playerVisible": true }),
    )
    .await;
    let v = ok(
        &t.dm,
        "PATCH",
        &format!("/api/encounters/{e}"),
        json!({ "status": "draft" }),
    )
    .await;
    assert_eq!(v["playerVisible"], false, "back to draft clears it");
    // Deleting the party takes its encounters with it.
    let (status, out) = t.dm.delete(&format!("/api/parties/{}", t.party)).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    for table in [
        "encounters",
        "encounter_creatures",
        "encounter_effects",
        "encounter_rules",
        "encounter_casts",
    ] {
        let n: i64 = sqlx::query(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(t.state.db.pool())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        assert_eq!(n, 0, "{table}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn starting_effects_and_rules_are_checked_and_follow_their_creatures() {
    let dir = tempfile::tempdir().unwrap();
    effects_and_rules(&memory_table(dir.path()).await).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_kill_switch_removes_the_routes() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_in(dir.path(), Db::open(&Location::Memory).await.unwrap());
    let router: Router = crate::encounters::routes_if(false)
        .route(
            "/api/{*rest}",
            axum::routing::any(|| async { StatusCode::NOT_FOUND }),
        )
        .with_state(state);
    let (status, _, _) = raw(&router, None, "GET", "/api/encounters/x", "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// The upgrade from the 0.2.x schema.

/// A database as 0.2.x left it (migrations 1 to 3): an admin, a DM with a party, a player's character in it.
async fn seed_0_2(location: &Location, data: &Path) {
    let old = Db::open_at(location, 3).await.unwrap();
    let router = router_on(data.to_path_buf(), old.clone());
    let admin = sign_up(&router, "up-admin").await;
    let dm = sign_up(&router, "up-dm").await;
    let player = sign_up(&router, "up-player").await;
    let party = new_party(&dm, "Before 0.3").await;
    join(&dm, &party, "Before 0.3", &player, "up-player", "Old Hand").await;
    let (_, log) = admin.get("/api/admin/activity").await;
    assert!(log["total"].as_i64().unwrap() > 0);
    let tables: i64 = sqlx::query(if old.kind() == "sqlite" {
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'encounters'"
    } else {
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = current_schema() AND table_name = 'encounters'"
    })
    .fetch_one(old.pool())
    .await
    .unwrap()
    .try_get(0)
    .unwrap();
    assert_eq!(tables, 0, "0.2.x has no encounters");
    old.pool().close().await;
}

/// HEAD over the seeded database: sign-in, the old party and its member, and encounters in it.
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
    assert_eq!(list["parties"][0]["memberCount"], 1);
    let e = new_encounter(
        &dm,
        &party,
        json!({ "name": "After the upgrade", "publicName": "Ambush" }),
    )
    .await;
    let v = add(
        &dm,
        &e,
        json!({ "elementId": GOBLIN, "count": 4, "hidden": false }),
    )
    .await;
    assert_eq!(v["creatures"].as_array().unwrap().len(), 4);
    ok(
        &dm,
        "PATCH",
        &format!("/api/encounters/{e}"),
        json!({ "status": "ready", "playerVisible": true }),
    )
    .await;
    let (status, seen) = player.get(&format!("/api/encounters/{e}")).await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!(seen["creatures"].as_array().unwrap().len(), 4);
    drop(Db::open(location).await.unwrap());
    // A database that says the step ran but has no encounters table (another numbering) gets it again. The
    // precast spells (step 9) refer to its creatures, so they go too, and step 9 runs again with it.
    for table in [
        "encounter_casts",
        "encounter_rules",
        "encounter_effects",
        "encounter_creatures",
        "encounters",
    ] {
        sqlx::query(&format!("DROP TABLE {table}"))
            .execute(db.pool())
            .await
            .unwrap();
    }
    let again = Db::open(location).await.unwrap();
    let router = router_on(data.to_path_buf(), again);
    let dm = sign_in(&router, "up-dm").await;
    let (status, out) = dm.get(&format!("/api/parties/{party}/encounters")).await;
    assert_eq!(
        (status, out["encounters"].as_array().map(Vec::len)),
        (StatusCode::OK, Some(0)),
        "{out}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn encounters_arrive_on_a_0_2_database() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("party.db");
    let data = dir.path().join("data");
    let location = Location::Sqlite(file.clone());
    seed_0_2(&location, &data).await;
    check_upgraded(&location, &data).await;
    // The instance still moves to another database, encounters and all.
    let db = Db::open(&location).await.unwrap();
    let router = router_on(data.clone(), db.clone());
    let dm = sign_in(&router, "up-dm").await;
    let (_, list) = dm.get("/api/parties").await;
    let party = list["parties"][0]["id"].as_str().unwrap().to_string();
    let e = new_encounter(&dm, &party, json!({ "name": "Moving" })).await;
    let v = add(&dm, &e, json!({ "elementId": OWLBEAR })).await;
    let c = creature_ids(&v)[0].clone();
    ok(
        &dm,
        "POST",
        &format!("/api/encounters/{e}/effects"),
        json!({ "creatureId": c, "name": "Bless" }),
    )
    .await;
    ok(
        &dm,
        "POST",
        &format!("/api/encounters/{e}/rules"),
        json!({ "name": "Rule", "creatureId": c }),
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
        "encounters 1",
        "encounter_creatures 1",
        "encounter_effects 1",
        "encounter_rules 1",
    ] {
        assert!(text.contains(want), "{text}");
    }
}

/// A fresh PostgreSQL database for one flow.
async fn postgres_table(url: &str, dir: &Path) -> Table {
    drop_all(url).await;
    let db = Db::open(&Location::Postgres(url.to_string()))
        .await
        .unwrap();
    table(state_in(dir, db)).await
}

/// Everything above on PostgreSQL. Runs when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.
#[tokio::test(flavor = "multi_thread")]
async fn encounters_on_postgres() {
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
    editions_and_controlled(&postgres_table(&url, &dir.path().join("editions")).await).await;
    overrides_and_refresh(&postgres_table(&url, &dir.path().join("overrides")).await).await;
    effects_and_rules(&postgres_table(&url, &dir.path().join("effects")).await).await;
    drop_all(&url).await;
}

// Opt-in: a real 5etools checkout (BOREALIS_TEST_5ETOOLS_DIR). Counts only; never names a creature or a book.

#[tokio::test(flavor = "multi_thread")]
async fn a_5etools_bestiary_builds_and_rates() {
    use tauri::Manager;
    let Ok(dir) = std::env::var("BOREALIS_TEST_5ETOOLS_DIR") else {
        eprintln!("skipped: set BOREALIS_TEST_5ETOOLS_DIR to a local 5etools source checkout");
        return;
    };
    let root = std::path::PathBuf::from(dir);
    let data = if root.join("data").is_dir() {
        root.join("data")
    } else {
        root
    };
    let index: Value =
        serde_json::from_str(&std::fs::read_to_string(data.join("bestiary/index.json")).unwrap())
            .unwrap();
    let sources: Vec<String> = index.as_object().unwrap().keys().cloned().collect();
    let tmp = tempfile::tempdir().unwrap();
    let t = memory_table(tmp.path()).await;
    let workspace = t.state.workspaces.get(&t.dm.id).await.unwrap();
    let folder = data.clone();
    let token = workspace
        .blocking(move |h| {
            let picked = h
                .state::<borealis_lib::state::AppState>()
                .picked
                .grant(
                    folder,
                    borealis_lib::picked_paths::PickPurpose::FivetoolsFolder,
                )
                .map_err(|e| crate::error::ApiError::Internal(e.message))?;
            Ok(picked.token)
        })
        .await
        .unwrap();
    let (status, out) =
        t.dm.rpc(
            "import_5etools",
            json!({ "request": { "folder": token, "sources": sources, "includeFluff": false } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{}", out["message"]);
    let ids: Vec<String> = workspace
        .blocking(|h| {
            let hub = h
                .state::<borealis_lib::state::AppState>()
                .hub()
                .map_err(|e| crate::error::ApiError::Internal(e.message))?;
            Ok(hub
                .store
                .by_type("Monster")
                .map(|s| s.element.id.clone())
                .collect())
        })
        .await
        .unwrap();
    assert!(ids.len() > 4000, "a full bestiary: {}", ids.len());
    // Every 40th creature, in encounters of up to 60.
    let (mut added, mut refused, mut rated) = (0u64, 0u64, 0u64);
    let picked: Vec<&String> = ids.iter().step_by(40).collect();
    for chunk in picked.chunks(60) {
        let e = new_encounter(&t.dm, &t.party, json!({ "name": "Corpus" })).await;
        for id in chunk {
            let (status, _) =
                t.dm.post(
                    &format!("/api/encounters/{e}/creatures"),
                    json!({ "elementId": id }),
                )
                .await;
            if status == StatusCode::OK {
                added += 1;
            } else {
                refused += 1;
            }
        }
        let (_, v) = t.dm.get(&format!("/api/encounters/{e}")).await;
        rated += v["creatures"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["statBlock"].is_object())
            .count() as u64;
        ok(
            &t.dm,
            "DELETE",
            &format!("/api/encounters/{e}"),
            Value::Null,
        )
        .await;
    }
    println!(
        "5etools corpus: {} monsters, {added} added, {refused} refused, {rated} with a block",
        ids.len()
    );
    assert_eq!(refused, 0, "every typed Monster element can be added");
    assert_eq!(rated, added, "every added creature shows its block");
}
