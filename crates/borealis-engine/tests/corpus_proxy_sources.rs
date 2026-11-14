//! Local-only count (no assertions on names): how many of the proxy items an Aurora file's
//! content lists do not resolve once converted 5etools packs are installed beside it, or instead
//! of it. Aurora characters keep these proxy ids in their inventory, so an unresolved one shows as
//! "inventory item ... is not installed".
//!
//! Enabled by `BOREALIS_LIBRARY_DIR`, an installed content library (one `pack.json` folder per
//! pack); skipped otherwise. Packs whose folder starts with `5etools-` count as converted, the
//! rest as Aurora files. Prints counts only.

use borealis_content::{ContentStoreBuilder, Ruleset};
use borealis_engine::Engine;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn engine(core: &Path, packs: &[PathBuf]) -> Engine {
    let mut b = ContentStoreBuilder::new()
        .add_pack_dir(core)
        .expect("core pack");
    for p in packs {
        b = match b.add_pack_dir(p) {
            Ok(b) => b,
            Err(e) => panic!("{}: {e}", p.display()),
        };
    }
    Engine::new(&b.build())
}

#[test]
fn aurora_proxies_resolve_beside_converted_packs() {
    let Ok(dir) = std::env::var("BOREALIS_LIBRARY_DIR") else {
        eprintln!(
            "aurora_proxies_resolve_beside_converted_packs: BOREALIS_LIBRARY_DIR not set, skipping"
        );
        return;
    };
    let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/core");
    let mut converted = Vec::new();
    let mut aurora = Vec::new();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("library folder")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("pack.json").is_file())
        .collect();
    dirs.sort();
    for p in dirs {
        let name = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if name.starts_with("5etools-") {
            converted.push(p);
        } else {
            aurora.push(p);
        }
    }
    let all: Vec<PathBuf> = converted.iter().chain(&aurora).cloned().collect();
    let only_aurora = engine(&core, &aurora);
    let only_converted = engine(&core, &converted);
    let both = engine(&core, &all);
    let mut failed = 0;
    for rs in [Ruleset::Dnd2014, Ruleset::Dnd2024] {
        let au = only_aurora.universe(rs);
        let cv = only_converted.universe(rs);
        let bo = both.universe(rs);
        // targets the converted packs also have (same type and name), to tell "not converted" apart
        let converted_names: HashSet<(String, String)> = cv
            .iter()
            .map(|e| (e.el.ty.clone(), e.el.name.to_lowercase()))
            .collect();
        let (mut listed, mut lost_beside, mut in_converted, mut lost_instead) = (0, 0, 0, 0);
        for p in au.proxies().all() {
            if p.source == "Core" || p.source == "Internal" {
                continue;
            }
            listed += 1;
            if bo.get(&p.id).is_none() {
                lost_beside += 1;
            }
            if converted_names.contains(&(p.target.el.ty.clone(), p.target.el.name.to_lowercase()))
            {
                in_converted += 1;
                if cv.get(&p.id).is_none() {
                    lost_instead += 1;
                }
            }
        }
        eprintln!(
            "{rs:?}: {listed} Aurora proxy ids; {lost_beside} unresolved with both installed; \
             {in_converted} whose target the converted packs have, {lost_instead} of them unresolved with only the converted packs"
        );
        failed += lost_beside;
    }
    assert_eq!(
        failed, 0,
        "Aurora proxy ids unresolved with the converted packs installed beside them"
    );
}
