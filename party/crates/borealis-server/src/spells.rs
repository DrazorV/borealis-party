//! Spells as mechanics (0.3.2): the account's spell index and what its DM decided about spells.
//!
//! * **One spell per edition** (the user's decision on 2026-11-13): the same spell from several sources (same
//!   normalized name, same ruleset) is ONE spell. [`Index`] groups the account's installed `Spell` elements so;
//!   the chosen copy is the DM's override when there is one (an `alias` row), else [`choose`]'s rule. The other copies
//!   are hidden, never deleted: the content files stay as they are, and a spell lists them as `alsoIn`. 2014 and 2024
//!   versions are different spells, and different names never merge.
//! * **Mechanics** (`spell_mechanics`, migration step 9), per account, by `kind`: `def` (an effect the DM built),
//!   `link` ("use that spell's mechanics") and `alias` (a name that means this spell). [`mechanics_for`] resolves a
//!   spell for combat: the account's def, then its link, then the curated SRD row (`party-spells`). A suggestion
//!   (a draft from the user's own 5etools tags) never binds by itself: the DM confirms it once, which saves a def.
//! * **Files:** a JSON export and import of the account's rows (not in homebrew packs yet, the user's Q5).
//! * **Who:** every route is the caller's own account. Nothing here reaches a player: a combat's player projection
//!   never carries mechanics, links or casting (combat.rs, `view_for`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use borealis_content::Element;
use borealis_lib::content_dto::{ContentPack, PackKind};
use borealis_lib::state::{AppState as Backend, ContentHub};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tauri::Manager;

use party_combat::spell::{cantrip_tier, CastSpec, MechOrigin, SpellLink, SpellMech};
use party_combat::{Combatant, Edition};
use party_spells::canonical::{Candidate, Catalog, Group, Link, Preferences, Reason};

use crate::auth::User;
use crate::db::now;
use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// The element type of a spell.
pub const SPELL: &str = "Spell";
/// `kind` of a DM-built effect.
pub const KIND_DEF: &str = "def";
/// `kind` of a link to another spell's mechanics.
pub const KIND_LINK: &str = "link";
/// `kind` of a name that means a spell.
pub const KIND_ALIAS: &str = "alias";
const KINDS: [&str; 3] = [KIND_DEF, KIND_LINK, KIND_ALIAS];
/// The rulesets a row may name.
const RULESETS: [&str; 3] = ["dnd2014", "dnd2024", "any"];
/// Where a row came from.
const ORIGINS: [&str; 3] = ["dm", "suggested", "import"];
/// The export file's `format`.
pub const FILE_FORMAT: &str = "borealis-party-spell-mechanics";
/// The export file's version.
const FILE_V: i64 = 1;
/// Rows one account may keep.
const MAX_ROWS: i64 = 5_000;
/// Longest spell key, alias name or target.
const MAX_KEY: usize = 200;
/// Largest effect definition, in bytes of JSON.
const MAX_DEF: usize = 64 * 1024;
/// Largest import file.
pub const MAX_IMPORT: usize = 4 * 1024 * 1024;
/// Default and largest page of [`list`].
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 500;
/// Accounts whose index is kept in memory at once.
const CACHED_ACCOUNTS: usize = 64;

fn upstream(e: &borealis_lib::error::ApiError) -> ApiError {
    ApiError::Internal(e.message.clone())
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::Invalid(message.into())
}

fn no_spell() -> ApiError {
    ApiError::NotFound("There is no such spell in your content.".into())
}

// ---------------------------------------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------------------------------------

/// A spell name as the "one spell" grouping compares it: lowercase letters and digits, words separated by one space;
/// apostrophes vanish (`Tasha's` = `Tashas`), other punctuation separates words. `Fireball` and `Delayed Blast
/// Fireball` stay different.
#[must_use]
pub fn normalize(name: &str) -> String {
    party_spells::canonical::normalize(name)
}

// ---------------------------------------------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------------------------------------------

/// One installed copy of a spell.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Copy {
    /// The element id: the spell key.
    pub key: String,
    /// The element's source name.
    pub source: String,
    /// The pack.
    pub pack_id: String,
    /// `bundled`, `5etools`, `index` or `custom`.
    pub pack_kind: &'static str,
}

/// One spell: all its copies of one name, ruleset and level (`party-spells` canonical groups).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Spell {
    /// The chosen copy's element id.
    pub key: String,
    /// Its name.
    pub name: String,
    /// `dnd2014`, `dnd2024` or `any`.
    pub ruleset: String,
    /// 0 for a cantrip.
    pub level: Option<u8>,
    /// The school, as the element's setter has it.
    pub school: Option<String>,
    /// From the element's `isConcentration` setter (never guessed from text).
    pub concentration: bool,
    /// From `isRitual`.
    pub ritual: bool,
    /// The chosen copy's source.
    pub source: String,
    /// The chosen copy's pack.
    pub pack_id: String,
    /// Why this copy: `only`, `dmPick`, `newest`, `srd`, `preferred` or `stable`.
    pub chosen_by: Reason,
    /// The other copies, hidden from new picks (never deleted).
    pub also_in: Vec<Copy>,
    #[serde(skip)]
    norm: String,
    /// Every copy's id, the chosen one first.
    #[serde(skip)]
    members: Vec<String>,
}

/// An installed copy with what the list shows.
#[derive(Debug, Clone)]
struct Entry {
    copy: Copy,
    name: String,
    norm: String,
    ruleset: String,
    level: Option<u8>,
    school: Option<String>,
    concentration: bool,
    ritual: bool,
}

/// An account's installed spells, for one content load.
pub struct Index {
    hub: Weak<ContentHub>,
    /// Every copy, by element id.
    entries: HashMap<String, Entry>,
    /// The resolver's input.
    candidates: Vec<Candidate>,
}

/// The index with the account's choices applied: one spell per name, ruleset and level.
pub struct Book {
    index: Arc<Index>,
    catalog: Catalog,
}

impl Book {
    /// The one spell `key` (any copy's id) is.
    #[must_use]
    pub fn spell(&self, key: &str) -> Option<Spell> {
        spell_of(&self.index, self.catalog.group(key)?)
    }

    /// `key` first, then the other copies of its spell.
    #[must_use]
    pub fn copies(&self, key: &str) -> Vec<String> {
        let mut out = vec![key.to_string()];
        if let Some(g) = self.catalog.group(key) {
            out.extend(
                g.members
                    .iter()
                    .map(|(id, _)| id.clone())
                    .filter(|id| id != key),
            );
        }
        out
    }
}

fn cache() -> &'static Mutex<HashMap<String, Arc<Index>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Index>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
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

fn setter<'a>(el: &'a Element, name: &str) -> Option<&'a str> {
    el.setter_value(name)
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn truthy(el: &Element, name: &str) -> bool {
    setter(el, name).is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

/// The pack an element was loaded from: its file is named `<pack id>/<path>`.
fn pack_of_file(file: &str) -> &str {
    file.split(['/', '\\']).next().unwrap_or(file)
}

struct PackInfo {
    kind: &'static str,
    ruleset: Option<String>,
}

fn build(backend: &Backend) -> ApiResult<Index> {
    let hub = backend.hub().map_err(|e| upstream(&e))?;
    let listed = backend.list_packs().map_err(|e| upstream(&e))?;
    let packs: HashMap<String, PackInfo> = listed
        .iter()
        .map(|p: &ContentPack| {
            (
                p.id.clone(),
                PackInfo {
                    kind: kind_of(&p.kind),
                    ruleset: match p.rulesets.as_slice() {
                        [one] => Some(text_value(one)),
                        _ => None,
                    },
                },
            )
        })
        .collect();
    let mut entries = HashMap::new();
    let mut candidates = Vec::new();
    for stored in hub.store.iter() {
        let el = &stored.element;
        if el.ty != SPELL {
            continue;
        }
        let pack_id = pack_of_file(&stored.file).to_string();
        let pack = packs.get(&pack_id);
        let ruleset = el
            .ruleset
            .map(|r| text_value(&r))
            .or_else(|| pack.and_then(|p| p.ruleset.clone()))
            .unwrap_or_else(|| "any".to_string());
        let level = setter(el, "level").and_then(|l| l.parse().ok());
        candidates.push(Candidate {
            id: el.id.clone(),
            name: el.name.clone(),
            ruleset: ruleset.clone(),
            source: el.source.clone(),
            level,
            date: party_spells::canonical::srd_date(&el.source).map(str::to_string),
        });
        entries.insert(
            el.id.clone(),
            Entry {
                copy: Copy {
                    key: el.id.clone(),
                    source: el.source.clone(),
                    pack_id,
                    pack_kind: pack.map_or("custom", |p| p.kind),
                },
                name: el.name.clone(),
                norm: normalize(&el.name),
                ruleset,
                level,
                school: setter(el, "school").map(str::to_string),
                concentration: truthy(el, "isConcentration"),
                ritual: truthy(el, "isRitual"),
            },
        );
    }
    Ok(Index {
        hub: Arc::downgrade(&hub),
        entries,
        candidates,
    })
}

/// The account's spell index: the cached one while the content is the same, else a fresh one. Spell details a
/// 5etools import captured are stored first ([`ingest`]).
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
    let keep = cached.clone();
    let built = workspace
        .blocking(move |handle| {
            let backend = handle.state::<Backend>();
            let hub = backend.hub().map_err(|e| upstream(&e))?;
            if keep.is_some_and(|c| Weak::ptr_eq(&c.hub, &Arc::downgrade(&hub))) {
                return Ok(None);
            }
            build(&backend).map(Some)
        })
        .await?;
    let Some(built) = built else {
        return Ok(cached.expect("fresh only with a cached index"));
    };
    let index = Arc::new(built);
    let mut map = cache().lock().unwrap_or_else(PoisonError::into_inner);
    if map.len() >= CACHED_ACCOUNTS && !map.contains_key(owner) {
        map.clear();
    }
    map.insert(owner.to_string(), index.clone());
    Ok(index)
}

/// The DM's choices as the resolver takes them, from the account's `alias` rows: an alias of the spell's own name is
/// the DM's pick of that copy; any other name is a link of that name to the spell.
async fn preferences(state: &AppState, owner: &str, index: &Index) -> ApiResult<Preferences> {
    let rows = sqlx::query(
        "SELECT alias_name, ruleset, spell_key FROM spell_mechanics WHERE account_id = $1 AND kind = $2",
    )
    .bind(owner)
    .bind(KIND_ALIAS)
    .fetch_all(state.db.pool())
    .await?;
    let mut prefs = Preferences::default();
    for row in rows {
        let alias: String = row.try_get(0)?;
        let ruleset: String = row.try_get(1)?;
        let key: String = row.try_get(2)?;
        match index.entries.get(&key) {
            Some(e) if e.norm == alias && e.ruleset == ruleset => {
                prefs.picks.insert(key.clone(), key);
            }
            Some(_) => {
                prefs.links.insert(format!("{ruleset}:{alias}"), key);
            }
            // A choice of a copy that is gone: the rule chooses again.
            None => {}
        }
    }
    Ok(prefs)
}

/// The account's spells with its choices applied.
///
/// # Errors
/// When the workspace or the database fails.
pub async fn book(state: &AppState, owner: &str) -> ApiResult<Book> {
    let index = index(state, owner).await?;
    let prefs = preferences(state, owner, &index).await?;
    let catalog = Catalog::build(&index.candidates, &prefs);
    Ok(Book { index, catalog })
}

// ---------------------------------------------------------------------------------------------------------------
// Hidden copies (one spell per edition in the pickers)
// ---------------------------------------------------------------------------------------------------------------

/// Most ids upstream's `hiddenElements` takes.
const MAX_HIDDEN: usize = 20_000;

/// Accounts whose hidden copies are up to date in this process.
fn synced() -> &'static Mutex<HashSet<String>> {
    static SYNCED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SYNCED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// The account's spells may have changed (content, a DM's pick): [`sync_hidden_if_stale`] works them out again.
pub fn mark_stale(account: &str) {
    synced()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(account);
}

/// Push the copies the canonical resolver hides (every copy of a spell but the chosen one, per name, edition and
/// level) into the account's engine settings (`hiddenElements`), so builder and spell pickers offer one copy. Only
/// when the set changed: every change rebuilds the account's engine. A character that chose a hidden copy keeps it
/// (upstream replays stored choices). Answers whether it changed.
///
/// # Errors
/// The workspace's and the database's.
pub async fn sync_hidden(state: &AppState, account: &str) -> ApiResult<bool> {
    let hidden: BTreeSet<String> = book(state, account)
        .await?
        .catalog
        .hidden()
        .into_iter()
        .filter(|id| id.len() <= MAX_KEY)
        .take(MAX_HIDDEN)
        .collect();
    let workspace = state.workspaces.get(account).await?;
    let changed = workspace
        .blocking(move |handle| {
            let backend = handle.state::<Backend>();
            let current = backend.settings().map_err(|e| upstream(&e))?;
            if current.hidden_elements == hidden {
                return Ok(false);
            }
            backend
                .save_settings(&json!({ "hiddenElements": hidden }))
                .map_err(|e| upstream(&e))?;
            Ok(true)
        })
        .await?;
    if changed {
        // Another hub: the index is built again on its next use.
        cache()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(account);
    }
    Ok(changed)
}

/// [`sync_hidden`] once per change: the first command of an account in this process, and after its content or its
/// DM's picks changed. A failure is logged and tried again on the next command; the command itself goes on.
pub async fn sync_hidden_if_stale(state: &AppState, account: &str) {
    if synced()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(account)
    {
        return;
    }
    match sync_hidden(state, account).await {
        Ok(_) => {
            synced()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(account.to_string());
        }
        Err(e) => tracing::warn!("spells: hidden copies not updated: {e:?}"),
    }
}

/// The spell of a canonical group.
fn spell_of(index: &Index, group: &Group) -> Option<Spell> {
    let e = index.entries.get(&group.canonical)?;
    Some(Spell {
        key: e.copy.key.clone(),
        name: e.name.clone(),
        ruleset: e.ruleset.clone(),
        level: e.level,
        school: e.school.clone(),
        concentration: e.concentration,
        ritual: e.ritual,
        source: e.copy.source.clone(),
        pack_id: e.copy.pack_id.clone(),
        chosen_by: group.reason,
        also_in: group
            .members
            .iter()
            .skip(1)
            .filter_map(|(id, _)| index.entries.get(id).map(|e| e.copy.clone()))
            .collect(),
        norm: e.norm.clone(),
        members: group.members.iter().map(|(id, _)| id.clone()).collect(),
    })
}

/// The one spell `key` (any copy's id) is.
///
/// # Errors
/// When the workspace or the database fails.
pub async fn canonical(state: &AppState, owner: &str, key: &str) -> ApiResult<Option<Spell>> {
    Ok(book(state, owner).await?.spell(key))
}

/// Resolve a printed spell name (a monster spellbook's line) in `ruleset` (then `any`): the DM's link of that name
/// first, then the one spell of that name. `None` when nothing matches, or when several spells of different levels
/// do (the DM picks once).
///
/// # Errors
/// When the workspace or the database fails.
pub async fn resolve_name(
    state: &AppState,
    owner: &str,
    name: &str,
    ruleset: &str,
) -> ApiResult<Option<Spell>> {
    let book = book(state, owner).await?;
    Ok(match book.catalog.resolve(name, ruleset, None) {
        Link::Linked { id } => book.spell(&id),
        _ => None,
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Captured 5etools spells (for "Suggested" drafts)
// ---------------------------------------------------------------------------------------------------------------

/// Where [`capture`] leaves its spool files, under the account's data folder.
pub const SPOOL_DIR: &str = "sync/spells";

/// One pack's captured spells, as [`capture`] spools them and [`ingest`] stores them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Spool {
    pack_id: String,
    pack_rev: String,
    captured_at: i64,
    /// The raw 5etools spell objects, by element id.
    spells: BTreeMap<String, Value>,
}

/// The raw 5etools spells of a data folder (its `spells/spells-*.json` and homebrew files).
fn read_spells(folder: &std::path::Path) -> Result<Vec<Value>, String> {
    let files = borealis_import_5etools::folder::data_files(folder).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for rel in files {
        if !(rel.starts_with("spells/spells-") || rel.starts_with("homebrew/")) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(folder.join(&rel)) else {
            continue;
        };
        let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(Value::Array(list)) = value.get_mut("spell").map(Value::take) {
            out.extend(list);
        }
    }
    Ok(out)
}

/// Read the raw 5etools data of the spells that `report` imported from `folder`, and spool it for [`ingest`] (one
/// file per pack): the tags the import drops (conditions, `miscTags`, the `{@damage}` dice) are what the drafter
/// reads. Answers how many spells were captured.
///
/// # Errors
/// When the folder can't be read or the spool can't be written; the import itself is not affected.
pub fn capture(
    backend: &Backend,
    folder: &std::path::Path,
    report: &borealis_lib::content_dto::FiveToolsImportReport,
) -> Result<usize, String> {
    use borealis_import_5etools::ids::{IdAlloc, IdKind};
    let wanted: std::collections::HashSet<String> =
        report.packs.iter().map(|p| p.id.clone()).collect();
    if wanted.is_empty() {
        return Ok(0);
    }
    let raw = read_spells(folder)?;
    let hub = backend.hub().map_err(|e| e.message)?;
    // Installed spells of those packs: by id, and by lowercase name per pack.
    let mut pack_of: HashMap<String, String> = HashMap::new();
    let mut names: HashMap<(String, String), Vec<String>> = HashMap::new();
    for stored in hub.store.iter() {
        let el = &stored.element;
        let pack = pack_of_file(&stored.file);
        if el.ty != SPELL || !wanted.contains(pack) {
            continue;
        }
        pack_of.insert(el.id.clone(), pack.to_string());
        names
            .entry((pack.to_string(), el.name.to_lowercase()))
            .or_default()
            .push(el.id.clone());
    }
    let dirs: HashMap<String, std::path::PathBuf> = backend
        .installed_packs()
        .into_iter()
        .filter(|p| wanted.contains(&p.manifest.id))
        .map(|p| (p.manifest.id, p.dir))
        .collect();
    let captured_at = now();
    let mut spools: BTreeMap<String, Spool> = BTreeMap::new();
    let mut got = 0;
    for spell in raw {
        let (Some(source), Some(name)) = (
            spell.get("source").and_then(Value::as_str),
            spell.get("name").and_then(Value::as_str),
        ) else {
            continue;
        };
        let pack = borealis_import_5etools::pack::pack_id(source);
        if !wanted.contains(&pack) {
            continue;
        }
        let plain = IdAlloc::new().named(IdKind::Spell, source, name);
        let id = if pack_of.get(&plain) == Some(&pack) {
            Some(plain)
        } else {
            names
                .get(&(pack.clone(), name.to_lowercase()))
                .filter(|ids| ids.len() == 1)
                .map(|ids| ids[0].clone())
        };
        let Some(id) = id else {
            continue;
        };
        let spool = spools.entry(pack.clone()).or_insert_with(|| Spool {
            pack_id: pack.clone(),
            pack_rev: dirs
                .get(&pack)
                .map(|d| crate::bestiary::pack_rev(d))
                .unwrap_or_default(),
            captured_at,
            spells: BTreeMap::new(),
        });
        spool.spells.insert(id, spell);
        got += 1;
    }
    let dir = backend.paths.data.join(SPOOL_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for spool in spools.values() {
        let name: String = spool
            .pack_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let file = dir.join(format!("{name}.json"));
        let tmp = file.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(spool).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
    }
    Ok(got)
}

/// [`capture`], with its outcome logged instead of returned: called after a successful 5etools import.
pub fn capture_logged(
    backend: &Backend,
    folder: &std::path::Path,
    report: &borealis_lib::content_dto::FiveToolsImportReport,
) {
    match capture(backend, folder, report) {
        Ok(n) => tracing::info!(spells = n, "spells: 5etools spell details captured"),
        Err(e) => tracing::warn!("spells: 5etools spell details not captured: {e}"),
    }
}

/// Move the account's spooled spell captures into `spell_meta`, one pack per transaction (its rows replaced whole).
/// A spool file that can't be read is removed.
///
/// # Errors
/// When the database fails; the spool file then stays for the next call.
pub async fn ingest(state: &AppState, owner: &str) -> ApiResult<usize> {
    let dir = state.workspaces.dir(owner).join(SPOOL_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };
    let files: Vec<std::path::PathBuf> = entries
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
            tracing::warn!("spells: unreadable capture {} removed", file.display());
            let _ = std::fs::remove_file(&file);
            continue;
        };
        let mut tx = state.db.pool().begin().await?;
        sqlx::query("DELETE FROM spell_meta WHERE owner_id = $1 AND pack_id = $2")
            .bind(owner)
            .bind(&spool.pack_id)
            .execute(&mut *tx)
            .await?;
        for (id, raw) in &spool.spells {
            sqlx::query(
                "INSERT INTO spell_meta (owner_id, element_id, pack_id, pack_rev, raw_json, captured_at)
                 VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING",
            )
            .bind(owner)
            .bind(id)
            .bind(&spool.pack_id)
            .bind(&spool.pack_rev)
            .bind(raw.to_string())
            .bind(spool.captured_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        let _ = std::fs::remove_file(&file);
        done += 1;
    }
    Ok(done)
}

/// The raw 5etools data captured for a spell element of the account, if any.
async fn raw_of(state: &AppState, owner: &str, key: &str) -> ApiResult<Option<Value>> {
    let row =
        sqlx::query("SELECT raw_json FROM spell_meta WHERE owner_id = $1 AND element_id = $2")
            .bind(owner)
            .bind(key)
            .fetch_optional(state.db.pool())
            .await?;
    Ok(row
        .map(|r| r.try_get::<String, _>(0))
        .transpose()?
        .and_then(|t| serde_json::from_str(&t).ok()))
}

/// What the drafter reads when nothing was captured: the tags the import keeps on the element (`el.data`), its name
/// and level. No dice or conditions: the draft then asks the DM for them.
fn tags_of(element: &Element) -> Value {
    let mut v = json!({
        "name": element.name,
        "level": setter(element, "level").and_then(|l| l.parse::<u8>().ok()).unwrap_or(0),
        "entries": [],
        "duration": [{ "type": "timed", "concentration": truthy(element, "isConcentration") }],
    });
    for (from, to) in [
        ("damage_inflict", "damageInflict"),
        ("saving_throw", "savingThrow"),
        ("scalingLevelDice", "scalingLevelDice"),
        ("spellAttack", "spellAttack"),
        ("areaTags", "areaTags"),
        ("affectsCreatureType", "affectsCreatureType"),
    ] {
        if let Some(x) = element.data.get(from) {
            v[to] = x.clone();
        }
    }
    v
}

// ---------------------------------------------------------------------------------------------------------------
// Mechanics rows
// ---------------------------------------------------------------------------------------------------------------

/// One `spell_mechanics` row, as the API and the export file carry it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MechRow {
    /// `def`, `link` or `alias`.
    pub kind: String,
    /// The spell's element id.
    pub spell_key: String,
    /// `dnd2014`, `dnd2024` or `any`.
    pub ruleset: String,
    /// For an alias: the normalized name. Empty otherwise.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub alias_name: String,
    /// For a def: the effect; for a link: `{"target": <spell key>}`. Empty for an alias.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub def: Value,
    /// The effect schema's version.
    #[serde(default = "one")]
    pub schema_v: i64,
    /// Bumped on every save.
    #[serde(default = "one")]
    pub rev: i64,
    /// `dm`, `suggested` or `import`.
    #[serde(default = "dm")]
    pub origin: String,
    /// The drafted text's hash, for "this changed, check it again".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    /// When it was saved last (seconds).
    #[serde(default)]
    pub updated_at: i64,
}

fn one() -> i64 {
    1
}

fn dm() -> String {
    "dm".into()
}

fn key_ok(key: &str) -> bool {
    !key.trim().is_empty() && key.len() <= MAX_KEY && !key.chars().any(char::is_control)
}

/// The engine's edition of a ruleset (`dnd2014` -> 2014); `None` for `any`.
#[must_use]
pub fn edition_of(ruleset: &str) -> Option<Edition> {
    match ruleset {
        "dnd2014" => Some(Edition::Srd51),
        "dnd2024" => Some(Edition::Srd521),
        _ => None,
    }
}

/// The ruleset of an engine edition.
#[must_use]
pub fn ruleset_of(edition: Edition) -> &'static str {
    match edition {
        Edition::Srd51 => "dnd2014",
        Edition::Srd521 => "dnd2024",
    }
}

/// A def as sent: the engine's closed [`SpellMech`] (no scripting), for this spell key and an edition that fits the
/// row's ruleset. Its `key` is the row's. Answers the stored JSON and its schema version.
fn check_def(def: &Value, key: &str, ruleset: &str) -> ApiResult<(Value, i64)> {
    if !def.is_object() {
        return Err(invalid("An effect definition is a JSON object."));
    }
    if def.to_string().len() > MAX_DEF {
        return Err(invalid("That effect definition is too large."));
    }
    let mut mech: SpellMech = serde_json::from_value(def.clone())
        .map_err(|e| invalid(format!("That effect definition doesn't fit: {e}")))?;
    if mech.v != party_combat::spell::MECH_VERSION {
        return Err(invalid("That effect definition is of another version."));
    }
    if let Some(edition) = edition_of(ruleset) {
        if mech.edition != edition {
            return Err(invalid("That effect definition is for the other edition."));
        }
    }
    mech.key = key.to_string();
    Ok((
        serde_json::to_value(&mech).map_err(|e| invalid(e.to_string()))?,
        i64::from(mech.v),
    ))
}

/// A row as sent, checked: the kind's fields present, everything in its limits.
fn check_row(row: &mut MechRow) -> ApiResult<()> {
    if !KINDS.contains(&row.kind.as_str()) {
        return Err(invalid("The kind is def, link or alias."));
    }
    if !RULESETS.contains(&row.ruleset.as_str()) {
        return Err(invalid("The ruleset is dnd2014, dnd2024 or any."));
    }
    if !ORIGINS.contains(&row.origin.as_str()) {
        return Err(invalid("The origin is dm, suggested or import."));
    }
    if !key_ok(&row.spell_key) {
        return Err(invalid("The spell key isn't valid."));
    }
    if row
        .source_hash
        .as_ref()
        .is_some_and(|h| h.len() > 128 || !h.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(invalid("The source hash isn't valid."));
    }
    match row.kind.as_str() {
        KIND_DEF => {
            row.alias_name.clear();
            let (def, v) = check_def(&row.def, &row.spell_key, &row.ruleset)?;
            row.def = def;
            row.schema_v = v;
        }
        KIND_LINK => {
            row.alias_name.clear();
            let target = row
                .def
                .get("target")
                .and_then(Value::as_str)
                .filter(|t| key_ok(t))
                .ok_or_else(|| invalid("A link names its target spell."))?
                .to_string();
            if target == row.spell_key {
                return Err(invalid("A spell can't link to itself."));
            }
            row.def = json!({ "target": target });
        }
        _ => {
            let name = normalize(&row.alias_name);
            if name.is_empty() || name.len() > MAX_KEY {
                return Err(invalid("An alias names the spell name it stands for."));
            }
            row.alias_name = name;
            row.def = Value::Null;
        }
    }
    Ok(())
}

fn row_from(r: &sqlx::any::AnyRow) -> sqlx::Result<MechRow> {
    let def: String = r.try_get(4)?;
    Ok(MechRow {
        kind: r.try_get(0)?,
        spell_key: r.try_get(1)?,
        ruleset: r.try_get(2)?,
        alias_name: r.try_get(3)?,
        def: serde_json::from_str(&def).unwrap_or(Value::Null),
        schema_v: r.try_get(5)?,
        rev: r.try_get(6)?,
        origin: r.try_get(7)?,
        source_hash: r.try_get(8)?,
        updated_at: r.try_get(9)?,
    })
}

const ROW_COLUMNS: &str =
    "kind, spell_key, ruleset, alias_name, def_json, schema_v, rev, origin, source_hash, updated_at";

/// Every row of the account, in a stable order.
///
/// # Errors
/// The database's.
pub async fn rows(state: &AppState, owner: &str) -> ApiResult<Vec<MechRow>> {
    let rows = sqlx::query(&format!(
        "SELECT {ROW_COLUMNS} FROM spell_mechanics WHERE account_id = $1 ORDER BY kind, ruleset, spell_key, alias_name"
    ))
    .bind(owner)
    .fetch_all(state.db.pool())
    .await?;
    Ok(rows.iter().map(row_from).collect::<sqlx::Result<_>>()?)
}

/// Save one row (insert or replace); a def's `rev` moves one past the stored one. Answers the stored row.
///
/// # Errors
/// A row that doesn't fit, too many rows, the database.
pub async fn save(state: &AppState, owner: &str, mut row: MechRow) -> ApiResult<MechRow> {
    check_row(&mut row)?;
    let mut tx = state.db.pool().begin().await?;
    let old: Option<(i64, i64)> = sqlx::query(
        "SELECT rev, created_at FROM spell_mechanics
         WHERE account_id = $1 AND spell_key = $2 AND ruleset = $3 AND kind = $4 AND alias_name = $5",
    )
    .bind(owner)
    .bind(&row.spell_key)
    .bind(&row.ruleset)
    .bind(&row.kind)
    .bind(&row.alias_name)
    .fetch_optional(&mut *tx)
    .await?
    .map(|r| Ok::<_, sqlx::Error>((r.try_get(0)?, r.try_get(1)?)))
    .transpose()?;
    if old.is_none() {
        let count: i64 = sqlx::query("SELECT COUNT(*) FROM spell_mechanics WHERE account_id = $1")
            .bind(owner)
            .fetch_one(&mut *tx)
            .await?
            .try_get(0)?;
        if count >= MAX_ROWS {
            return Err(invalid(format!(
                "An account keeps at most {MAX_ROWS} spell rules."
            )));
        }
    }
    // An alias is one per name: the DM's new choice replaces the old one.
    if row.kind == KIND_ALIAS {
        sqlx::query(
            "DELETE FROM spell_mechanics WHERE account_id = $1 AND kind = $2 AND ruleset = $3 AND alias_name = $4",
        )
        .bind(owner)
        .bind(KIND_ALIAS)
        .bind(&row.ruleset)
        .bind(&row.alias_name)
        .execute(&mut *tx)
        .await?;
    } else {
        // A spell is either defined or linked, never both.
        sqlx::query(
            "DELETE FROM spell_mechanics WHERE account_id = $1 AND spell_key = $2 AND ruleset = $3
             AND kind IN ($4, $5)",
        )
        .bind(owner)
        .bind(&row.spell_key)
        .bind(&row.ruleset)
        .bind(KIND_DEF)
        .bind(KIND_LINK)
        .execute(&mut *tx)
        .await?;
    }
    let at = now();
    row.rev = old.map_or(row.rev.max(1), |(rev, _)| rev + 1);
    row.updated_at = at;
    let def = if row.def.is_null() {
        "{}".to_string()
    } else {
        row.def.to_string()
    };
    sqlx::query(
        "INSERT INTO spell_mechanics (account_id, spell_key, ruleset, kind, alias_name, def_json, schema_v, rev,
            origin, source_hash, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(owner)
    .bind(&row.spell_key)
    .bind(&row.ruleset)
    .bind(&row.kind)
    .bind(&row.alias_name)
    .bind(def)
    .bind(row.schema_v)
    .bind(row.rev)
    .bind(&row.origin)
    .bind(&row.source_hash)
    .bind(old.map_or(at, |(_, created)| created))
    .bind(at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if row.kind == KIND_ALIAS {
        row.def = Value::Null;
        // A pick changes which copy is hidden (the route pushes it once it is done).
        mark_stale(owner);
    }
    Ok(row)
}

/// Remove one row. Answers whether there was one.
///
/// # Errors
/// The database's.
pub async fn remove(
    state: &AppState,
    owner: &str,
    kind: &str,
    spell_key: &str,
    ruleset: &str,
    alias_name: &str,
) -> ApiResult<bool> {
    let alias = if kind == KIND_ALIAS {
        normalize(alias_name)
    } else {
        String::new()
    };
    let done = sqlx::query(
        "DELETE FROM spell_mechanics
         WHERE account_id = $1 AND kind = $2 AND spell_key = $3 AND ruleset = $4 AND alias_name = $5",
    )
    .bind(owner)
    .bind(kind)
    .bind(spell_key)
    .bind(ruleset)
    .bind(alias)
    .execute(state.db.pool())
    .await?;
    if kind == KIND_ALIAS && done.rows_affected() > 0 {
        mark_stale(owner);
    }
    Ok(done.rows_affected() > 0)
}

// ---------------------------------------------------------------------------------------------------------------
// Resolving mechanics for combat
// ---------------------------------------------------------------------------------------------------------------

/// Where a spell's mechanics come from, frozen into the cast (replay and undo never depend on later edits).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    /// The effect definition.
    pub def: Value,
    /// `def` (the account's), `link` (another spell's, through the account's link) or `srd` (the curated table).
    pub origin: &'static str,
    /// The spell key whose definition it is.
    pub from_key: String,
    /// That definition's revision.
    pub rev: i64,
}

/// The curated SRD definition of a spell key for an edition (`party-spells`).
fn curated(key: &str, ruleset: &str) -> Option<(Value, i64)> {
    let mech = party_spells::data::curated(key)?;
    if edition_of(ruleset).is_some_and(|e| e != mech.edition) {
        return None;
    }
    Some((serde_json::to_value(mech).ok()?, i64::from(mech.rev)))
}

/// An account's spells and spell rows, read once: what a combat start (which holds its one connection) or a cast
/// compiles from without touching the database again.
pub struct Library {
    book: Book,
    /// The account's `def` and `link` rows.
    rows: Vec<MechRow>,
}

/// The account's [`Library`].
///
/// # Errors
/// The workspace's and the database's.
pub async fn library(state: &AppState, owner: &str) -> ApiResult<Library> {
    let book = book(state, owner).await?;
    let rows = rows(state, owner)
        .await?
        .into_iter()
        .filter(|r| r.kind != KIND_ALIAS)
        .collect();
    Ok(Library { book, rows })
}

impl Library {
    /// The chosen copy of the one spell a printed name is in `ruleset` (then `any`), through the DM's links; `None`
    /// when it is unlinked or ambiguous.
    #[must_use]
    pub fn resolve(&self, name: &str, ruleset: &str) -> Option<String> {
        match self.book.catalog.resolve(name, ruleset, None) {
            Link::Linked { id } => Some(id),
            _ => None,
        }
    }

    /// The mechanics of `key` cast in edition `ruleset`. The key is the one spell it is a copy of, so a definition
    /// of any copy counts (an SRD definition applies to the chosen 5etools copy, and back), the key's own first: the
    /// account's def, then its link (one hop, to the target's def or curated row), then the curated row. `None`: a
    /// ruling.
    #[must_use]
    pub fn mechanics(&self, ruleset: &str, key: &str) -> Option<Resolved> {
        let fits = |r: &&MechRow| r.ruleset == ruleset || r.ruleset == "any";
        let copies = self.book.copies(key);
        for copy in &copies {
            let own = || {
                self.rows
                    .iter()
                    .filter(fits)
                    .filter(|r| &r.spell_key == copy)
            };
            if let Some(d) = own().find(|r| r.kind == KIND_DEF) {
                return Some(Resolved {
                    def: d.def.clone(),
                    origin: "def",
                    from_key: copy.clone(),
                    rev: d.rev,
                });
            }
            if let Some(target) = own()
                .find(|r| r.kind == KIND_LINK)
                .and_then(|l| l.def.get("target").and_then(Value::as_str))
            {
                let def = self
                    .rows
                    .iter()
                    .filter(fits)
                    .find(|r| r.spell_key == target && r.kind == KIND_DEF)
                    .map(|r| (r.def.clone(), r.rev))
                    .or_else(|| curated(target, ruleset));
                if let Some((def, rev)) = def {
                    return Some(Resolved {
                        def,
                        origin: "link",
                        from_key: target.to_string(),
                        rev,
                    });
                }
            }
        }
        copies.iter().find_map(|copy| {
            curated(copy, ruleset).map(|(def, rev)| Resolved {
                def,
                origin: "srd",
                from_key: copy.clone(),
                rev,
            })
        })
    }

    /// A spell compiled for one caster (`caster`'s first spellcasting block: DC, attack, modifier and cantrip
    /// tier), from the account's mechanics for the combat's edition. With no mechanics: `fallback` when the stat
    /// block entry has its own, else a ruling built from the installed spell (it spends the slot and takes
    /// concentration from the element's `isConcentration`, never from its text). `Ok(None)` when the spell isn't
    /// installed and has no entry.
    ///
    /// # Errors
    /// A stored definition that no longer fits the engine's schema.
    pub fn compile(
        &self,
        edition: Edition,
        key: &str,
        caster: &Combatant,
        link: Option<&SpellLink>,
        fallback: Option<&CastSpec>,
    ) -> ApiResult<Option<CastSpec>> {
        let casting = caster.casting.first();
        let modifier = match (casting, caster.stats.as_ref()) {
            (Some(c), Some(stats)) => stats.modifier(c.ability),
            _ => 0,
        };
        let tier = casting
            .and_then(|c| c.cantrip_tier.or_else(|| c.caster_level.map(cantrip_tier)))
            .unwrap_or(1);
        let mut spec = if let Some(r) = self.mechanics(ruleset_of(edition), key) {
            let mut mech: SpellMech = serde_json::from_value(r.def.clone()).map_err(|e| {
                invalid(format!(
                    "The saved effect of this spell no longer fits: {e}"
                ))
            })?;
            mech.key = key.to_string();
            mech.edition = edition;
            let rev = u32::try_from(r.rev).unwrap_or(u32::MAX);
            let origin = if r.origin == "srd" {
                MechOrigin::Srd {
                    key: r.from_key.clone(),
                    rev,
                }
            } else {
                MechOrigin::Def {
                    id: r.from_key.clone(),
                    rev,
                }
            };
            CastSpec {
                v: party_combat::spell::MECH_VERSION,
                origin,
                mech,
                dc: None,
                attack: None,
                modifier: 0,
                cantrip_tier: 1,
                baked_in: false,
            }
        } else if let Some(f) = fallback {
            f.clone()
        } else {
            match self.book.spell(key) {
                Some(spell) => CastSpec::ruling(
                    key,
                    &spell.name,
                    edition,
                    spell.level.or(link.map(|l| l.level)).unwrap_or(0),
                    spell.concentration,
                ),
                None => return Ok(None),
            }
        };
        if let Some(c) = casting {
            spec.dc = c.dc.or(spec.dc);
            spec.attack = c.attack.or(spec.attack);
        }
        spec.modifier = modifier;
        spec.cantrip_tier = tier;
        spec.baked_in = link.is_some_and(|l| l.baked_in);
        Ok(Some(spec))
    }
}

/// The mechanics of each of `keys` (cast in edition `ruleset`), by key ([`Library::mechanics`]). A key with none is
/// absent: a ruling.
///
/// # Errors
/// The workspace's and the database's.
pub async fn mechanics_for(
    state: &AppState,
    owner: &str,
    ruleset: &str,
    keys: &[String],
) -> ApiResult<HashMap<String, Resolved>> {
    let lib = library(state, owner).await?;
    Ok(keys
        .iter()
        .filter_map(|k| lib.mechanics(ruleset, k).map(|r| (k.clone(), r)))
        .collect())
}

/// A suggested effect for a spell the account hasn't defined: `party-spells`' drafter over the user's own installed
/// copy, from its captured 5etools data, else the tags the import kept on the element. Never binds by itself: the DM
/// saves it as a def once (origin `suggested`). `None` for a spell of no edition (the drafter needs one).
async fn suggestion(
    state: &AppState,
    owner: &str,
    element: &Element,
    ruleset: &str,
) -> ApiResult<Option<party_spells::draft::Draft>> {
    let Some(edition) = edition_of(ruleset) else {
        return Ok(None);
    };
    let raw = match raw_of(state, owner, &element.id).await? {
        Some(raw) => raw,
        None => tags_of(element),
    };
    Ok(Some(party_spells::draft::from_5etools(
        &element.id,
        edition,
        &raw,
    )))
}

/// The hash of the text a suggestion was drafted from: a def saved from it keeps it, so a changed spell is flagged.
#[must_use]
pub fn text_hash(element: &Element) -> String {
    let mut h = Sha256::new();
    h.update(element.name.as_bytes());
    h.update(b"\n");
    h.update(element.description.as_bytes());
    h.finalize()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ---------------------------------------------------------------------------------------------------------------
// The list and a spell's details
// ---------------------------------------------------------------------------------------------------------------

/// `GET /api/spells`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListQuery {
    /// Part of the name.
    #[serde(default)]
    pub text: Option<String>,
    /// `dnd2014` or `dnd2024` (also lists the spells of no edition), or `any`.
    #[serde(default)]
    pub ruleset: Option<String>,
    /// One level.
    #[serde(default)]
    pub level: Option<u8>,
    /// Page size.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Page start.
    #[serde(default)]
    pub offset: Option<usize>,
}

/// The spell keys the account defined (`defined`) or linked (`linked`), by `(key, ruleset)`.
async fn badges(
    state: &AppState,
    owner: &str,
) -> ApiResult<HashMap<(String, String), &'static str>> {
    let rows = sqlx::query(
        "SELECT spell_key, ruleset, kind FROM spell_mechanics WHERE account_id = $1 AND kind <> $2",
    )
    .bind(owner)
    .bind(KIND_ALIAS)
    .fetch_all(state.db.pool())
    .await?;
    let mut out = HashMap::new();
    for r in rows {
        let kind: String = r.try_get(2)?;
        out.insert(
            (r.try_get(0)?, r.try_get(1)?),
            if kind == KIND_DEF {
                "defined"
            } else {
                "linked"
            },
        );
    }
    Ok(out)
}

/// A spell's coverage badge: `defined` or `linked` (the account's, on any copy), `auto`, `partial`, `utility` or
/// `ruling` (the curated tier), else `ruling` (no definition: the cast spends and the DM rules).
fn coverage_of(badges: &HashMap<(String, String), &'static str>, spell: &Spell) -> String {
    for copy in &spell.members {
        for rs in [spell.ruleset.as_str(), "any"] {
            if let Some(b) = badges.get(&(copy.clone(), rs.to_string())) {
                return (*b).to_string();
            }
        }
    }
    for copy in &spell.members {
        if let Some(m) = party_spells::data::curated(copy) {
            if edition_of(&spell.ruleset).is_none_or(|e| e == m.edition) {
                return text_value(&m.tier);
            }
        }
    }
    "ruling".to_string()
}

/// The account's spells, one per name, edition and level (`GET /api/spells`).
///
/// # Errors
/// A bad query, the workspace or the database.
pub async fn list(state: &AppState, owner: &str, q: &ListQuery) -> ApiResult<Value> {
    let ruleset = match q.ruleset.as_deref() {
        None | Some("") => None,
        Some(r) if RULESETS.contains(&r) => Some(r.to_string()),
        Some(r) => return Err(invalid(format!("Unknown ruleset: {r}."))),
    };
    let text = q.text.as_deref().map(normalize).unwrap_or_default();
    if text.len() > 100 {
        return Err(invalid("That search text is too long."));
    }
    let book = book(state, owner).await?;
    let marks = badges(state, owner).await?;
    let mut items: Vec<Spell> = book
        .catalog
        .groups()
        .filter_map(|g| spell_of(&book.index, g))
        .filter(|s| {
            ruleset
                .as_deref()
                .is_none_or(|want| s.ruleset == want || s.ruleset == "any")
        })
        .filter(|s| text.is_empty() || s.norm.contains(&text))
        .filter(|s| q.level.is_none_or(|l| s.level == Some(l)))
        .collect();
    items.sort_by(|a, b| {
        a.norm
            .cmp(&b.norm)
            .then_with(|| a.ruleset.cmp(&b.ruleset))
            .then_with(|| a.level.cmp(&b.level))
    });
    let total = items.len();
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).min(total);
    let page: Vec<Value> = items
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|s| {
            let coverage = coverage_of(&marks, &s);
            let mut v = serde_json::to_value(&s).unwrap_or(Value::Null);
            v["coverage"] = json!(coverage);
            v
        })
        .collect();
    Ok(json!({ "total": total, "offset": offset, "items": page }))
}

/// One spell: the one spell it is a copy of, the account's rows about it, the mechanics combat would use, and a
/// suggestion when it has none (`GET /api/spells/{key}`).
///
/// # Errors
/// An unknown key, the workspace or the database.
pub async fn detail(state: &AppState, owner: &str, key: &str) -> ApiResult<Value> {
    let spell = canonical(state, owner, key).await?.ok_or_else(no_spell)?;
    let workspace = state.workspaces.get(owner).await?;
    let wanted = key.to_string();
    let element: Option<Element> = workspace
        .blocking(move |handle| {
            let backend = handle.state::<Backend>();
            let hub = backend.hub().map_err(|e| upstream(&e))?;
            let found = hub
                .store
                .iter()
                .find(|s| s.element.id == wanted)
                .map(|s| s.element.clone());
            Ok(found)
        })
        .await?;
    let element = element.ok_or_else(no_spell)?;
    let mine: Vec<MechRow> = rows(state, owner)
        .await?
        .into_iter()
        .filter(|r| {
            spell.members.contains(&r.spell_key)
                || (r.kind == KIND_ALIAS && r.alias_name == spell.norm)
        })
        .collect();
    let rs = if spell.ruleset == "any" {
        None
    } else {
        Some(spell.ruleset.as_str())
    };
    let mechanics = match rs {
        Some(rs) => mechanics_for(state, owner, rs, &[key.to_string()])
            .await?
            .remove(key),
        None => None,
    };
    let suggested = match (&mechanics, rs) {
        (None, Some(rs)) => suggestion(state, owner, &element, rs).await?,
        _ => None,
    };
    Ok(json!({
        "spell": spell,
        "element": {
            "id": element.id,
            "name": element.name,
            "source": element.source,
            "description": element.description,
        },
        "rows": mine,
        "mechanics": mechanics,
        "suggested": suggested,
        "sourceHash": text_hash(&element),
    }))
}

// ---------------------------------------------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------------------------------------------

/// The export file.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MechFile {
    /// [`FILE_FORMAT`].
    pub format: String,
    /// [`FILE_V`].
    pub v: i64,
    /// When it was written.
    #[serde(default)]
    pub exported_at: i64,
    /// The rows.
    pub rows: Vec<MechRow>,
}

/// The account's rows as a file.
///
/// # Errors
/// The database's.
pub async fn export(state: &AppState, owner: &str) -> ApiResult<MechFile> {
    Ok(MechFile {
        format: FILE_FORMAT.into(),
        v: FILE_V,
        exported_at: now(),
        rows: rows(state, owner).await?,
    })
}

/// How an import treats the account's own rows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// Rows in the file replace the same rows; others stay.
    #[default]
    Merge,
    /// The account's rows are replaced by the file's.
    Replace,
}

/// Import a file: every row is checked first, and nothing is written when one doesn't fit. Rows come in with origin
/// `import` unless they say `suggested`, and a def's revision moves on from the account's.
///
/// # Errors
/// A file that isn't ours, a row that doesn't fit, too many rows, the database.
pub async fn import(
    state: &AppState,
    owner: &str,
    bytes: &[u8],
    mode: ImportMode,
) -> ApiResult<Value> {
    if bytes.len() > MAX_IMPORT {
        return Err(invalid("That file is too large."));
    }
    let file: MechFile = serde_json::from_slice(bytes)
        .map_err(|e| invalid(format!("That isn't a spell rules file: {e}")))?;
    if file.format != FILE_FORMAT {
        return Err(invalid("That isn't a spell rules file."));
    }
    if file.v > FILE_V {
        return Err(invalid(
            "That file comes from a newer version. Update the server first.",
        ));
    }
    if i64::try_from(file.rows.len()).unwrap_or(i64::MAX) > MAX_ROWS {
        return Err(invalid(format!(
            "A file holds at most {MAX_ROWS} spell rules."
        )));
    }
    let mut checked = Vec::with_capacity(file.rows.len());
    for (n, mut row) in file.rows.into_iter().enumerate() {
        if row.origin != "suggested" {
            row.origin = "import".into();
        }
        check_row(&mut row).map_err(|e| match e {
            ApiError::Invalid(m) => invalid(format!("Row {}: {m}", n + 1)),
            other => other,
        })?;
        checked.push(row);
    }
    if mode == ImportMode::Replace {
        sqlx::query("DELETE FROM spell_mechanics WHERE account_id = $1")
            .bind(owner)
            .execute(state.db.pool())
            .await?;
        mark_stale(owner);
    }
    let mut saved = 0;
    for row in checked {
        save(state, owner, row).await?;
        saved += 1;
    }
    Ok(json!({ "imported": saved, "total": rows(state, owner).await?.len() }))
}

// ---------------------------------------------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------------------------------------------

async fn list_route(
    State(state): State<AppState>,
    user: User,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    Ok(Json(list(&state, &user.id, &q).await?))
}

async fn detail_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(key): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    if !key_ok(&key) {
        return Err(no_spell());
    }
    Ok(Json(detail(&state, &user.id, &key).await?))
}

/// The installed copy `key` (its normalized name and ruleset), or the one 404.
async fn copy_of(state: &AppState, owner: &str, key: &str) -> ApiResult<(String, String)> {
    if !key_ok(key) {
        return Err(no_spell());
    }
    let index = index(state, owner).await?;
    let e = index.entries.get(key).ok_or_else(no_spell)?;
    Ok((e.norm.clone(), e.ruleset.clone()))
}

/// `POST /api/spells/{key}/choose`: this copy is the one spell of its name and edition (the DM's pick, an alias row).
async fn choose_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(key): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    let (norm, ruleset) = copy_of(&state, &user.id, &key).await?;
    save(
        &state,
        &user.id,
        MechRow {
            kind: KIND_ALIAS.into(),
            spell_key: key.clone(),
            ruleset,
            alias_name: norm,
            def: Value::Null,
            schema_v: 1,
            rev: 1,
            origin: "dm".into(),
            source_hash: None,
            updated_at: 0,
        },
    )
    .await?;
    sync_hidden_if_stale(&state, &user.id).await;
    Ok(Json(
        json!({ "spell": canonical(&state, &user.id, &key).await? }),
    ))
}

/// `DELETE /api/spells/{key}/choose`: back to the rule's choice.
async fn unchoose_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(key): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    let (norm, ruleset) = copy_of(&state, &user.id, &key).await?;
    let book = book(&state, &user.id).await?;
    for copy in book.copies(&key) {
        sqlx::query(
            "DELETE FROM spell_mechanics WHERE account_id = $1 AND kind = $2 AND ruleset = $3 AND alias_name = $4
             AND spell_key = $5",
        )
        .bind(&user.id)
        .bind(KIND_ALIAS)
        .bind(&ruleset)
        .bind(&norm)
        .bind(&copy)
        .execute(state.db.pool())
        .await?;
    }
    mark_stale(&user.id);
    sync_hidden_if_stale(&state, &user.id).await;
    Ok(Json(
        json!({ "spell": canonical(&state, &user.id, &key).await? }),
    ))
}

async fn rows_route(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "rows": rows(&state, &user.id).await? })))
}

async fn save_route(
    State(state): State<AppState>,
    user: User,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    let row: MechRow = serde_json::from_slice(&body)
        .map_err(|e| invalid(format!("The request doesn't fit: {e}")))?;
    let row = save(&state, &user.id, row).await?;
    sync_hidden_if_stale(&state, &user.id).await;
    Ok(Json(json!({ "row": row })))
}

/// `DELETE /api/spell-mechanics`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoveBody {
    kind: String,
    spell_key: String,
    ruleset: String,
    #[serde(default)]
    alias_name: String,
}

async fn remove_route(
    State(state): State<AppState>,
    user: User,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    let b: RemoveBody = serde_json::from_slice(&body)
        .map_err(|e| invalid(format!("The request doesn't fit: {e}")))?;
    let removed = remove(
        &state,
        &user.id,
        &b.kind,
        &b.spell_key,
        &b.ruleset,
        &b.alias_name,
    )
    .await?;
    sync_hidden_if_stale(&state, &user.id).await;
    Ok(Json(json!({ "removed": removed })))
}

async fn export_route(State(state): State<AppState>, user: User) -> ApiResult<Response> {
    let file = export(&state, &user.id).await?;
    let mut response = Json(file).into_response();
    if let Ok(v) =
        HeaderValue::from_str("attachment; filename=\"spell-rules.borealis-spells.json\"")
    {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(response)
}

#[derive(Debug, Default, Deserialize)]
struct ImportQuery {
    #[serde(default)]
    mode: ImportMode,
}

async fn import_route(
    State(state): State<AppState>,
    user: User,
    Query(q): Query<ImportQuery>,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    let out = import(&state, &user.id, &body, q.mode).await?;
    sync_hidden_if_stale(&state, &user.id).await;
    Ok(Json(out))
}

/// The spell routes, while parties are on in the server config (spells in combat are a party feature).
pub fn routes() -> axum::Router<AppState> {
    routes_if(crate::config::features().parties)
}

/// The spell routes when `on`; none otherwise.
pub(crate) fn routes_if(on: bool) -> axum::Router<AppState> {
    use axum::routing::{get, post};
    if !on {
        return axum::Router::new();
    }
    axum::Router::new()
        .route("/api/spells", get(list_route))
        .route("/api/spells/{key}", get(detail_route))
        .route(
            "/api/spells/{key}/choose",
            post(choose_route).delete(unchoose_route),
        )
        .route(
            "/api/spell-mechanics",
            get(rows_route).put(save_route).delete(remove_route),
        )
        .route("/api/spell-mechanics/export", get(export_route))
        .route(
            "/api/spell-mechanics/import",
            post(import_route).layer(axum::extract::DefaultBodyLimit::max(MAX_IMPORT)),
        )
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, private"),
        ))
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn names_compare_without_case_apostrophes_or_punctuation() {
        assert_eq!(normalize("Fireball"), "fireball");
        assert_eq!(
            normalize("  Tasha's  Hideous-Laughter "),
            "tashas hideous laughter"
        );
        assert_eq!(
            normalize("Tasha\u{2019}s Hideous Laughter"),
            "tashas hideous laughter"
        );
        assert_ne!(normalize("Delayed Blast Fireball"), normalize("Fireball"));
        assert_eq!(normalize("!!"), "");
    }
}
