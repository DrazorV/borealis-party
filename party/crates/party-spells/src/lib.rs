//! Spell data for Borealis Party (release 0.3.2, "Spells and mechanics in combat").
//!
//! * [`data`]: the curated SRD 5.1 and SRD 5.2.1 spell mechanics (`data/*.json`, CC-BY-4.0, numbers and enums only).
//! * [`canonical`]: one spell per name and ruleset; the other copies are hidden from new picks, never deleted.
//! * [`draft`]: "Suggested" mechanics from a user's own 5etools spell tags, which the DM confirms once (never
//!   shipped, never guessed from prose).

pub mod canonical;
pub mod data;
pub mod draft;
