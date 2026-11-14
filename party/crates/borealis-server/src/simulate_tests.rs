//! Encounter simulation on the server (0.3.1, D26): read only (D25: a simulation next to a live combat changes no
//! row, no `seq` and sends no event), DM only (a player of the same party and a stranger get the generic not
//! found), no combat ids, and a whole run through the tools. SQLite in memory; PostgreSQL too when
//! `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.

use std::path::Path;

use axum::http::StatusCode;
use serde_json::{json, Map, Value};
use sqlx::Row;

use super::{drop_all, new_character, router_of, sign_up, state_with, Client, Setup};
use crate::auth::User;
use crate::db::{Db, Location};
use crate::simulate::{self, SimError};
use crate::AppState;

const OGRE: &str = "ID_SRD521_MONSTER_OGRE";
const GOBLIN: &str = "ID_SRD521_MONSTER_GOBLIN_WARRIOR";
const MAGE: &str = "ID_SRD521_MONSTER_MAGE";
const FIREBALL: &str = "ID_2024_SPELL_FIREBALL";

/// The session store is one per process: these tests take turns.
static SIM_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

static REQUESTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn request_id() -> String {
    format!(
        "sim-req-{}",
        REQUESTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    )
}

async fn ok(client: &Client, method: &str, path: &str, body: Value) -> Value {
    let (status, out) = super::send(&client.router, Some(&client.token), method, path, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {path}: {out}");
    out
}

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

async fn join(dm: &Client, party: &str, player: &Client, username: &str, who: &str) {
    let c = character(player, who).await;
    ok(
        dm,
        "POST",
        &format!("/api/parties/{party}/invites"),
        json!({ "kind": "username", "username": username }),
    )
    .await;
    let (_, invites) = player.get("/api/invites").await;
    let invite = invites["invites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["party"]["name"] == "Sim Party")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        player,
        "POST",
        &format!("/api/invites/{invite}/accept"),
        json!({ "characterId": c }),
    )
    .await;
}

struct World {
    state: AppState,
    dm: Client,
    player: Client,
    outsider: Client,
    party: String,
    encounter: String,
    combat: String,
}

fn user(c: &Client, name: &str) -> User {
    User {
        id: c.id.clone(),
        username: name.into(),
        email: None,
        admin: false,
        must_change_password: false,
    }
}

async fn world(state: AppState) -> World {
    let router = router_of(state.clone(), Setup::default().max_upload);
    sign_up(&router, "sim-admin").await;
    let dm = sign_up(&router, "sim-dm").await;
    let player = sign_up(&router, "sim-player").await;
    let player2 = sign_up(&router, "sim-player2").await;
    let outsider = sign_up(&router, "sim-outsider").await;
    ok(&dm, "POST", "/api/me/roles/dm", json!({})).await;
    let party = ok(&dm, "POST", "/api/parties", json!({ "name": "Sim Party" })).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    join(&dm, &party, &player, "sim-player", "Hero One").await;
    join(&dm, &party, &player2, "sim-player2", "Hero Two").await;
    let enc = |name: &str| json!({ "name": name, "publicName": name });
    let encounter = ok(
        &dm,
        "POST",
        &format!("/api/parties/{party}/encounters"),
        enc("Bridge"),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let path = format!("/api/encounters/{encounter}/creatures");
    ok(
        &dm,
        "POST",
        &path,
        json!({ "elementId": OGRE, "dmLabel": "Ogre" }),
    )
    .await;
    ok(
        &dm,
        "POST",
        &path,
        json!({ "elementId": GOBLIN, "dmLabel": "Goblin", "count": 2 }),
    )
    .await;
    // A live combat from another encounter of the same party, which the simulation must never touch.
    let live = ok(
        &dm,
        "POST",
        &format!("/api/parties/{party}/encounters"),
        enc("Live"),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &dm,
        "POST",
        &format!("/api/encounters/{live}/creatures"),
        json!({ "elementId": GOBLIN, "dmLabel": "Live Goblin" }),
    )
    .await;
    let view = ok(
        &dm,
        "POST",
        &format!("/api/encounters/{live}/start"),
        json!({ "requestId": request_id() }),
    )
    .await;
    let combat = view["id"].as_str().unwrap().to_string();
    World {
        state,
        dm,
        player,
        outsider,
        party,
        encounter,
        combat,
    }
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

async fn call(w: &World, who: &Client, tool: &str, a: Value) -> Result<Value, SimError> {
    simulate::call(&w.state, &user(who, "x"), tool, args(a)).await
}

/// Every table's row count, and the live combat's `seq`.
async fn census(db: &Db, combat: &str) -> (Vec<(String, i64)>, i64) {
    let names: Vec<String> = match sqlx::query(
        "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
    )
    .fetch_all(db.pool())
    .await
    {
        Ok(rows) => rows
            .iter()
            .map(|r| r.try_get::<String, _>(0).unwrap())
            .collect(),
        Err(_) => sqlx::query(
            "SELECT tablename::text FROM pg_tables WHERE schemaname = current_schema() ORDER BY 1",
        )
        .fetch_all(db.pool())
        .await
        .unwrap()
        .iter()
        .map(|r| r.try_get::<String, _>(0).unwrap())
        .collect(),
    };
    let mut counts = Vec::new();
    for n in names {
        let c: i64 = sqlx::query(&format!("SELECT COUNT(*) FROM \"{n}\""))
            .fetch_one(db.pool())
            .await
            .unwrap()
            .try_get(0)
            .unwrap();
        counts.push((n, c));
    }
    let seq: i64 = sqlx::query("SELECT seq FROM combats WHERE id = $1")
        .bind(combat)
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    (counts, seq)
}

async fn whole_run(w: &World) {
    simulate::clear_sessions();
    let before = census(&w.state.db, &w.combat).await;
    let mut subs = vec![
        w.state.hub.subscribe(&w.dm.id, "sim-dm", None),
        w.state.hub.subscribe(&w.player.id, "sim-p", None),
    ];
    // A run the agent plays (the foes), through the tools.
    let start = call(
        w,
        &w.dm,
        "sim_start",
        json!({ "encounterId": w.encounter, "seed": 7, "opts": { "maxRounds": 6 } }),
    )
    .await
    .unwrap();
    let sim = start["simId"].as_str().unwrap().to_string();
    assert_eq!(start["roster"].as_array().unwrap().len(), 5, "{start}");
    assert!(
        start["coverage"]["not_simulated"]
            .to_string()
            .contains("party card"),
        "PCs without a loaded sheet use their card: {start}"
    );
    assert!(
        !start.to_string().contains(&w.combat),
        "no combat id in a simulation"
    );
    let mut card = start["card"].clone();
    let mut acted = 0;
    while !card.is_null() && acted < 200 {
        let step = card["step"].as_u64().unwrap();
        let o = card["options"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| !o["targets"].as_array().unwrap().is_empty())
            .cloned();
        let d = match o {
            Some(o) => json!({ "o": o["o"], "t": [o["targets"][0]], "why": "nearest threat" }),
            None => json!({ "o": "end", "mv": "near" }),
        };
        let out = call(
            w,
            &w.dm,
            "sim_act",
            json!({ "simId": sim, "step": step, "d": d }),
        )
        .await
        .unwrap();
        assert!(out.get("error").is_none(), "{out}");
        // A retried (stale) step changes nothing and returns the current card.
        let again = call(
            w,
            &w.dm,
            "sim_act",
            json!({ "simId": sim, "step": step, "d": { "o": "end" } }),
        )
        .await
        .unwrap();
        if !out["card"].is_null() {
            assert_eq!(again["stale"], true, "{again}");
            assert_eq!(again["card"]["step"], out["card"]["step"]);
        }
        card = out["card"].clone();
        acted += 1;
    }
    assert!(acted > 0);
    let state = call(w, &w.dm, "sim_state", json!({ "simId": sim }))
        .await
        .unwrap();
    assert!(state["combatants"].as_array().unwrap().len() == 5);
    let report = call(w, &w.dm, "sim_report", json!({ "simId": sim }))
        .await
        .unwrap();
    assert!(
        report["steering"]["agent"].as_u64().unwrap() > 0,
        "{report}"
    );
    assert_eq!(report["replay"]["seed"], 7);
    assert_eq!(
        call(w, &w.dm, "sim_card", json!({ "simId": sim }))
            .await
            .unwrap_err(),
        SimError::NotFound,
        "the report ends it"
    );
    // Replay from the report.
    let replayed = call(
        w,
        &w.dm,
        "sim_replay",
        json!({
            "encounterId": w.encounter, "seed": 7, "opts": { "maxRounds": 6 },
            "transcript": report["transcript"], "fingerprints": report["fingerprints"],
        }),
    )
    .await
    .unwrap();
    assert_eq!(replayed["identical"], true, "{replayed}");
    // Basic only: sim_auto to the end, and a batch.
    let start = call(
        w,
        &w.dm,
        "sim_start",
        json!({ "encounterId": w.encounter, "seed": 9 }),
    )
    .await
    .unwrap();
    let sim = start["simId"].as_str().unwrap().to_string();
    let step = start["card"]["step"].as_u64().unwrap_or(0);
    let out = call(
        w,
        &w.dm,
        "sim_auto",
        json!({ "simId": sim, "step": step, "until": "end" }),
    )
    .await
    .unwrap();
    assert!(out["card"].is_null(), "{out}");
    call(w, &w.dm, "sim_report", json!({ "simId": sim }))
        .await
        .unwrap();
    let b = call(
        w,
        &w.dm,
        "sim_batch",
        json!({ "encounterId": w.encounter, "runs": 10, "seed": 3 }),
    )
    .await
    .unwrap();
    let o = &b["outcomes"];
    assert_eq!(o["of"], 10, "{b}");
    let sum: u64 = [
        "party_standing",
        "creatures_standing",
        "both_withdrew",
        "round_cap",
    ]
    .iter()
    .map(|k| o[k].as_u64().unwrap())
    .sum();
    assert_eq!(sum, 10);
    let b2 = call(
        w,
        &w.dm,
        "sim_batch",
        json!({ "encounterId": w.encounter, "runs": 10, "seed": 3 }),
    )
    .await
    .unwrap();
    assert_eq!(b, b2, "a batch is deterministic");
    // D25: nothing changed anywhere, and no one heard anything.
    let after = census(&w.state.db, &w.combat).await;
    assert_eq!(before, after);
    for s in &mut subs {
        let mut n = 0;
        while s.rx.try_recv().is_ok() {
            n += 1;
        }
        assert_eq!(n, 0, "the hub saw no events");
    }
}

async fn only_the_dm(w: &World) {
    simulate::clear_sessions();
    for who in [&w.player, &w.outsider] {
        for (tool, a) in [
            ("sim_start", json!({ "encounterId": w.encounter })),
            (
                "sim_batch",
                json!({ "encounterId": w.encounter, "runs": 2 }),
            ),
            (
                "sim_replay",
                json!({ "encounterId": w.encounter, "seed": 1, "transcript": [] }),
            ),
        ] {
            assert_eq!(
                call(w, who, tool, a).await.unwrap_err(),
                SimError::NotFound,
                "{tool}"
            );
        }
    }
    let start = call(
        w,
        &w.dm,
        "sim_start",
        json!({ "encounterId": w.encounter, "seed": 1 }),
    )
    .await
    .unwrap();
    let sim = start["simId"].as_str().unwrap();
    for who in [&w.player, &w.outsider] {
        for tool in ["sim_card", "sim_state", "sim_report"] {
            assert_eq!(
                call(w, who, tool, json!({ "simId": sim }))
                    .await
                    .unwrap_err(),
                SimError::NotFound,
                "{tool}"
            );
        }
        let e = call(
            w,
            who,
            "sim_act",
            json!({ "simId": sim, "step": 0, "d": { "o": "end" } }),
        )
        .await
        .unwrap_err();
        assert_eq!(e, SimError::NotFound);
    }
    // Combat ids are not encounters, and a combatId argument is refused outright.
    assert_eq!(
        call(w, &w.dm, "sim_start", json!({ "encounterId": w.combat }))
            .await
            .unwrap_err(),
        SimError::NotFound
    );
    assert!(matches!(
        call(w, &w.dm, "sim_start", json!({ "combatId": w.combat }))
            .await
            .unwrap_err(),
        SimError::Invalid(_)
    ));
    assert!(matches!(
        call(
            w,
            &w.dm,
            "sim_batch",
            json!({ "encounterId": w.encounter, "runs": 101 })
        )
        .await
        .unwrap_err(),
        SimError::Invalid(_)
    ));
    // Two open sessions per account.
    call(w, &w.dm, "sim_start", json!({ "encounterId": w.encounter }))
        .await
        .unwrap();
    assert!(matches!(
        call(w, &w.dm, "sim_start", json!({ "encounterId": w.encounter }))
            .await
            .unwrap_err(),
        SimError::Busy(_)
    ));
    // The server never calls a model (0.3.3): there is no assistant oracle.
    let e = call(w, &w.dm, "sim_auto", json!({ "simId": sim, "step": start["card"]["step"], "until": "end", "oracle": "assistant" })).await.unwrap_err();
    assert!(matches!(e, SimError::Invalid(_)), "{e:?}");
    simulate::clear_sessions();
}

/// Spell casters cast with the DM's own mechanics (0.3.3): the SRD Mage's spellbook is automated, the definitions'
/// revisions are in the fingerprints, a DM edit makes an old replay report the divergence, and none of it writes.
async fn spell_casters(w: &World) {
    simulate::clear_sessions();
    let tower = ok(
        &w.dm,
        "POST",
        &format!("/api/parties/{}/encounters", w.party),
        json!({ "name": "Tower", "publicName": "Tower" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &w.dm,
        "POST",
        &format!("/api/encounters/{tower}/creatures"),
        json!({ "elementId": MAGE, "dmLabel": "Mage" }),
    )
    .await;
    let ruleset: String = sqlx::query("SELECT ruleset FROM parties WHERE id = $1")
        .bind(&w.party)
        .fetch_one(w.state.db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    let before = census(&w.state.db, &w.combat).await;
    let opts = json!({ "control": { "agent": "none" }, "maxRounds": 4 });
    let start = call(
        w,
        &w.dm,
        "sim_start",
        json!({ "encounterId": tower, "seed": 3, "opts": opts }),
    )
    .await
    .unwrap();
    assert!(
        start["coverage"]["automated"]
            .to_string()
            .contains("Mage: Fireball"),
        "{}",
        start["coverage"]
    );
    let sim = start["simId"].as_str().unwrap().to_string();
    let report = call(w, &w.dm, "sim_report", json!({ "simId": sim }))
        .await
        .unwrap();
    let key = format!("spell:{FIREBALL}");
    assert_eq!(
        report["fingerprints"][&key], "srd:1",
        "{}",
        report["fingerprints"]
    );
    assert_eq!(
        census(&w.state.db, &w.combat).await,
        before,
        "a simulation writes nothing"
    );
    let batch = call(
        w,
        &w.dm,
        "sim_batch",
        json!({ "encounterId": tower, "runs": 3, "seed": 1 }),
    )
    .await
    .unwrap();
    assert_eq!(batch["outcomes"]["of"], 3, "{batch}");
    assert_eq!(
        census(&w.state.db, &w.combat).await,
        before,
        "a batch writes nothing"
    );
    // The DM's own Fireball: its revision is what the simulation fingerprints.
    let mut def = serde_json::to_value(party_spells::data::curated(FIREBALL).unwrap()).unwrap();
    def["name"] = json!("Fireball");
    let saved = ok(
        &w.dm,
        "PUT",
        "/api/spell-mechanics",
        json!({ "kind": "def", "spellKey": FIREBALL, "ruleset": ruleset, "def": def }),
    )
    .await;
    let rev = saved["row"]["rev"].as_i64().unwrap();
    let replayed = call(
        w,
        &w.dm,
        "sim_replay",
        json!({ "encounterId": tower, "seed": 3, "opts": opts,
                "transcript": report["transcript"], "fingerprints": report["fingerprints"] }),
    )
    .await
    .unwrap();
    assert_eq!(replayed["identical"], false, "{replayed}");
    assert!(
        replayed["divergence"]["fingerprints"]
            .to_string()
            .contains(&key),
        "{replayed}"
    );
    let again = call(
        w,
        &w.dm,
        "sim_start",
        json!({ "encounterId": tower, "seed": 3, "opts": opts }),
    )
    .await
    .unwrap();
    let sim = again["simId"].as_str().unwrap().to_string();
    let report = call(w, &w.dm, "sim_report", json!({ "simId": sim }))
        .await
        .unwrap();
    assert_eq!(
        report["fingerprints"][&key],
        format!("def:{FIREBALL}:{rev}")
    );
    simulate::clear_sessions();
}

/// D16 over the tool table: no difficulty words in names, titles, descriptions or schemas.
#[test]
fn the_tool_table_has_no_difficulty_words() {
    let defs = simulate::tool_defs();
    assert_eq!(defs.len(), 8);
    for d in defs {
        let text = format!("{} {} {} {}", d.name, d.title, d.description, d.schema).to_lowercase();
        for w in [
            "xp",
            "challenge rating",
            "difficulty",
            "easy",
            "medium",
            "hard",
            "deadly",
            "balanced",
            "score",
        ] {
            let bad = text.match_indices(w).any(|(i, _)| {
                let before = text[..i]
                    .chars()
                    .last()
                    .is_none_or(|c| !c.is_ascii_alphanumeric());
                let after = text[i + w.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_ascii_alphanumeric());
                before && after
            });
            assert!(
                !bad || text.contains("never rate difficulty") && w == "difficulty",
                "{}: {w}",
                d.name
            );
        }
    }
}

fn state_in(dir: &Path, db: Db) -> AppState {
    state_with(dir.to_path_buf(), db, Setup::default())
}

#[tokio::test(flavor = "multi_thread")]
async fn simulations_read_copies_only_and_only_for_the_dm() {
    let _one = SIM_TESTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let w = world(state_in(
        dir.path(),
        Db::open(&Location::Memory).await.unwrap(),
    ))
    .await;
    whole_run(&w).await;
    only_the_dm(&w).await;
    spell_casters(&w).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn simulations_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    let _one = super::POSTGRES.lock().await;
    let _two = SIM_TESTS.lock().await;
    let dir = tempfile::tempdir().unwrap();
    drop_all(&url).await;
    let db = Db::open(&Location::Postgres(url.clone())).await.unwrap();
    let w = world(state_in(dir.path(), db)).await;
    whole_run(&w).await;
    only_the_dm(&w).await;
    spell_casters(&w).await;
    drop_all(&url).await;
}
