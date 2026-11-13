//! The live party layer end to end (0.2.1): party events on `GET /api/events` for the two audiences, what each
//! account may see of a member character (D6), the DM's read-only view, revocation, resync, and the cards a player's
//! play refreshes.
//!
//! Runs on SQLite, and on PostgreSQL too when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::{
    drop_all, new_character, router_of, send, sign_in, sign_up, state_with, Client, Setup,
};
use crate::db::{Db, Location};
use crate::AppState;

/// How long an event may take to arrive, and how long a stream is watched to show nothing comes.
const ARRIVES: Duration = Duration::from_secs(2);
const QUIET: Duration = Duration::from_millis(700);

/// The hub's events (the workspace's own may start with `party://` too).
const HUB_EVENTS: [&str; 3] = [
    crate::hub::PARTY_EVENT,
    crate::hub::MINE_EVENT,
    crate::hub::RESYNC_EVENT,
];

/// One server-sent event.
#[derive(Debug, Clone)]
struct Sse {
    id: Option<u64>,
    event: String,
    payload: Value,
}

/// An open `GET /api/events`.
struct Events {
    body: Body,
    buffer: String,
    ended: bool,
}

async fn open_events(router: &Router, token: &str, after: Option<u64>) -> Events {
    let uri = match after {
        Some(n) => format!("/api/events?after={n}"),
        None => "/api/events".to_string(),
    };
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("x-accel-buffering").unwrap(),
        "no",
        "proxies must not buffer the stream"
    );
    Events {
        body: response.into_body(),
        buffer: String::new(),
        ended: false,
    }
}

impl Events {
    /// The next event within `wait`, or none.
    async fn next(&mut self, wait: Duration) -> Option<Sse> {
        let until = Instant::now() + wait;
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let block: String = self.buffer.drain(..end + 2).collect();
                let mut id = None;
                let mut data = String::new();
                for line in block.lines() {
                    if let Some(v) = line.strip_prefix("id:") {
                        id = v.trim().parse().ok();
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data.push_str(v.trim());
                    }
                }
                if data.is_empty() {
                    continue;
                }
                let value: Value = serde_json::from_str(&data).unwrap();
                return Some(Sse {
                    id,
                    event: value["event"].as_str().unwrap().to_string(),
                    payload: value["payload"].clone(),
                });
            }
            if self.ended {
                return None;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            match tokio::time::timeout(left, self.body.frame()).await {
                Ok(Some(Ok(frame))) => {
                    if let Ok(bytes) = frame.into_data() {
                        self.buffer.push_str(&String::from_utf8_lossy(&bytes));
                    }
                }
                Ok(_) => self.ended = true,
                Err(_) => return None,
            }
        }
    }

    /// The next party event within `wait`; workspace events are skipped.
    async fn party(&mut self, wait: Duration) -> Option<Sse> {
        let until = Instant::now() + wait;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let e = self.next(left).await?;
            if HUB_EVENTS.contains(&e.event.as_str()) {
                return Some(e);
            }
        }
    }

    /// Every party event that arrives within `wait`.
    async fn parties(&mut self, wait: Duration) -> Vec<Sse> {
        let mut out = Vec::new();
        while let Some(e) = self.party(wait).await {
            out.push(e);
        }
        out
    }

    /// True when the stream ends within `wait`.
    async fn ends(&mut self, wait: Duration) -> bool {
        let until = Instant::now() + wait;
        while !self.ended {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            if self.next(left).await.is_none() && !self.ended {
                return false;
            }
        }
        true
    }
}

/// The id of the slot titled `title` in a snapshot.
fn slot(snapshot: &Value, title: &str) -> String {
    snapshot["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == title)
        .unwrap_or_else(|| panic!("no slot {title}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A saved level-1 fighter of `client`'s, with hit points; its id.
async fn fighter(client: &Client, name: &str) -> String {
    let (status, out) = client.rpc("create_character", new_character(name)).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let id = out["id"].as_str().unwrap().to_string();
    for (title, element) in [
        ("Race", "ID_2024_SPECIES_HUMAN"),
        ("Class", "ID_2024_CLASS_FIGHTER"),
    ] {
        let (_, snap) = client
            .rpc("get_snapshot", json!({ "characterId": id }))
            .await;
        let (status, out) = client
            .rpc(
                "apply_selection",
                json!({ "characterId": id, "slotId": slot(&snap, title), "elementId": element }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{title}: {out}");
    }
    let (status, out) = client
        .rpc("save_character", json!({ "characterId": id }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (status, out) = client
        .rpc(
            "close_character",
            json!({ "characterId": id, "discard": false }),
        )
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

/// `player` joins `party` with `character`, through a join code.
async fn join(dm: &Client, party: &str, player: &Client, character: &str) {
    let (status, out) = dm
        .post(
            &format!("/api/parties/{party}/invites"),
            json!({ "kind": "code", "uses": 1 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let code = out["code"].as_str().unwrap();
    let (status, out) = player
        .post(
            "/api/join",
            json!({ "code": code, "characterId": character }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

async fn damage(client: &Client, character: &str, amount: u32) {
    let (status, out) = client
        .rpc(
            "take_damage",
            json!({ "characterId": character, "amount": amount }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

/// The roster member for `character` as `client` sees it, and the roster's `pseq`.
async fn member(client: &Client, party: &str, character: &str) -> (Value, u64) {
    let (status, out) = client.get(&format!("/api/parties/{party}/roster")).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let m = out["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["characterId"] == character)
        .unwrap_or_else(|| panic!("no {character} in {out}"))
        .clone();
    (m, out["pseq"].as_u64().unwrap())
}

/// The open sessions of an account's workspace, and its file's digest and stamp.
async fn sessions(state: &AppState, owner: &str) -> usize {
    use tauri::Manager;
    match state.workspaces.loaded_now(owner) {
        Some(w) => w
            .blocking(|app| {
                Ok(app
                    .state::<borealis_lib::state::AppState>()
                    .sessions()
                    .map(|s| s.len())
                    .unwrap_or(0))
            })
            .await
            .unwrap(),
        None => 0,
    }
}

fn file_stamp(state: &AppState, owner: &str, character: &str) -> (Vec<u8>, std::time::SystemTime) {
    let file = state
        .workspaces
        .dir(owner)
        .join("characters")
        .join(format!("{character}.json"));
    (
        std::fs::read(&file).unwrap(),
        std::fs::metadata(&file).unwrap().modified().unwrap(),
    )
}

fn assert_d6(member: &Value) {
    let keys: Vec<&str> = member
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    for hidden in ["stats", "ownerId", "online", "readAt"] {
        assert!(
            !keys.contains(&hidden),
            "{hidden} shown to another member: {member}"
        );
    }
    for shown in ["name", "level", "classes", "band", "state"] {
        assert!(keys.contains(&shown), "{shown} missing: {member}");
    }
    let text = member.to_string();
    assert!(
        !text.contains("armorClass") && !text.contains("hpMax"),
        "{text}"
    );
}

/// The whole live flow. `state` is the server's, to look inside it.
async fn live_flow(state: AppState) {
    let router = router_of(state.clone(), 64 * 1024 * 1024);
    let admin = sign_up(&router, "live-admin").await;
    let dm = sign_up(&router, "live-dm").await;
    let alice = sign_up(&router, "live-alice").await;
    let bob = sign_up(&router, "live-bob").await;
    let carol = sign_up(&router, "live-carol").await;
    let outsider = sign_up(&router, "live-outsider").await;
    let other_dm = sign_up(&router, "live-other-dm").await;

    let party = new_party(&dm, "Live Party").await;
    let other = new_party(&other_dm, "Other Party").await;
    let a = fighter(&alice, "Aria").await;
    let b = fighter(&bob, "Bram").await;
    let c = fighter(&carol, "Cass").await;
    let o = fighter(&outsider, "Orin").await;
    join(&dm, &party, &alice, &a).await;
    join(&dm, &party, &bob, &b).await;
    join(&other_dm, &other, &carol, &c).await;

    // Nobody signed out gets a stream.
    let (status, _) = send(&router, None, "GET", "/api/events", Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let mut dm_events = open_events(&router, &dm.token, None).await;
    let mut alice_events = open_events(&router, &alice.token, None).await;
    let mut bob_events = open_events(&router, &bob.token, None).await;
    let mut carol_events = open_events(&router, &carol.token, None).await;
    let mut outsider_events = open_events(&router, &outsider.token, None).await;
    let mut other_dm_events = open_events(&router, &other_dm.token, None).await;

    // The DM's roster: the card was read when the player played, with the numbers a DM needs.
    let (status, out) = alice
        .rpc("load_character", json!({ "characterId": a }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (card, _) = member(&dm, &party, &a).await;
    assert_eq!(card["name"], "Aria");
    assert_eq!(card["classes"][0]["name"], "Fighter", "{card}");
    assert_eq!(card["online"], true, "{card}");
    let max = card["stats"]["hpMax"].as_u64().unwrap();
    assert!(max >= 6, "{card}");
    assert_eq!(card["stats"]["hp"].as_u64().unwrap(), max);
    assert!(card["stats"]["armorClass"].as_i64().unwrap() > 0, "{card}");
    assert!(card["stats"]["passivePerception"].as_i64().is_some());
    dm_events.parties(QUIET).await;
    bob_events.parties(QUIET).await;
    alice_events.parties(QUIET).await;

    // Another member sees the D6 fields only, in the roster and in the party's detail.
    let (seen, bob_seq) = member(&bob, &party, &a).await;
    assert_d6(&seen);
    assert_eq!(seen["band"], "unhurt");
    let (_, detail) = bob.get(&format!("/api/parties/{party}")).await;
    let in_detail = detail["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["characterId"] == a)
        .unwrap();
    assert_d6(in_detail);
    // The owner sees their own card whole.
    let (own, _) = member(&alice, &party, &a).await;
    assert_eq!(own["stats"]["hpMax"].as_u64().unwrap(), max);

    // A scratch: the DM hears at once, with the new hit points; nobody else hears anything.
    let (_, dm_seq) = member(&dm, &party, &a).await;
    damage(&alice, &a, 1).await;
    let e = dm_events
        .party(ARRIVES)
        .await
        .expect("the DM hears of the damage");
    assert_eq!(e.event, "party://party");
    assert_eq!(e.payload["party"], party.as_str());
    assert_eq!(e.payload["kind"], "card");
    assert_eq!(e.payload["id"], a.as_str());
    assert!(e.payload["pseq"].as_u64().unwrap() > dm_seq);
    assert!(e.id.is_some(), "party events carry ids");
    let (card, _) = member(&dm, &party, &a).await;
    assert_eq!(card["stats"]["hp"].as_u64().unwrap(), max - 1);
    // Still unhurt: the players' view didn't change, so their sequence and their streams stay still.
    assert!(bob_events.parties(QUIET).await.is_empty());
    let (seen, seq) = member(&bob, &party, &a).await;
    assert_eq!(seen["band"], "unhurt");
    assert_eq!(
        seq, bob_seq,
        "the players' sequence moved for a hidden change"
    );
    for (name, events) in [
        ("carol", &mut carol_events),
        ("outsider", &mut outsider_events),
        ("other dm", &mut other_dm_events),
    ] {
        assert!(events.parties(Duration::ZERO).await.is_empty(), "{name}");
    }

    // Bloodied: now the players hear, with their own sequence.
    let to_bloodied = u32::try_from(max - 1 - max / 2).unwrap();
    damage(&alice, &a, to_bloodied).await;
    assert!(dm_events.party(ARRIVES).await.is_some());
    let e = bob_events
        .party(ARRIVES)
        .await
        .expect("bob sees the band change");
    assert_eq!(e.payload["kind"], "card");
    assert_eq!(e.payload["pseq"].as_u64().unwrap(), bob_seq + 1);
    let (seen, _) = member(&bob, &party, &a).await;
    assert_eq!(seen["band"], "bloodied");
    assert_d6(&seen);
    for (name, events) in [
        ("carol", &mut carol_events),
        ("outsider", &mut outsider_events),
        ("other dm", &mut other_dm_events),
    ] {
        assert!(events.parties(QUIET).await.is_empty(), "{name} heard");
    }

    // Outsiders and other parties get the same 404 for everything: the roster, the view, any id.
    let view = |p: &str, owner: &str, ch: &str| {
        format!("/api/parties/{p}/members/{owner}/{ch}/view/snapshot")
    };
    for client in [&outsider, &carol, &other_dm] {
        let (status, out) = client.get(&format!("/api/parties/{party}/roster")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
        let (status, out) = client.post(&view(&party, &alice.id, &a), json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{out}");
        assert!(!out.to_string().contains("Aria"));
    }
    // Members can't view each other, nor themselves through the DM's route.
    for (client, owner, ch) in [(&bob, &alice.id, &a), (&alice, &alice.id, &a)] {
        let (status, _) = client.post(&view(&party, owner, ch), json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    // A DM can't view a character of another party through their own, nor a made-up one.
    for (owner, ch) in [
        (&carol.id, &c),
        (&outsider.id, &o),
        (&alice.id, &"c_00000000000000000000000000000000".to_string()),
    ] {
        let (status, _) = dm.post(&view(&party, owner, ch), json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    let (status, _) = dm
        .post(&view(&party, "..", &format!("..%2F{a}")), json!({}))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The DM's view: what the owner has now, read-only. It opens no session and writes nothing.
    let before_file = file_stamp(&state, &alice.id, &a);
    let before_sessions = sessions(&state, &alice.id).await;
    let (status, out) = dm.post(&view(&party, &alice.id, &a), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["snapshot"]["name"], "Aria");
    assert_eq!(
        out["snapshot"]["play"]["hitPoints"]["current"]
            .as_u64()
            .unwrap(),
        max - 1 - u64::from(to_bloodied)
    );
    for hidden in ["displayPath", "dirty", "vaultNote", "group", "favorite"] {
        assert!(out["snapshot"].get(hidden).is_none(), "{hidden}");
    }
    assert!(!out
        .to_string()
        .contains(&state.workspaces.dir(&alice.id).display().to_string()));
    assert_eq!(out["card"]["stats"]["hpMax"].as_u64().unwrap(), max);
    assert_eq!(file_stamp(&state, &alice.id, &a), before_file);
    assert_eq!(sessions(&state, &alice.id).await, before_sessions);
    // Bob's character isn't open: the view reads the saved file, and still opens no session.
    let before_file = file_stamp(&state, &bob.id, &b);
    assert_eq!(sessions(&state, &bob.id).await, 0);
    let (status, out) = dm.post(&view(&party, &bob.id, &b), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["snapshot"]["name"], "Bram");
    assert_eq!(sessions(&state, &bob.id).await, 0);
    assert_eq!(file_stamp(&state, &bob.id, &b), before_file);
    dm_events.parties(QUIET).await;
    bob_events.parties(QUIET).await;
    alice_events.parties(QUIET).await;

    // Resync: a stream opened after the last event it saw needs none; one that missed events gets one.
    let last = dm_events_last(&state, &dm.id);
    let mut fresh = open_events(&router, &dm.token, Some(last)).await;
    assert!(fresh.parties(QUIET).await.is_empty());
    drop(fresh);
    let mut behind = open_events(&router, &dm.token, Some(0)).await;
    let e = behind.party(ARRIVES).await.expect("a resync");
    assert_eq!(e.event, "party://resync");
    assert_eq!(e.id, Some(last));
    drop(behind);

    // Removal: bob's character leaves; bob hears only that his own list changed, then nothing of the party.
    let (status, out) = dm
        .delete(&format!("/api/parties/{party}/members/{}/{b}", bob.id))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let e = bob_events.party(ARRIVES).await.expect("bob's list changed");
    assert_eq!(e.event, "party://mine");
    assert_eq!(e.payload, json!({ "kind": "parties" }));
    let e = alice_events
        .party(ARRIVES)
        .await
        .expect("alice sees the roster change");
    assert_eq!(e.payload["kind"], "roster");
    dm_events.parties(QUIET).await;
    damage(&alice, &a, 100).await;
    let e = alice_events
        .party(ARRIVES)
        .await
        .expect("alice sees herself down");
    assert_eq!(e.payload["kind"], "card");
    assert!(dm_events.party(ARRIVES).await.is_some());
    assert!(
        bob_events.parties(QUIET).await.is_empty(),
        "a removed member still hears the party"
    );
    let (status, _) = bob.get(&format!("/api/parties/{party}/roster")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, list) = bob.get("/api/parties").await;
    assert_eq!(list["parties"], json!([]));

    // Deleting a character takes it out of its party (carol's, in the other party).
    let (status, out) = carol
        .rpc("delete_character", json!({ "characterId": c }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let e = other_dm_events
        .party(ARRIVES)
        .await
        .expect("the other DM sees carol leave");
    assert_eq!(e.payload["kind"], "roster");
    let (_, roster) = other_dm.get(&format!("/api/parties/{other}/roster")).await;
    assert_eq!(roster["members"], json!([]));
    let heard = carol_events.parties(QUIET).await;
    assert!(heard.iter().all(|e| e.event == "party://mine"), "{heard:?}");
    assert!(
        dm_events.parties(QUIET).await.is_empty(),
        "another party's change reached the DM"
    );

    // Signing out ends the session's streams at once; other sessions keep theirs.
    let second = sign_in(&router, "live-alice").await;
    let mut second_events = open_events(&router, &second.token, None).await;
    let (status, _) = second.post("/api/auth/logout", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        second_events.ends(ARRIVES).await,
        "a signed-out stream stays open"
    );
    assert!(!alice_events.ends(QUIET).await);
    // A new password ends the other sessions' streams.
    let third = sign_in(&router, "live-alice").await;
    let mut third_events = open_events(&router, &third.token, None).await;
    let (status, out) = alice
        .post(
            "/api/me/password",
            json!({ "current": super::PASSWORD, "password": "another long passphrase" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert!(third_events.ends(ARRIVES).await);
    assert!(!alice_events.ends(QUIET).await);
    // The admin's "sign out everywhere" ends every stream of the account.
    let (status, out) = admin
        .post(
            &format!("/api/admin/accounts/{}/sign-out", bob.id),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert!(bob_events.ends(ARRIVES).await);

    // Deleting the party: its players hear their list changed, and nothing of it again.
    let (status, out) = dm.delete(&format!("/api/parties/{party}")).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let e = alice_events
        .party(ARRIVES)
        .await
        .expect("alice's list changed");
    assert_eq!(e.event, "party://mine");
    alice_events.parties(QUIET).await;
    dm_events.parties(QUIET).await;
    let (status, out) = alice
        .rpc(
            "regain_hit_points",
            json!({ "characterId": a, "amount": 5 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert!(dm_events.parties(QUIET).await.is_empty());
    assert!(alice_events.parties(Duration::ZERO).await.is_empty());
    // Its cards are gone from the server's folder too.
    assert!(!state
        .workspaces
        .dir(&alice.id)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("party-cards")
        .join(&alice.id)
        .join(format!("{a}.json"))
        .exists());

    // Disabling an account ends its streams.
    let (status, out) = admin
        .post(
            &format!("/api/admin/accounts/{}/status", outsider.id),
            json!({ "status": "disabled" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert!(outsider_events.ends(ARRIVES).await);
}

fn dm_events_last(state: &AppState, dm: &str) -> u64 {
    state.hub.last_id(dm)
}

/// Every name in `PLAY_COMMANDS` is a command the server offers: an upstream rename fails here.
async fn play_commands_are_offered(router: &Router) {
    let player = sign_up(router, "live-player").await;
    for name in crate::parties::PLAY_COMMANDS {
        let (_, out) = player
            .rpc(
                name,
                json!({ "characterId": "c_00000000000000000000000000000000" }),
            )
            .await;
        assert_ne!(
            out["message"], "The server does not offer that.",
            "{name} is not offered"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_party_is_live_and_shows_each_viewer_their_own_projection() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    let state = state_with(dir.path().join("data"), db, Setup::default());
    // The first account is the admin: the flow's.
    live_flow(state.clone()).await;
    play_commands_are_offered(&router_of(state, 1024 * 1024)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_kill_switch_removes_the_view_route() {
    let router: axum::Router<AppState> = crate::parties::routes_if(false);
    assert!(!router.has_routes());
    assert_eq!(
        crate::parties::card_target(
            "get_snapshot",
            json!({ "characterId": "c_00000000000000000000000000000000" })
                .as_object()
                .unwrap()
        ),
        crate::parties::CardTarget::None
    );
    for name in crate::parties::PLAY_COMMANDS {
        assert!(matches!(
            crate::parties::card_target(
                name,
                json!({ "characterId": "c_00000000000000000000000000000000" })
                    .as_object()
                    .unwrap()
            ),
            crate::parties::CardTarget::Character(_)
        ));
    }
    // Never a path: an id that isn't upstream's names nothing.
    assert_eq!(
        crate::parties::card_target(
            "take_damage",
            json!({ "characterId": "../c_1" }).as_object().unwrap()
        ),
        crate::parties::CardTarget::None
    );
}

/// The same on PostgreSQL. Runs when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.
#[tokio::test(flavor = "multi_thread")]
async fn the_party_is_live_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = super::POSTGRES.lock().await;
    drop_all(&url).await;
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Postgres(url)).await.unwrap();
    live_flow(state_with(dir.path().join("data"), db, Setup::default())).await;
}
