//! The admin page's trash: characters an account deleted in the last 30 days, and a Restore for each.
//!
//! The only thing the server keeps after a delete is a character's file: `delete_character` (and a push that loses
//! to a newer edit) copies it to the account's sync trash, `users/<id>/sync/trash/<ms>-<character>.json`, swept after
//! [`TRASH_RETENTION_DAYS`] (`sync.rs`). Encounters, parties, packs, homebrew and accounts are deleted for good, so
//! they have no trash. A trash entry whose character has no live file is a deleted character; the newest entry is
//! the one restored.
//!
//! What the admin sees is the account, the character's name and id, when it was deleted and when it goes: never the
//! sheet itself. A restore moves the newest copy back as the live file (stamped now, so every synced device brings it
//! back too), under the account's sync lock, and tells the account's open apps.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::Json;
use borealis_bundle::{is_plain_key, trash_dir, TRASH_RETENTION_DAYS};
use borealis_lib::state::AppState as Backend;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::Manager;

use super::{record, target, Admin};
use crate::db::Activity;
use crate::error::{ApiError, ApiResult};
use crate::AppState;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    ApiError::Internal(format!("trash: {e}"))
}

/// A trash entry's time and character id: `<ms>-<id>.json` with a plain id. Device copies (`.device.json`) and
/// anything else don't count.
fn entry_of(name: &str) -> Option<(u64, &str)> {
    let (ms, id) = name.strip_suffix(".json")?.split_once('-')?;
    let ms = ms.parse::<u64>().ok()?;
    is_plain_key(id).then_some((ms, id))
}

/// The newest entry per deleted character (no live file in `characters`), still within the retention.
fn deleted_in(trash: &Path, characters: &Path, now: u64) -> BTreeMap<String, (u64, PathBuf)> {
    let cutoff = now.saturating_sub(TRASH_RETENTION_DAYS * DAY_MS);
    let mut found: BTreeMap<String, (u64, PathBuf)> = BTreeMap::new();
    for entry in std::fs::read_dir(trash).into_iter().flatten().flatten() {
        // A regular file only: no links out of the folder.
        if !entry.path().symlink_metadata().is_ok_and(|m| m.is_file()) {
            continue;
        }
        let name = entry.file_name();
        let Some((ms, id)) = name.to_str().and_then(entry_of) else {
            continue;
        };
        if ms < cutoff || characters.join(format!("{id}.json")).exists() {
            continue;
        }
        match found.get(id) {
            Some((newest, _)) if *newest >= ms => {}
            _ => {
                found.insert(id.to_string(), (ms, entry.path()));
            }
        }
    }
    found
}

/// The name the sheet gives itself, or `None` when the copy doesn't read.
fn name_of(path: &Path) -> Option<String> {
    borealis_character::Character::load(path)
        .ok()
        .map(|c| c.meta.name.clone())
}

/// `GET /api/admin/trash`: every account's deleted characters, newest first.
pub async fn list(State(state): State<AppState>, _: Admin) -> ApiResult<Json<Value>> {
    let users = state.workspaces.dir("");
    let now = now_ms();
    let found: Vec<(String, String, u64, Option<String>)> =
        tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for entry in std::fs::read_dir(&users).into_iter().flatten().flatten() {
                let Some(account) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                let dir = entry.path();
                if !dir.symlink_metadata().is_ok_and(|m| m.is_dir()) {
                    continue;
                }
                for (id, (ms, path)) in deleted_in(&trash_dir(&dir), &dir.join("characters"), now) {
                    out.push((account.clone(), id, ms, name_of(&path)));
                }
            }
            out
        })
        .await
        .map_err(internal)?;
    let mut items = Vec::new();
    for (account, id, ms, name) in found {
        // Folders without an account (a delete whose folder step failed) are not listed.
        let Some(row) = state.db.account(&account).await? else {
            continue;
        };
        items.push(json!({
            "accountId": account,
            "username": row.user.username,
            "kind": "character",
            "characterId": id,
            "name": name,
            "deletedAtMs": ms,
            "purgeAtMs": ms + TRASH_RETENTION_DAYS * DAY_MS,
        }));
    }
    items.sort_by(|a, b| b["deletedAtMs"].as_u64().cmp(&a["deletedAtMs"].as_u64()));
    Ok(Json(
        json!({ "items": items, "retentionDays": TRASH_RETENTION_DAYS }),
    ))
}

/// The restore form.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreForm {
    account_id: String,
    character_id: String,
}

/// Move the newest trash copy of `id` back as the live file, stamped now. Its name, or an error.
fn restore_now(backend: &Backend, id: &str) -> ApiResult<Option<String>> {
    let characters = crate::sync::characters_dir(backend)?;
    let file = characters.join(format!("{id}.json"));
    if file.exists() {
        return Err(ApiError::Conflict(
            "That character exists again already; there is nothing to restore.".into(),
        ));
    }
    let trash = trash_dir(&backend.paths.data);
    let Some((_, copy)) = deleted_in(&trash, &characters, now_ms()).remove(id) else {
        return Err(ApiError::NotFound(
            "That character isn't in the trash (any more).".into(),
        ));
    };
    let name = name_of(&copy);
    std::fs::create_dir_all(&characters).map_err(internal)?;
    // Beside the live file first, then renamed into place: the file is never half written.
    let temp = characters.join(format!(
        ".party-restore-{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let moved = std::fs::copy(&copy, &temp)
        .and_then(|_| {
            std::fs::File::options()
                .write(true)
                .open(&temp)?
                .set_modified(SystemTime::now())
        })
        .and_then(|()| std::fs::rename(&temp, &file));
    if let Err(e) = moved {
        let _ = std::fs::remove_file(&temp);
        return Err(internal(e));
    }
    // The copy is the live file now; older copies of it stay until the sweep, as replaced versions.
    let _ = std::fs::remove_file(&copy);
    Ok(name)
}

/// `POST /api/admin/trash/restore` `{ accountId, characterId }`: the character is back in the account's list.
pub async fn restore(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(form): Json<RestoreForm>,
) -> ApiResult<Json<Value>> {
    if !is_plain_key(&form.character_id) {
        return Err(ApiError::NotFound(
            "That character isn't in the trash (any more).".into(),
        ));
    }
    let account = target(&state, &form.account_id).await?;
    let workspace = state.workspaces.get(&form.account_id).await?;
    let name = {
        let _sync = workspace.sync_lock.lock().await;
        let id = form.character_id.clone();
        workspace
            .blocking(move |app| restore_now(&app.state::<Backend>(), &id))
            .await?
    };
    workspace.emit(
        crate::sync::CHARACTERS_CHANGED,
        json!({ "ids": [form.character_id] }),
    );
    record(
        &state,
        Activity::new("character_restored")
            .by(&admin)
            .on(&form.account_id, &account.user.username)
            .detail(form.character_id.clone()),
    )
    .await;
    Ok(Json(
        json!({ "characterId": form.character_id, "name": name }),
    ))
}

#[cfg(test)]
mod unit {
    use super::entry_of;

    #[test]
    fn only_plain_character_entries_count() {
        assert_eq!(
            entry_of("1700000000000-c_ab.json"),
            Some((1_700_000_000_000, "c_ab"))
        );
        assert_eq!(entry_of("1700000000000-c_ab.device.json"), None);
        assert_eq!(entry_of("x-c_ab.json"), None);
        assert_eq!(entry_of("1700000000000-../evil.json"), None);
        assert_eq!(entry_of("1700000000000-c_ab.txt"), None);
    }
}
