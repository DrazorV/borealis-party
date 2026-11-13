//! Legendary actions and Legendary Resistance are per creature (C16, C17): each legendary creature has its own
//! pool, sized from its own stat block, refilled at the start of its own turn. A pool several creatures share
//! (the heads of one multi-headed creature) exists only when the DM links them with
//! [`Command::SetLegendaryPool`](crate::Command::SetLegendaryPool).
//!
//! The helpers here read the counts from a stat block's text, and [`per_creature_pools`] brings a combat saved
//! before pools were per creature (0.3.0) to one pool per legendary creature.

use std::collections::BTreeMap;

use crate::model::{CombatState, CombatantId, LegendaryPool};

/// Legendary actions when a block says none: the SRD default.
pub const DEFAULT_LEGENDARY_ACTIONS: u32 = 3;

const NUMBER_WORDS: [&str; 10] = [
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
];

/// A number written as digits or as a word ("3", "three").
fn number(word: &str) -> Option<u32> {
    let w = word
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    if let Ok(n) = w.parse::<u32>() {
        return Some(n);
    }
    NUMBER_WORDS
        .iter()
        .position(|n| *n == w)
        .and_then(|i| u32::try_from(i + 1).ok())
}

/// The legendary actions a creature takes per round: [`legendary_count`] without whether it was a guess.
#[must_use]
pub fn actions_per_round(per_round: Option<u8>, preamble: &str) -> u32 {
    legendary_count(per_round, preamble).0
}

/// The legendary actions a creature takes per round, and whether that is a guess: the block's own count, else
/// the number its preamble prints (2014 "can take 3 legendary actions", 2024 "Legendary Action Uses: 3", the
/// first number of "3 (4 in Lair)"), else 3 as a guess the DM can change. Never 0.
#[must_use]
pub fn legendary_count(per_round: Option<u8>, preamble: &str) -> (u32, bool) {
    if let Some(n) = per_round.filter(|n| *n > 0) {
        return (u32::from(n), false);
    }
    match printed_count(preamble) {
        Some(n) => (n, false),
        None => (DEFAULT_LEGENDARY_ACTIONS, true),
    }
}

fn printed_count(preamble: &str) -> Option<u32> {
    let words: Vec<&str> = preamble.split_whitespace().collect();
    // SRD 5.2.1: "Legendary Action Uses: 3 (4 in Lair)."
    let lower: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    for (i, w) in lower.iter().enumerate() {
        if w.starts_with("uses") && i > 0 && lower[i - 1] == "action" {
            if let Some(n) = words.get(i + 1).and_then(|n| number(n)).filter(|n| *n > 0) {
                return Some(n);
            }
        }
    }
    for (i, w) in words.iter().enumerate() {
        let next = words.get(i + 1).map(|n| n.to_lowercase());
        if next.as_deref().is_some_and(|n| n.starts_with("legendary")) {
            if let Some(n) = number(w).filter(|n| *n > 0) {
                return Some(n);
            }
        }
    }
    None
}

/// The uses per day of a "Legendary Resistance (3/Day)" trait whose usage the parser didn't read: the first
/// "N/Day" in its name, then in its text. `None` when neither says.
#[must_use]
pub fn resistance_uses(name: &str, text: &str) -> Option<u32> {
    [name, text].into_iter().find_map(|s| {
        let lower = s.to_lowercase();
        let at = lower.find("/day")?;
        let digits: String = lower[..at]
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        digits.parse::<u32>().ok().filter(|n| *n > 0)
    })
}

/// Brings a combat to one legendary pool per legendary creature, once (C16).
///
/// `sizes` is each legendary creature's own count from its stat block and whether it is a guess
/// ([`legendary_count`]), by combatant id. A combat saved before
/// pools were per creature had no explicit links, so every pool with several members is split: each member gets
/// its own pool (the first keeps what was left, the others start full). Single pools take their creature's own
/// size, and a legendary creature without a pool (a copy added mid-fight) gets a full one. After this, pools with
/// several members are DM links and stay as they are. Returns false when it had already run.
pub fn per_creature_pools(state: &mut CombatState, sizes: &BTreeMap<String, (u32, bool)>) -> bool {
    if state.legendary_per_creature {
        return false;
    }
    state.legendary_per_creature = true;
    let mut pools: BTreeMap<String, LegendaryPool> = BTreeMap::new();
    for (key, pool) in std::mem::take(&mut state.legendary_pools) {
        if pool.members.len() <= 1 {
            pools.insert(key, pool);
            continue;
        }
        for (i, m) in pool.members.iter().enumerate() {
            let (max, guessed) = sizes.get(&m.0).copied().unwrap_or((pool.max, false));
            let max = max.max(1);
            let remaining = if i == 0 { pool.remaining.min(max) } else { max };
            pools.insert(
                m.0.clone(),
                LegendaryPool {
                    max,
                    remaining,
                    members: vec![m.clone()],
                    guessed,
                },
            );
        }
    }
    for pool in pools.values_mut() {
        let Some((size, guessed)) = pool.members.first().and_then(|m| sizes.get(&m.0)).copied()
        else {
            continue;
        };
        let size = size.max(1);
        pool.guessed = guessed;
        if size > pool.max {
            pool.remaining += size - pool.max;
        }
        pool.max = size;
        pool.remaining = pool.remaining.min(size);
    }
    for c in state.combatants.iter().filter(|c| !c.removed) {
        let Some(&(size, guessed)) = sizes.get(&c.id.0) else {
            continue;
        };
        if pools.values().any(|p| p.members.contains(&c.id)) {
            continue;
        }
        let size = size.max(1);
        pools.insert(
            c.id.0.clone(),
            LegendaryPool {
                max: size,
                remaining: size,
                members: vec![CombatantId::new(c.id.0.clone())],
                guessed,
            },
        );
    }
    state.legendary_pools = pools;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_from_the_block_or_its_preamble() {
        assert_eq!(
            actions_per_round(Some(2), "can take 3 legendary actions"),
            2
        );
        assert_eq!(
            actions_per_round(None, "The lich can take 3 legendary actions, choosing from"),
            3
        );
        assert_eq!(
            actions_per_round(None, "The hydra can take two legendary actions."),
            2
        );
        assert_eq!(
            actions_per_round(None, "Legendary Action Uses: 4 (5 in Lair)."),
            4
        );
        assert_eq!(actions_per_round(None, ""), 3);
        assert_eq!(actions_per_round(Some(0), "can take 1 legendary action"), 1);
    }

    #[test]
    fn resistance_uses_from_the_name_or_text() {
        assert_eq!(resistance_uses("Legendary Resistance (3/Day)", ""), Some(3));
        assert_eq!(
            resistance_uses("Legendary Resistance (2/Day, or 3/Day in Lair)", ""),
            Some(2)
        );
        assert_eq!(
            resistance_uses("Legendary Resistance", "1/Day. If it fails a save"),
            Some(1)
        );
        assert_eq!(
            resistance_uses("Legendary Resistance", "It succeeds."),
            None
        );
    }
}
