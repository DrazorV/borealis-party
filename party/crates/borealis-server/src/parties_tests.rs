//! Parties end to end: the role matrix (every route against the DM, members, invitees, outsiders, pending and
//! signed-out callers), the invite and join flows, the upgrade from the 0.1.4 schema and the schema itself.
//!
//! Runs on SQLite, and on PostgreSQL too when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.

use std::path::Path;

use axum::http::StatusCode;
use axum::Router;
use serde_json::{json, Value};
use sqlx::Row;

use super::{
    drop_all, make_old, new_character, register, router_of, router_on, router_with, send, sign_in,
    sign_up, state_with, Client, Setup, PASSWORD,
};
use crate::db::{Db, Location};

/// A saved character of `client`'s; its id.
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

async fn dm_on(client: &Client) {
    let (status, out) = client.post("/api/me/roles/dm", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["dm"], true);
}

async fn new_party(client: &Client, name: &str) -> String {
    let (status, out) = client.post("/api/parties", json!({ "name": name })).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    out["id"].as_str().unwrap().to_string()
}

async fn invite_name(dm: &Client, party: &str, username: &str) -> (StatusCode, Value) {
    dm.post(
        &format!("/api/parties/{party}/invites"),
        json!({ "kind": "username", "username": username }),
    )
    .await
}

async fn invite_code(dm: &Client, party: &str, uses: i64) -> (String, String) {
    let (status, out) = dm
        .post(
            &format!("/api/parties/{party}/invites"),
            json!({ "kind": "code", "uses": uses }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    (
        out["invite"]["id"].as_str().unwrap().to_string(),
        out["code"].as_str().unwrap().to_string(),
    )
}

/// The id of the open invite `client` sees for the party named `party`.
async fn my_invite(client: &Client, party: &str) -> String {
    let (status, out) = client.get("/api/invites").await;
    assert_eq!(status, StatusCode::OK, "{out}");
    out["invites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["party"]["name"] == party)
        .unwrap_or_else(|| panic!("no invite to {party}: {out}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn accept(client: &Client, invite: &str, character: &str) -> (StatusCode, Value) {
    client
        .post(
            &format!("/api/invites/{invite}/accept"),
            json!({ "characterId": character }),
        )
        .await
}

/// A token for an account that isn't active: a session row made by hand, which the server must not honour.
async fn token_for_inactive(router: &Router, db: &Db, name: &str) -> String {
    let (status, out) = register(router, name, &format!("{name}@example.com")).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["status"], "pending", "{out}");
    let login = db.find_login(name).await.unwrap().unwrap();
    let token = format!("inactive-{name}");
    db.insert_session(
        &crate::auth::token_hash(&token),
        &login.user.id,
        crate::db::now() + 3600,
    )
    .await
    .unwrap();
    token
}

/// Who calls.
#[derive(Clone, Copy, Debug)]
enum Who {
    Dm,
    Member,
    Other,
    Invitee,
    Outsider,
    Pending,
    Anonymous,
}

const EVERYONE: [Who; 7] = [
    Who::Dm,
    Who::Member,
    Who::Other,
    Who::Invitee,
    Who::Outsider,
    Who::Pending,
    Who::Anonymous,
];

struct Cast {
    router: Router,
    dm: Client,
    member: Client,
    other: Client,
    invitee: Client,
    outsider: Client,
    pending: String,
}

impl Cast {
    fn token(&self, who: Who) -> Option<&str> {
        match who {
            Who::Dm => Some(&self.dm.token),
            Who::Member => Some(&self.member.token),
            Who::Other => Some(&self.other.token),
            Who::Invitee => Some(&self.invitee.token),
            Who::Outsider => Some(&self.outsider.token),
            Who::Pending => Some(&self.pending),
            Who::Anonymous => None,
        }
    }

    async fn call(&self, who: Who, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        send(&self.router, self.token(who), method, path, body).await
    }
}

/// The role matrix: each route against each caller. Routes that change something are asked by everyone who may
/// not first, and the party must look the same afterwards; their own callers are covered by the flows.
async fn role_matrix(router: Router, db: Db) {
    let dm = sign_up(&router, "matrix-dm").await;
    let member = sign_up(&router, "matrix-member").await;
    let other = sign_up(&router, "matrix-other").await;
    let invitee = sign_up(&router, "matrix-invitee").await;
    let outsider = sign_up(&router, "matrix-outsider").await;
    // Accounts made from here on wait for approval.
    let admin = sign_in(&router, "matrix-dm").await;
    let (status, out) = admin
        .patch("/api/admin/settings", json!({ "requireApproval": true }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let pending = token_for_inactive(&router, &db, "matrix-pending").await;
    let cast = Cast {
        router: router.clone(),
        dm,
        member,
        other,
        invitee,
        outsider,
        pending,
    };
    dm_on(&cast.dm).await;
    let party = new_party(&cast.dm, "Matrix Party").await;
    let dm_pc = character(&cast.dm, "Dm Pc").await;
    let (status, out) = cast
        .dm
        .post(
            &format!("/api/parties/{party}/members"),
            json!({ "characterId": dm_pc }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let member_pc = character(&cast.member, "Member Pc").await;
    let other_pc = character(&cast.other, "Other Pc").await;
    let invitee_pc = character(&cast.invitee, "Invitee Pc").await;
    let outsider_pc = character(&cast.outsider, "Outsider Pc").await;
    for (client, pc) in [(&cast.member, &member_pc), (&cast.other, &other_pc)] {
        let (status, _) = invite_name(&cast.dm, &party, &client_name(&cast, client)).await;
        assert_eq!(status, StatusCode::OK);
        let invite = my_invite(client, "Matrix Party").await;
        let (status, out) = accept(client, &invite, pc).await;
        assert_eq!(status, StatusCode::OK, "{out}");
    }
    // The invitee is invited and has an offer pending.
    invite_name(&cast.dm, &party, "matrix-invitee").await;
    let invite = my_invite(&cast.invitee, "Matrix Party").await;
    let (status, out) = cast
        .invitee
        .post(
            &format!("/api/invites/{invite}/offer"),
            json!({ "characterId": invitee_pc }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let offer = out["offer"]["id"].as_str().unwrap().to_string();
    let (code_invite, code) = invite_code(&cast.dm, &party, 3).await;
    let before = cast.dm.get(&format!("/api/parties/{party}")).await.1;
    let players_before = cast
        .other
        .get(&format!("/api/parties/{party}/roster"))
        .await
        .1;

    let m = &cast.member.id;
    let o = &cast.other.id;
    let unauth = StatusCode::UNAUTHORIZED;
    let ok = StatusCode::OK;
    let nf = StatusCode::NOT_FOUND;
    // (method, path, body, expected for Dm, Member, Other, Invitee, Outsider, Pending, Anonymous)
    let rows: Vec<(&str, String, Value, [StatusCode; 7])> = vec![
        // Reads.
        (
            "GET",
            "/api/me/roles".into(),
            Value::Null,
            [ok, ok, ok, ok, ok, unauth, unauth],
        ),
        (
            "GET",
            "/api/parties".into(),
            Value::Null,
            [ok, ok, ok, ok, ok, unauth, unauth],
        ),
        (
            "GET",
            format!("/api/parties/{party}"),
            Value::Null,
            [ok, ok, ok, nf, nf, unauth, unauth],
        ),
        (
            "GET",
            format!("/api/parties/{party}/roster"),
            Value::Null,
            [ok, ok, ok, nf, nf, unauth, unauth],
        ),
        // The DM's read-only view of a member character (0.2.1): the DM only, its owner included.
        (
            "POST",
            format!("/api/parties/{party}/members/{m}/{member_pc}/view/snapshot"),
            json!({}),
            [ok, nf, nf, nf, nf, unauth, unauth],
        ),
        (
            "POST",
            format!("/api/parties/{party}/members/{o}/{other_pc}/view/snapshot"),
            json!({ "characterId": outsider_pc, "path": "../x" }),
            [ok, nf, nf, nf, nf, unauth, unauth],
        ),
        (
            "GET",
            "/api/invites".into(),
            Value::Null,
            [ok, ok, ok, ok, ok, unauth, unauth],
        ),
        // The DM's routes: everyone else gets 404, the members of the party too.
        (
            "PATCH",
            format!("/api/parties/{party}"),
            json!({ "name": "Taken Over" }),
            [ok, nf, nf, nf, nf, unauth, unauth],
        ),
        (
            "POST",
            format!("/api/parties/{party}/invites"),
            json!({ "kind": "username", "username": "nobody-here" }),
            [ok, nf, nf, nf, nf, unauth, unauth],
        ),
        (
            "POST",
            format!("/api/parties/{party}/members"),
            json!({ "characterId": outsider_pc }),
            [StatusCode::BAD_REQUEST, nf, nf, nf, nf, unauth, unauth],
        ),
        (
            "DELETE",
            format!("/api/parties/{party}/invites/{code_invite}"),
            Value::Null,
            [StatusCode::OK, nf, nf, nf, nf, unauth, unauth],
        ),
        (
            "DELETE",
            format!("/api/parties/{party}/offers/{offer}"),
            Value::Null,
            [StatusCode::OK, nf, nf, nf, nf, unauth, unauth],
        ),
        (
            "POST",
            format!("/api/parties/{party}/offers/{offer}/accept"),
            json!({}),
            [ok, nf, nf, nf, nf, unauth, unauth],
        ),
        (
            "DELETE",
            format!("/api/parties/{party}"),
            Value::Null,
            [ok, nf, nf, nf, nf, unauth, unauth],
        ),
        // A member's character: its owner or the DM only.
        (
            "DELETE",
            format!("/api/parties/{party}/members/{m}/{member_pc}"),
            Value::Null,
            [ok, ok, nf, nf, nf, unauth, unauth],
        ),
        (
            "DELETE",
            format!("/api/parties/{party}/members/{o}/{other_pc}"),
            Value::Null,
            [ok, nf, ok, nf, nf, unauth, unauth],
        ),
        // The invitee's invite: nobody else's to act on, the DM's included.
        (
            "POST",
            format!("/api/invites/{invite}/accept"),
            json!({ "characterId": invitee_pc }),
            [nf, nf, nf, ok, nf, unauth, unauth],
        ),
        (
            "POST",
            format!("/api/invites/{invite}/offer"),
            json!({ "characterId": invitee_pc }),
            [nf, nf, nf, StatusCode::CONFLICT, nf, unauth, unauth],
        ),
        (
            "DELETE",
            format!("/api/invites/{invite}/offer"),
            Value::Null,
            [nf, nf, nf, ok, nf, unauth, unauth],
        ),
        (
            "POST",
            format!("/api/invites/{invite}/decline"),
            json!({}),
            [nf, nf, nf, ok, nf, unauth, unauth],
        ),
        (
            "POST",
            "/api/join".into(),
            json!({ "code": "0000-0000-0000-0000" }),
            [nf, nf, nf, nf, nf, unauth, unauth],
        ),
        (
            "POST",
            "/api/me/roles/dm".into(),
            json!({}),
            [ok, ok, ok, ok, ok, unauth, unauth],
        ),
        (
            "DELETE",
            "/api/me/roles/dm".into(),
            Value::Null,
            [StatusCode::CONFLICT, ok, ok, ok, ok, unauth, unauth],
        ),
        // Everyone else switched the capability off again in the row above.
        (
            "POST",
            "/api/parties".into(),
            json!({ "name": "x" }),
            [
                ok,
                StatusCode::FORBIDDEN,
                StatusCode::FORBIDDEN,
                StatusCode::FORBIDDEN,
                StatusCode::FORBIDDEN,
                unauth,
                unauth,
            ],
        ),
    ];
    // Writes the matrix only refuses: the allowed caller's column is skipped (marked by being first allowed).
    let writes_skip_allowed = ["PATCH", "POST", "DELETE"];
    for (method, path, body, expected) in &rows {
        for (index, who) in EVERYONE.iter().enumerate() {
            let want = expected[index];
            let mutating = writes_skip_allowed.contains(method);
            // Allowed writes change the party: they run in the flows instead, except the harmless ones.
            if mutating && want.is_success() && !harmless(method, path) {
                continue;
            }
            if mutating && want == StatusCode::CONFLICT {
                continue;
            }
            let (status, out) = cast.call(*who, method, path, body.clone()).await;
            assert_eq!(status, want, "{who:?} {method} {path}: {out}");
            if status == StatusCode::NOT_FOUND {
                // One answer for everything not visible, so nothing tells a missing party from another's.
                assert_eq!(out["code"], "not_found", "{out}");
                assert_eq!(
                    out["message"], "There is no such party, invite or character here.",
                    "{who:?} {method} {path}"
                );
            }
        }
    }
    // Nothing the matrix refused changed the party, nor moved what its players see. The DM's view moved once: the
    // one invite the matrix lets the DM send.
    let after = cast.dm.get(&format!("/api/parties/{party}")).await.1;
    assert_eq!(
        after["pseq"].as_u64().unwrap(),
        before["pseq"].as_u64().unwrap() + 1,
        "only the allowed invite moved the DM's view"
    );
    let players_after = cast
        .other
        .get(&format!("/api/parties/{party}/roster"))
        .await
        .1;
    assert_eq!(
        players_after["pseq"], players_before["pseq"],
        "a refused call moved the players' view"
    );
    assert_eq!(
        strip_times(before.clone()),
        strip_times(after),
        "refused calls changed nothing"
    );
    // What each caller sees of the roster.
    let (_, seen) = cast.member.get(&format!("/api/parties/{party}")).await;
    assert!(
        seen.get("invites").is_none() && seen.get("offers").is_none(),
        "{seen}"
    );
    assert_eq!(seen["role"], "player");
    let members = seen["members"].as_array().unwrap();
    assert_eq!(members.len(), 3, "{seen}");
    for card in members {
        let mine = card["characterId"] == member_pc.as_str();
        assert_eq!(card["mine"], mine);
        assert_eq!(
            card.get("ownerId").is_some(),
            mine,
            "only the DM sees other owners' ids: {card}"
        );
        assert_eq!(card["state"], "ok");
        assert!(card["name"].is_string() && card["level"].is_u64(), "{card}");
    }
    let (_, seen) = cast.dm.get(&format!("/api/parties/{party}")).await;
    assert_eq!(seen["role"], "dm");
    assert!(seen["members"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["ownerId"].is_string()));
    assert_eq!(seen["myCharacters"], json!([dm_pc]), "{seen}");
    assert_eq!(seen["offers"].as_array().unwrap().len(), 1, "{seen}");
    // The outsider's list doesn't have it.
    let (_, list) = cast.outsider.get("/api/parties").await;
    assert!(list["parties"].as_array().unwrap().is_empty(), "{list}");
    // The preview needs no sign-in; a wrong code is the same 404.
    let (status, out) = send(
        &router,
        None,
        "POST",
        "/api/join/preview",
        json!({ "code": code }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["party"]["name"], "Matrix Party");
    let (status, _) = send(
        &router,
        None,
        "POST",
        "/api/join/preview",
        json!({ "code": "ZZZZ" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A NUL in an invite id is a 404 too, never a database error.
    for (method, path) in [
        ("POST", "/api/invites/%00/accept"),
        ("DELETE", "/api/invites/%00/offer"),
    ] {
        let (status, out) = cast
            .call(
                Who::Invitee,
                method,
                path,
                json!({ "characterId": invitee_pc }),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {out}");
    }
    // Ids from another party, or malformed, are the same 404 for the DM.
    let elsewhere = new_party(&cast.dm, "Other Table").await;
    for path in [
        format!("/api/parties/{elsewhere}/offers/{offer}/accept"),
        format!("/api/parties/{elsewhere}/invites/{code_invite}"),
        format!("/api/parties/{elsewhere}/members/{m}/{member_pc}"),
        format!("/api/parties/{party}/members/{m}/..%2F..%2Fx"),
        format!("/api/parties/{party}/members/{m}/c_%00"),
        "/api/parties/no-such-party".to_string(),
        "/api/parties/%00".to_string(),
        format!("/api/parties/{party}/invites/%00"),
        format!("/api/parties/{party}/offers/%00/accept"),
        format!("/api/parties/{party}/members/%00/{member_pc}"),
    ] {
        let method = if path.ends_with("accept") {
            "POST"
        } else {
            "DELETE"
        };
        let (status, out) = cast.call(Who::Dm, method, &path, json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {out}");
    }
}

/// Writes whose allowed caller can run in the matrix without spoiling the rows after them.
fn harmless(method: &str, path: &str) -> bool {
    (method == "POST"
        && (path.ends_with("/roles/dm") || path.ends_with("/invites") || path == "/api/parties"))
        || (method == "DELETE" && path.ends_with("/roles/dm"))
}

fn client_name(cast: &Cast, client: &Client) -> String {
    if client.id == cast.member.id {
        "matrix-member".into()
    } else {
        "matrix-other".into()
    }
}

/// A party detail without the times and renewed invites the matrix's allowed calls touch.
fn strip_times(mut detail: Value) -> Value {
    detail["updatedAt"] = Value::Null;
    detail["pseq"] = Value::Null;
    if let Some(invites) = detail["invites"].as_array_mut() {
        invites.retain(|i| i["username"] != "nobody-here");
        for i in invites {
            i["createdAt"] = Value::Null;
        }
    }
    detail
}

/// The flows: create, invite, join, offer, remove, leave, delete; the anti-enumeration and atomicity rules.
async fn party_flow(router: Router, data: &Path) {
    let dm = sign_up(&router, "flow-dm").await;
    let alice = sign_up(&router, "flow-alice").await;
    let bob = sign_up(&router, "flow-bob").await;
    let carol = sign_up(&router, "flow-carol").await;

    // Only DM-capable accounts create parties.
    let (status, out) = dm
        .post("/api/parties", json!({ "name": "Tuesday Party" }))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(out["code"], "dm_required");
    let (_, roles) = dm.get("/api/me/roles").await;
    assert_eq!(roles["dm"], false);
    dm_on(&dm).await;
    dm_on(&dm).await;
    let (status, out) = dm.post("/api/parties", json!({ "name": "  " })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    let (status, out) = dm
        .post(
            "/api/parties",
            json!({ "name": "Tuesday Party", "ruleset": "dnd3" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{out}");
    let party = new_party(&dm, "Tuesday Party").await;

    // A username invite answers the same whether the account exists or not.
    let (s1, real) = invite_name(&dm, &party, "flow-alice").await;
    let (s2, ghost) = invite_name(&dm, &party, "flow-nobody").await;
    assert_eq!((s1, s2), (StatusCode::OK, StatusCode::OK));
    let shape = |v: &Value| {
        let mut keys: Vec<String> = v["invite"].as_object().unwrap().keys().cloned().collect();
        keys.sort();
        (v["sent"].clone(), keys, v["invite"]["kind"].clone())
    };
    assert_eq!(shape(&real), shape(&ghost));
    // Sending again renews the same invite.
    let (_, again) = invite_name(&dm, &party, "FLOW-ALICE").await;
    assert_eq!(again["invite"]["id"], real["invite"]["id"]);

    // Alice joins with one of her characters; not with another account's, nor a malformed id.
    let mira = character(&alice, "Mira").await;
    let bobs = character(&bob, "Brann").await;
    let invite = my_invite(&alice, "Tuesday Party").await;
    let (_, mine) = alice.get("/api/invites").await;
    assert!(mine["invites"][0]["party"].get("id").is_none(), "{mine}");
    for bad in [
        bobs.as_str(),
        "..",
        "../c_0123",
        "c_..%2F",
        "c_0123456789abcdef0123456789abcde/",
        "c_0123456789abcdef0123456789abcde\\",
        "c_0123456789abcdef0123456789abcde\0",
        "C_0123456789ABCDEF0123456789ABCDEF",
        "c_00000000000000000000000000000000",
    ] {
        let (status, out) = accept(&alice, &invite, bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}: {out}");
    }
    let (status, out) = accept(&alice, &invite, &mira).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["party"]["role"], "player");
    assert_eq!(out["party"]["myCharacters"], json!([mira]));
    // Used: gone from her list, and from the DM's.
    let (status, _) = accept(&alice, &invite, &mira).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, detail) = dm.get(&format!("/api/parties/{party}")).await;
    let invites = detail["invites"].as_array().unwrap();
    assert!(
        invites.iter().all(|i| i["username"] != "flow-alice"),
        "{detail}"
    );
    assert!(
        invites.iter().any(|i| i["username"] == "flow-nobody"),
        "{detail}"
    );

    // A character is in one party at most.
    let other = new_party(&dm, "Other Party").await;
    let (_, code) = invite_code(&dm, &other, 5).await;
    let (status, out) = alice
        .post("/api/join", json!({ "code": code, "characterId": mira }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{out}");

    // A join code with one use is taken once, even by two at the same time.
    let (_, single) = invite_code(&dm, &other, 1).await;
    let carols = character(&carol, "Cass").await;
    let typed = single.to_lowercase().replace('-', " ");
    let (a, b) = tokio::join!(
        bob.post("/api/join", json!({ "code": single, "characterId": bobs })),
        carol.post("/api/join", json!({ "code": typed, "characterId": carols })),
    );
    let mut statuses = [a.0, b.0];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::NOT_FOUND],
        "{a:?} {b:?}"
    );
    // Whoever lost leaves; the other one is in.
    let (_, detail) = dm.get(&format!("/api/parties/{other}")).await;
    assert_eq!(detail["members"].as_array().unwrap().len(), 1, "{detail}");
    let joined = &detail["members"][0];
    let (winner, loser_pc) = if joined["characterId"] == bobs.as_str() {
        (&bob, carols.clone())
    } else {
        (&carol, bobs.clone())
    };
    let loser = if winner.id == bob.id { &carol } else { &bob };

    // The loser keeps the five-use code on the account first, then offers; the DM accepts.
    let (status, out) = loser.post("/api/join", json!({ "code": code })).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["invite"]["kind"], "code");
    let held = out["invite"]["id"].as_str().unwrap().to_string();
    let (status, out) = loser
        .post(
            &format!("/api/invites/{held}/offer"),
            json!({ "characterId": loser_pc }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let offer = out["offer"]["id"].as_str().unwrap().to_string();
    let (status, _) = loser
        .post(
            &format!("/api/invites/{held}/offer"),
            json!({ "characterId": loser_pc }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, mine) = loser.get("/api/invites").await;
    assert_eq!(mine["invites"][0]["offer"]["id"], offer.as_str(), "{mine}");
    // The DM sees the offered character's card, and can't accept it into a different party.
    let (_, detail) = dm.get(&format!("/api/parties/{other}")).await;
    assert_eq!(detail["offers"][0]["state"], "ok", "{detail}");
    let (status, _) = dm
        .post(
            &format!("/api/parties/{party}/offers/{offer}/accept"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, out) = dm
        .post(
            &format!("/api/parties/{other}/offers/{offer}/accept"),
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["characterId"], loser_pc.as_str());
    let (_, detail) = dm.get(&format!("/api/parties/{other}")).await;
    assert_eq!(detail["members"].as_array().unwrap().len(), 2);
    assert!(detail["offers"].as_array().unwrap().is_empty());

    // Decline: the invite leaves the invitee's list, the DM's list doesn't change.
    let (_, _) = invite_name(&dm, &party, "flow-carol").await;
    let before = dm.get(&format!("/api/parties/{party}")).await.1["invites"].clone();
    let carol_invite = my_invite(&carol, "Tuesday Party").await;
    let (status, out) = carol
        .post(&format!("/api/invites/{carol_invite}/decline"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, mine) = carol.get("/api/invites").await;
    assert!(mine["invites"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["party"]["name"] != "Tuesday Party"));
    let after = dm.get(&format!("/api/parties/{party}")).await.1["invites"].clone();
    assert_eq!(before, after);

    // The DM adds their own character (D4) and stays DM; is a player in another account's party.
    let dms = character(&dm, "Dara").await;
    let (status, out) = dm
        .post(
            &format!("/api/parties/{party}/members"),
            json!({ "characterId": dms }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["dmCharacter"], true);
    let (status, _) = dm
        .post(
            &format!("/api/parties/{party}/members"),
            json!({ "characterId": mira }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "only the DM's own characters"
    );
    dm_on(&alice).await;
    let alices = new_party(&alice, "Alice's Table").await;
    let (_, _) = invite_name(&alice, &alices, "flow-dm").await;
    let (status, out) = accept(&dm, &my_invite(&dm, "Alice's Table").await, &dms).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "Dara is in Tuesday Party: {out}"
    );
    let dms2 = character(&dm, "Dara Two").await;
    let (status, _) = accept(&dm, &my_invite(&dm, "Alice's Table").await, &dms2).await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = dm.get("/api/parties").await;
    let roles: Vec<(String, String)> = list["parties"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap().into(),
                p["role"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert!(
        roles.contains(&("Tuesday Party".into(), "dm".into())),
        "{list}"
    );
    assert!(
        roles.contains(&("Alice's Table".into(), "player".into())),
        "{list}"
    );
    let (_, there) = dm.get(&format!("/api/parties/{alices}")).await;
    assert!(there.get("invites").is_none(), "player view in Q: {there}");
    let (status, _) = dm
        .patch(&format!("/api/parties/{alices}"), json!({ "name": "Mine" }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A removed member gets 404 on the next fetch; one who leaves too.
    let (status, _) = dm
        .delete(&format!("/api/parties/{party}/members/{}/{mira}", alice.id))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = alice.get(&format!("/api/parties/{party}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = winner
        .delete(&format!(
            "/api/parties/{other}/members/{}/{}",
            winner.id,
            if winner.id == bob.id { &bobs } else { &carols }
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = winner.get(&format!("/api/parties/{other}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Mira is free again and joins elsewhere.
    let (status, out) = alice
        .post("/api/join", json!({ "code": code, "characterId": mira }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");

    // A file that went away shows as missing, and the character files were never touched by the party.
    let file = super::character_file(data, &alice.id, &mira);
    let sha = std::fs::read(&file).unwrap();
    let (_, detail) = dm.get(&format!("/api/parties/{other}/roster")).await;
    assert!(detail["members"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["characterId"] == mira.as_str()));
    assert_eq!(std::fs::read(&file).unwrap(), sha);
    let (status, _) = alice
        .rpc("delete_character", json!({ "characterId": mira }))
        .await;
    assert_eq!(status, StatusCode::OK);
    // Deleting a character takes it out of its party (0.2.1; it showed as missing before).
    let (_, detail) = dm.get(&format!("/api/parties/{other}/roster")).await;
    assert!(
        detail["members"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["characterId"] != mira.as_str()),
        "{detail}"
    );

    // Renaming, the DM capability that can't go while parties run, and deleting.
    let (status, out) = dm
        .patch(
            &format!("/api/parties/{party}"),
            json!({ "name": "Wednesday", "ruleset": "dnd2014" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(
        (out["name"].as_str(), out["ruleset"].as_str()),
        (Some("Wednesday"), Some("dnd2014"))
    );
    let (status, _) = dm.delete("/api/me/roles/dm").await;
    assert_eq!(status, StatusCode::CONFLICT);
    for p in [&party, &other] {
        let (status, _) = dm.delete(&format!("/api/parties/{p}")).await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, out) = dm.delete("/api/me/roles/dm").await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["dm"], false);
    let (status, _) = dm.post("/api/parties", json!({ "name": "Again" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Its own character files are still there.
    assert!(super::character_file(data, &dm.id, &dms).is_file());
    // Twenty parties at most.
    dm_on(&dm).await;
    for n in 0..crate::parties::MAX_PARTIES {
        new_party(&dm, &format!("P{n}")).await;
    }
    let (status, _) = dm
        .post("/api/parties", json!({ "name": "One too many" }))
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

/// D39: a code typed at sign-up stays on an account that waits for approval, and shows once it is approved.
async fn pending_keeps_its_code(router: Router) {
    let dm = sign_up(&router, "d39-dm").await;
    dm_on(&dm).await;
    let party = new_party(&dm, "Waiting Room").await;
    let (_, code) = invite_code(&dm, &party, 2).await;
    let (status, out) = dm
        .patch("/api/admin/settings", json!({ "requireApproval": true }))
        .await;
    assert_eq!(status, StatusCode::OK, "admin: {out}");
    let (status, out) = send(
        &router,
        None,
        "POST",
        "/api/auth/register",
        json!({ "username": "d39-new", "email": "d39-new@example.com", "password": PASSWORD, "joinCode": code }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["status"], "pending", "an invite is no approval: {out}");
    let (status, out) = send(
        &router,
        None,
        "POST",
        "/api/join/preview",
        json!({ "code": code }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(out["party"]["name"], "Waiting Room");
    // A bad code never fails a sign-up.
    let (status, out) = send(
        &router,
        None,
        "POST",
        "/api/auth/register",
        json!({ "username": "d39-bad", "email": "d39-bad@example.com", "password": PASSWORD, "joinCode": "nope" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, list) = dm.get("/api/admin/accounts?search=d39-new").await;
    let id = list["items"][0]["id"].as_str().unwrap().to_string();
    let (status, _) = dm
        .post(
            &format!("/api/admin/accounts/{id}/status"),
            json!({ "status": "active" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let newbie = sign_in(&router, "d39-new").await;
    let invite = my_invite(&newbie, "Waiting Room").await;
    let pc = character(&newbie, "Newbie").await;
    let (status, out) = accept(&newbie, &invite, &pc).await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, detail) = dm.get(&format!("/api/parties/{party}")).await;
    assert_eq!(detail["invites"][0]["usesLeft"], 1, "{detail}");
    let (status, out) = dm
        .patch("/api/admin/settings", json!({ "requireApproval": false }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn parties_role_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    role_matrix(router_on(dir.path().join("data"), db.clone()), db).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn parties_invites_joins_and_limits() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    party_flow(super::router(data.clone()).await, &data).await;
}

/// Events waiting on a subscription.
fn drain(sub: &mut crate::hub::Subscription) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(m) = sub.rx.try_recv() {
        out.push(m.payload);
    }
    out
}

/// The party's edition (0.3.2): a party made without a ruleset still gets 2024 (API clients before 0.3.2), one made
/// with a ruleset keeps it, and the DM's PATCH changes it and tells the DM and the players, so open encounter
/// screens read it again.
#[tokio::test(flavor = "multi_thread")]
async fn changing_the_edition_updates_the_party_and_tells_everyone() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&Location::Memory).await.unwrap();
    let state = state_with(dir.path().join("data"), db, Setup::default());
    let router = router_of(state.clone(), Setup::default().max_upload);
    sign_up(&router, "ed-admin").await;
    let dm = sign_up(&router, "ed-dm").await;
    let player = sign_up(&router, "ed-player").await;
    dm_on(&dm).await;

    let (status, out) = dm
        .post("/api/parties", json!({ "name": "Old Client" }))
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["ruleset"], "dnd2024", "no ruleset: the old default");
    let (status, out) = dm
        .post(
            "/api/parties",
            json!({ "name": "Picked", "ruleset": "dnd2014" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(out["ruleset"], "dnd2014");
    let party = out["id"].as_str().unwrap().to_string();

    let hero = character(&player, "Hero").await;
    let (status, out) = invite_name(&dm, &party, "ed-player").await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let invite = my_invite(&player, "Picked").await;
    let (status, out) = accept(&player, &invite, &hero).await;
    assert_eq!(status, StatusCode::OK, "{out}");

    let mut dm_sub = state.hub.subscribe(&dm.id, "ed-dm", None);
    let mut player_sub = state.hub.subscribe(&player.id, "ed-player", None);
    drain(&mut dm_sub);
    drain(&mut player_sub);

    // A player can't change it.
    let (status, _) = player
        .patch(
            &format!("/api/parties/{party}"),
            json!({ "ruleset": "dnd2024" }),
        )
        .await;
    assert_ne!(status, StatusCode::OK);
    assert!(drain(&mut player_sub).is_empty());

    let (status, out) = dm
        .patch(
            &format!("/api/parties/{party}"),
            json!({ "ruleset": "dnd2024" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    assert_eq!(
        (out["ruleset"].as_str(), out["name"].as_str()),
        (Some("dnd2024"), Some("Picked"))
    );
    let (_, out) = player.get(&format!("/api/parties/{party}")).await;
    assert_eq!(out["ruleset"], "dnd2024", "the players see it too");
    for (who, sub) in [("DM", &mut dm_sub), ("player", &mut player_sub)] {
        let events = drain(sub);
        assert!(
            events
                .iter()
                .any(|e| e["party"] == party.as_str() && e["kind"] == "party"),
            "the {who} hears of it: {events:?}"
        );
    }

    let (status, _) = dm
        .patch(
            &format!("/api/parties/{party}"),
            json!({ "ruleset": "dnd5" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_join_code_survives_sign_up_while_pending() {
    let dir = tempfile::tempdir().unwrap();
    pending_keeps_its_code(super::router(dir.path().join("data")).await).await;
}

#[test]
fn the_kill_switch_removes_every_party_route() {
    assert!(!crate::parties::routes_if(false).has_routes());
    assert!(crate::parties::routes_if(true).has_routes());
    // Off by config, and on by default.
    let off = crate::config::Settings::parse(
        crate::config::DEFAULT_FILE,
        [(
            "BOREALIS__FEATURES__PARTIES".to_string(),
            "false".to_string(),
        )],
    )
    .unwrap();
    assert!(!off.features.parties);
    assert!(
        crate::config::Settings::parse(crate::config::DEFAULT_FILE, [])
            .unwrap()
            .features
            .parties
    );
}

#[test]
fn join_codes_and_character_ids() {
    use crate::parties::{normal_code, valid_character_id};
    assert_eq!(
        normal_code("k7qm-3xtr 9bwd-hn2f").as_deref(),
        Some("K7QM3XTR9BWDHN2F")
    );
    assert_eq!(
        normal_code("OIL0-0000-0000-0000").as_deref(),
        Some("0110000000000000")
    );
    assert_eq!(normal_code("K7QM-3XTR-9BWD-HN2"), None);
    assert_eq!(
        normal_code("K7QM-3XTR-9BWD-HN2U"),
        None,
        "U is not in the alphabet"
    );
    assert!(valid_character_id("c_0123456789abcdef0123456789abcdef"));
    for bad in [
        "",
        "c_",
        "c_0123456789abcdef0123456789abcdeF",
        "x_0123456789abcdef0123456789abcdef",
        "c_../../../etc",
    ] {
        assert!(!valid_character_id(bad), "{bad}");
    }
}

// The upgrade from the production schema and the schema itself.

/// The production schema of 0.1.4 (migration 1), with what a live server has: an admin, active, pending and disabled
/// accounts, sessions, a saved setting, the activity log, a saved character, and the groups table left behind.
async fn seed_0_1_4(location: &Location, url: &str, data: &Path) {
    let old = Db::open_at(location, 1).await.unwrap();
    let router = router_on(data.to_path_buf(), old.clone());
    let admin = sign_up(&router, "up-admin").await;
    let player = sign_up(&router, "up-player").await;
    sign_up(&router, "up-disabled").await;
    character(&player, "Old Hand").await;
    let (_, list) = admin.get("/api/admin/accounts?search=up-disabled").await;
    let id = list["items"][0]["id"].as_str().unwrap().to_string();
    let (status, _) = admin
        .post(
            &format!("/api/admin/accounts/{id}/status"),
            json!({ "status": "disabled" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    admin
        .patch("/api/admin/settings", json!({ "requireApproval": true }))
        .await;
    let (status, out) = register(&router, "up-pending", "up-pending@example.com").await;
    assert_eq!(
        (status, out["status"].as_str()),
        (StatusCode::OK, Some("pending"))
    );
    old.pool().close().await;
    make_old(url).await;
}

/// HEAD over the seeded database: sign-in works as before, and so do parties with the old characters.
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
    let admin = sign_in(&router, "up-admin").await;
    let player = sign_in(&router, "up-player").await;
    for (name, code) in [
        ("up-pending", "account_pending"),
        ("up-disabled", "account_disabled"),
    ] {
        let (status, out) = super::login(&router, name, PASSWORD).await;
        assert_eq!(
            (status, out["code"].as_str()),
            (StatusCode::FORBIDDEN, Some(code)),
            "{name}"
        );
    }
    let (_, stats) = admin.get("/api/admin/stats").await;
    assert_eq!(stats["accounts"]["total"], 4, "{stats}");
    let (_, log) = admin.get("/api/admin/activity").await;
    assert!(log["total"].as_i64().unwrap() > 0);
    let (_, list) = player.rpc("list_characters", json!({})).await;
    let old = list[0]["id"].as_str().unwrap().to_string();
    dm_on(&admin).await;
    let party = new_party(&admin, "After the Upgrade").await;
    invite_name(&admin, &party, "up-player").await;
    let (status, out) = accept(
        &player,
        &my_invite(&player, "After the Upgrade").await,
        &old,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{out}");
    let (_, roster) = admin.get(&format!("/api/parties/{party}/roster")).await;
    assert_eq!(roster["members"][0]["name"], "Old Hand", "{roster}");
    // Opening again changes nothing.
    Db::open(location).await.unwrap();
    // Deleting the DM's account takes the party and the other players' memberships with it; files stay.
    let (_, list) = admin.get("/api/admin/accounts?search=up-player").await;
    let player_id = list["items"][0]["id"].as_str().unwrap().to_string();
    let members: i64 = sqlx::query("SELECT COUNT(*) FROM party_members WHERE owner_id = $1")
        .bind(&player_id)
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(members, 1);
    let dm2 = sign_up_active(&router, &admin, "up-dm2").await;
    dm_on(&dm2).await;
    let p2 = new_party(&dm2, "Doomed").await;
    let (_, list) = admin.get("/api/admin/accounts?search=up-dm2").await;
    let dm2_id = list["items"][0]["id"].as_str().unwrap().to_string();
    assert!(db.delete_user(&dm2_id).await.unwrap());
    let left: i64 = sqlx::query("SELECT COUNT(*) FROM parties WHERE id = $1")
        .bind(&p2)
        .fetch_one(db.pool())
        .await
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(left, 0, "the DM's parties go with the account");
}

/// An account made active by the admin, on a server that requires approval.
async fn sign_up_active(router: &Router, admin: &Client, name: &str) -> Client {
    let (status, _) = register(router, name, &format!("{name}@example.com")).await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = admin
        .get(&format!("/api/admin/accounts?search={name}"))
        .await;
    let id = list["items"][0]["id"].as_str().unwrap().to_string();
    admin
        .post(
            &format!("/api/admin/accounts/{id}/status"),
            json!({ "status": "active" }),
        )
        .await;
    sign_in(router, name).await
}

#[tokio::test(flavor = "multi_thread")]
async fn parties_arrive_on_a_0_1_4_database() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("party.db");
    let data = dir.path().join("data");
    let location = Location::Sqlite(file.clone());
    let url = format!("sqlite://{}?mode=rwc", file.display());
    seed_0_1_4(&location, &url, &data).await;
    check_upgraded(&location, &data).await;
    // And the instance still moves to another database, parties and all.
    let moved = Db::open(&Location::Memory).await.unwrap();
    let report = crate::migrate::adopt_sqlite(&moved, &file)
        .await
        .unwrap()
        .unwrap();
    let text = crate::migrate::describe(&report);
    assert!(
        text.contains("parties 1,") && text.contains("party_members 1"),
        "{text}"
    );
    let router = router_on(data, moved);
    let admin = sign_in(&router, "up-admin").await;
    let (_, list) = admin.get("/api/parties").await;
    assert_eq!(list["parties"][0]["name"], "After the Upgrade", "{list}");
}

/// Every table but `schema_version` (and the groups table a database may have left over) is in the copy's list,
/// parents first, with every column, and a nullable number exactly where the list says `optint`.
async fn schema_matches_the_copy(db: &Db) {
    let sqlite = db.kind() == "sqlite";
    let tables: Vec<String> = sqlx::query(if sqlite {
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'"
    } else {
        "SELECT CAST(table_name AS TEXT) FROM information_schema.tables WHERE table_schema = current_schema()"
    })
    .fetch_all(db.pool())
    .await
    .unwrap()
    .iter()
    .map(|r| r.try_get::<String, _>(0).unwrap())
    .filter(|t| !["schema_version", "party_groups"].contains(&t.as_str()))
    .collect();
    let listed = crate::migrate::tables();
    let mut want: Vec<&str> = listed.iter().map(|(t, _)| *t).collect();
    let mut have: Vec<&str> = tables.iter().map(String::as_str).collect();
    want.sort_unstable();
    have.sort_unstable();
    assert_eq!(have, want, "migrate.rs TABLES lists every table");
    for (position, (table, columns)) in listed.iter().enumerate() {
        let found: Vec<(String, String, bool)> = if sqlite {
            sqlx::query(&format!("PRAGMA table_info({table})"))
                .fetch_all(db.pool())
                .await
                .unwrap()
                .iter()
                .map(|r| {
                    (
                        r.try_get::<String, _>(1).unwrap(),
                        r.try_get::<String, _>(2).unwrap().to_lowercase(),
                        r.try_get::<i64, _>(3).unwrap() == 0
                            && r.try_get::<i64, _>(5).unwrap() == 0,
                    )
                })
                .collect()
        } else {
            sqlx::query(
                "SELECT CAST(column_name AS TEXT), CAST(data_type AS TEXT), CAST(is_nullable AS TEXT) FROM information_schema.columns
                 WHERE table_schema = current_schema() AND table_name = $1",
            )
            .bind(*table)
            .fetch_all(db.pool())
            .await
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r.try_get::<String, _>(0).unwrap(),
                    r.try_get::<String, _>(1).unwrap().to_lowercase(),
                    r.try_get::<String, _>(2).unwrap() == "YES",
                )
            })
            .collect()
        };
        assert_eq!(found.len(), columns.len(), "{table}: {found:?}");
        for (name, kind) in columns {
            let (_, ty, nullable) = found
                .iter()
                .find(|(n, _, _)| n == name)
                .unwrap_or_else(|| panic!("{table}.{name} is not in the database"));
            match *kind {
                "text" => assert_eq!(ty, "text", "{table}.{name}"),
                "int" => assert!(
                    ty == "bigint" && !nullable,
                    "{table}.{name}: {ty} nullable={nullable}"
                ),
                _ => assert!(
                    ty == "bigint" && *nullable,
                    "{table}.{name}: {ty} nullable={nullable}"
                ),
            }
        }
        if sqlite {
            // Parents first: every table a foreign key points at comes earlier in the list.
            let parents = sqlx::query(&format!("PRAGMA foreign_key_list({table})"))
                .fetch_all(db.pool())
                .await
                .unwrap();
            for parent in parents {
                let parent: String = parent.try_get(2).unwrap();
                let at = listed.iter().position(|(t, _)| *t == parent).unwrap();
                assert!(at < position, "{table} comes after {parent}");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_copy_lists_every_table_and_column() {
    schema_matches_the_copy(&Db::open(&Location::Memory).await.unwrap()).await;
}

/// All of the above on PostgreSQL. Runs when `BOREALIS_TEST_POSTGRES_URL` names a database it may wipe.
#[tokio::test(flavor = "multi_thread")]
async fn parties_on_postgres() {
    let Ok(url) = std::env::var("BOREALIS_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set BOREALIS_TEST_POSTGRES_URL to run against PostgreSQL");
        return;
    };
    // Shares the database with the other PostgreSQL test: one at a time.
    let _one = super::POSTGRES.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let location = Location::Postgres(url.clone());
    drop_all(&url).await;
    seed_0_1_4(&location, &url, &data).await;
    check_upgraded(&location, &data).await;
    let db = Db::open(&location).await.unwrap();
    schema_matches_the_copy(&db).await;
    // Each flow on a database of its own, as on SQLite: the first account is the admin.
    drop_all(&url).await;
    let db = Db::open(&location).await.unwrap();
    role_matrix(router_on(dir.path().join("data2"), db.clone()), db).await;
    drop_all(&url).await;
    let db = Db::open(&location).await.unwrap();
    party_flow(
        router_on(dir.path().join("data3"), db),
        &dir.path().join("data3"),
    )
    .await;
    drop_all(&url).await;
    let db = Db::open(&location).await.unwrap();
    pending_keeps_its_code(router_with(dir.path().join("data4"), db, Setup::default())).await;
    drop_all(&url).await;
}
