//! Application Settings on the server.
//!
//! Each account's settings are upstream's own `settings.json` in its data folder. The server only keeps the settings
//! that belong to the machine out of a client's reach: they are dropped from what a client saves, and reported as
//! fixed values in what it reads.

use std::path::Path;

use borealis_lib::settings::{AiSettings, AppSettings};
use serde_json::{json, Map, Value};

use crate::error::{ApiError, ApiResult};
use crate::workspaces::Workspace;

/// The settings commands, whose answers go through [`shown`].
pub const COMMANDS: [&str; 3] = ["get_settings", "save_settings", "restore_default_settings"];

/// The folders on the server's disk, which no client chooses.
const FOLDERS: [&str; 4] = [
    "documentsDir",
    "secondaryContentDir",
    "portraitLibraryDir",
    "vaultDir",
];

/// What else a client can't set: the updater, the AI CLIs, the flags upstream pins per build, and the spell copies
/// the server hides from new picks (one spell per edition: [`crate::spells::sync_hidden`]).
const FIXED: [&str; 6] = [
    "checkAppUpdates",
    "ai",
    "mobile",
    "storeBuild",
    "aiAvailable",
    "hiddenElements",
];

/// The party's own settings, which upstream's file has no field for, with their defaults. Each account keeps them in
/// [`PARTY_FILE`] in its folder; a client reads and saves them with the rest.
/// - `skipPersonalityChoices`: the selection flow leaves out a background's personality trait, ideal, bond and flaw
///   lists (ui/party/api/overrides/personality.ts).
const PARTY: [(&str, bool); 1] = [("skipPersonalityChoices", true)];

/// The account's party settings, beside upstream's `settings.json`.
pub const PARTY_FILE: &str = "party-settings.json";

/// The party settings saved in an account folder (none for a missing or unreadable file).
fn party_saved(dir: &Path) -> Map<String, Value> {
    std::fs::read(dir.join(PARTY_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// Drop the server-owned settings from a `save_settings` patch, so they never reach the account's file, and keep the
/// party's own ones in [`PARTY_FILE`] (a patch without them leaves that file as it is).
pub fn strip(patch: &mut Value, dir: &Path) -> ApiResult<()> {
    let Some(patch) = patch.as_object_mut() else {
        return Ok(());
    };
    for key in FOLDERS.iter().chain(&FIXED) {
        patch.remove(*key);
    }
    let mut saved = party_saved(dir);
    let mut changed = false;
    for (key, _) in PARTY {
        if let Some(value) = patch.remove(key) {
            if value.is_boolean() {
                changed |= saved.insert(key.to_string(), value.clone()) != Some(value);
            }
        }
    }
    if changed {
        let bytes = serde_json::to_vec_pretty(&Value::Object(saved))
            .map_err(|e| ApiError::Internal(format!("settings: {e}")))?;
        std::fs::create_dir_all(dir)
            .and_then(|()| std::fs::write(dir.join(PARTY_FILE), bytes))
            .map_err(|e| ApiError::Internal(format!("settings: {e}")))?;
    }
    Ok(())
}

/// Settings as a client of a server sees them: no folders, no update checks, no AI, and upstream's flags for "no
/// native file pickers" (`mobile`) and "no updater" (`storeBuild`).
pub fn shown(mut settings: Value, dir: &Path) -> Value {
    if !settings.is_object() {
        return settings;
    }
    let saved = party_saved(dir);
    for (key, default) in PARTY {
        settings[key] = saved
            .get(key)
            .filter(|v| v.is_boolean())
            .cloned()
            .unwrap_or(json!(default));
    }
    for key in FOLDERS {
        settings[key] = Value::Null;
    }
    settings["checkAppUpdates"] = json!(false);
    settings["ai"] = serde_json::to_value(AiSettings::default()).unwrap_or(Value::Null);
    settings["mobile"] = json!(true);
    settings["storeBuild"] = json!(true);
    settings["aiAvailable"] = json!(false);
    settings
}

/// Upstream's Restore Defaults, as a save: its own command also re-links folders on the machine, which a server
/// has none of. The party's own settings go back to their defaults too.
pub async fn restore_defaults(workspace: &Workspace, dir: &Path) -> ApiResult<Value> {
    match std::fs::remove_file(dir.join(PARTY_FILE)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(ApiError::Internal(format!("settings: {e}")));
        }
        _ => {}
    }
    let current = workspace.call("get_settings", Map::new()).await?;
    let current: AppSettings = serde_json::from_value(current)
        .map_err(|e| ApiError::Internal(format!("settings: {e}")))?;
    let mut patch = serde_json::to_value(current.restored_defaults())
        .map_err(|e| ApiError::Internal(format!("settings: {e}")))?;
    strip(&mut patch, dir)?;
    let mut args = Map::new();
    args.insert("patch".into(), patch);
    workspace.call("save_settings", args).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn party_settings_default_on_and_are_kept_beside_upstreams() {
        let dir = tempfile::tempdir().unwrap();
        let shown_now = || shown(json!({ "accentColor": "blue" }), dir.path());
        assert_eq!(shown_now()["skipPersonalityChoices"], true, "the default");

        let mut patch =
            json!({ "skipPersonalityChoices": false, "accentColor": "green", "vaultDir": "/tmp" });
        strip(&mut patch, dir.path()).unwrap();
        assert_eq!(
            patch,
            json!({ "accentColor": "green" }),
            "only upstream's settings go on to upstream"
        );
        assert_eq!(shown_now()["skipPersonalityChoices"], false);

        // A patch without it (upstream's Save sends its own fields) leaves it as it is; a non-boolean is ignored.
        strip(&mut json!({ "accentColor": "red" }), dir.path()).unwrap();
        strip(&mut json!({ "skipPersonalityChoices": "yes" }), dir.path()).unwrap();
        assert_eq!(shown_now()["skipPersonalityChoices"], false);

        strip(&mut json!({ "skipPersonalityChoices": true }), dir.path()).unwrap();
        assert_eq!(shown_now()["skipPersonalityChoices"], true, "and back on");
    }

    #[test]
    fn the_source_restrict_toggles_go_on_to_the_accounts_settings() {
        // Sources page "Restrict playtest / third-party content": upstream settings, so each account's own file keeps them.
        let dir = tempfile::tempdir().unwrap();
        let mut patch = json!({ "restrictPlaytest": true, "restrictThirdParty": false });
        strip(&mut patch, dir.path()).unwrap();
        assert_eq!(
            patch,
            json!({ "restrictPlaytest": true, "restrictThirdParty": false })
        );
        assert!(!dir.path().join(PARTY_FILE).exists());
    }

    #[test]
    fn an_unreadable_party_file_reads_as_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(PARTY_FILE), "not json").unwrap();
        assert_eq!(shown(json!({}), dir.path())["skipPersonalityChoices"], true);
        let mut patch = json!({ "skipPersonalityChoices": false });
        strip(&mut patch, dir.path()).unwrap();
        assert_eq!(
            shown(json!({}), dir.path())["skipPersonalityChoices"],
            false
        );
    }
}
