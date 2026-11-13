//! CI lints for the curated SRD tables (plan "spells in combat", section 2):
//! - every row's id is a spell of that edition in `content/srd-*/spells`, with its name;
//! - the setters cross-check: level and concentration;
//! - every dice literal of a row appears in that edition's SRD text of the spell;
//! - `srdTextSha` matches the SRD text, so a changed text flags the row for re-review;
//! - every SRD spell is curated or listed as uncurated, exactly once;
//! - the engine's contract fixtures and the tables agree.
//!
//! A coverage report prints with `--nocapture`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use party_combat::Edition;
use party_spells::data::{coverage, fingerprint, table};
use serde_json::Value;

struct SrdSpell {
    name: String,
    level: u8,
    concentration: bool,
    description: String,
    text: String,
}

fn srd(edition: Edition) -> BTreeMap<String, SrdSpell> {
    let dir = match edition {
        Edition::Srd51 => "srd-5.1/spells",
        Edition::Srd521 => "srd-5.2.1/spells",
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../content")
        .join(dir);
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let pack: Value =
            serde_json::from_str(&std::fs::read_to_string(e.unwrap().path()).unwrap()).unwrap();
        for el in pack["elements"].as_array().unwrap() {
            let setter = |n: &str| {
                el["setters"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|s| s["name"] == n)
                    .and_then(|s| s["value"].as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let description = el["description"].as_str().unwrap_or("").to_string();
            let higher = el["data"]["higher_level"].as_str().unwrap_or("");
            out.insert(
                el["id"].as_str().unwrap().to_string(),
                SrdSpell {
                    name: el["name"].as_str().unwrap().to_string(),
                    level: setter("level").parse().unwrap(),
                    concentration: setter("isConcentration") == "true",
                    text: format!("{description}\n{higher}").to_lowercase(),
                    description,
                },
            );
        }
    }
    out
}

/// Every dice literal of a definition: `NdM` in its formulas, and `NdM` of a dice term (a lone die may print as
/// "a d4").
fn dice_literals(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::String(s) => {
            let b = s.as_bytes();
            let mut i = 0;
            while i < b.len() {
                if b[i].is_ascii_digit() && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
                    let start = i;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i < b.len() && b[i] == b'd' {
                        let mid = i;
                        i += 1;
                        while i < b.len() && b[i].is_ascii_digit() {
                            i += 1;
                        }
                        if i > mid + 1 {
                            out.insert(s[start..i].to_string());
                        }
                    }
                } else {
                    i += 1;
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| dice_literals(x, out)),
        Value::Object(m) => {
            if let (Some(c), Some(s)) = (m.get("count"), m.get("sides")) {
                out.insert(format!("{c}d{s}"));
            }
            for (k, x) in m {
                // notes are our own words; a die named there isn't a number the engine uses
                if k != "notes" && k != "question" && k != "label" && k != "text" {
                    dice_literals(x, out);
                }
            }
        }
        _ => {}
    }
}

fn appears(text: &str, literal: &str) -> bool {
    if text.contains(literal) {
        return true;
    }
    // "a d4", "roll a d8": a single die prints without its count.
    literal
        .strip_prefix("1d")
        .is_some_and(|s| text.contains(&format!(" d{s}")) || text.contains(&format!("a d{s}")))
}

#[test]
fn tables_check_against_the_srd() {
    let mut problems = Vec::new();
    for edition in [Edition::Srd51, Edition::Srd521] {
        let t = table(edition);
        assert_eq!(t.edition, edition);
        assert_eq!(t.format, "party-spells/1");
        assert!(t.attribution.contains("Creative Commons Attribution 4.0"));
        let spells = srd(edition);
        let mut seen = BTreeSet::new();
        for row in &t.rows {
            let m = &row.mech;
            let tag = format!("{}: {}", t.srd, m.key);
            if !seen.insert(m.key.clone()) {
                problems.push(format!("{tag}: listed twice"));
            }
            assert_eq!(m.v, 1, "{tag}");
            assert_eq!(m.edition, edition, "{tag}");
            let Some(s) = spells.get(&m.key) else {
                problems.push(format!("{tag}: not a spell of SRD {}", t.srd));
                continue;
            };
            if s.name != m.name {
                problems.push(format!("{tag}: named {} in the SRD", s.name));
            }
            if s.level != m.level {
                problems.push(format!("{tag}: level {} in the SRD", s.level));
            }
            if s.concentration != m.concentration {
                problems.push(format!("{tag}: concentration {} in the SRD", s.concentration));
            }
            if fingerprint(&s.description) != row.srd_text_sha {
                problems.push(format!("{tag}: the SRD text changed; re-review the row"));
            }
            let mut dice = BTreeSet::new();
            dice_literals(&serde_json::to_value(m).unwrap(), &mut dice);
            for d in dice {
                if !appears(&s.text, &d) {
                    problems.push(format!("{tag}: {d} isn't in the SRD text"));
                }
            }
        }
        for u in &t.uncurated {
            if !seen.insert(u.id.clone()) {
                problems.push(format!("{}: {} listed twice", t.srd, u.id));
            }
            match spells.get(&u.id) {
                Some(s) if s.name == u.name && s.level == u.level => {}
                _ => problems.push(format!("{}: uncurated {} doesn't match the SRD", t.srd, u.id)),
            }
        }
        for id in spells.keys() {
            if !seen.contains(id) {
                problems.push(format!("{}: {id} is neither curated nor uncurated", t.srd));
            }
        }
        println!("SRD {} coverage: {:?}", t.srd, coverage(edition));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The engine's contract fixtures (party-combat/fixtures/spells) are the tables' rows, unchanged.
#[test]
fn fixtures_are_the_curated_rows() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../party-combat/fixtures/spells");
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let list: Vec<party_combat::spell::SpellMech> =
            serde_json::from_str(&std::fs::read_to_string(e.unwrap().path()).unwrap()).unwrap();
        for m in list {
            let row = party_spells::data::curated(&m.key)
                .unwrap_or_else(|| panic!("{} isn't curated", m.key));
            assert_eq!(row, &m, "{}", m.key);
            n += 1;
        }
    }
    assert!(n >= 14);
}

/// Lookups: by id, by name per edition (curated or not), and the editions stay apart.
#[test]
fn lookups() {
    use party_spells::data::{curated, srd_spell};
    assert_eq!(curated("ID_PHB_SPELL_FIREBALL").unwrap().level, 3);
    assert!(curated("ID_NOT_A_SPELL").is_none());
    assert_eq!(
        srd_spell("fireball", Edition::Srd51),
        Some(("ID_PHB_SPELL_FIREBALL", 3))
    );
    assert_eq!(
        srd_spell("Fireball", Edition::Srd521),
        Some(("ID_2024_SPELL_FIREBALL", 3))
    );
    assert_eq!(srd_spell("wish", Edition::Srd51).map(|s| s.1), Some(9));
    assert!(srd_spell("Not A Spell", Edition::Srd51).is_none());
}
