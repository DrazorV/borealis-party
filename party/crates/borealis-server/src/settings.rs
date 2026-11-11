//! Application Settings on the server.
//!
//! Each account's settings are upstream's own `settings.json` in its data folder. The server only keeps the settings
//! that belong to the machine out of a client's reach: they are dropped from what a client saves, and reported as
//! fixed values in what it reads.

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

/// What else a client can't set: the updater, the AI CLIs and the flags upstream pins per build.
const FIXED: [&str; 5] = [
    "checkAppUpdates",
    "ai",
    "mobile",
    "storeBuild",
    "aiAvailable",
];

/// Drop the server-owned settings from a `save_settings` patch, so they never reach the account's file.
pub fn strip(patch: &mut Value) {
    if let Some(patch) = patch.as_object_mut() {
        for key in FOLDERS.iter().chain(&FIXED) {
            patch.remove(*key);
        }
    }
}

/// Settings as a client of a server sees them: no folders, no update checks, no AI, and upstream's flags for "no
/// native file pickers" (`mobile`) and "no updater" (`storeBuild`).
pub fn shown(mut settings: Value) -> Value {
    if !settings.is_object() {
        return settings;
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
/// has none of.
pub async fn restore_defaults(workspace: &Workspace) -> ApiResult<Value> {
    let current = workspace.call("get_settings", Map::new()).await?;
    let current: AppSettings = serde_json::from_value(current)
        .map_err(|e| ApiError::Internal(format!("settings: {e}")))?;
    let mut patch = serde_json::to_value(current.restored_defaults())
        .map_err(|e| ApiError::Internal(format!("settings: {e}")))?;
    strip(&mut patch);
    let mut args = Map::new();
    args.insert("patch".into(), patch);
    workspace.call("save_settings", args).await
}
