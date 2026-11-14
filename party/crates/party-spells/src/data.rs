//! The curated SRD spell mechanics shipped with Borealis Connect: `data/srd-5.1.json` and `data/srd-5.2.1.json`.
//!
//! Each table lists, per bundled SRD spell element id, a [`SpellMech`] with numbers and enums only (notes are our
//! own words, never rules prose), and the fingerprint of the SRD text it was reviewed against (`srdTextSha`, FNV-1a
//! 64 over the element's description): when the SRD text changes, the lint flags the row for re-review. Every other
//! SRD spell is listed as explicitly uncurated. The tables are CC-BY-4.0 derived data; see their `attribution`.
//!
//! The coverage (user decision Q2, 2026-11-13): every spell on an SRD monster stat block in both editions, every
//! SRD summon or conjure spell (info only, nothing spawns), and the common player spells.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use party_combat::spell::SpellMech;
use party_combat::Edition;
use serde::{Deserialize, Serialize};

/// One curated row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Row {
    /// FNV-1a 64 (16 hex digits) of the SRD element's description the row was reviewed against.
    pub srd_text_sha: String,
    /// The definition.
    pub mech: SpellMech,
}

/// An SRD spell with no definition yet: its cast spends the slot and asks the DM to rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uncurated {
    /// The SRD element id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its level.
    pub level: u8,
}

/// One edition's table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Table {
    /// `party-spells/1`.
    pub format: String,
    /// `5.1` or `5.2.1`.
    pub srd: String,
    /// The edition.
    pub edition: Edition,
    /// The CC-BY attribution.
    pub attribution: String,
    /// What the table holds.
    #[serde(default)]
    pub note: String,
    /// The curated rows.
    pub rows: Vec<Row>,
    /// Every other SRD spell.
    pub uncurated: Vec<Uncurated>,
}

const SRD_51: &str = include_str!("../data/srd-5.1.json");
const SRD_521: &str = include_str!("../data/srd-5.2.1.json");

fn parse(text: &str) -> Table {
    // The tables are checked by this crate's tests; a broken table is a build-time bug, not a runtime state.
    serde_json::from_str(text).unwrap_or_else(|e| panic!("party-spells table: {e}"))
}

/// The table of an edition.
#[must_use]
pub fn table(edition: Edition) -> &'static Table {
    static T51: OnceLock<Table> = OnceLock::new();
    static T521: OnceLock<Table> = OnceLock::new();
    match edition {
        Edition::Srd51 => T51.get_or_init(|| parse(SRD_51)),
        Edition::Srd521 => T521.get_or_init(|| parse(SRD_521)),
    }
}

fn by_key() -> &'static BTreeMap<&'static str, &'static SpellMech> {
    static MAP: OnceLock<BTreeMap<&'static str, &'static SpellMech>> = OnceLock::new();
    MAP.get_or_init(|| {
        [Edition::Srd51, Edition::Srd521]
            .iter()
            .flat_map(|e| table(*e).rows.iter())
            .map(|r| (r.mech.key.as_str(), &r.mech))
            .collect()
    })
}

/// The curated definition of an SRD spell element id (the Aurora PHB and SRD packs share ids for SRD spells, so
/// this applies to those too).
#[must_use]
pub fn curated(key: &str) -> Option<&'static SpellMech> {
    by_key().get(key).copied()
}

/// (is 2024, normalized name) → (id, level); Edition isn't Ord.
type NameIndex = BTreeMap<(bool, String), (&'static str, u8)>;

/// An SRD spell of an edition by name (case and punctuation ignored): its element id and level, curated or not.
#[must_use]
pub fn srd_spell(name: &str, edition: Edition) -> Option<(&'static str, u8)> {
    static INDEX: OnceLock<NameIndex> = OnceLock::new();
    let index = INDEX.get_or_init(|| {
        let mut m = BTreeMap::new();
        for e in [Edition::Srd51, Edition::Srd521] {
            let t = table(e);
            for r in &t.rows {
                m.insert(
                    (
                        e == Edition::Srd521,
                        crate::canonical::normalize(&r.mech.name),
                    ),
                    (r.mech.key.as_str(), r.mech.level),
                );
            }
            for u in &t.uncurated {
                m.insert(
                    (e == Edition::Srd521, crate::canonical::normalize(&u.name)),
                    (u.id.as_str(), u.level),
                );
            }
        }
        m
    });
    index
        .get(&(
            edition == Edition::Srd521,
            crate::canonical::normalize(name),
        ))
        .copied()
}

/// How many spells of a table are curated, by tier, and how many are not.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    /// Rows by tier (`auto`, `partial`, `utility`, `ruling`).
    pub by_tier: BTreeMap<String, usize>,
    /// Uncurated SRD spells.
    pub uncurated: usize,
}

/// The coverage of an edition's table (the CI coverage report).
#[must_use]
pub fn coverage(edition: Edition) -> Coverage {
    let t = table(edition);
    let mut out = Coverage {
        uncurated: t.uncurated.len(),
        ..Coverage::default()
    };
    for r in &t.rows {
        let tier = serde_json::to_value(r.mech.tier)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        *out.by_tier.entry(tier).or_default() += 1;
    }
    out
}

/// FNV-1a 64 of a text, as 16 hex digits (the `srdTextSha` fingerprint).
#[must_use]
pub fn fingerprint(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}
