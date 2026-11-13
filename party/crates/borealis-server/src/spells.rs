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

use std::collections::{BTreeMap, HashMap};
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
    let mut out = String::with_capacity(name.len());
    let mut gap = false;
    for c in name.chars() {
        if matches!(c, '\'' | '\u{2019}' | '\u{2018}') {
            continue;
        }
        if c.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.extend(c.to_lowercase());
        } else {
            gap = true;
        }
    }
    out
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

/// One spell (all its copies of one ruleset).
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
    /// The chosen copy's source and pack.
    pub source: String,
    /// The pack.
    pub pack_id: String,
    /// `rule` (the stable rule chose it) or `dm` (the DM's override).
    pub chosen_by: &'static str,
    /// The other copies, hidden from pickers.
    pub also_in: Vec<Copy>,
    #[serde(skip)]
    norm: String,
}

/// An installed copy with what the grouping needs.
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
    /// The SRD's own copy (a bundled SRD pack).
    srd: bool,
}

/// An account's spells, for one content load.
pub struct Index {
    hub: Weak<ContentHub>,
    /// Every copy, by element id.
    entries: HashMap<String, Entry>,
    /// The copies of each spell, by `(normalized name, ruleset)`, in [`choose`]'s order.
    groups: BTreeMap<(String, String), Vec<String>>,
}

impl Index {
    /// The group a copy is in.
    fn group_of(&self, key: &str) -> Option<&(String, String)> {
        let e = self.entries.get(key)?;
        let g = (e.norm.clone(), e.ruleset.clone());
        self.groups.get_key_value(&g).map(|(k, _)| k)
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

/// The stable "which copy" rule, best first (until the content layer knows publication dates for every source):
/// the SRD's own copy, then bundled content, then a 5etools import, an index and custom content; then by source name
/// and id so the choice never flips between loads.
fn choose(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    let rank = |e: &Entry| {
        let kind = match e.copy.pack_kind {
            "bundled" => 0,
            "5etools" => 1,
            "index" => 2,
            _ => 3,
        };
        (u8::from(!e.srd), kind)
    };
    rank(a)
        .cmp(&rank(b))
        .then_with(|| a.copy.source.cmp(&b.copy.source))
        .then_with(|| a.copy.key.cmp(&b.copy.key))
}

struct PackInfo {
    kind: &'static str,
    ruleset: Option<String>,
    srd: bool,
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
                    srd: matches!(p.kind, PackKind::Bundled) && p.id.to_lowercase().contains("srd"),
                },
            )
        })
        .collect();
    let mut entries = HashMap::new();
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
        let entry = Entry {
            copy: Copy {
                key: el.id.clone(),
                source: el.source.clone(),
                pack_id,
                pack_kind: pack.map_or("custom", |p| p.kind),
            },
            name: el.name.clone(),
            norm: normalize(&el.name),
            ruleset,
            level: setter(el, "level").and_then(|l| l.parse().ok()),
            school: setter(el, "school").map(str::to_string),
            concentration: truthy(el, "isConcentration"),
            ritual: truthy(el, "isRitual"),
            srd: pack.is_some_and(|p| p.srd),
        };
        entries.insert(el.id.clone(), entry);
    }
    let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for e in entries.values() {
        if e.norm.is_empty() {
            continue;
        }
        groups
            .entry((e.norm.clone(), e.ruleset.clone()))
            .or_default()
            .push(e.copy.key.clone());
    }
    for keys in groups.values_mut() {
        keys.sort_by(|a, b| choose(&entries[a], &entries[b]));
    }
    Ok(Index {
        hub: Arc::downgrade(&hub),
        entries,
        groups,
    })
}

/// The account's spell index: the cached one while the content is the same, else a fresh one.
///
/// # Errors
/// When the workspace fails.
pub async fn index(state: &AppState, owner: &str) -> ApiResult<Arc<Index>> {
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

/// The DM's "this copy" choices: `(normalized name, ruleset)` to element id.
async fn overrides(state: &AppState, owner: &str) -> ApiResult<HashMap<(String, String), String>> {
    let rows = sqlx::query(
        "SELECT alias_name, ruleset, spell_key FROM spell_mechanics WHERE account_id = $1 AND kind = $2",
    )
    .bind(owner)
    .bind(KIND_ALIAS)
    .fetch_all(state.db.pool())
    .await?;
    let mut out = HashMap::new();
    for row in rows {
        out.insert((row.try_get(0)?, row.try_get(1)?), row.try_get(2)?);
    }
    Ok(out)
}

/// The copies that are one spell of `norm` as edition `ruleset` sees it: that edition's copies, then the copies that
/// name no edition (`any`: Aurora XML and homebrew without a ruleset). Seen as `any`, only the `any` copies.
fn candidates(index: &Index, norm: &str, ruleset: &str) -> Vec<String> {
    let mut keys = index
        .groups
        .get(&(norm.to_string(), ruleset.to_string()))
        .cloned()
        .unwrap_or_default();
    if ruleset != "any" {
        if let Some(any) = index.groups.get(&(norm.to_string(), "any".to_string())) {
            keys.extend(any.iter().cloned());
        }
    }
    keys
}

/// The one spell of `norm` in edition `ruleset`: the DM's chosen copy when it is installed, else the rule's.
fn spell_of(
    index: &Index,
    norm: &str,
    ruleset: &str,
    chosen: &HashMap<(String, String), String>,
) -> Option<Spell> {
    let keys = candidates(index, norm, ruleset);
    let (pick, by) = match chosen
        .get(&(norm.to_string(), ruleset.to_string()))
        .filter(|k| keys.contains(k))
    {
        Some(k) => (k.clone(), "dm"),
        None => (keys.first()?.clone(), "rule"),
    };
    let e = index.entries.get(&pick)?;
    Some(Spell {
        key: e.copy.key.clone(),
        name: e.name.clone(),
        ruleset: ruleset.to_string(),
        level: e.level,
        school: e.school.clone(),
        concentration: e.concentration,
        ritual: e.ritual,
        source: e.copy.source.clone(),
        pack_id: e.copy.pack_id.clone(),
        chosen_by: by,
        also_in: keys
            .iter()
            .filter(|k| **k != pick)
            .filter_map(|k| index.entries.get(k).map(|e| e.copy.clone()))
            .collect(),
        norm: e.norm.clone(),
    })
}

/// The one spell `key` (any copy's id) belongs to, in edition `ruleset` (default: the copy's own).
///
/// # Errors
/// When the workspace or the database fails.
pub async fn canonical(
    state: &AppState,
    owner: &str,
    key: &str,
    ruleset: Option<&str>,
) -> ApiResult<Option<Spell>> {
    let index = index(state, owner).await?;
    let Some((norm, own)) = index.group_of(key).cloned() else {
        return Ok(None);
    };
    // A copy of one edition is never another edition's spell; an `any` copy is whichever edition asks.
    let rs = match ruleset {
        Some(r) if own == "any" && RULESETS.contains(&r) => r.to_string(),
        _ => own,
    };
    let chosen = overrides(state, owner).await?;
    Ok(spell_of(&index, &norm, &rs, &chosen))
}

/// Resolve a spell name (a monster spellbook's line) in `ruleset`: the DM's alias first (a different name may stand
/// for a spell), then the one spell of that name. `None` when nothing matches.
///
/// # Errors
/// When the workspace or the database fails.
pub async fn resolve_name(
    state: &AppState,
    owner: &str,
    name: &str,
    ruleset: &str,
) -> ApiResult<Option<Spell>> {
    let norm = normalize(name);
    let index = index(state, owner).await?;
    let chosen = overrides(state, owner).await?;
    if let Some(alias) = chosen.get(&(norm.clone(), ruleset.to_string())) {
        if let Some((n, _)) = index.group_of(alias).cloned() {
            if n != norm {
                return Ok(spell_of(&index, &n, ruleset, &chosen));
            }
        }
    }
    Ok(spell_of(&index, &norm, ruleset, &chosen))
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

/// The effect schema a def must fit. S1's `party_combat::spell` is the source of truth; until a def is compiled
/// against it, a def is any JSON object of at most [`MAX_DEF`] bytes with a numeric `v`.
fn check_def(def: &Value) -> ApiResult<i64> {
    let Some(obj) = def.as_object() else {
        return Err(invalid("An effect definition is a JSON object."));
    };
    if def.to_string().len() > MAX_DEF {
        return Err(invalid("That effect definition is too large."));
    }
    Ok(obj.get("v").and_then(Value::as_i64).unwrap_or(1).max(1))
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
            row.schema_v = check_def(&row.def)?;
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

/// The rows of some spells (combat start and casts).
async fn rows_of(state: &AppState, owner: &str, keys: &[String]) -> ApiResult<Vec<MechRow>> {
    let mut out = Vec::new();
    // Few keys at a time; one query each keeps the SQL the same on both databases.
    for key in keys {
        let rows = sqlx::query(&format!(
            "SELECT {ROW_COLUMNS} FROM spell_mechanics WHERE account_id = $1 AND spell_key = $2 AND kind <> $3"
        ))
        .bind(owner)
        .bind(key)
        .bind(KIND_ALIAS)
        .fetch_all(state.db.pool())
        .await?;
        for r in &rows {
            out.push(row_from(r)?);
        }
    }
    Ok(out)
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
    /// `def` (the account's), `link` (another spell's def, through the account's link) or `srd` (the curated table).
    pub origin: &'static str,
    /// The spell key whose definition it is.
    pub from_key: String,
    /// That definition's revision.
    pub rev: i64,
}

/// The curated SRD definition of a spell key, when `party-spells` has one. Hooked to S2's table at merge; none until
/// then, so every spell is the account's to define.
fn curated(_key: &str, _ruleset: &str) -> Option<(Value, i64)> {
    None
}

/// The mechanics of each of `keys` (the casting edition `ruleset`), by key: the account's def, then its link (one
/// hop, to the target's def or curated row), then the curated row. A key with none is absent: a ruling.
///
/// # Errors
/// The database's.
pub async fn mechanics_for(
    state: &AppState,
    owner: &str,
    ruleset: &str,
    keys: &[String],
) -> ApiResult<HashMap<String, Resolved>> {
    let fits = |r: &MechRow| r.ruleset == ruleset || r.ruleset == "any";
    let mine = rows_of(state, owner, keys).await?;
    let mut out = HashMap::new();
    let mut targets = Vec::new();
    for key in keys {
        let own: Vec<&MechRow> = mine
            .iter()
            .filter(|r| &r.spell_key == key && fits(r))
            .collect();
        if let Some(d) = own.iter().find(|r| r.kind == KIND_DEF) {
            out.insert(
                key.clone(),
                Resolved {
                    def: d.def.clone(),
                    origin: "def",
                    from_key: key.clone(),
                    rev: d.rev,
                },
            );
        } else if let Some(t) = own
            .iter()
            .find(|r| r.kind == KIND_LINK)
            .and_then(|l| l.def.get("target").and_then(Value::as_str))
        {
            targets.push((key.clone(), t.to_string()));
        } else if let Some((def, rev)) = curated(key, ruleset) {
            out.insert(
                key.clone(),
                Resolved {
                    def,
                    origin: "srd",
                    from_key: key.clone(),
                    rev,
                },
            );
        }
    }
    if !targets.is_empty() {
        let wanted: Vec<String> = targets.iter().map(|(_, t)| t.clone()).collect();
        let theirs = rows_of(state, owner, &wanted).await?;
        for (key, target) in targets {
            let def = theirs
                .iter()
                .find(|r| r.spell_key == target && r.kind == KIND_DEF && fits(r))
                .map(|r| (r.def.clone(), r.rev))
                .or_else(|| curated(&target, ruleset));
            if let Some((def, rev)) = def {
                out.insert(
                    key,
                    Resolved {
                        def,
                        origin: "link",
                        from_key: target,
                        rev,
                    },
                );
            }
        }
    }
    Ok(out)
}

/// A suggested effect for a spell the account hasn't defined: S2's drafter over the user's own installed copy (its
/// 5etools tags). Never binds by itself; the DM saves it as a def once. None until the drafter is hooked in.
fn suggestion(_element: &Element) -> Option<Value> {
    None
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
    /// `dnd2014`, `dnd2024` (also lists `any`).
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

/// The coverage badge of each spell key: `defined`, `linked` or none (curated and suggested come with the data).
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

fn coverage_of(badges: &HashMap<(String, String), &'static str>, spell: &Spell) -> &'static str {
    for rs in [spell.ruleset.as_str(), "any"] {
        if let Some(b) = badges.get(&(spell.key.clone(), rs.to_string())) {
            return b;
        }
    }
    if curated(&spell.key, &spell.ruleset).is_some() {
        return "auto";
    }
    "ruling"
}

/// The account's spells, one per edition (`GET /api/spells`).
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
    let index = index(state, owner).await?;
    let chosen = overrides(state, owner).await?;
    let marks = badges(state, owner).await?;
    // Seen as one edition, a name is one spell (its `any` copies with it); else every group is its own.
    let views: std::collections::BTreeSet<(String, String)> = index
        .groups
        .keys()
        .filter_map(|(norm, rs)| match ruleset.as_deref() {
            None => Some((norm.clone(), rs.clone())),
            Some(want) if rs == want => Some((norm.clone(), rs.clone())),
            Some(want) if rs == "any" => Some((norm.clone(), want.to_string())),
            Some(_) => None,
        })
        .collect();
    let mut items: Vec<Spell> = views
        .iter()
        .filter_map(|(norm, rs)| spell_of(&index, norm, rs, &chosen))
        .filter(|s| text.is_empty() || s.norm.contains(&text))
        .filter(|s| q.level.is_none_or(|l| s.level == Some(l)))
        .collect();
    items.sort_by(|a, b| a.norm.cmp(&b.norm).then_with(|| a.ruleset.cmp(&b.ruleset)));
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

/// One spell: the one spell it belongs to, the account's rows about it, the mechanics combat would use, and a
/// suggestion when it has none (`GET /api/spells/{key}`).
///
/// # Errors
/// An unknown key, the workspace or the database.
pub async fn detail(
    state: &AppState,
    owner: &str,
    key: &str,
    ruleset: Option<&str>,
) -> ApiResult<Value> {
    let spell = canonical(state, owner, key, ruleset)
        .await?
        .ok_or_else(no_spell)?;
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
            r.spell_key == spell.key
                || r.spell_key == key
                || (r.kind == KIND_ALIAS && r.alias_name == spell.norm)
        })
        .collect();
    let mut keys = vec![spell.key.clone()];
    if key != spell.key {
        keys.push(key.to_string());
    }
    let resolved = mechanics_for(state, owner, &spell.ruleset, &keys).await?;
    let mechanics = resolved
        .get(key)
        .or_else(|| resolved.get(&spell.key))
        .cloned();
    let suggested = if mechanics.is_none() {
        suggestion(&element)
    } else {
        None
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

/// `?ruleset=`: the edition a copy that names none is seen as.
#[derive(Debug, Default, Deserialize)]
struct EditionQuery {
    #[serde(default)]
    ruleset: Option<String>,
}

async fn detail_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(key): UrlPath<String>,
    Query(q): Query<EditionQuery>,
) -> ApiResult<Json<Value>> {
    if !key_ok(&key) {
        return Err(no_spell());
    }
    Ok(Json(
        detail(&state, &user.id, &key, q.ruleset.as_deref()).await?,
    ))
}

/// The one spell `key` is a copy of, as `ruleset` sees it: its normalized name and edition.
async fn view_of(
    state: &AppState,
    owner: &str,
    key: &str,
    ruleset: Option<&str>,
) -> ApiResult<(String, String)> {
    let spell = canonical(state, owner, key, ruleset)
        .await?
        .ok_or_else(no_spell)?;
    Ok((spell.norm, spell.ruleset))
}

/// `POST /api/spells/{key}/choose`: this copy is the one spell of its name and edition.
async fn choose_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(key): UrlPath<String>,
    Query(q): Query<EditionQuery>,
) -> ApiResult<Json<Value>> {
    if !key_ok(&key) {
        return Err(no_spell());
    }
    let (norm, ruleset) = view_of(&state, &user.id, &key, q.ruleset.as_deref()).await?;
    save(
        &state,
        &user.id,
        MechRow {
            kind: KIND_ALIAS.into(),
            spell_key: key.clone(),
            ruleset: ruleset.clone(),
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
    Ok(Json(json!({
        "spell": canonical(&state, &user.id, &key, Some(&ruleset)).await?,
    })))
}

/// `DELETE /api/spells/{key}/choose`: back to the rule's choice.
async fn unchoose_route(
    State(state): State<AppState>,
    user: User,
    UrlPath(key): UrlPath<String>,
    Query(q): Query<EditionQuery>,
) -> ApiResult<Json<Value>> {
    if !key_ok(&key) {
        return Err(no_spell());
    }
    let (norm, ruleset) = view_of(&state, &user.id, &key, q.ruleset.as_deref()).await?;
    sqlx::query(
        "DELETE FROM spell_mechanics WHERE account_id = $1 AND kind = $2 AND ruleset = $3 AND alias_name = $4",
    )
    .bind(&user.id)
    .bind(KIND_ALIAS)
    .bind(&ruleset)
    .bind(&norm)
    .execute(state.db.pool())
    .await?;
    Ok(Json(json!({
        "spell": canonical(&state, &user.id, &key, Some(&ruleset)).await?,
    })))
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
    Ok(Json(json!({ "row": save(&state, &user.id, row).await? })))
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
    Ok(Json(import(&state, &user.id, &body, q.mode).await?))
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
