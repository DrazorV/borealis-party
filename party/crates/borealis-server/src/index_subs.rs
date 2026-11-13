//! The account's index files and the packs that came from them.
//!
//! - **Names.** Upstream names an index from the index file on its first download and shows its address until then.
//!   An index followed because a bundle brought its pack (`bundle_accept`) has not been downloaded here yet. The list
//!   ([`described`]) names it after that pack and counts the pack's files. It also says the content is already
//!   installed, instead of upstream's "Not downloaded yet".
//! - **Bundles.** Accepting a bundle follows the index of every pack in it that came from one
//!   ([`indexes_to_follow`]), the packs this account already had included. An index this account removed itself is
//!   left out unless the bundle changed its pack. Each removal is recorded in this account's `party-index-removed.json`
//!   and the record is cleared when the index is added again.
//! - **Repair.** Additional Content lists the installed packs from an index the account does not follow and offers to
//!   add it (`ui/party/content/UnfollowedIndexes.tsx`, from `GET /api/content/pack-sources`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use borealis_bundle::{Bundle, ItemKind, Limits};
use borealis_lib::content_dto::ContentIndex;
use borealis_lib::content_index::raw_file_url;
use borealis_lib::content_packs::PACK_META_FILE;
use borealis_lib::state::AppState;
use serde_json::Value;

/// What the list says of an index not downloaded here whose pack is installed.
pub const INSTALLED_NOT_CHECKED: &str =
    "Its content is installed. Press Update to check it for newer files.";

/// The indexes this account removed itself, in its own data folder.
const REMOVED_FILE: &str = "party-index-removed.json";

/// An installed pack that came from an index.
struct FromIndex {
    name: String,
    files: usize,
}

/// The installed packs that came from an index, by that index's address (raw-file form).
fn packs_by_index(state: &AppState) -> BTreeMap<String, FromIndex> {
    let mut out = BTreeMap::new();
    for pack in state.installed_packs().into_iter().filter(|p| !p.bundled) {
        let Some(meta) = read_meta(&pack.dir) else {
            continue;
        };
        let Some(url) = meta["indexUrl"].as_str().filter(|u| !u.trim().is_empty()) else {
            continue;
        };
        let files = meta["files"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        out.entry(raw_file_url(url)).or_insert(FromIndex {
            name: pack.manifest.name.clone(),
            files,
        });
    }
    out
}

fn read_meta(dir: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(dir.join(PACK_META_FILE)).ok()?).ok()
}

/// Upstream's index list, with an index not downloaded here named after its installed pack (see the module docs).
pub fn described(state: &AppState, list: Vec<ContentIndex>) -> Vec<ContentIndex> {
    let packs = packs_by_index(state);
    list.into_iter()
        .map(|mut index| {
            let Some(pack) = packs.get(&raw_file_url(&index.url)) else {
                return index;
            };
            if index.name.trim().is_empty() || index.name == index.url {
                index.name.clone_from(&pack.name);
            }
            if index.last_checked.is_none() {
                if index.file_count == 0 {
                    index.file_count = pack.files;
                }
                if index.problem.is_none() {
                    index.problem = Some(INSTALLED_NOT_CHECKED.to_string());
                }
            }
            index
        })
        .collect()
}

fn removed_file(state: &AppState) -> PathBuf {
    state.paths.data.join(REMOVED_FILE)
}

/// The indexes this account removed itself and has not added again.
pub fn removed(state: &AppState) -> BTreeSet<String> {
    std::fs::read_to_string(removed_file(state))
        .ok()
        .and_then(|text| serde_json::from_str::<BTreeSet<String>>(&text).ok())
        .unwrap_or_default()
}

fn save_removed(state: &AppState, set: &BTreeSet<String>) {
    let path = removed_file(state);
    let written = if set.is_empty() {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        serde_json::to_string(set)
            .map_err(std::io::Error::other)
            .and_then(|text| std::fs::write(&path, text))
    };
    // Only a later bundle's choice depends on it: the removal itself went through.
    if let Err(e) = written {
        tracing::warn!("index removals not recorded: {e}");
    }
}

/// Record that the account removed `url` itself (or, `false`, that it follows it again).
pub fn set_removed(state: &AppState, url: &str, is_removed: bool) {
    let url = raw_file_url(url.trim());
    let mut set = removed(state);
    let changed = if is_removed {
        set.insert(url)
    } else {
        set.remove(&url)
    };
    if changed {
        save_removed(state, &set);
    }
}

/// The ids of the packs in the bundle at `path`, or none if it can't be read.
pub fn bundle_pack_ids(path: &Path) -> BTreeSet<String> {
    let read = || -> Option<BTreeSet<String>> {
        let file = std::fs::File::open(path).ok()?;
        let bundle = Bundle::open_with(file, Limits::default()).ok()?;
        Some(
            bundle
                .manifest()
                .items
                .iter()
                .filter(|item| item.kind == ItemKind::Pack)
                .map(|item| item.key.clone())
                .collect(),
        )
    };
    read().unwrap_or_default()
}

/// Each installed pack that came from an index: its id, the index's address and its whole `pack-meta.json` (which
/// changes whenever the pack is installed again).
pub fn index_packs(state: &AppState) -> BTreeMap<String, (String, String)> {
    std::fs::read_dir(state.paths.library())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let text = std::fs::read_to_string(entry.path().join(PACK_META_FILE)).ok()?;
            let meta: Value = serde_json::from_str(&text).ok()?;
            let url = meta["indexUrl"].as_str()?.to_string();
            Some((
                entry.file_name().to_string_lossy().into_owned(),
                (url, text),
            ))
        })
        .collect()
}

/// The indexes a bundle accept follows. `before` is [`index_packs`] before the accept, `now` after it, and
/// `in_bundle` the ids of the bundle's packs. An index whose pack the bundle added or changed is always followed.
/// An index whose pack was already here is followed unless it is in `removed`.
pub fn indexes_to_follow(
    before: &BTreeMap<String, (String, String)>,
    now: &BTreeMap<String, (String, String)>,
    in_bundle: &BTreeSet<String>,
    removed: &BTreeSet<String>,
) -> BTreeSet<String> {
    now.iter()
        .filter_map(|(id, entry)| {
            let (url, _) = entry;
            let changed = before.get(id) != Some(entry);
            let kept = in_bundle.contains(id) && !removed.contains(&raw_file_url(url));
            (changed || kept).then(|| url.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packs(list: &[(&str, &str, &str)]) -> BTreeMap<String, (String, String)> {
        list.iter()
            .map(|(id, url, meta)| ((*id).to_string(), ((*url).to_string(), (*meta).to_string())))
            .collect()
    }

    fn set(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_bundle_follows_the_indexes_of_all_its_packs_unless_removed_on_purpose() {
        const A: &str = "https://example.invalid/a.index";
        const B: &str = "https://example.invalid/b.index";
        const C: &str = "https://example.invalid/c.index";
        const D: &str = "https://example.invalid/d.index";
        // a: new; b: already here, identical; c: already here, removed on purpose; d: here but not in the bundle.
        let before = packs(&[("b", B, "1"), ("c", C, "1"), ("d", D, "1")]);
        let now = packs(&[("a", A, "1"), ("b", B, "1"), ("c", C, "1"), ("d", D, "1")]);
        let follow = indexes_to_follow(&before, &now, &set(&["a", "b", "c"]), &set(&[C]));
        assert_eq!(follow, set(&[A, B]));
        // The bundle changed c's pack: its content came back, so the index does too.
        let now = packs(&[("a", A, "1"), ("b", B, "1"), ("c", C, "2"), ("d", D, "1")]);
        let follow = indexes_to_follow(&before, &now, &set(&["a", "b", "c"]), &set(&[C]));
        assert_eq!(follow, set(&[A, B, C]));
    }
}
