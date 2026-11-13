//! Live party updates, multiplexed into each account's `GET /api/events` stream (plan 3.7), and the roster card
//! cache.
//!
//! * **Subscribers** are the open event streams, each known by its account and the hash of the session that opened
//!   it. Signing out, a new password, a disabled or deleted account close the streams of that session or account at
//!   once ([`PartyHub::close_session`], [`PartyHub::close_user`]); a stream also re-checks its session now and then.
//! * **Party events are invalidations only:** `{ party, pseq, kind }`. The client refetches its own projection.
//!   There are two audiences per party, each with its own sequence: the DM's, and the players'. The players' only
//!   moves when what players see changes ([`card_view_for`] for a member), so damage that keeps a character in the
//!   same health band, an invite or an offer moves nothing a player can count or time.
//! * **Who gets an event** is decided by the caller from the database at send time (`parties.rs`): the DM of the
//!   party now, and the owners of its member characters now. A removed member or a deleted party gets nothing more.
//! * **Every event a stream sends has an id**, counted per account. `?after=<id>` on a new stream sends a resync when
//!   the account missed anything; a stream that falls behind gets a resync instead of a silent drop.
//!
//! **Cards** are what the roster shows of a member character, made from the owner's session or saved file
//! ([`Card::from_snapshot`]) whenever the owner plays it, and kept in memory and in
//! `<data>/party-cards/<owner>/<character>.json`, outside every account's Borealis folder, so the roster never opens
//! a workspace to show them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::{mpsc, watch};

/// A party's view changed: `{ party, pseq, kind }`. Refetch the party.
pub const PARTY_EVENT: &str = "party://party";
/// The account's own party list or invites changed: `{ kind }` (`parties` or `invites`). Refetch them.
pub const MINE_EVENT: &str = "party://mine";
/// Events may have been missed: refetch everything shown.
pub const RESYNC_EVENT: &str = "party://resync";

/// Party events one stream may have waiting before it counts as behind (it then gets a resync).
const QUEUE: usize = 64;

/// Who an event is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audience {
    /// The party's DM.
    Dm,
    /// The owners of its member characters.
    Players,
}

/// One party event for one stream.
#[derive(Debug, Clone)]
pub struct Message {
    /// The account's event id.
    pub id: u64,
    /// The event name.
    pub event: &'static str,
    /// Its payload.
    pub payload: Value,
}

struct Subscriber {
    user: String,
    session: String,
    tx: mpsc::Sender<Message>,
    lagged: Arc<AtomicBool>,
    close: watch::Sender<bool>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Seqs {
    dm: u64,
    players: u64,
}

#[derive(Default)]
struct Inner {
    next: u64,
    subscribers: HashMap<u64, Subscriber>,
    /// The last event id sent to each account.
    sent: HashMap<String, u64>,
    parties: HashMap<String, Seqs>,
}

/// The live layer of parties, shared by every route ([`crate::AppState`]).
pub struct PartyHub {
    inner: Mutex<Inner>,
    /// The roster cards.
    pub cards: CardCache,
}

/// A stream's place in the hub; it leaves when this is dropped.
pub struct Subscription {
    hub: Arc<PartyHub>,
    id: u64,
    user: String,
    /// Party events for this stream.
    pub rx: mpsc::Receiver<Message>,
    /// Set when party events were dropped because the stream fell behind.
    pub lagged: Arc<AtomicBool>,
    /// Turns true when the stream must end (its session or account was revoked).
    pub closed: watch::Receiver<bool>,
    /// Ask for a resync first: the client missed events since the id it gave.
    pub resync: bool,
}

impl Subscription {
    /// The account the stream is for.
    pub fn user(&self) -> &str {
        &self.user
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.hub.lock().subscribers.remove(&self.id);
    }
}

impl PartyHub {
    /// A hub keeping its cards under `cards_dir`.
    pub fn new(cards_dir: PathBuf) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            cards: CardCache::new(cards_dir),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Register a stream of `user`'s, opened with the session whose hash is `session`. `after`: the last event id the
    /// client saw, if it saw any.
    pub fn subscribe(
        self: &Arc<Self>,
        user: &str,
        session: &str,
        after: Option<u64>,
    ) -> Subscription {
        let (tx, rx) = mpsc::channel(QUEUE);
        let (close, closed) = watch::channel(false);
        let lagged = Arc::new(AtomicBool::new(false));
        let mut inner = self.lock();
        inner.next += 1;
        let id = inner.next;
        let sent = inner.sent.get(user).copied().unwrap_or(0);
        inner.subscribers.insert(
            id,
            Subscriber {
                user: user.to_string(),
                session: session.to_string(),
                tx,
                lagged: Arc::clone(&lagged),
                close,
            },
        );
        Subscription {
            hub: Arc::clone(self),
            id,
            user: user.to_string(),
            rx,
            lagged,
            closed,
            resync: after.is_some_and(|after| after != sent),
        }
    }

    /// The last event id sent to `user`.
    pub fn last_id(&self, user: &str) -> u64 {
        self.lock().sent.get(user).copied().unwrap_or(0)
    }

    /// Send an event to every stream of `user`'s. Nothing is kept for an account with no stream open.
    pub fn deliver(&self, user: &str, event: &'static str, payload: &Value) {
        let mut inner = self.lock();
        if !inner.subscribers.values().any(|s| s.user == user) {
            return;
        }
        let id = {
            let sent = inner.sent.entry(user.to_string()).or_insert(0);
            *sent += 1;
            *sent
        };
        for s in inner.subscribers.values().filter(|s| s.user == user) {
            let message = Message {
                id,
                event,
                payload: payload.clone(),
            };
            if s.tx.try_send(message).is_err() {
                s.lagged.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Move a party's sequences: the DM's, and the players' when `players`. The new values.
    pub fn bump(&self, party: &str, players: bool) -> (u64, Option<u64>) {
        let mut inner = self.lock();
        let seqs = inner.parties.entry(party.to_string()).or_default();
        seqs.dm += 1;
        if players {
            seqs.players += 1;
        }
        (seqs.dm, players.then_some(seqs.players))
    }

    /// A party's current sequence for an audience (what a fetch of its view is as new as).
    pub fn seq(&self, party: &str, audience: Audience) -> u64 {
        let inner = self.lock();
        let seqs = inner.parties.get(party).copied().unwrap_or_default();
        match audience {
            Audience::Dm => seqs.dm,
            Audience::Players => seqs.players,
        }
    }

    /// A deleted party.
    pub fn forget_party(&self, party: &str) {
        self.lock().parties.remove(party);
    }

    /// True while `user` has a stream open.
    pub fn online(&self, user: &str) -> bool {
        self.lock().subscribers.values().any(|s| s.user == user)
    }

    /// End every stream of the account.
    pub fn close_user(&self, user: &str) {
        self.close_where(|s| s.user == user);
    }

    /// End the streams opened with one session.
    pub fn close_session(&self, session: &str) {
        self.close_where(|s| s.session == session);
    }

    /// End the account's streams of every session but `keep`.
    pub fn close_sessions_except(&self, user: &str, keep: &str) {
        self.close_where(|s| s.user == user && s.session != keep);
    }

    fn close_where(&self, which: impl Fn(&Subscriber) -> bool) {
        let mut inner = self.lock();
        inner.subscribers.retain(|_, s| {
            if which(s) {
                let _ = s.close.send(true);
                false
            } else {
                true
            }
        });
    }

    /// Tests: streams open for `user`.
    #[cfg(test)]
    pub fn streams_of(&self, user: &str) -> usize {
        self.lock()
            .subscribers
            .values()
            .filter(|s| s.user == user)
            .count()
    }
}

// Cards.

/// One class of a character, as the roster shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassLine {
    /// The class name.
    pub name: String,
    /// Levels in it.
    pub level: u32,
}

/// A spellcasting class's spell save DC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpellDc {
    /// The class.
    pub class: String,
    /// Its spell save DC.
    pub dc: i32,
}

/// What a DM's roster shows at the table. Only the DM and the owner ever see it ([`card_view_for`]).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    /// Current hit points.
    pub hp: u32,
    /// Hit point maximum.
    pub hp_max: u32,
    /// Temporary hit points.
    pub temp_hp: u32,
    /// Armor class.
    pub armor_class: i32,
    /// Initiative bonus.
    pub initiative: i32,
    /// Walking speed in feet.
    pub speed: i32,
    /// Passive Perception.
    pub passive_perception: i32,
    /// Spell save DCs, one per spellcasting class.
    pub spell_dcs: Vec<SpellDc>,
    /// Exhaustion level.
    pub exhaustion: u32,
    /// The spell being concentrated on.
    pub concentration: Option<String>,
    /// Has Heroic Inspiration.
    pub inspiration: bool,
}

/// A member character's card: the identity everyone in the party may see, and the stats only the DM and the owner
/// may.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Card {
    /// The character's name.
    pub name: Option<String>,
    /// Total level.
    pub level: Option<u32>,
    /// Edition, as upstream names it.
    pub ruleset: Option<Value>,
    /// Classes, main class first.
    pub classes: Vec<ClassLine>,
    /// None until the character was read with its content (it then has numbers to show).
    pub stats: Option<Stats>,
    /// When these were read, in seconds.
    pub read_at: Option<i64>,
}

impl Card {
    /// From upstream's snapshot of the character, as its owner's session or saved file has it now.
    pub fn from_snapshot(snap: &borealis_lib::dto::CharacterSnapshot) -> Self {
        let play = &snap.play;
        let hp = |key: &str| {
            play.hit_points
                .get(key)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0)
        };
        let mut classes: Vec<(bool, ClassLine)> = snap
            .level
            .classes
            .iter()
            .filter(|c| c.level > 0)
            .filter_map(|c| {
                Some((
                    c.is_main,
                    ClassLine {
                        name: c.name.clone().filter(|n| !n.is_empty())?,
                        level: c.level,
                    },
                ))
            })
            .collect();
        classes.sort_by_key(|(main, _)| !*main);
        Self {
            name: Some(snap.name.clone()),
            level: Some(snap.level.total),
            ruleset: serde_json::to_value(snap.ruleset).ok(),
            classes: classes.into_iter().map(|(_, c)| c).collect(),
            stats: Some(Stats {
                hp: hp("current"),
                hp_max: hp("max"),
                temp_hp: hp("temp"),
                armor_class: snap.stats.armor_class,
                initiative: snap.stats.initiative,
                speed: snap.stats.speed,
                passive_perception: snap.stats.passive_perception,
                spell_dcs: snap
                    .spellcasting
                    .classes
                    .iter()
                    .map(|c| SpellDc {
                        class: c.class_name.clone(),
                        dc: c.save_dc,
                    })
                    .collect(),
                exhaustion: play
                    .exhaustion
                    .get("level")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(0),
                concentration: play
                    .concentration
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                inspiration: play.inspiration,
            }),
            read_at: Some(crate::db::now()),
        }
    }

    /// The same card, leaving out when it was read.
    fn same(&self, other: &Self) -> bool {
        Self {
            read_at: None,
            ..self.clone()
        } == Self {
            read_at: None,
            ..other.clone()
        }
    }
}

/// How hurt a character looks to the rest of the party (D6). None when its hit points aren't known.
pub fn health_band(card: &Card) -> Option<&'static str> {
    let stats = card.stats.as_ref()?;
    if stats.hp_max == 0 {
        return None;
    }
    Some(if stats.hp == 0 {
        "down"
    } else if stats.hp.saturating_mul(2) <= stats.hp_max {
        "bloodied"
    } else {
        "unhurt"
    })
}

/// Who looks at a member character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Viewer {
    /// The party's DM.
    Dm,
    /// The character's owner.
    Owner,
    /// Another player in the party; `in_combat` while the party is in a combat ([`crate::parties::in_combat`]).
    Member {
        /// The party is in a combat: the health band is shown.
        in_combat: bool,
    },
}

/// The one projection of a member character's card for a viewer (plan 3.5, D6). Every response that shows a card
/// goes through here. The DM and the owner get it all; another member gets the name, classes and level, the edition
/// and whether it can be read, and during a combat only, a health band (the user's D6 choice: no `band` field at all
/// outside combat).
pub fn card_view_for(viewer: Viewer, card: &Card, state: &str) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("name".into(), json!(card.name));
    out.insert("level".into(), json!(card.level));
    out.insert(
        "ruleset".into(),
        card.ruleset.clone().unwrap_or(Value::Null),
    );
    out.insert("classes".into(), json!(card.classes));
    out.insert("state".into(), json!(state));
    match viewer {
        Viewer::Dm | Viewer::Owner => {
            out.insert("stats".into(), json!(card.stats));
            out.insert("readAt".into(), json!(card.read_at));
        }
        Viewer::Member { in_combat } => {
            if in_combat {
                out.insert("band".into(), json!(health_band(card)));
            }
        }
    }
    out
}

/// What changed when a card was stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Changed {
    /// The DM's view of it.
    pub dm: bool,
    /// The other members' view of it.
    pub players: bool,
}

/// Cards by `(owner, character)`, in memory and on file.
pub struct CardCache {
    dir: PathBuf,
    cards: Mutex<HashMap<(String, String), Card>>,
    /// One refresh of an owner's cards at a time, so an older read never overwrites a newer one.
    refreshing: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// Account and character ids as the server makes them; nothing else is joined into a path.
fn plain(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl CardCache {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            cards: Mutex::new(HashMap::new()),
            refreshing: Mutex::new(HashMap::new()),
        }
    }

    fn map(&self) -> MutexGuard<'_, HashMap<(String, String), Card>> {
        self.cards
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn file(&self, owner: &str, character: &str) -> Option<PathBuf> {
        (plain(owner) && plain(character))
            .then(|| self.dir.join(owner).join(format!("{character}.json")))
    }

    /// Hold while refreshing `owner`'s cards.
    pub async fn refreshing(&self, owner: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut map = self
                .refreshing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(map.entry(owner.to_string()).or_default())
        };
        lock.lock_owned().await
    }

    /// The card, from memory or its file (blocking: reads a small file the first time).
    pub fn get(&self, owner: &str, character: &str) -> Option<Card> {
        let key = (owner.to_string(), character.to_string());
        if let Some(card) = self.map().get(&key) {
            return Some(card.clone());
        }
        let file = self.file(owner, character)?;
        let card: Card = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
        self.map().insert(key, card.clone());
        Some(card)
    }

    /// Store a card (blocking: writes its file when it changed), and say what changed for whom. `in_combat`: the
    /// party is in a combat, so the other members see the health band and a change of band is a change for them.
    pub fn put(&self, owner: &str, character: &str, card: Card, in_combat: bool) -> Changed {
        let member = Viewer::Member { in_combat };
        let before = self.get(owner, character);
        let changed = match &before {
            None => Changed {
                dm: true,
                players: true,
            },
            Some(old) if old.same(&card) => Changed::default(),
            Some(old) => Changed {
                dm: true,
                players: card_view_for(member, old, "ok") != card_view_for(member, &card, "ok"),
            },
        };
        if changed.dm {
            if let Some(file) = self.file(owner, character) {
                if let Err(e) = write_card(&file, &card) {
                    tracing::warn!("party: writing a card of {owner}: {e}");
                }
            }
            self.map()
                .insert((owner.to_string(), character.to_string()), card);
        }
        changed
    }

    /// A character that left its party (blocking: deletes its file).
    pub fn forget(&self, owner: &str, character: &str) {
        self.map()
            .remove(&(owner.to_string(), character.to_string()));
        if let Some(file) = self.file(owner, character) {
            let _ = std::fs::remove_file(file);
        }
    }

    /// A deleted account (blocking: deletes its folder of cards).
    pub fn forget_owner(&self, owner: &str) {
        self.map().retain(|(o, _), _| o != owner);
        if plain(owner) {
            let _ = std::fs::remove_dir_all(self.dir.join(owner));
        }
    }
}

/// Write a card beside its old copy and rename it over it, so a reader never sees half a file.
fn write_card(file: &Path, card: &Card) -> std::io::Result<()> {
    let dir = file.parent().unwrap_or(file);
    std::fs::create_dir_all(dir)?;
    let temp = dir.join(format!(".card-{}.tmp", uuid::Uuid::new_v4().simple()));
    let bytes = serde_json::to_vec(card).map_err(std::io::Error::other)?;
    std::fs::write(&temp, bytes)
        .and_then(|()| std::fs::rename(&temp, file))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(hp: u32, max: u32) -> Card {
        Card {
            name: Some("Aria".into()),
            level: Some(3),
            ruleset: Some(json!("dnd2024")),
            classes: vec![ClassLine {
                name: "Fighter".into(),
                level: 3,
            }],
            stats: Some(Stats {
                hp,
                hp_max: max,
                armor_class: 16,
                passive_perception: 12,
                ..Stats::default()
            }),
            read_at: Some(1),
        }
    }

    #[test]
    fn bands_follow_hit_points() {
        assert_eq!(health_band(&card(20, 20)), Some("unhurt"));
        assert_eq!(health_band(&card(11, 20)), Some("unhurt"));
        assert_eq!(health_band(&card(10, 20)), Some("bloodied"));
        assert_eq!(health_band(&card(1, 20)), Some("bloodied"));
        assert_eq!(health_band(&card(0, 20)), Some("down"));
        assert_eq!(health_band(&card(0, 0)), None);
        assert_eq!(health_band(&Card::default()), None);
    }

    #[test]
    fn another_member_sees_only_the_d6_fields() {
        let full = card(7, 20);
        // Outside combat: no band field at all.
        let member = card_view_for(Viewer::Member { in_combat: false }, &full, "ok");
        let keys: Vec<&str> = member.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["classes", "level", "name", "ruleset", "state"],
            "{member:?}"
        );
        let text = Value::Object(member).to_string();
        for secret in [
            "16",
            "armorClass",
            "stats",
            "hp",
            "readAt",
            "band",
            "bloodied",
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        // In combat: the band too, and still no numbers.
        let member = card_view_for(Viewer::Member { in_combat: true }, &full, "ok");
        let keys: Vec<&str> = member.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["band", "classes", "level", "name", "ruleset", "state"],
            "{member:?}"
        );
        assert_eq!(member["band"], "bloodied");
        let text = Value::Object(member).to_string();
        for secret in ["16", "armorClass", "stats", "hp", "readAt"] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        for viewer in [Viewer::Dm, Viewer::Owner] {
            let all = card_view_for(viewer, &full, "ok");
            assert_eq!(all["stats"]["armorClass"], 16);
            assert_eq!(all["stats"]["hp"], 7);
            assert!(all.get("band").is_none());
        }
    }

    #[test]
    fn the_players_view_moves_only_when_the_band_does() {
        let dir = tempfile::tempdir().unwrap();
        let cache = CardCache::new(dir.path().to_path_buf());
        let first = cache.put("o-1", "c_1", card(20, 20), true);
        assert_eq!(
            first,
            Changed {
                dm: true,
                players: true
            }
        );
        // The same numbers, read later: nothing changed.
        let mut again = card(20, 20);
        again.read_at = Some(99);
        assert_eq!(cache.put("o-1", "c_1", again, true), Changed::default());
        // Hurt, still unhurt: the DM's view only.
        assert_eq!(
            cache.put("o-1", "c_1", card(15, 20), true),
            Changed {
                dm: true,
                players: false
            }
        );
        // Bloodied in combat: both.
        assert_eq!(
            cache.put("o-1", "c_1", card(10, 20), true),
            Changed {
                dm: true,
                players: true
            }
        );
        // Outside combat the players see no band, so down is no change for them.
        assert_eq!(
            cache.put("o-1", "c_1", card(0, 20), false),
            Changed {
                dm: true,
                players: false
            }
        );
        assert_eq!(
            cache.put("o-1", "c_1", card(9, 20), true),
            Changed {
                dm: true,
                players: true
            }
        );
        // Kept on file: a new cache reads it back.
        let reread = CardCache::new(dir.path().to_path_buf());
        assert_eq!(reread.get("o-1", "c_1").unwrap().stats.unwrap().hp, 9);
        reread.forget("o-1", "c_1");
        assert!(CardCache::new(dir.path().to_path_buf())
            .get("o-1", "c_1")
            .is_none());
        // Ids that aren't the server's are never a path.
        assert!(cache.file("../x", "c_1").is_none());
        assert!(cache.file("o-1", "a/b").is_none());
    }

    #[tokio::test]
    async fn events_reach_only_their_account_and_revoked_streams_end() {
        let dir = tempfile::tempdir().unwrap();
        let hub = Arc::new(PartyHub::new(dir.path().to_path_buf()));
        let mut a1 = hub.subscribe("a", "s1", None);
        let mut a2 = hub.subscribe("a", "s2", None);
        let mut b = hub.subscribe("b", "s3", None);
        hub.deliver("a", MINE_EVENT, &json!({ "kind": "parties" }));
        assert_eq!(a1.rx.try_recv().unwrap().id, 1);
        assert_eq!(a2.rx.try_recv().unwrap().id, 1);
        assert!(b.rx.try_recv().is_err());
        // Nothing is counted for an account with no stream.
        hub.deliver("c", MINE_EVENT, &json!({}));
        assert_eq!(hub.last_id("c"), 0);
        // A reconnect that saw everything needs no resync; one that missed an event does.
        assert!(!hub.subscribe("a", "s1", Some(1)).resync);
        assert!(hub.subscribe("a", "s1", Some(0)).resync);
        assert!(!hub.subscribe("a", "s1", None).resync);
        // A stream that falls behind is told so.
        for _ in 0..=QUEUE {
            hub.deliver("b", MINE_EVENT, &json!({}));
        }
        assert!(b.lagged.load(Ordering::SeqCst));
        // Sessions and accounts close.
        hub.close_sessions_except("a", "s1");
        assert!(*a2.closed.borrow_and_update());
        assert!(!*a1.closed.borrow());
        hub.close_session("s1");
        assert!(*a1.closed.borrow_and_update());
        assert_eq!(hub.streams_of("a"), 0);
        hub.close_user("b");
        assert!(*b.closed.borrow_and_update());
        drop(b);
        assert_eq!(hub.streams_of("b"), 0);
    }

    #[test]
    fn party_sequences_are_per_audience() {
        let dir = tempfile::tempdir().unwrap();
        let hub = PartyHub::new(dir.path().to_path_buf());
        assert_eq!(hub.bump("p", false), (1, None));
        assert_eq!(hub.bump("p", true), (2, Some(1)));
        assert_eq!(hub.seq("p", Audience::Dm), 2);
        assert_eq!(hub.seq("p", Audience::Players), 1);
        assert_eq!(hub.seq("q", Audience::Players), 0);
        hub.forget_party("p");
        assert_eq!(hub.seq("p", Audience::Dm), 0);
    }
}
