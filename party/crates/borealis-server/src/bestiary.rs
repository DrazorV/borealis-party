//! The bestiary layer (0.3.0): what an account's installed content holds in creatures, and a fast search over them
//! for the encounter builder. The API is `party/docs/bestiary-api.md`.
//!
//! - Only typed `Monster` elements are creatures here (D17): their stat block is `MonsterData`. Aurora content has
//!   none, only `Companion` setters, which the coverage report counts and labels "companions only".
//! - The 5etools import drops some details the model has no field for: mythic actions, coven CR, alternate speeds,
//!   environment, page, variant sidebars, the reactions' introduction (D15). [`capture`] reads them from the same
//!   staged 5etools folder right after upstream's import, inside the workspace, and writes one spool file per pack
//!   under the account's `sync/bestiary/`. [`ingest`] moves the spool into `monster_meta` and `bestiary_imports`
//!   (migration step 4) before any bestiary call reads them. The workspace has no database, hence the spool.
//! - Rows are keyed by account and element id and carry the pack revision ([`pack_rev`]); a row of another revision
//!   is ignored, and an import of the pack replaces all of its rows.
//! - [`search`] filters an in-memory list of the account's creatures ([`Index`]), built once per content load (the
//!   content hub is compared by identity) and once per captured import.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use axum::extract::{Path as UrlPath, State};
use axum::Json;
use borealis_content::monster::{CreatureSize, CreatureType};
use borealis_content::{Cr, Element, MonsterData};
use borealis_import_5etools::ids::{IdAlloc, IdKind};
use borealis_import_5etools::text::{strip_tags, Renderer};
use borealis_lib::content_dto::{ContentPack, FiveToolsImportReport, PackKind};
use borealis_lib::state::{AppState as Backend, ContentHub};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tauri::Manager;

use crate::auth::User;
use crate::db::now;
use crate::error::{ApiError, ApiResult};
use crate::workspaces::Workspace;
use crate::AppState;

/// The element type of a typed stat block.
pub const MONSTER: &str = "Monster";
/// The element type of an Aurora companion (counted, never listed: D17).
pub const COMPANION: &str = "Companion";
/// `kind` of a typed stat block.
pub const KIND_MONSTER: &str = "monster";
/// `kind` of an Aurora companion.
pub const KIND_COMPANION: &str = "companion";
/// Where [`capture`] leaves its spool files, under the account's data folder.
pub const SPOOL_DIR: &str = "sync/bestiary";
/// Default and largest page of [`search`].
pub const DEFAULT_LIMIT: usize = 50;
/// Largest page of [`search`].
pub const MAX_LIMIT: usize = 200;
/// Longest search text.
pub const MAX_TEXT: usize = 100;
/// Most values one filter list takes.
pub const MAX_VALUES: usize = 50;
/// Accounts whose creature list is kept in memory at once.
const CACHED_ACCOUNTS: usize = 64;
/// Longest reason or note kept in the coverage report.
const MAX_NOTE: usize = 160;

// The importer's own prefixes of its monster notes (`convert::monster`).
const SKIP_PREFIX: &str = "creature stat block: ";
const SECTION_PREFIX: &str = "creature section: ";
const FIELD_PREFIX: &str = "creature field: ";
const MONSTER_KIND: &str = "monster";

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

fn upstream(e: &borealis_lib::error::ApiError) -> ApiError {
    ApiError::Internal(e.message.clone())
}

// ---------------------------------------------------------------------------------------------------------------
// Captured details (D15)
// ---------------------------------------------------------------------------------------------------------------

/// A named block of text: a mythic action, a variant sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Named {
    /// As printed, tags removed.
    pub name: String,
    /// Plain text.
    pub text: String,
}

/// Mythic actions: the introduction and the options. The mythic trait itself is one of the stat block's traits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mythic {
    /// The text before the options.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub intro: String,
    /// The options in printed order.
    pub actions: Vec<Named>,
}

/// The challenge rating of a creature as part of a coven.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CovenCr {
    /// As printed (`"12"`). No XP: there is none anywhere in Borealis Party (the user's D16).
    pub cr: String,
}

/// One more speed of a mode, under a condition (`40 ft. (in wolf form)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AltSpeed {
    /// Feet.
    pub feet: i64,
    /// The condition as printed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub condition: String,
}

/// What the 5etools import drops for one creature.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    /// Lowercase environment words.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environments: Vec<String>,
    /// The book page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    /// Mythic actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mythic: Option<Mythic>,
    /// Coven challenge rating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coven_cr: Option<CovenCr>,
    /// More speeds by mode (`walk`, `fly`, ...).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub alt_speeds: BTreeMap<String, Vec<AltSpeed>>,
    /// Variant sidebars, as text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Named>,
    /// The reactions' introduction.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reaction_intro: String,
    /// The 5etools data has lore text for it (not imported).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fluff: bool,
    /// The 5etools data has a token image for it (not imported).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub token: bool,
    /// The raw 5etools `spellcasting` blocks, as the data has them (0.3.2): the monster spellbook parser
    /// (`party-encounter` spellbook) reads their spell lists, slots, uses per day, DC and ability, which the
    /// imported text keeps only as prose. Account data like the rest; never shipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spellcasting: Option<Value>,
}

fn text_of(v: Option<&Value>) -> String {
    v.and_then(Value::as_array)
        .map(|e| Renderer::plain(e))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn named_list(v: Option<&Value>) -> Vec<Named> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let name = strip_tags(f.get("name").and_then(Value::as_str).unwrap_or(""))
                .trim()
                .to_string();
            let text = text_of(f.get("entries"));
            (!name.is_empty() || !text.is_empty()).then_some(Named { name, text })
        })
        .collect()
}

/// The details of one resolved 5etools creature that the stat block model does not hold.
#[must_use]
pub fn meta_of(raw: &Value) -> Meta {
    let mut environments: Vec<String> = raw
        .get("environment")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    environments.sort();
    environments.dedup();
    let mythic = raw.get("mythic").and_then(Value::as_array).map(|_| Mythic {
        intro: text_of(raw.get("mythicHeader")),
        actions: named_list(raw.get("mythic")),
    });
    let coven_cr = raw
        .pointer("/cr/coven")
        .and_then(Value::as_str)
        .map(|cr| CovenCr { cr: cr.to_string() });
    let mut alt_speeds = BTreeMap::new();
    if let Some(modes) = raw.pointer("/speed/alternate").and_then(Value::as_object) {
        for (mode, list) in modes {
            let speeds: Vec<AltSpeed> = list
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| match s {
                    Value::Number(n) => n.as_i64().map(|feet| AltSpeed {
                        feet,
                        condition: String::new(),
                    }),
                    Value::Object(o) => {
                        o.get("number")
                            .and_then(Value::as_i64)
                            .map(|feet| AltSpeed {
                                feet,
                                condition: strip_tags(
                                    o.get("condition").and_then(Value::as_str).unwrap_or(""),
                                )
                                .trim()
                                .to_string(),
                            })
                    }
                    _ => None,
                })
                .collect();
            if !speeds.is_empty() {
                alt_speeds.insert(mode.clone(), speeds);
            }
        }
    }
    let flag = |k: &str| raw.get(k).and_then(Value::as_bool).unwrap_or(false);
    Meta {
        environments,
        page: raw.get("page").and_then(Value::as_i64),
        mythic,
        coven_cr,
        alt_speeds,
        variants: named_list(raw.get("variant")),
        reaction_intro: text_of(raw.get("reactionHeader")),
        fluff: flag("hasFluff") || flag("hasFluffImages"),
        token: flag("hasToken"),
        spellcasting: raw
            .get("spellcasting")
            .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
            .cloned(),
    }
}

/// The resolved creatures of a 5etools data folder (`_copy` and `_versions` applied, as the importer does), and the
/// source of each creature that could not be resolved.
///
/// # Errors
/// When the folder is not 5etools data.
pub fn read_creatures(folder: &Path) -> Result<(Vec<Value>, Vec<String>), String> {
    let files = borealis_import_5etools::folder::data_files(folder).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    let mut templates = Vec::new();
    for rel in files {
        let bestiary = rel.starts_with("bestiary/bestiary-") || rel == "bestiary/template.json";
        if !(bestiary || rel.starts_with("homebrew/")) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(folder.join(&rel)) else {
            continue;
        };
        let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        for (prop, into) in [("monster", &mut raw), ("monsterTemplate", &mut templates)] {
            if let Some(Value::Array(list)) = value.get_mut(prop).map(Value::take) {
                into.extend(list);
            }
        }
    }
    let resolved =
        borealis_import_5etools::copy::resolve_array_with_templates("monster", &raw, &templates);
    let failed = resolved
        .failed_origins
        .iter()
        .filter_map(|&i| raw.get(i)?.get("source")?.as_str().map(str::to_string))
        .collect();
    Ok((resolved.entries, failed))
}

/// A pack's revision: a digest of its manifest and of the size and edit time of every file it lists. It changes
/// whenever the pack is written again (an import or update), and costs no file reads beyond `pack.json`.
#[must_use]
pub fn pack_rev(dir: &Path) -> String {
    let mut hash = Sha256::new();
    let manifest = std::fs::read(dir.join("pack.json")).unwrap_or_default();
    hash.update(&manifest);
    let files: Vec<String> = serde_json::from_slice::<Value>(&manifest)
        .ok()
        .and_then(|m| m.get("files").cloned())
        .and_then(|f| f.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|f| f.get("path").and_then(Value::as_str).map(str::to_string))
        .collect();
    for rel in files {
        let stamp = std::fs::metadata(dir.join(&rel)).ok().map(|m| {
            let at = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            (m.len(), at)
        });
        hash.update(format!("\n{rel}:{stamp:?}").as_bytes());
    }
    hash.finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A value and how often it occurs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Count {
    /// The value.
    pub value: String,
    /// How often.
    pub count: usize,
}

fn counts(map: BTreeMap<String, usize>) -> Vec<Count> {
    map.into_iter()
        .map(|(value, count)| Count { value, count })
        .collect()
}

/// One pack's capture, as [`capture`] spools it and [`ingest`] stores it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spool {
    /// The pack.
    pub pack_id: String,
    /// Its revision when captured.
    pub pack_rev: String,
    /// The 5etools source (book) abbreviation.
    pub source: String,
    /// Creatures of that source in the data (converted and skipped).
    pub in_data: usize,
    /// Skipped creatures by reason.
    pub skipped: Vec<Count>,
    /// Creatures kept with a detail dropped, by what was dropped (one creature can count under several).
    pub partial: Vec<Count>,
    /// Skipped creatures.
    pub skipped_total: usize,
    /// Creatures kept with a detail dropped.
    pub partial_total: usize,
    /// When.
    pub captured_at: i64,
    /// Details by element id.
    pub monsters: BTreeMap<String, Meta>,
}

fn short(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() <= MAX_NOTE {
        return t.to_string();
    }
    let cut: String = t.chars().take(MAX_NOTE - 1).collect();
    format!("{cut}…")
}

/// What a partial note says was dropped: `"mythic actions"`, `"special speeds ..."`.
fn dropped(reason: &str) -> String {
    let r = reason
        .strip_prefix(SECTION_PREFIX)
        .or_else(|| reason.strip_prefix(FIELD_PREFIX))
        .unwrap_or(reason);
    short(r.strip_suffix(" not imported").unwrap_or(r))
}

/// The pack each Monster element of `packs` is in, and per pack its ids and lowercase names.
struct Installed {
    pack_of: HashMap<String, String>,
    names: HashMap<String, HashMap<String, Vec<String>>>,
}

fn installed(hub: &ContentHub, packs: &HashSet<String>) -> Installed {
    let mut out = Installed {
        pack_of: HashMap::new(),
        names: HashMap::new(),
    };
    for stored in hub.store.iter() {
        let el = &stored.element;
        if el.ty != MONSTER {
            continue;
        }
        let pack = pack_of_file(&stored.file);
        if !packs.contains(pack) {
            continue;
        }
        out.pack_of.insert(el.id.clone(), pack.to_string());
        out.names
            .entry(pack.to_string())
            .or_default()
            .entry(el.name.to_lowercase())
            .or_default()
            .push(el.id.clone());
    }
    out
}

/// The pack an element was loaded from: its file is named `<pack id>/<path>`.
fn pack_of_file(file: &str) -> &str {
    file.split(['/', '\\']).next().unwrap_or(file)
}

/// The source abbreviation in a report note's `Name|SOURCE`.
fn note_source(element: &str) -> Option<&str> {
    element.rsplit_once('|').map(|(_, s)| s.trim())
}

/// Read the details of the creatures that `report` imported from `folder`, and spool them for [`ingest`] (one file
/// per pack). Answers how many creatures got details.
///
/// # Errors
/// When the folder can't be read or the spool can't be written; the import itself is not affected.
pub fn capture(
    backend: &Backend,
    folder: &Path,
    report: &FiveToolsImportReport,
) -> Result<usize, String> {
    let wanted: HashSet<String> = report.packs.iter().map(|p| p.id.clone()).collect();
    if wanted.is_empty() {
        return Ok(0);
    }
    let (creatures, failed) = read_creatures(folder)?;
    let hub = backend.hub().map_err(|e| e.message)?;
    let found = installed(&hub, &wanted);
    let dirs: HashMap<String, PathBuf> = backend
        .installed_packs()
        .into_iter()
        .filter(|p| wanted.contains(&p.manifest.id))
        .map(|p| (p.manifest.id, p.dir))
        .collect();
    let captured_at = now();
    let mut spools: BTreeMap<String, Spool> = BTreeMap::new();
    let spool_for = |spools: &mut BTreeMap<String, Spool>, source: &str| -> Option<String> {
        let pack = borealis_import_5etools::pack::pack_id(source);
        if !wanted.contains(&pack) {
            return None;
        }
        spools.entry(pack.clone()).or_insert_with(|| Spool {
            pack_id: pack.clone(),
            pack_rev: dirs.get(&pack).map(|d| pack_rev(d)).unwrap_or_default(),
            source: source.to_string(),
            in_data: 0,
            skipped: Vec::new(),
            partial: Vec::new(),
            skipped_total: 0,
            partial_total: 0,
            captured_at,
            monsters: BTreeMap::new(),
        });
        Some(pack)
    };
    let mut got = 0;
    for raw in &creatures {
        let (Some(source), Some(name)) = (
            raw.get("source").and_then(Value::as_str),
            raw.get("name").and_then(Value::as_str),
        ) else {
            continue;
        };
        let Some(pack) = spool_for(&mut spools, source) else {
            continue;
        };
        let spool = spools.get_mut(&pack).expect("spooled above");
        spool.in_data += 1;
        let plain = IdAlloc::new().named(IdKind::Monster, source, name);
        let id = if found.pack_of.get(&plain) == Some(&pack) {
            Some(plain)
        } else {
            found
                .names
                .get(&pack)
                .and_then(|n| n.get(&name.to_lowercase()))
                .filter(|ids| ids.len() == 1)
                .map(|ids| ids[0].clone())
        };
        if let Some(id) = id {
            spool.monsters.insert(id, meta_of(raw));
            got += 1;
        }
    }
    for source in &failed {
        if let Some(pack) = spool_for(&mut spools, source) {
            spools.get_mut(&pack).expect("spooled above").in_data += 1;
        }
    }
    let pack_of_note = |element: &str| -> Option<String> {
        found
            .pack_of
            .get(element)
            .cloned()
            .or_else(|| note_source(element).map(borealis_import_5etools::pack::pack_id))
    };
    let mut skipped: HashMap<String, BTreeMap<String, usize>> = HashMap::new();
    let mut skipped_ids: HashMap<String, BTreeSet<String>> = HashMap::new();
    for note in report.skipped.iter().filter(|n| n.kind == MONSTER_KIND) {
        let Some(pack) = pack_of_note(&note.element) else {
            continue;
        };
        let reason = short(
            note.reason
                .strip_prefix(SKIP_PREFIX)
                .unwrap_or(&note.reason),
        );
        *skipped
            .entry(pack.clone())
            .or_default()
            .entry(reason)
            .or_default() += 1;
        skipped_ids
            .entry(pack)
            .or_default()
            .insert(note.element.clone());
    }
    let mut partial: HashMap<String, BTreeMap<String, usize>> = HashMap::new();
    let mut partial_ids: HashMap<String, BTreeSet<String>> = HashMap::new();
    for note in report.partial.iter().filter(|n| n.kind == MONSTER_KIND) {
        let Some(pack) = pack_of_note(&note.element) else {
            continue;
        };
        *partial
            .entry(pack.clone())
            .or_default()
            .entry(dropped(&note.reason))
            .or_default() += 1;
        partial_ids
            .entry(pack)
            .or_default()
            .insert(note.element.clone());
    }
    for (pack, spool) in &mut spools {
        spool.skipped = counts(skipped.remove(pack).unwrap_or_default());
        spool.partial = counts(partial.remove(pack).unwrap_or_default());
        spool.skipped_total = skipped_ids.get(pack).map_or(0, BTreeSet::len);
        spool.partial_total = partial_ids.get(pack).map_or(0, BTreeSet::len);
    }
    let dir = backend.paths.data.join(SPOOL_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for spool in spools.values() {
        let text = serde_json::to_vec(spool).map_err(|e| e.to_string())?;
        let file = dir.join(format!("{}.json", spool_name(&spool.pack_id)));
        let tmp = file.with_extension("tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
    }
    Ok(got)
}

/// A pack id as a file name (pack ids are ASCII words; anything else is replaced).
fn spool_name(pack: &str) -> String {
    pack.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// [`capture`], with its outcome logged instead of returned: called after a successful import.
pub fn capture_logged(backend: &Backend, folder: &Path, report: &FiveToolsImportReport) {
    match capture(backend, folder, report) {
        Ok(n) => tracing::info!(creatures = n, "bestiary: creature details captured"),
        Err(e) => tracing::warn!("bestiary: creature details not captured: {e}"),
    }
}

/// The staged download folder of the last `update_5etools_url` (upstream keeps one: `<data>/<STAGE_DIR>/<id>/data`).
#[must_use]
pub fn staged_download(data: &Path) -> Option<PathBuf> {
    let stage = data.join(borealis_lib::content_fivetools_url::STAGE_DIR);
    std::fs::read_dir(stage)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path().join("data"))
        .find(|p| p.is_dir())
}

// ---------------------------------------------------------------------------------------------------------------
// The database side
// ---------------------------------------------------------------------------------------------------------------

/// Move the account's spooled captures into the database, one pack per transaction (its rows replaced whole). A
/// spool file that can't be read is removed, so it can't block the next call.
///
/// # Errors
/// When the database fails; the spool file then stays for the next call.
pub async fn ingest(state: &AppState, owner: &str) -> ApiResult<usize> {
    let dir = state.workspaces.dir(owner).join(SPOOL_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };
    let files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    let mut done = 0;
    for file in files {
        let spool = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice::<Spool>(&b).ok());
        let Some(spool) = spool else {
            tracing::warn!("bestiary: unreadable capture {} removed", file.display());
            let _ = std::fs::remove_file(&file);
            continue;
        };
        store(state, owner, &spool).await?;
        let _ = std::fs::remove_file(&file);
        done += 1;
    }
    if done > 0 {
        forget(owner);
    }
    Ok(done)
}

async fn store(state: &AppState, owner: &str, spool: &Spool) -> ApiResult<()> {
    let mut tx = state.db.pool().begin().await?;
    sqlx::query("DELETE FROM monster_meta WHERE owner_id = $1 AND pack_id = $2")
        .bind(owner)
        .bind(&spool.pack_id)
        .execute(&mut *tx)
        .await?;
    for (id, meta) in &spool.monsters {
        let environments = serde_json::to_string(&meta.environments).map_err(internal)?;
        let extras = Meta {
            environments: Vec::new(),
            page: None,
            ..meta.clone()
        };
        sqlx::query(
            "INSERT INTO monster_meta (owner_id, element_id, pack_id, pack_rev, environment_json, page, extras_json,
                captured_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT DO NOTHING",
        )
        .bind(owner)
        .bind(id)
        .bind(&spool.pack_id)
        .bind(&spool.pack_rev)
        .bind(environments)
        .bind(meta.page)
        .bind(serde_json::to_string(&extras).map_err(internal)?)
        .bind(spool.captured_at)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM bestiary_imports WHERE owner_id = $1 AND pack_id = $2")
        .bind(owner)
        .bind(&spool.pack_id)
        .execute(&mut *tx)
        .await?;
    let total = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
    let skipped = json!({ "total": spool.skipped_total, "reasons": spool.skipped });
    let partial = json!({ "total": spool.partial_total, "notes": spool.partial });
    sqlx::query(
        "INSERT INTO bestiary_imports (owner_id, pack_id, pack_rev, source, in_data, skipped_json, partial_json,
            captured_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(owner)
    .bind(&spool.pack_id)
    .bind(&spool.pack_rev)
    .bind(&spool.source)
    .bind(total(spool.in_data))
    .bind(skipped.to_string())
    .bind(partial.to_string())
    .bind(spool.captured_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The account's stored details, by element id: `(pack revision, details)`.
async fn stored_meta(state: &AppState, owner: &str) -> ApiResult<HashMap<String, (String, Meta)>> {
    let rows = sqlx::query(
        "SELECT element_id, pack_rev, environment_json, page, extras_json FROM monster_meta WHERE owner_id = $1",
    )
    .bind(owner)
    .fetch_all(state.db.pool())
    .await?;
    let mut out = HashMap::with_capacity(rows.len());
    for row in rows {
        let id: String = row.try_get(0)?;
        let rev: String = row.try_get(1)?;
        let env: String = row.try_get(2)?;
        let page: Option<i64> = row.try_get(3)?;
        let extras: String = row.try_get(4)?;
        let mut meta: Meta = serde_json::from_str(&extras).unwrap_or_default();
        meta.environments = serde_json::from_str(&env).unwrap_or_default();
        meta.page = page;
        out.insert(id, (rev, meta));
    }
    Ok(out)
}

/// The raw 5etools `spellcasting` blocks captured for `element` of `owner`'s content, when the current pack revision
/// has them (0.3.2: the input of the monster spellbook parser). `None` for Aurora and SRD creatures, which have only
/// their text.
///
/// # Errors
/// When the workspace or the database fails.
pub async fn spellcasting_of(
    state: &AppState,
    owner: &str,
    element: &str,
) -> ApiResult<Option<Value>> {
    let index = index(state, owner).await?;
    Ok(index.meta.get(element).and_then(|m| m.spellcasting.clone()))
}

/// One pack's import record: `(revision, in data, skipped {total, reasons}, partial {total, notes})`.
type ImportRow = (String, i64, Value, Value);

async fn stored_imports(state: &AppState, owner: &str) -> ApiResult<HashMap<String, ImportRow>> {
    let rows = sqlx::query(
        "SELECT pack_id, pack_rev, in_data, skipped_json, partial_json FROM bestiary_imports WHERE owner_id = $1",
    )
    .bind(owner)
    .fetch_all(state.db.pool())
    .await?;
    let mut out = HashMap::new();
    for row in rows {
        let pack: String = row.try_get(0)?;
        let rev: String = row.try_get(1)?;
        let in_data: i64 = row.try_get(2)?;
        let skipped: String = row.try_get(3)?;
        let partial: String = row.try_get(4)?;
        let parse = |s: &str| serde_json::from_str::<Value>(s).unwrap_or(Value::Null);
        out.insert(pack, (rev, in_data, parse(&skipped), parse(&partial)));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------------
// The creature list
// ---------------------------------------------------------------------------------------------------------------

/// One creature in the list: what search filters and answers.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    /// Element id.
    pub id: String,
    /// Name.
    pub name: String,
    /// `monster` (a typed stat block) or `companion` (an Aurora `Companion` element, D17 b).
    pub kind: &'static str,
    /// Challenge rating as printed.
    pub cr: Option<String>,
    /// Its value (`0.25` for `1/4`).
    pub cr_value: Option<f64>,
    /// Creature type, lowercase; `None` for a companion that names none.
    #[serde(rename = "type")]
    pub creature_type: Option<String>,
    /// The parenthetical tags.
    pub tags: Vec<String>,
    /// Sizes.
    pub size: Vec<String>,
    /// Armor class.
    pub ac: Option<u32>,
    /// Hit points.
    pub hp: Option<u32>,
    /// Hit dice.
    pub hp_formula: Option<String>,
    /// The element's source name.
    pub source: String,
    /// The pack.
    pub pack_id: String,
    /// The pack's revision.
    pub pack_rev: String,
    /// `dnd2014`, `dnd2024` or `any`.
    pub ruleset: String,
    /// Has legendary actions.
    pub legendary: bool,
    /// Has mythic actions (captured).
    pub mythic: bool,
    /// Has lair actions.
    pub lair: bool,
    /// Environments (captured).
    pub environments: Vec<String>,
    /// Book page (captured).
    pub page: Option<i64>,
    #[serde(skip)]
    name_lower: String,
    #[serde(skip)]
    cr_rank: Option<Cr>,
}

/// A pack as the list knows it.
#[derive(Debug, Clone)]
struct PackInfo {
    id: String,
    name: String,
    kind: &'static str,
    enabled: bool,
    rev: String,
    /// The pack's ruleset when it targets exactly one (for companions without their own).
    ruleset: Option<String>,
}

/// An account's creatures, for one content load and one set of captured details.
pub struct Index {
    hub: Weak<ContentHub>,
    items: Vec<Summary>,
    packs: Vec<PackInfo>,
    meta: HashMap<String, Meta>,
}

fn cache() -> &'static Mutex<HashMap<String, Arc<Index>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Index>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Drop an account's list (new details were captured).
fn forget(owner: &str) {
    cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(owner);
}

/// Drop an account's list, as a server restart does (tests).
#[cfg(test)]
pub fn forget_for_tests(owner: &str) {
    forget(owner);
}

fn kind_of(kind: &PackKind) -> &'static str {
    match kind {
        PackKind::Bundled => "bundled",
        PackKind::LocalImport => "5etools",
        PackKind::Downloaded => "index",
        PackKind::Imported | PackKind::Homebrew => "custom",
    }
}

fn text_value<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn summary(el: &Element, m: &MonsterData, pack: &PackInfo) -> Summary {
    Summary {
        id: el.id.clone(),
        name: el.name.clone(),
        kind: KIND_MONSTER,
        cr: m.cr.map(Cr::as_text),
        cr_value: m.cr.map(Cr::as_f64),
        creature_type: Some(m.creature_type.as_str().to_string()),
        tags: m.tags.clone(),
        size: m.size.iter().map(|s| s.as_str().to_string()).collect(),
        ac: m.armor_class(),
        hp: m.hit_points(),
        hp_formula: m.hp.as_ref().and_then(|h| h.formula.clone()),
        source: el.source.clone(),
        pack_id: pack.id.clone(),
        pack_rev: pack.rev.clone(),
        ruleset: text_value(&m.ruleset),
        legendary: m.legendary.is_some(),
        mythic: false,
        lair: m.lair_actions.is_some(),
        environments: Vec::new(),
        page: None,
        name_lower: el.name.to_lowercase(),
        cr_rank: m.cr,
    }
}

/// The leading number of a setter (`10 (3d4+3)` -> 10).
fn leading_u32(text: &str) -> Option<u32> {
    let digits: String = text
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn setter<'a>(el: &'a Element, name: &str) -> Option<&'a str> {
    el.setter_value(name)
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// The hit dice in a companion's `hp` setter (`10 (3d4+3)` -> `3d4+3`).
fn hp_formula(text: &str) -> Option<String> {
    let inner = text.split_once('(')?.1.split_once(')')?.0;
    let formula: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
    (formula.contains('d') && formula.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .then_some(formula)
}

/// A companion's `type` setter as type and tags (`Humanoid (goblinoid)` -> `humanoid`, `["goblinoid"]`).
fn companion_type(text: Option<&str>) -> (Option<String>, Vec<String>) {
    let Some(text) = text else {
        return (None, Vec::new());
    };
    let (head, tags) = match text.split_once('(') {
        Some((head, rest)) => (
            head,
            rest.trim_end_matches(')')
                .split(',')
                .map(|t| t.trim().to_lowercase())
                .filter(|t| !t.is_empty())
                .collect(),
        ),
        None => (text, Vec::new()),
    };
    let head = head.trim().to_lowercase();
    ((!head.is_empty()).then_some(head), tags)
}

/// A companion's `size` setter as sizes (`Medium or Small` -> both); unknown words are left out.
fn companion_sizes(text: Option<&str>) -> Vec<String> {
    let known: Vec<&str> = CreatureSize::ALL.iter().map(|s| s.as_str()).collect();
    text.unwrap_or("")
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| known.contains(w))
        .map(str::to_string)
        .collect()
}

/// An Aurora `Companion` element in the list: only what its setters print; anything missing stays `None`.
fn companion_summary(el: &Element, pack: &PackInfo) -> Summary {
    let cr = setter(el, "challenge").and_then(Cr::parse);
    let (creature_type, tags) = companion_type(setter(el, "type"));
    let hp = setter(el, "hp");
    Summary {
        id: el.id.clone(),
        name: el.name.clone(),
        kind: KIND_COMPANION,
        cr: cr.map(Cr::as_text),
        cr_value: cr.map(Cr::as_f64),
        creature_type,
        tags,
        size: companion_sizes(setter(el, "size")),
        ac: setter(el, "ac").and_then(leading_u32),
        hp: hp.and_then(leading_u32),
        hp_formula: hp.and_then(hp_formula),
        source: el.source.clone(),
        pack_id: pack.id.clone(),
        pack_rev: pack.rev.clone(),
        ruleset: el
            .ruleset
            .map(|r| text_value(&r))
            .or_else(|| pack.ruleset.clone())
            .unwrap_or_else(|| "any".to_string()),
        legendary: false,
        mythic: false,
        lair: false,
        environments: Vec::new(),
        page: None,
        name_lower: el.name.to_lowercase(),
        cr_rank: cr,
    }
}

/// The list's content part, built in the workspace.
struct Built {
    hub: Weak<ContentHub>,
    items: Vec<Summary>,
    packs: Vec<PackInfo>,
}

fn build(backend: &Backend) -> ApiResult<Built> {
    let hub = backend.hub().map_err(|e| upstream(&e))?;
    let listed = backend.list_packs().map_err(|e| upstream(&e))?;
    let dirs: HashMap<String, PathBuf> = backend
        .installed_packs()
        .into_iter()
        .map(|p| (p.manifest.id, p.dir))
        .collect();
    let packs: Vec<PackInfo> = listed
        .iter()
        .map(|p: &ContentPack| PackInfo {
            id: p.id.clone(),
            name: p.name.clone(),
            kind: kind_of(&p.kind),
            enabled: p.enabled,
            rev: dirs.get(&p.id).map(|d| pack_rev(d)).unwrap_or_default(),
            ruleset: match p.rulesets.as_slice() {
                [one] => Some(text_value(one)),
                _ => None,
            },
        })
        .collect();
    let by_id: HashMap<&str, &PackInfo> = packs.iter().map(|p| (p.id.as_str(), p)).collect();
    let unknown = PackInfo {
        id: String::new(),
        name: String::new(),
        kind: "custom",
        enabled: true,
        rev: String::new(),
        ruleset: None,
    };
    let mut items = Vec::new();
    for stored in hub.store.iter() {
        let el = &stored.element;
        let pack_id = pack_of_file(&stored.file);
        let pack = by_id.get(pack_id).copied().unwrap_or(&unknown);
        let mut s = if el.ty == COMPANION {
            companion_summary(el, pack)
        } else if el.ty == MONSTER {
            let Ok(m) = MonsterData::from_element(el) else {
                continue;
            };
            summary(el, &m, pack)
        } else {
            continue;
        };
        if s.pack_id.is_empty() {
            s.pack_id = pack_id.to_string();
        }
        items.push(s);
    }
    items.sort_by(|a, b| {
        a.name_lower
            .cmp(&b.name_lower)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(Built {
        hub: Arc::downgrade(&hub),
        items,
        packs,
    })
}

/// The account's creature list: the cached one while the content and the captured details are the same, else a
/// fresh one.
///
/// # Errors
/// When the workspace or the database fails.
pub async fn index(state: &AppState, owner: &str) -> ApiResult<Arc<Index>> {
    ingest(state, owner).await?;
    let workspace = state.workspaces.get(owner).await?;
    let cached = cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(owner)
        .cloned();
    let built = rebuild_if_stale(&workspace, cached.clone()).await?;
    let Some(built) = built else {
        return Ok(cached.expect("fresh only with a cached list"));
    };
    let meta = stored_meta(state, owner).await?;
    let mut items = built.items;
    let mut kept = HashMap::new();
    for item in &mut items {
        let Some((rev, m)) = meta.get(&item.id) else {
            continue;
        };
        if *rev != item.pack_rev {
            continue;
        }
        item.mythic = m.mythic.is_some();
        item.environments.clone_from(&m.environments);
        item.page = m.page;
        kept.insert(item.id.clone(), m.clone());
    }
    let index = Arc::new(Index {
        hub: built.hub,
        items,
        packs: built.packs,
        meta: kept,
    });
    let mut map = cache().lock().unwrap_or_else(PoisonError::into_inner);
    if map.len() >= CACHED_ACCOUNTS && !map.contains_key(owner) {
        map.clear();
    }
    map.insert(owner.to_string(), index.clone());
    Ok(index)
}

/// `None` when `cached` is still the list of the workspace's current content.
async fn rebuild_if_stale(
    workspace: &Arc<Workspace>,
    cached: Option<Arc<Index>>,
) -> ApiResult<Option<Built>> {
    workspace
        .blocking(move |handle| {
            let backend = handle.state::<Backend>();
            let hub = backend.hub().map_err(|e| upstream(&e))?;
            if cached.is_some_and(|c| Weak::ptr_eq(&c.hub, &Arc::downgrade(&hub))) {
                return Ok(None);
            }
            build(&backend).map(Some)
        })
        .await
}

// ---------------------------------------------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------------------------------------------

/// `POST /api/bestiary/search`: every field optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchQuery {
    /// Part of the name.
    #[serde(default)]
    pub text: Option<String>,
    /// Lowest CR.
    #[serde(default)]
    pub cr_min: Option<String>,
    /// Highest CR.
    #[serde(default)]
    pub cr_max: Option<String>,
    /// Creature types.
    #[serde(default)]
    pub types: Vec<String>,
    /// Sizes.
    #[serde(default)]
    pub sizes: Vec<String>,
    /// Environments.
    #[serde(default)]
    pub environments: Vec<String>,
    /// Pack ids or source names.
    #[serde(default)]
    pub sources: Vec<String>,
    /// `dnd2014` or `dnd2024`.
    #[serde(default)]
    pub ruleset: Option<String>,
    /// With or without legendary actions.
    #[serde(default)]
    pub legendary: Option<bool>,
    /// `monster`, `companion`; both when empty.
    #[serde(default)]
    pub kinds: Vec<String>,
    /// `name` or `cr`.
    #[serde(default)]
    pub sort: Option<String>,
    /// Page size.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Page start.
    #[serde(default)]
    pub offset: Option<usize>,
}

/// A page of [`search`].
#[derive(Debug, Clone, Serialize)]
pub struct SearchPage {
    /// Matches in all.
    pub total: usize,
    /// This page's start.
    pub offset: usize,
    /// This page's size.
    pub limit: usize,
    /// The page.
    pub items: Vec<Summary>,
}

/// A checked query.
struct Filter {
    text: String,
    cr_min: Option<f64>,
    cr_max: Option<f64>,
    types: HashSet<String>,
    sizes: HashSet<String>,
    environments: HashSet<String>,
    sources: HashSet<String>,
    ruleset: Option<String>,
    legendary: Option<bool>,
    kinds: HashSet<String>,
    by_cr: bool,
    limit: usize,
    offset: usize,
}

fn lowered(list: &[String], what: &str, allowed: Option<&[&str]>) -> ApiResult<HashSet<String>> {
    if list.len() > MAX_VALUES {
        return Err(ApiError::Invalid(format!(
            "Choose at most {MAX_VALUES} {what}."
        )));
    }
    let mut out = HashSet::new();
    for v in list {
        let v = v.trim().to_lowercase();
        if v.is_empty() {
            continue;
        }
        if allowed.is_some_and(|a| !a.contains(&v.as_str())) {
            return Err(ApiError::Invalid(format!("Unknown {what}: {v}.")));
        }
        out.insert(v);
    }
    Ok(out)
}

fn cr_bound(text: Option<&String>) -> ApiResult<Option<f64>> {
    match text.map(|t| t.trim()).filter(|t| !t.is_empty()) {
        None => Ok(None),
        Some(t) => Cr::parse(t)
            .map(|c| Some(c.as_f64()))
            .ok_or_else(|| ApiError::Invalid(format!("Unknown challenge rating: {t}."))),
    }
}

impl Filter {
    fn new(q: &SearchQuery) -> ApiResult<Self> {
        let text = q.text.as_deref().unwrap_or("").trim().to_lowercase();
        if text.chars().count() > MAX_TEXT {
            return Err(ApiError::Invalid("The search text is too long.".into()));
        }
        let types: Vec<&str> = CreatureType::ALL.iter().map(|t| t.as_str()).collect();
        let sizes: Vec<&str> = CreatureSize::ALL.iter().map(|s| s.as_str()).collect();
        let ruleset = match q
            .ruleset
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        {
            None => None,
            Some(r @ ("dnd2014" | "dnd2024")) => Some(r.to_string()),
            Some(r) => return Err(ApiError::Invalid(format!("Unknown ruleset: {r}."))),
        };
        let by_cr = match q.sort.as_deref().map(str::trim) {
            None | Some("" | "name") => false,
            Some("cr") => true,
            Some(s) => return Err(ApiError::Invalid(format!("Unknown sort: {s}."))),
        };
        let limit = q.limit.unwrap_or(DEFAULT_LIMIT);
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(ApiError::Invalid(format!(
                "The page size is 1 to {MAX_LIMIT}."
            )));
        }
        if q.sources.len() > MAX_VALUES {
            return Err(ApiError::Invalid(format!(
                "Choose at most {MAX_VALUES} sources."
            )));
        }
        Ok(Self {
            text,
            cr_min: cr_bound(q.cr_min.as_ref())?,
            cr_max: cr_bound(q.cr_max.as_ref())?,
            types: lowered(&q.types, "creature types", Some(types.as_slice()))?,
            sizes: lowered(&q.sizes, "sizes", Some(sizes.as_slice()))?,
            environments: lowered(&q.environments, "environments", None)?,
            sources: q
                .sources
                .iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            ruleset,
            legendary: q.legendary,
            kinds: lowered(&q.kinds, "kinds", Some(&[KIND_MONSTER, KIND_COMPANION]))?,
            by_cr,
            limit,
            offset: q.offset.unwrap_or(0),
        })
    }

    fn matches(&self, s: &Summary) -> bool {
        if !self.text.is_empty() && !s.name_lower.contains(&self.text) {
            return false;
        }
        if self.cr_min.is_some() || self.cr_max.is_some() {
            let Some(cr) = s.cr_value else {
                return false;
            };
            if self.cr_min.is_some_and(|m| cr < m) || self.cr_max.is_some_and(|m| cr > m) {
                return false;
            }
        }
        (self.kinds.is_empty() || self.kinds.contains(s.kind))
            && (self.types.is_empty()
                || s.creature_type
                    .as_ref()
                    .is_some_and(|t| self.types.contains(t)))
            && (self.sizes.is_empty() || s.size.iter().any(|z| self.sizes.contains(z)))
            && (self.environments.is_empty()
                || s.environments.iter().any(|e| self.environments.contains(e)))
            && (self.sources.is_empty()
                || self.sources.contains(&s.pack_id)
                || self.sources.contains(&s.source))
            && self
                .ruleset
                .as_ref()
                .is_none_or(|r| s.ruleset == *r || s.ruleset == "any")
            && self.legendary.is_none_or(|l| s.legendary == l)
    }
}

/// Search the creature list.
fn run(index: &Index, filter: &Filter) -> SearchPage {
    let mut hits: Vec<&Summary> = index.items.iter().filter(|s| filter.matches(s)).collect();
    let starts = |s: &Summary| !filter.text.is_empty() && s.name_lower.starts_with(&filter.text);
    hits.sort_by(|a, b| {
        let cr = |s: &Summary| s.cr_value.unwrap_or(f64::INFINITY);
        let first = if filter.by_cr {
            cr(a).total_cmp(&cr(b))
        } else {
            std::cmp::Ordering::Equal
        };
        first
            .then_with(|| starts(b).cmp(&starts(a)))
            .then_with(|| a.name_lower.cmp(&b.name_lower))
            .then_with(|| a.id.cmp(&b.id))
    });
    SearchPage {
        total: hits.len(),
        offset: filter.offset,
        limit: filter.limit,
        items: hits
            .into_iter()
            .skip(filter.offset)
            .take(filter.limit)
            .cloned()
            .collect(),
    }
}

/// Search the typed Monster and Aurora Companion elements of `owner`'s own content (the API for the encounter
/// builder).
///
/// # Errors
/// 400 for a bad filter; the workspace's and the database's errors.
pub async fn search(state: &AppState, owner: &str, query: &SearchQuery) -> ApiResult<SearchPage> {
    let filter = Filter::new(query)?;
    let index = index(state, owner).await?;
    Ok(run(&index, &filter))
}

fn facet<'a>(values: impl Iterator<Item = &'a str>) -> Vec<Count> {
    let mut map: BTreeMap<String, usize> = BTreeMap::new();
    for v in values {
        *map.entry(v.to_string()).or_default() += 1;
    }
    counts(map)
}

fn cr_facet(crs: impl Iterator<Item = Cr>) -> Vec<Count> {
    let mut map: HashMap<Cr, usize> = HashMap::new();
    for cr in crs {
        *map.entry(cr).or_default() += 1;
    }
    let mut out: Vec<(f64, Count)> = map
        .into_iter()
        .map(|(cr, count)| {
            (
                cr.as_f64(),
                Count {
                    value: cr.as_text(),
                    count,
                },
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.into_iter().map(|(_, c)| c).collect()
}

fn facets_of(index: &Index) -> Value {
    let items = &index.items;
    let names: HashMap<&str, &str> = index
        .packs
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();
    let sources: Vec<Value> = facet(items.iter().map(|s| s.pack_id.as_str()))
        .into_iter()
        .map(|c| {
            let name = names.get(c.value.as_str()).copied().unwrap_or(&c.value);
            json!({ "value": c.value, "name": name, "count": c.count })
        })
        .collect();
    json!({
        "total": items.len(),
        "kinds": facet(items.iter().map(|s| s.kind)),
        "types": facet(items.iter().filter_map(|s| s.creature_type.as_deref())),
        "sizes": facet(items.iter().flat_map(|s| s.size.iter().map(String::as_str))),
        "environments": facet(items.iter().flat_map(|s| s.environments.iter().map(String::as_str))),
        "rulesets": facet(items.iter().map(|s| s.ruleset.as_str())),
        "crs": cr_facet(items.iter().filter_map(|s| s.cr_rank)),
        "sources": sources,
    })
}

/// An Aurora companion's block: what its setters and linked elements print, nothing filled in.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionDetail {
    /// Armor class.
    pub ac: Option<u32>,
    /// The `ac` setter as printed.
    pub ac_text: Option<String>,
    /// Hit points.
    pub hp: Option<u32>,
    /// The `hp` setter as printed (`10 (3d4+3)`).
    pub hp_text: Option<String>,
    /// Hit dice.
    pub hp_formula: Option<String>,
    /// The `speed` setter as printed.
    pub speed: Option<String>,
    /// Speeds in feet by mode (`walk`, `fly`, `climb`, `swim`, `burrow`), the ones printed.
    pub speeds: BTreeMap<String, u32>,
    /// Ability scores `str` .. `cha`; `null` when not printed.
    pub abilities: BTreeMap<String, Option<u32>>,
    /// Saving throw bonuses printed (proficient saves), `str` .. `cha`.
    pub saves: BTreeMap<String, i32>,
    /// Proficiency bonus: printed, else the one of its CR; `null` without either.
    pub proficiency: Option<i32>,
    /// Skills, senses, languages, challenge, size, type and alignment as printed.
    pub skills: Option<String>,
    /// Senses.
    pub senses: Option<String>,
    /// Languages.
    pub languages: Option<String>,
    /// Challenge rating as printed.
    pub challenge: Option<String>,
    /// Size.
    pub size: Option<String>,
    /// Type.
    #[serde(rename = "type")]
    pub creature_type: Option<String>,
    /// Alignment.
    pub alignment: Option<String>,
    /// Damage and condition lines (`Damage Resistances`: `cold; ...`).
    pub defenses: Vec<Named>,
    /// Linked `Companion Trait` elements.
    pub traits: Vec<Named>,
    /// Linked `Companion Action` elements.
    pub actions: Vec<Named>,
    /// Linked `Companion Reaction` elements.
    pub reactions: Vec<Named>,
}

const ABILITY_KEYS: [(&str, &str); 6] = [
    ("str", "strength"),
    ("dex", "dexterity"),
    ("con", "constitution"),
    ("int", "intelligence"),
    ("wis", "wisdom"),
    ("cha", "charisma"),
];

fn named(features: Vec<borealis_lib::content_dto::CompanionFeature>) -> Vec<Named> {
    features
        .into_iter()
        .map(|f| Named {
            name: f.name,
            text: f.text,
        })
        .collect()
}

/// The block of an Aurora `Companion` element, read with upstream's companion reader for its linked elements.
fn companion_detail(el: &Element, hub: &ContentHub) -> CompanionDetail {
    let block = borealis_lib::companion_block::element_block(el, &|id| hub.element(id).cloned());
    let text = |name: &str| setter(el, name).map(str::to_string);
    let mut speeds = BTreeMap::new();
    if let Some(walk) = setter(el, "speed").and_then(leading_u32) {
        speeds.insert("walk".to_string(), walk);
    }
    for (mode, feet) in ["fly", "climb", "swim", "burrow"].iter().zip(block.speeds) {
        if let Ok(feet) = u32::try_from(feet) {
            if feet > 0 {
                speeds.insert((*mode).to_string(), feet);
            }
        }
    }
    let printed = setter(el, "proficiency").is_some()
        || setter(el, "challenge").and_then(Cr::parse).is_some();
    CompanionDetail {
        ac: setter(el, "ac").and_then(leading_u32),
        ac_text: text("ac"),
        hp: setter(el, "hp").and_then(leading_u32),
        hp_text: text("hp"),
        hp_formula: setter(el, "hp").and_then(hp_formula),
        speed: text("speed"),
        speeds,
        abilities: ABILITY_KEYS
            .iter()
            .map(|(key, name)| ((*key).to_string(), setter(el, name).and_then(leading_u32)))
            .collect(),
        saves: ABILITY_KEYS
            .iter()
            .zip(block.lines.save_bonuses)
            .filter_map(|((key, _), bonus)| bonus.map(|b| ((*key).to_string(), b)))
            .collect(),
        proficiency: printed.then_some(block.proficiency),
        skills: text("skills"),
        senses: text("senses"),
        languages: text("languages"),
        challenge: text("challenge"),
        size: text("size"),
        creature_type: text("type"),
        alignment: text("alignment"),
        defenses: named(block.lines.defenses),
        traits: named(block.traits),
        actions: named(block.actions),
        reactions: named(block.reactions),
    }
}

/// One creature with its stat block, for an encounter snapshot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonsterDetail {
    /// Element id.
    pub id: String,
    /// Name.
    pub name: String,
    /// `monster` or `companion`.
    pub kind: &'static str,
    /// Source name.
    pub source: String,
    /// Pack.
    pub pack_id: String,
    /// Pack revision.
    pub pack_rev: String,
    /// `dnd2014`, `dnd2024` or `any`.
    pub ruleset: String,
    /// The typed stat block (`kind` monster).
    pub monster: Option<MonsterData>,
    /// The companion's block (`kind` companion).
    pub companion: Option<CompanionDetail>,
    /// The element's `maps_to`.
    pub maps_to: Option<Value>,
    /// Captured details for this pack revision.
    pub meta: Option<Meta>,
}

/// One typed Monster or Aurora Companion element of `owner`'s own content; `None` for any other id.
///
/// # Errors
/// The workspace's and the database's errors.
pub async fn monster(state: &AppState, owner: &str, id: &str) -> ApiResult<Option<MonsterDetail>> {
    let index = index(state, owner).await?;
    let Some(summary) = index.items.iter().find(|s| s.id == id).cloned() else {
        return Ok(None);
    };
    let meta = index.meta.get(id).cloned();
    let workspace = state.workspaces.get(owner).await?;
    let wanted = id.to_string();
    let found = workspace
        .blocking(move |handle| {
            let backend = handle.state::<Backend>();
            let hub = backend.hub().map_err(|e| upstream(&e))?;
            Ok(hub.store.element(&wanted).and_then(|el| {
                let maps_to = el
                    .maps_to
                    .as_ref()
                    .and_then(|x| serde_json::to_value(x).ok());
                if el.ty == COMPANION {
                    return Some((None, Some(companion_detail(el, &hub)), maps_to));
                }
                let mut m = MonsterData::from_element(el).ok()?;
                // No XP anywhere (the user's D16): the block is sent without it.
                m.xp = None;
                m.xp_lair = None;
                Some((Some(m), None, maps_to))
            }))
        })
        .await?;
    Ok(found.map(|(monster, companion, maps_to)| MonsterDetail {
        id: summary.id,
        name: summary.name,
        kind: summary.kind,
        source: summary.source,
        pack_id: summary.pack_id,
        pack_rev: summary.pack_rev,
        ruleset: summary.ruleset,
        monster,
        companion,
        maps_to,
        meta,
    }))
}

// ---------------------------------------------------------------------------------------------------------------
// Coverage
// ---------------------------------------------------------------------------------------------------------------

fn extras_counts(metas: &[&Meta]) -> Value {
    let n = |f: &dyn Fn(&Meta) -> bool| metas.iter().filter(|m| f(m)).count();
    json!({
        "environment": n(&|m| !m.environments.is_empty()),
        "page": n(&|m| m.page.is_some()),
        "mythic": n(&|m| m.mythic.is_some()),
        "covenCr": n(&|m| m.coven_cr.is_some()),
        "altSpeeds": n(&|m| !m.alt_speeds.is_empty()),
        "variants": n(&|m| !m.variants.is_empty()),
        "reactionIntro": n(&|m| !m.reaction_intro.is_empty()),
        "fluff": n(&|m| m.fluff),
        "tokens": n(&|m| m.token),
        "spellcasting": n(&|m| m.spellcasting.is_some()),
    })
}

/// The coverage report of `owner`'s content (`GET /api/bestiary/coverage`).
///
/// # Errors
/// The workspace's and the database's errors.
pub async fn coverage(state: &AppState, owner: &str) -> ApiResult<Value> {
    let index = index(state, owner).await?;
    let imports = stored_imports(state, owner).await?;
    let mut by_pack: BTreeMap<&str, Vec<&Summary>> = BTreeMap::new();
    for s in &index.items {
        by_pack.entry(s.pack_id.as_str()).or_default().push(s);
    }
    let mut packs = Vec::new();
    let (mut total_monsters, mut total_companions) = (0, 0);
    for pack in &index.packs {
        let items = by_pack.remove(pack.id.as_str()).unwrap_or_default();
        if items.is_empty() {
            continue;
        }
        let monsters = items.iter().filter(|s| s.kind == KIND_MONSTER).count();
        let companions = items.len() - monsters;
        total_monsters += monsters;
        total_companions += companions;
        let metas: Vec<&Meta> = items.iter().filter_map(|s| index.meta.get(&s.id)).collect();
        let record = imports.get(&pack.id).filter(|(rev, ..)| *rev == pack.rev);
        let (in_data, skipped, partial, reasons, notes) = match (pack.kind, record) {
            (_, Some((_, in_data, skipped, partial))) => (
                Some(usize::try_from(*in_data).unwrap_or(0)),
                skipped["total"].as_u64().unwrap_or(0),
                partial["total"].as_u64().unwrap_or(0),
                skipped["reasons"].clone(),
                partial["notes"].clone(),
            ),
            ("bundled", None) => (Some(monsters), 0, 0, json!([]), json!([])),
            _ => (None, 0, 0, json!([]), json!([])),
        };
        let status = if monsters == 0 {
            "companions_only"
        } else if in_data.is_none() {
            if pack.kind == "5etools" {
                "unknown"
            } else {
                "installed"
            }
        } else if skipped == 0 && partial == 0 && in_data == Some(monsters) {
            "complete"
        } else {
            "partial"
        };
        let source_names = facet(items.iter().map(|s| s.source.as_str()));
        packs.push(json!({
            "packId": pack.id,
            "name": pack.name,
            "kind": pack.kind,
            "enabled": pack.enabled,
            "packRev": pack.rev,
            "status": status,
            "monsters": monsters,
            "companions": companions,
            "inData": in_data,
            "skipped": skipped,
            "partial": partial,
            "skippedReasons": if reasons.is_array() { reasons } else { json!([]) },
            "partialNotes": if notes.is_array() { notes } else { json!([]) },
            "sources": source_names.iter().map(|c| json!({ "name": c.value, "monsters": c.count })).collect::<Vec<_>>(),
            "byCr": cr_facet(items.iter().filter_map(|s| s.cr_rank)),
            "byType": facet(items.iter().filter_map(|s| s.creature_type.as_deref())),
            "rulesets": facet(items.iter().map(|s| s.ruleset.as_str())),
            "extras": extras_counts(&metas),
            "captured": record.is_some() && !metas.is_empty(),
        }));
    }
    Ok(json!({
        "totals": { "packs": packs.len(), "monsters": total_monsters, "companions": total_companions },
        "packs": packs,
    }))
}

// ---------------------------------------------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------------------------------------------

async fn coverage_route(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    Ok(Json(coverage(&state, &user.id).await?))
}

async fn search_route(
    State(state): State<AppState>,
    user: User,
    Json(query): Json<SearchQuery>,
) -> ApiResult<Json<SearchPage>> {
    Ok(Json(search(&state, &user.id, &query).await?))
}

async fn facets_route(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    let index = index(&state, &user.id).await?;
    Ok(Json(facets_of(&index)))
}

async fn monster_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<MonsterDetail>> {
    monster(&state, &user.id, &id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("No such creature.".into()))
}

pub fn routes() -> axum::Router<AppState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/bestiary/coverage", get(coverage_route))
        .route("/api/bestiary/search", post(search_route))
        .route("/api/bestiary/facets", get(facets_route))
        .route("/api/bestiary/monsters/{id}", get(monster_route))
}
