//! The encounter builder's stat block copies for Borealis Party (release 0.3.0).
//!
//! * [`creature`]: the copy of a creature an encounter keeps (a typed `Monster` element's `MonsterData`, or an Aurora
//!   `Companion` element's printed block, plan D17 b), its fingerprint for "Refresh stat block", and what players see
//!   of it (hidden creatures nothing at all, revealed ones their whole stat block, plan D20 b).
//! * [`overrides`]: the per-encounter changes to a copy (HP, AC, saves, added traits, a DM note; plan D41 option A),
//!   each keeping the source value it replaced, and the refresh that reports conflicts.
//!
//! There is no XP budget or difficulty rating here: the user decided CR and XP are not reliable indicators (plan
//! D16, 2026-11-13). A creature's CR is only part of its printed stat block, and XP is left out of every copy shown.

pub mod creature;
pub mod overrides;
