//! Sync for the apps, the server's side: what an account holds, one character's file, one pack as one content file,
//! and the delete records a device needs to drop a character it still has.
//!
//! * `party_sync_state` saves unsaved edits first, then lists the characters and the user packs with upstream's own
//!   bundle digests and edit stamps (`borealis_bundle::scan_*`), and the characters deleted in the last 30 days.
//! * `party_sync_get {id}` is one character's file, with `meta.id` set to its id, so a device that imports it under
//!   an id of its own can still tell which character it is.
//! * `party_sync_pack {id}` is one user pack merged into one Borealis content file, for upstream's `import_pack` on
//!   the device (which keeps the elements and their editions, and drops the name, version, license and description).
//!   A pack downloaded from an index the account still has reports that index's address in the state instead, so
//!   the device can download the same pack itself.
//!
//! A deleted character leaves a copy in the account's trash (`sync/trash/<ms>-<id>.json`, upstream's own folder and
//! naming, swept after 30 days); a trash entry with no live character file is a delete record. All of it runs under
//! the workspace's sync lock. Nothing here writes a character.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use borealis_bundle::{
    is_plain_key, item_digest, list_packs, mtime_ms, read_pack_dir, scan_characters, sha256_hex,
    sweep_trash, trash_dir, FileEntry, Limits,
};
use borealis_content::{ContentFile, PackFileKind, PackManifest, PackOrigin, Ruleset};
use borealis_lib::content_import::{import_pack_id, MAX_PACK_BYTES};
use borealis_lib::state::AppState as Backend;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager};

use crate::error::{ApiError, ApiResult};
use crate::workspaces::Workspace;

/// The sync commands, under `POST /api/rpc/<name>`.
pub const COMMANDS: [&str; 3] = ["party_sync_state", "party_sync_get", "party_sync_pack"];

/// Sent after a command wrote or deleted character files: `{ ids }`, the ids it knows of (empty: any of them).
pub const CHARACTERS_CHANGED: &str = "party://characters-changed";

/// Upstream commands that write or delete character files.
const WRITES_CHARACTERS: [&str; 10] = [
    "create_character",
    "save_character",
    "save_character_as",
    "close_character",
    "delete_character",
    "duplicate_character",
    "import_character",
    "recheck_missing_content",
    "apply_convert_to_2024",
    "bundle_accept",
];

/// Upstream commands that delete or replace character files, run under the sync lock.
const LOCKED: [&str; 2] = ["delete_character", "bundle_accept"];

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(e.to_string())
}

/// The `id` argument: a plain key, never a path.
fn key_arg(args: &Map<String, Value>) -> ApiResult<String> {
    args.get("id")
        .and_then(Value::as_str)
        .filter(|id| is_plain_key(id))
        .map(str::to_string)
        .ok_or_else(|| ApiError::Invalid("Give the id of a character or pack.".into()))
}

/// The account's own characters folder. The server never lets it point elsewhere; this makes sure.
fn characters_dir(backend: &Backend) -> ApiResult<PathBuf> {
    let dir = backend.characters_dir();
    if dir.starts_with(&backend.paths.data) {
        Ok(dir)
    } else {
        Err(ApiError::Invalid(
            "Characters are kept outside this account's folder.".into(),
        ))
    }
}

/// Run a sync command.
pub async fn call(
    workspace: &Workspace,
    command: &str,
    args: &Map<String, Value>,
) -> ApiResult<Value> {
    let _sync = workspace.sync_lock.lock().await;
    match command {
        "party_sync_state" => workspace.blocking(state).await,
        "party_sync_get" => {
            let id = key_arg(args)?;
            workspace.blocking(move |app| character(app, &id)).await
        }
        "party_sync_pack" => {
            let id = key_arg(args)?;
            workspace.blocking(move |app| pack(app, &id)).await
        }
        _ => Err(ApiError::NotFound("The server does not offer that.".into())),
    }
}

/// Run an upstream command that writes characters: under the sync lock when it deletes or replaces files, with a
/// delete record first for `delete_character`, and [`CHARACTERS_CHANGED`] after it worked.
pub async fn call_upstream(
    workspace: &Workspace,
    command: &str,
    args: Map<String, Value>,
) -> ApiResult<Value> {
    if !WRITES_CHARACTERS.contains(&command) {
        return workspace.call(command, args).await;
    }
    let named = args
        .get("characterId")
        .and_then(Value::as_str)
        .map(str::to_string);
    // Closing saves only unsaved edits; a plain close changes no file.
    if command == "close_character" && !named.as_deref().is_some_and(|id| workspace.is_dirty(id)) {
        return workspace.call(command, args).await;
    }
    let _sync = if LOCKED.contains(&command) {
        Some(workspace.sync_lock.lock().await)
    } else {
        None
    };
    let record = match named
        .clone()
        .filter(|id| command == "delete_character" && is_plain_key(id))
    {
        Some(id) => {
            workspace
                .blocking(move |app| keep_deleted(app, &id))
                .await?
        }
        None => None,
    };
    let result = workspace.call(command, args).await;
    if result.is_err() {
        if let Some(record) = record {
            let _ = std::fs::remove_file(record);
        }
    }
    let result = result?;
    let mut ids: BTreeSet<String> = named.into_iter().collect();
    for found in [
        result.get("id"),
        result.pointer("/summary/id"),
        result.get("characterId"),
    ] {
        if let Some(id) = found.and_then(Value::as_str) {
            ids.insert(id.to_string());
        }
    }
    if matches!(command, "recheck_missing_content" | "bundle_accept") {
        ids.clear();
    }
    workspace.emit(CHARACTERS_CHANGED, json!({ "ids": ids }));
    Ok(result)
}

/// Copy a character's file into the trash before it is deleted; the copy is the delete record. `None` when there is
/// no file (a character never saved has nothing to sync).
fn keep_deleted(app: &AppHandle, id: &str) -> ApiResult<Option<PathBuf>> {
    let backend = app.state::<Backend>();
    let file = characters_dir(&backend)?.join(format!("{id}.json"));
    if !file.is_file() {
        return Ok(None);
    }
    let trash = trash_dir(&backend.paths.data);
    std::fs::create_dir_all(&trash).map_err(internal)?;
    let record = trash.join(format!("{}-{id}.json", now_ms()));
    // An open session with unsaved edits: the copy holds them, as the delete drops the session.
    let sessions = backend.sessions().map_err(internal)?;
    match sessions.values().find(|s| s.dirty && s.path == file) {
        Some(session) => session.character.save(&record).map_err(internal)?,
        None => {
            std::fs::copy(&file, &record).map_err(internal)?;
        }
    }
    Ok(Some(record))
}

/// Characters with a trash entry and no file, with the newest entry's time: `<ms>-<id>.json`, as upstream names it.
fn deleted(trash: &Path, live: &BTreeSet<String>) -> BTreeMap<String, u64> {
    let mut found = BTreeMap::new();
    for entry in std::fs::read_dir(trash).into_iter().flatten().flatten() {
        if !entry.path().symlink_metadata().is_ok_and(|m| m.is_file()) {
            continue;
        }
        let name = entry.file_name();
        let Some((ms, id)) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".json"))
            .and_then(|n| n.split_once('-'))
        else {
            continue;
        };
        let Ok(ms) = ms.parse::<u64>() else {
            continue;
        };
        if is_plain_key(id) && !live.contains(id) {
            let newest = found.entry(id.to_string()).or_insert(ms);
            *newest = (*newest).max(ms);
        }
    }
    found
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// `party_sync_state`.
fn state(app: &AppHandle) -> ApiResult<Value> {
    // The scans see files only.
    crate::backend::save_all(app).map_err(ApiError::Internal)?;
    let backend = app.state::<Backend>();
    let trash = trash_dir(&backend.paths.data);
    let _ = sweep_trash(&trash, now_ms());
    // Only what party_sync_get and party_sync_pack can fetch (upstream also allows names with spaces).
    let mut characters = scan_characters(&characters_dir(&backend)?).map_err(internal)?;
    characters.retain(|c| is_plain_key(&c.key));
    let live: BTreeSet<String> = characters.iter().map(|c| c.key.clone()).collect();
    let deleted: Vec<Value> = deleted(&trash, &live)
        .into_iter()
        .map(|(id, ms)| json!({ "id": id, "deletedAtMs": ms }))
        .collect();
    let prefs = backend.content_prefs();
    let indexes: BTreeSet<&str> = prefs.indexes.iter().map(|i| i.url.as_str()).collect();
    let library = backend.paths.library();
    let mut packs = Vec::new();
    for (key, dir) in list_packs(&library).map_err(internal)? {
        if !is_plain_key(&key) {
            continue;
        }
        let (sha256, edited_at_ms) = pack_digest(&backend, &key, &dir)?;
        let pack_id = read_json(&dir.join("pack.json"))
            .and_then(|m| m["id"].as_str().map(str::to_string))
            .unwrap_or_else(|| key.clone());
        // Downloaded from an index the account still has: the device downloads it from there.
        let index_url = read_json(&dir.join("pack-meta.json"))
            .filter(|m| m["kind"] == "downloaded")
            .and_then(|m| m["indexUrl"].as_str().map(str::to_string))
            .filter(|url| indexes.contains(url.as_str()));
        packs.push(json!({
            "id": key,
            "editedAtMs": edited_at_ms,
            "sha256": sha256,
            "enabled": !prefs.disabled_packs.contains(&pack_id),
            "indexUrl": index_url,
        }));
    }
    let characters: Vec<Value> = characters
        .into_iter()
        .map(|c| json!({ "id": c.key, "editedAtMs": c.edited_at_ms, "sha256": c.sha256 }))
        .collect();
    Ok(json!({
        "nowMs": now_ms(),
        "appVersion": borealis_lib::commands::app_version(),
        "characters": characters,
        "packs": packs,
        "deleted": deleted,
    }))
}

/// `party_sync_get`: the file, its digest and edit stamp as the state lists them, and the bytes with `meta.id` set.
pub(crate) fn character(app: &AppHandle, id: &str) -> ApiResult<Value> {
    crate::backend::save_all(app).map_err(ApiError::Internal)?;
    let backend = app.state::<Backend>();
    let path = characters_dir(&backend)?.join(format!("{id}.json"));
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::NotFound(
                "That character no longer exists.".into(),
            ))
        }
        Err(e) => return Err(internal(e)),
    };
    let sha256 = sha256_hex(&bytes);
    let edited_at_ms = mtime_ms(&path);
    let content = with_meta_id(bytes, id)?;
    Ok(json!({
        "id": id,
        "sha256": sha256,
        "editedAtMs": edited_at_ms,
        "contentBase64": base64::engine::general_purpose::STANDARD.encode(content),
    }))
}

/// A character file whose `meta.id` is `id`. Upstream leaves it empty in some files.
fn with_meta_id(bytes: Vec<u8>, id: &str) -> ApiResult<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::Internal(format!("character {id} is not JSON: {e}")))?;
    let Some(meta) = value.get_mut("meta").and_then(Value::as_object_mut) else {
        return Ok(bytes);
    };
    if meta.get("id").and_then(Value::as_str) == Some(id) {
        return Ok(bytes);
    }
    meta.insert("id".into(), Value::String(id.to_string()));
    let mut text = serde_json::to_string_pretty(&value).map_err(internal)?;
    text.push('\n');
    Ok(text.into_bytes())
}

/// `party_sync_pack`.
fn pack(app: &AppHandle, id: &str) -> ApiResult<Value> {
    let backend = app.state::<Backend>();
    let library = backend.paths.library();
    let Some((_, dir)) = list_packs(&library)
        .map_err(internal)?
        .into_iter()
        .find(|(key, _)| key == id)
    else {
        return Err(ApiError::NotFound("That pack is not installed.".into()));
    };
    let (sha256, edited_at_ms) = pack_digest(&backend, id, &dir)?;
    let (file_name, merged) = merge_pack(&dir, id, &chosen_editions(&backend, id)?)?;
    Ok(json!({
        "id": id,
        "sha256": sha256,
        "editedAtMs": edited_at_ms,
        "fileName": file_name,
        "contentBase64": base64::engine::general_purpose::STANDARD.encode(merged),
    }))
}

/// One pack's digest and edit stamp, over the files `scan_packs` hashes, plus the editions the account chose for its
/// books when there are any (they change what loads without changing a file, so a device must fetch it again).
pub(crate) fn pack_digest(backend: &Backend, key: &str, dir: &Path) -> ApiResult<(String, u64)> {
    let files = read_pack_dir(dir, &Limits::default()).map_err(internal)?;
    let mut entries = Vec::new();
    for (rel, path) in &files.files {
        let bytes = std::fs::read(path).map_err(internal)?;
        entries.push(FileEntry {
            path: format!("packs/{key}/{rel}"),
            sha256: sha256_hex(&bytes),
            size: bytes.len() as u64,
        });
    }
    let mut sha256 = item_digest(&entries);
    let chosen = chosen_editions(backend, key)?;
    if !chosen.is_empty() {
        let chosen = serde_json::to_string(&chosen).map_err(internal)?;
        sha256 = sha256_hex(format!("{sha256}\n{chosen}").as_bytes());
    }
    Ok((sha256, files.edited_at_ms))
}

/// The loaded edition of each element of pack `key` whose edition comes from the account's choice for its book
/// (upstream `set_source_edition`), by element id. Upstream applies that choice only while content loads, and a
/// device has none of it, so the merged file carries the edition each element loads with here.
pub(crate) fn chosen_editions(
    backend: &Backend,
    key: &str,
) -> ApiResult<BTreeMap<String, Ruleset>> {
    let hub = backend.hub().map_err(internal)?;
    let prefix = format!("{key}/");
    Ok(hub
        .store
        .iter()
        .filter(|s| s.user_edition_book.is_some() && s.file.starts_with(&prefix))
        .filter_map(|s| Some((s.element.id.clone(), s.element.ruleset?)))
        .collect())
}

fn cannot_copy(id: &str, why: &str) -> ApiError {
    ApiError::Invalid(format!(
        "The pack \"{id}\" cannot be copied to a device: {why}."
    ))
}

/// One user pack as one content file that upstream's `import_pack` installs under the same pack id, loading the
/// same elements: files in load order, ignored files and index files left out, each element tagged with the edition
/// it loads with here (`chosen`, from [`chosen_editions`], overrides the files), and the appends kept in order.
/// The file name gives the pack id.
///
/// # Errors
/// A pack this cannot copy faithfully: an id that is not its own folder name in slug form, core load rules, an
/// element with no edition in a pack of several editions, or more than an import takes.
pub fn merge_pack(
    dir: &Path,
    id: &str,
    chosen: &BTreeMap<String, Ruleset>,
) -> ApiResult<(String, Vec<u8>)> {
    let text = std::fs::read_to_string(dir.join("pack.json")).map_err(internal)?;
    let manifest =
        PackManifest::from_json_str(&text).map_err(|e| cannot_copy(id, &e.to_string()))?;
    let file_name = format!("{id}.json");
    if manifest.id != id || import_pack_id(&file_name) != id {
        return Err(cannot_copy(id, "its id is not a plain name"));
    }
    if manifest.origin != PackOrigin::Custom {
        return Err(cannot_copy(id, "it loads with the core content's rules"));
    }
    let only = (manifest.rulesets.len() == 1).then(|| manifest.rulesets[0]);
    let mut elements = Vec::new();
    let mut appends = Vec::new();
    for entry in &manifest.files {
        if !matches!(entry.kind, PackFileKind::Content) {
            continue;
        }
        let rel = Path::new(&entry.path);
        if entry.path.is_empty()
            || !rel
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(cannot_copy(id, "it lists a file outside its folder"));
        }
        let text = std::fs::read_to_string(dir.join(rel)).map_err(internal)?;
        let file =
            ContentFile::from_json_str(&text).map_err(|e| cannot_copy(id, &e.to_string()))?;
        if file.is_ignored() {
            continue;
        }
        let ruleset = file.ruleset.or(only);
        for mut element in file.elements {
            element.ruleset = chosen
                .get(&element.id)
                .copied()
                .or(element.ruleset)
                .or(ruleset);
            if element.ruleset.is_none() {
                return Err(cannot_copy(
                    id,
                    &format!("the element {} has no edition", element.id),
                ));
            }
            elements.push(element);
        }
        appends.extend(file.appends);
    }
    let mut merged = ContentFile::new(elements);
    let first = merged.elements.first().and_then(|e| e.ruleset);
    merged.ruleset = if merged.elements.iter().all(|e| e.ruleset == first) {
        first.or(only)
    } else {
        manifest.rulesets.first().copied()
    };
    merged.appends = appends;
    let bytes = serde_json::to_vec(&merged).map_err(internal)?;
    if bytes.len() > MAX_PACK_BYTES {
        return Err(cannot_copy(id, "it is larger than an import may be"));
    }
    Ok((file_name, bytes))
}
