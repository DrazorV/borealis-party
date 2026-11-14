//! Opt-in corpus checks of the canonical-spell resolver (counts only, nothing is written or committed): the bundled
//! SRD spells together with a local 5etools copy (`BOREALIS_TEST_5ETOOLS_DIR`, with its books' publication dates)
//! and a local Aurora Legacy cache (`BOREALIS_TEST_AURORA_DIR`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use party_spells::canonical::{Candidate, Catalog, Preferences};
use serde_json::Value;

fn srd() -> Vec<Candidate> {
    let mut out = Vec::new();
    for (dir, ruleset, source) in [
        ("srd-5.1/spells", "dnd2014", "SRD 5.1"),
        ("srd-5.2.1/spells", "dnd2024", "SRD 5.2.1"),
    ] {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../content")
            .join(dir);
        for e in std::fs::read_dir(dir).unwrap() {
            let pack: Value =
                serde_json::from_str(&std::fs::read_to_string(e.unwrap().path()).unwrap()).unwrap();
            for el in pack["elements"].as_array().unwrap() {
                let level = el["setters"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|s| s["name"] == "level")
                    .and_then(|s| s["value"].as_str())
                    .and_then(|v| v.parse().ok());
                out.push(Candidate {
                    id: el["id"].as_str().unwrap().to_string(),
                    name: el["name"].as_str().unwrap().to_string(),
                    ruleset: ruleset.into(),
                    source: source.into(),
                    level,
                    date: None,
                });
            }
        }
    }
    out
}

/// 5etools spells, with their books' publication dates; 2024 sources by the book's edition marker.
fn five_etools(root: &Path) -> Vec<Candidate> {
    let mut dates = BTreeMap::new();
    let mut modern = std::collections::BTreeSet::new();
    for file in ["books.json", "adventures.json"] {
        let Ok(text) = std::fs::read_to_string(root.join("data").join(file)) else {
            continue;
        };
        let v: Value = serde_json::from_str(&text).unwrap();
        for b in v["book"]
            .as_array()
            .into_iter()
            .chain(v["adventure"].as_array())
            .flatten()
        {
            let id = b["id"].as_str().unwrap_or("").to_string();
            if let Some(p) = b["published"].as_str() {
                dates.insert(id.clone(), p.to_string());
            }
            if b["edition"].as_str() == Some("one") {
                modern.insert(id);
            }
        }
    }
    let mut out = Vec::new();
    for e in std::fs::read_dir(root.join("data/spells")).unwrap() {
        let p = e.unwrap().path();
        if !p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("spells-")
        {
            continue;
        }
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        for s in v["spell"].as_array().into_iter().flatten() {
            let source = s["source"].as_str().unwrap_or("").to_string();
            // The importer's rule: the entry's own `edition`, else a source published on or after the 2024
            // Player's Handbook.
            let late = dates
                .get(&source)
                .is_some_and(|d| d.as_str() >= "2024-09-17");
            let ruleset =
                if s["edition"].as_str() == Some("one") || modern.contains(&source) || late {
                    "dnd2024"
                } else {
                    "dnd2014"
                };
            out.push(Candidate {
                id: format!("5E_{source}_{}", s["name"].as_str().unwrap_or("")),
                name: s["name"].as_str().unwrap_or("").to_string(),
                ruleset: ruleset.into(),
                level: s["level"].as_u64().and_then(|n| u8::try_from(n).ok()),
                date: dates.get(&source).cloned(),
                source,
            });
        }
    }
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let i = tag.find(&key)? + key.len();
    let end = tag[i..].find('"')?;
    Some(tag[i..i + end].to_string())
}

/// Aurora Spell elements of every XML file (a light scan of `<element ... type="Spell">` and its level setter).
fn aurora(root: &Path) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path: PathBuf = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|x| x != "xml") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut rest = text.as_str();
            while let Some(i) = rest.find("<element ") {
                let after = &rest[i..];
                let Some(end) = after.find('>') else { break };
                let tag = &after[..end];
                let body_end = after.find("</element>").unwrap_or(after.len());
                let body = &after[..body_end];
                rest = &after[end..];
                if attr(tag, "type").as_deref() != Some("Spell") {
                    continue;
                }
                let (Some(id), Some(name)) = (attr(tag, "id"), attr(tag, "name")) else {
                    continue;
                };
                let source = attr(tag, "source").unwrap_or_default();
                let level = body
                    .find("name=\"level\">")
                    .and_then(|j| body[j + 13..].split('<').next())
                    .and_then(|v| v.trim().parse().ok());
                out.push(Candidate {
                    id,
                    name,
                    ruleset: if source.contains("2024") {
                        "dnd2024".into()
                    } else {
                        "dnd2014".into()
                    },
                    source,
                    level,
                    date: None,
                });
            }
        }
    }
    out
}

fn report(label: &str, cat: &Catalog) {
    let groups = cat.groups().count();
    let shared = cat.groups().filter(|g| g.members.len() > 1).count();
    let split = cat.groups().filter(|g| g.key.contains('@')).count();
    println!(
        "{label}: {groups} spells, {shared} with copies, {} copies hidden, {split} split by level",
        cat.hidden().len()
    );
}

#[test]
fn five_etools_and_srd() {
    let Some(dir) = std::env::var_os("BOREALIS_TEST_5ETOOLS_DIR") else {
        eprintln!("BOREALIS_TEST_5ETOOLS_DIR not set: skipped");
        return;
    };
    let srd = srd();
    let mut all = srd.clone();
    all.extend(five_etools(Path::new(&dir)));
    let cat = Catalog::build(&all, &Preferences::default());
    report("SRD + 5etools", &cat);
    // The SRD versions are newer than the books they come from: every SRD spell is its own chosen copy.
    let lost: Vec<&str> = srd
        .iter()
        .filter(|c| cat.is_hidden(&c.id))
        .map(|c| c.name.as_str())
        .collect();
    assert!(lost.is_empty(), "{lost:?}");
    // Editions never merge.
    for g in cat.groups() {
        let rulesets: std::collections::BTreeSet<&str> = g
            .members
            .iter()
            .filter_map(|(id, _)| all.iter().find(|c| &c.id == id))
            .map(|c| c.ruleset.as_str())
            .collect();
        assert_eq!(rulesets.len(), 1, "{}", g.key);
    }
}

#[test]
fn aurora_and_srd() {
    let Some(dir) = std::env::var_os("BOREALIS_TEST_AURORA_DIR") else {
        eprintln!("BOREALIS_TEST_AURORA_DIR not set: skipped");
        return;
    };
    let mut all = srd();
    let theirs = aurora(Path::new(&dir));
    println!("Aurora: {} spell elements", theirs.len());
    all.extend(theirs);
    let cat = Catalog::build(&all, &Preferences::default());
    report("SRD + Aurora", &cat);
    assert!(cat.groups().count() > 0);
}
