//! Legendary actions and Legendary Resistance are per creature (C16, C17, user 2026-11-13): two legendary
//! creatures with 3 and 2 actions spend and refill their own pools; a shared pool is only a DM link; a combat
//! saved by 0.3.0 with a shared pool loads as one pool per creature.

mod common;
use std::collections::BTreeMap;

use common::*;
use party_combat::legendary::{actions_per_round, per_creature_pools};
use party_combat::*;

fn use_ab(id_: &str, ability: &str) -> Command {
    Command::UseAbility {
        id: id(id_),
        ability: ability.into(),
        slot: None,
    }
}

fn own_pool(who: &str, max: u32) -> Command {
    Command::SetLegendaryPool {
        pool: who.into(),
        value: LegendaryPool {
            max,
            remaining: max,
            members: vec![id(who)],
            guessed: false,
        },
    }
}

fn left(combat: &Combat, pool: &str) -> u32 {
    combat.state.legendary_pools[pool].remaining
}

/// A dragon (3 actions) and a smaller dragon (2), each with its own pool, and a wizard between them.
fn two_dragons() -> Combat {
    let mut combat = Combat::new(Edition::Srd521, 1);
    add(&mut combat, adult_red_dragon("big"));
    add(&mut combat, adult_red_dragon("small"));
    add(&mut combat, wizard_pc("w"));
    init(&mut combat, "big", 20);
    init(&mut combat, "w", 15);
    init(&mut combat, "small", 10);
    dm(&mut combat, own_pool("big", 3));
    dm(&mut combat, own_pool("small", 2));
    combat
}

#[test]
fn each_creature_spends_its_own_pool() {
    let mut combat = two_dragons();
    dm(&mut combat, Command::Start); // big
    dm(&mut combat, Command::NextTurn); // w
    dm(&mut combat, use_ab("big", "wing")); // big 3 -> 1
    assert_eq!((left(&combat, "big"), left(&combat, "small")), (1, 2));
    dm(&mut combat, use_ab("small", "wing")); // small 2 -> 0
    assert_eq!((left(&combat, "big"), left(&combat, "small")), (1, 0));
    // big still has one; small has none.
    dm(&mut combat, use_ab("big", "tail"));
    assert!(matches!(
        dm_err(&mut combat, use_ab("small", "tail")),
        CombatError::Unavailable(Unavailable::NotEnoughLegendary)
    ));
}

#[test]
fn each_pool_refills_on_its_own_creatures_turn() {
    let mut combat = two_dragons();
    dm(&mut combat, Command::Start); // big
    dm(&mut combat, Command::NextTurn); // w
    dm(&mut combat, use_ab("big", "wing"));
    dm(&mut combat, use_ab("small", "tail"));
    assert_eq!((left(&combat, "big"), left(&combat, "small")), (1, 1));
    let ev = dm(&mut combat, Command::NextTurn); // small's turn: only small refills
    assert_eq!((left(&combat, "big"), left(&combat, "small")), (1, 2));
    let resets: Vec<&str> = ev
        .dm
        .iter()
        .filter_map(|e| match &e.kind {
            DmEventKind::LegendaryReset { pool } => Some(pool.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(resets, ["small"]);
    // small can't on its own turn; big can.
    assert!(matches!(
        dm_err(&mut combat, use_ab("small", "tail")),
        CombatError::Unavailable(Unavailable::OwnTurn)
    ));
    dm(&mut combat, use_ab("big", "tail"));
    assert_eq!(left(&combat, "big"), 0);
    dm(&mut combat, Command::NextTurn); // round 2, big refills
    assert_eq!((left(&combat, "big"), left(&combat, "small")), (3, 2));
}

#[test]
fn legendary_resistance_is_per_creature() {
    let mut combat = two_dragons();
    dm(
        &mut combat,
        Command::AdjustLegendaryResistance {
            id: id("big"),
            delta: -2,
        },
    );
    assert_eq!(c(&combat, "big").legendary_resistance.unwrap().remaining, 1);
    assert_eq!(
        c(&combat, "small").legendary_resistance.unwrap().remaining,
        3
    );
    // Per day: a new round doesn't restore it.
    dm(&mut combat, Command::Start);
    dm(&mut combat, Command::NextTurn);
    dm(&mut combat, Command::NextTurn);
    dm(&mut combat, Command::NextTurn);
    assert_eq!(c(&combat, "big").legendary_resistance.unwrap().remaining, 1);
}

/// A DM link (heads of one creature) is opt-in: the members leave their own pools for the linked one.
#[test]
fn a_dm_link_moves_the_members_into_one_pool() {
    let mut combat = two_dragons();
    dm(
        &mut combat,
        Command::SetLegendaryPool {
            pool: "heads".into(),
            value: LegendaryPool {
                max: 3,
                remaining: 3,
                members: vec![id("big"), id("small")],
                guessed: false,
            },
        },
    );
    let keys: Vec<&String> = combat.state.legendary_pools.keys().collect();
    assert_eq!(keys, ["heads"]);
    dm(&mut combat, Command::Start);
    dm(&mut combat, Command::NextTurn); // w
    dm(&mut combat, use_ab("small", "wing"));
    dm(&mut combat, use_ab("big", "tail"));
    assert_eq!(left(&combat, "heads"), 0);
    // Unlinking: own pools again.
    dm(&mut combat, own_pool("big", 3));
    dm(&mut combat, own_pool("small", 2));
    let keys: Vec<&String> = combat.state.legendary_pools.keys().collect();
    assert_eq!(keys, ["big", "small"]);
}

/// A combat 0.3.0 saved with one pool for two creatures loads as one pool each, sized from their blocks.
#[test]
fn an_old_shared_pool_loads_as_one_pool_per_creature() {
    let mut combat = two_dragons();
    combat.state.legendary_pools.clear();
    combat.state.legendary_pools.insert(
        "pair".into(),
        LegendaryPool {
            max: 3,
            remaining: 1,
            members: vec![id("big"), id("small")],
            guessed: false,
        },
    );
    let mut json = serde_json::to_value(&combat.state).unwrap();
    json.as_object_mut()
        .unwrap()
        .remove("legendary_per_creature");
    let mut old: CombatState = serde_json::from_value(json).unwrap();
    assert!(!old.legendary_per_creature);

    let sizes = BTreeMap::from([
        ("big".to_string(), (3, false)),
        ("small".to_string(), (2, false)),
    ]);
    assert!(per_creature_pools(&mut old, &sizes));
    assert!(old.legendary_per_creature);
    let pools: Vec<(&str, u32, u32, Vec<CombatantId>)> = old
        .legendary_pools
        .iter()
        .map(|(k, p)| (k.as_str(), p.max, p.remaining, p.members.clone()))
        .collect();
    assert_eq!(
        pools,
        [
            ("big", 3, 1, vec![id("big")]),
            ("small", 2, 2, vec![id("small")])
        ]
    );
    // Once only: a link made afterwards stays a link.
    old.legendary_pools.clear();
    old.legendary_pools.insert(
        "heads".into(),
        LegendaryPool {
            max: 3,
            remaining: 3,
            members: vec![id("big"), id("small")],
            guessed: false,
        },
    );
    assert!(!per_creature_pools(&mut old, &sizes));
    assert_eq!(old.legendary_pools["heads"].members.len(), 2);
    // And it still runs as a combat.
    let mut back = Combat::new(Edition::Srd521, 1);
    back.state = old;
    dm(&mut back, Command::Start);
}

/// A single pool takes its creature's own size, and a legendary creature without a pool gets one.
#[test]
fn the_migration_sizes_single_pools_and_fills_gaps() {
    let mut combat = two_dragons();
    combat.state.legendary_pools.remove("small");
    combat.state.legendary_per_creature = false;
    combat
        .state
        .legendary_pools
        .get_mut("big")
        .unwrap()
        .remaining = 1;
    let sizes = BTreeMap::from([
        ("big".to_string(), (2, false)),
        ("small".to_string(), (2, true)),
    ]);
    per_creature_pools(&mut combat.state, &sizes);
    let big = &combat.state.legendary_pools["big"];
    assert_eq!((big.max, big.remaining), (2, 1));
    let small = &combat.state.legendary_pools["small"];
    assert_eq!((small.max, small.remaining, small.guessed), (2, 2, true));
}

/// D20 b still holds: no player view carries a legendary count.
#[test]
fn players_see_no_legendary_counts() {
    let mut combat = two_dragons();
    dm(&mut combat, Command::Reveal { id: id("big") });
    dm(&mut combat, Command::Reveal { id: id("small") });
    dm(&mut combat, Command::Start);
    dm(&mut combat, Command::NextTurn);
    let ev = dm(&mut combat, use_ab("small", "tail"));
    assert!(ev.players.is_empty(), "{:?}", ev.players);
    for who in ["big", "small"] {
        let view = player_view_of(&combat.state, &id(who)).unwrap();
        let text = view.to_string();
        assert!(
            !text.contains("legendary_resistance")
                && !text.contains("legendary_pools")
                && !text.contains("legendary_per_creature"),
            "{text}"
        );
    }
}

#[test]
fn the_count_comes_from_the_block() {
    assert_eq!(
        actions_per_round(None, "The hydra can take 2 legendary actions."),
        2
    );
    assert_eq!(actions_per_round(Some(1), ""), 1);
    assert_eq!(actions_per_round(None, "No number here."), 3);
    assert_eq!(
        party_combat::legendary::legendary_count(None, "Legendary Action Uses: 3 (4 in Lair)."),
        (3, false)
    );
    assert_eq!(
        party_combat::legendary::legendary_count(None, "No number here."),
        (3, true)
    );
}
