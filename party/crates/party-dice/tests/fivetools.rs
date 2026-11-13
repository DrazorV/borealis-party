//! Opt-in: every `{@damage …}` and `{@dice …}` tag in a local 5etools checkout's bestiary parses.
//!
//! Set `BOREALIS_TEST_5ETOOLS_DIR` to the folder that holds `data/`; the test is skipped when it is unset. It prints
//! counts only and never names a creature, and no 5etools data is committed.

use party_dice::parse_notation;
use serde_json::Value;
use std::path::PathBuf;

fn bestiary_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("BOREALIS_TEST_5ETOOLS_DIR")?)
        .join("data")
        .join("bestiary");
    dir.is_dir().then_some(dir)
}

fn strings<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        Value::Object(o) => o.values().for_each(|x| strings(x, out)),
        _ => {}
    }
}

/// The notation inside `{@damage X}` / `{@dice X}` (before any `|`).
fn tags(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for prefix in ["{@damage ", "{@dice "] {
        let mut rest = s;
        while let Some(i) = rest.find(prefix) {
            let after = &rest[i + prefix.len()..];
            let Some(end) = after.find('}') else { break };
            out.push(after[..end].split('|').next().unwrap_or("").trim());
            rest = &after[end..];
        }
    }
    out
}

#[test]
fn bestiary_dice_tags_parse() {
    let Some(dir) = bestiary_dir() else {
        eprintln!("skipped: BOREALIS_TEST_5ETOOLS_DIR is not set");
        return;
    };
    let (mut files, mut ok, mut bad) = (0, 0usize, 0usize);
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !(name.starts_with("bestiary-") && name.ends_with(".json")) {
            continue;
        }
        files += 1;
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut all = Vec::new();
        strings(&value, &mut all);
        for tag in all.iter().flat_map(|s| tags(s)) {
            // Dice with a variable count ("1d6 × level") or a multiplier ("1d4 × 10") are not plain notation.
            if tag.contains('×')
                || tag
                    .chars()
                    .any(|c| c.is_ascii_alphabetic() && c != 'd' && c != 'D')
            {
                continue;
            }
            if parse_notation(tag).is_ok() {
                ok += 1;
            } else {
                bad += 1;
            }
        }
    }
    eprintln!("5etools bestiary: {files} files, {ok} dice tags parse, {bad} don't");
    assert!(files > 0, "no bestiary files under {}", dir.display());
    assert!(ok > 1000, "suspiciously few dice tags: {ok}");
    assert!(
        bad * 100 <= ok,
        "more than 1% of dice tags fail: {bad} of {}",
        ok + bad
    );
}
