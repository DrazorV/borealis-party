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
//! * `party_sync_push {id, baseSha, editedAtMs, contentBase64}` is a character edited on a device, sent back. When
//!   the server's file is still the one the device's copy was made from (`baseSha`), the device's copy replaces it
//!   whatever the clocks say. When both changed, the newest edit wins (upstream's `decide`, on the server's clock:
//!   the device sends its edit time corrected by the measured offset, and a time in the future counts as now). A
//!   character deleted here after the device's edit stays deleted; one edited after its delete comes back. Unsaved
//!   edits of an open session are saved first and compared by their save time (upstream keeps no time of the edit
//!   itself); with autosave on, that is within seconds of the edit.
//!
//!   Since 0.3.6 a device sends `ask: true`, and then nothing is decided for it: a push whose `baseSha` is not the
//!   server's file (another device or the DM changed it meanwhile, or it was deleted here) answers `conflict` with the
//!   server file's digest (none when deleted) and writes nothing, not even to the trash: the device keeps its edit and
//!   asks its user. "Keep mine" sends the same copy again with `forceOver`, the digest of the server copy the user
//!   saw (empty when it was deleted): it is written only while the server still holds that copy (which goes to the
//!   trash first, as always); a copy changed since is a conflict again. "Use the server's" sends the device's copy
//!   with `discard: true`: it only goes to the trash as the device's (`discarded`), so it can be recovered for 30
//!   days. A push without `ask` (the apps before 0.3.6) is decided by the clocks as above.
//!
//! A deleted character leaves a copy in the account's trash (`sync/trash/<ms>-<id>.json`, upstream's own folder and
//! naming, swept after 30 days); a trash entry with no live character file is a delete record. A copy replaced by a
//! push goes there too, under the same name, as a bundle's would; a device's copy that lost goes there as
//! `<ms>-<id>.device.json`, which is no delete record. All of it runs under the workspace's sync lock, as do the
//! upstream commands that write character files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use borealis_bundle::{
    decide, is_plain_key, item_digest, list_packs, mtime_ms, read_pack_dir, scan_characters,
    sha256_hex, sweep_trash, trash_dir, Action, FileEntry, ItemKind, ItemValidator, Limits,
    LocalItem,
};
use borealis_content::{ContentFile, PackFileKind, PackManifest, PackOrigin, Ruleset};
use borealis_lib::content_import::{import_pack_id, MAX_PACK_BYTES};
use borealis_lib::device_sync::validate::AppValidator;
use borealis_lib::state::AppState as Backend;
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager};

use crate::error::{ApiError, ApiResult};
use crate::workspaces::Workspace;

/// The sync commands, under `POST /api/rpc/<name>`.
pub const COMMANDS: [&str; 4] = [
    "party_sync_state",
    "party_sync_get",
    "party_sync_pack",
    "party_sync_push",
];

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
pub(crate) fn characters_dir(backend: &Backend) -> ApiResult<PathBuf> {
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
        "party_sync_push" => {
            let sent = Pushed::from_args(args)?;
            let id = sent.id.clone();
            let out = workspace.blocking(move |app| push(app, &sent)).await?;
            if out["result"] == "applied" {
                // `replaced`: an open page of it re-reads it (a save of its own does not need to).
                workspace.emit(CHARACTERS_CHANGED, json!({ "ids": [id], "replaced": true }));
            }
            Ok(out)
        }
        _ => Err(ApiError::NotFound("The server does not offer that.".into())),
    }
}

/// A character sent back by a device.
struct Pushed {
    id: String,
    /// The digest of the server file the device's copy was made from; none when the device does not know it.
    base_sha: Option<String>,
    /// When it was last edited on the device, on the server's clock.
    edited_at_ms: u64,
    bytes: Vec<u8>,
    /// A stale push is answered `conflict` and never applied (0.3.6 devices); without it, the newest edit wins.
    ask: bool,
    /// "Keep mine": the digest of the server copy the user chose over (empty: it was deleted). Written only while the
    /// server still holds that copy.
    force_over: Option<String>,
    /// "Use the server's": the device drops its copy, which is only kept in the trash.
    discard: bool,
}

impl Pushed {
    fn from_args(args: &Map<String, Value>) -> ApiResult<Self> {
        let id = key_arg(args)?;
        let base_sha = args
            .get("baseSha")
            .and_then(Value::as_str)
            .map(str::to_string);
        let edited_at_ms = args
            .get("editedAtMs")
            .and_then(Value::as_u64)
            .ok_or_else(|| ApiError::Invalid("Give the time of the edit.".into()))?;
        let bytes = args
            .get("contentBase64")
            .and_then(Value::as_str)
            .and_then(|text| {
                base64::engine::general_purpose::STANDARD
                    .decode(text.trim())
                    .ok()
            })
            .ok_or_else(|| {
                ApiError::Invalid("The character's content is not valid base64.".into())
            })?;
        let flag = |name: &str| args.get(name).and_then(Value::as_bool).unwrap_or(false);
        Ok(Self {
            id,
            base_sha,
            edited_at_ms,
            bytes,
            ask: flag("ask"),
            force_over: args
                .get("forceOver")
                .and_then(Value::as_str)
                .map(str::to_string),
            discard: flag("discard"),
        })
    }
}

/// What a push does with the device's copy.
enum Outcome {
    /// The server's file already holds it.
    Identical,
    /// It becomes the server's file, stamped with this time.
    Write(u64),
    /// The server's copy is newer, or the character was deleted after the edit: it goes to the trash.
    Lost(&'static str),
    /// The server's file changed since the device's copy was made, and the device asks its user: nothing is written.
    Conflict,
}

/// `party_sync_push`. Answers `{ result, sha256, editedAtMs }`: `applied`, `identical`, `kept-server`, `deleted`,
/// (with `ask`) `conflict`, or (with `discard`) `discarded`, with the server file's digest and stamp afterwards (none when deleted).
fn push(app: &AppHandle, sent: &Pushed) -> ApiResult<Value> {
    let backend = app.state::<Backend>();
    let id = sent.id.as_str();
    let file = characters_dir(&backend)?.join(format!("{id}.json"));
    // The same checks as a bundle's character: upstream's parser, and an id that is empty or this one.
    let bytes = AppValidator
        .character(id, &sent.bytes)
        .map_err(|why| ApiError::Invalid(format!("The character can't be taken: {why}.")))?;
    let incoming_sha = sha256_hex(&bytes);
    let now = now_ms();
    let edited = sent.edited_at_ms.min(now);
    let trash = trash_dir(&backend.paths.data);
    let _ = sweep_trash(&trash, now);
    let hub = backend.hub().map_err(|e| internal(e.message))?;
    if sent.discard {
        // The device's user took the server's copy: theirs is kept where every losing copy is, and nothing else changes.
        std::fs::create_dir_all(&trash).map_err(internal)?;
        std::fs::write(trash_file(&trash, id, ".device.json"), &bytes).map_err(internal)?;
        let (sha256, edited_at_ms) = file_stamp(&file);
        return Ok(json!({ "result": "discarded", "sha256": sha256, "editedAtMs": edited_at_ms }));
    }
    let written = {
        let mut sessions = backend.sessions().map_err(internal)?;
        // Edits not saved yet are the server's newest copy: on file first, so they are compared and never dropped.
        if let Some(session) = sessions.get_mut(id).filter(|s| s.dirty) {
            session.character.save(&session.path).map_err(internal)?;
            session.dirty = false;
        }
        let outcome = match std::fs::read(&file) {
            Ok(current) => {
                let local = LocalItem {
                    kind: ItemKind::Character,
                    key: id.to_string(),
                    sha256: sha256_hex(&current),
                    edited_at_ms: mtime_ms(&file),
                };
                let chosen = sent.force_over.as_deref() == Some(local.sha256.as_str());
                if local.sha256 == incoming_sha {
                    Outcome::Identical
                } else if chosen || sent.base_sha.as_deref() == Some(local.sha256.as_str()) {
                    // Only the device changed it (or its user chose it): its copy goes on, after the server's
                    // whatever the clocks say.
                    Outcome::Write(edited.max(local.edited_at_ms + 1))
                } else if sent.ask {
                    Outcome::Conflict
                } else {
                    match decide(&incoming_sha, edited, Some(&local)) {
                        Action::ReplaceWithIncoming => Outcome::Write(edited),
                        _ => Outcome::Lost("kept-server"),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let chosen = sent.force_over.as_deref() == Some("");
                match deleted(&trash, &BTreeSet::new()).get(id) {
                    // Deleted here after the device's copy was made: its user decides whether it comes back.
                    Some(_) if sent.ask && !chosen => Outcome::Conflict,
                    Some(&at) if edited <= at && !sent.ask => Outcome::Lost("deleted"),
                    _ => Outcome::Write(edited),
                }
            }
            Err(e) => return Err(internal(e)),
        };
        match outcome {
            Outcome::Identical => false,
            Outcome::Conflict => {
                let (sha256, edited_at_ms) = file_stamp(&file);
                return Ok(
                    json!({ "result": "conflict", "sha256": sha256, "editedAtMs": edited_at_ms }),
                );
            }
            Outcome::Write(stamp) => {
                // An open session takes the new copy in place, as upstream's `reload_if_open` would, but under this
                // same lock: no edit can land on the old copy between the write and the reload and be dropped.
                // Nothing unsaved is lost, it went on file above.
                let reloaded = match sessions.get(id) {
                    Some(_) => {
                        let text = std::str::from_utf8(&bytes).map_err(internal)?;
                        let mut character =
                            borealis_character::Character::from_json_str(text).map_err(internal)?;
                        let _ =
                            borealis_lib::renamed_ids::migrate(&mut character.build, &hub.store);
                        Some(character)
                    }
                    None => None,
                };
                write_character(&file, &trash, id, &bytes, stamp)?;
                if let (Some(character), Some(session)) = (reloaded, sessions.get_mut(id)) {
                    session.character = character;
                    session.dirty = false;
                    session.rolls.clear();
                }
                true
            }
            Outcome::Lost(result) => {
                std::fs::create_dir_all(&trash).map_err(internal)?;
                std::fs::write(trash_file(&trash, id, ".device.json"), &bytes).map_err(internal)?;
                let (sha256, edited_at_ms) = file_stamp(&file);
                return Ok(
                    json!({ "result": result, "sha256": sha256, "editedAtMs": edited_at_ms }),
                );
            }
        }
    };
    let (sha256, edited_at_ms) = file_stamp(&file);
    let result = if written { "applied" } else { "identical" };
    Ok(json!({
        "result": result,
        "sha256": sha256,
        "editedAtMs": edited_at_ms,
    }))
}

/// A free trash name `<ms>-<id><ext>`: now, or the first later millisecond not taken, so two copies of one character
/// in the same millisecond do not overwrite each other.
fn trash_file(trash: &Path, id: &str, ext: &str) -> PathBuf {
    let mut ms = now_ms();
    loop {
        let path = trash.join(format!("{ms}-{id}{ext}"));
        if !path.exists() {
            return path;
        }
        ms += 1;
    }
}

/// A character file's digest and stamp, or nulls when there is none.
fn file_stamp(file: &Path) -> (Value, Value) {
    match std::fs::read(file) {
        Ok(bytes) => (json!(sha256_hex(&bytes)), json!(mtime_ms(file))),
        Err(_) => (Value::Null, Value::Null),
    }
}

/// Write a pushed character in place, as upstream's bundle apply does: the old file copied to the trash, the new
/// one written beside it, stamped, and renamed over it (the file is never missing or half written).
fn write_character(file: &Path, trash: &Path, id: &str, bytes: &[u8], stamp: u64) -> ApiResult<()> {
    if file.is_file() {
        std::fs::create_dir_all(trash).map_err(internal)?;
        std::fs::copy(file, trash_file(trash, id, ".json")).map_err(internal)?;
    }
    let dir = file.parent().unwrap_or(file);
    std::fs::create_dir_all(dir).map_err(internal)?;
    let temp = dir.join(format!(".party-push-{}.tmp", uuid::Uuid::new_v4().simple()));
    let written = std::fs::write(&temp, bytes)
        .and_then(|()| {
            std::fs::File::options()
                .write(true)
                .open(&temp)?
                .set_modified(UNIX_EPOCH + std::time::Duration::from_millis(stamp))
        })
        .and_then(|()| std::fs::rename(&temp, file));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(internal(e));
    }
    Ok(())
}

/// Run an upstream command that writes characters: under the sync lock (a push never sees one half done, and a save
/// never writes an old copy over a pushed one), with a delete record first for `delete_character`, and
/// [`CHARACTERS_CHANGED`] after it worked.
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
    let _sync = workspace.sync_lock.lock().await;
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
    let record = trash_file(&trash, id, ".json");
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
