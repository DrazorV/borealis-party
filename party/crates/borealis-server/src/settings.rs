//! Application Settings on the server, split by who owns each field.
//!
//! Upstream keeps one `settings.json` per install. Here every field has one owner:
//!
//! | Fields | Owner | Kept in |
//! |---|---|---|
//! | [`USER_FIELDS`] (look, sheet, search, confirmations, autosave, player name) | each account | the `user_settings` table |
//! | [`DM_FIELDS`] (content library behaviour) | each Dungeon Master | the `user_settings` table |
//! | [`RULE_FIELDS`] in a group (house rules) | the group's DM | `groups/<g>/dm/settings.json`, which members link to |
//! | [`RULE_FIELDS`] outside groups | each player | `users/<p>/personal/settings.json` |
//! | anything else (folders, updates, AI, devices) | the server's administrator | never changed by a client |
//!
//! A workspace's settings in memory are its file plus its owner's account settings laid over them, so the upstream
//! commands (a new character's player name, the sheet's look) see the right values without knowing any of this.

use borealis_lib::settings::{AiSettings, AppSettings};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::auth::{Role, User};
use crate::error::{ApiError, ApiResult};
use crate::workspaces::{Workspace, WsKey};
use crate::AppState;

/// One copy per account, the same in every workspace the account opens.
pub const USER_FIELDS: [&str; 16] = [
    "theme",
    "accentColor",
    "portraitSize",
    "density",
    "rowHeight",
    "tileSize",
    "highContrast",
    "sheet",
    "search",
    "selectionFlow",
    "confirmRemoveLevel",
    "confirmEquipWeaponNotInAttacks",
    "autosave",
    "playerName",
    "skillsPanelExpanded",
    "savingThrowsExpanded",
];

/// A Dungeon Master's content library. Players never see them.
pub const DM_FIELDS: [&str; 2] = ["autoContentUpdate", "showImportNotice"];

/// The rules engine's limits: a group's house rules, set by its DM; a player's own outside groups.
pub const RULE_FIELDS: [&str; 6] = [
    "abilityMethod",
    "pointBuyBudget",
    "abilityMinBase",
    "abilityMaxBase",
    "abilityCapEnabled",
    "defaultRuleset",
];

/// The machine's, not a client's: set by the administrator in the server's configuration.
pub const SERVER_FIELDS: [&str; 6] = [
    "documentsDir",
    "secondaryContentDir",
    "portraitLibraryDir",
    "vaultDir",
    "checkAppUpdates",
    "ai",
];

/// Read-only flags the build pins; patches may carry them back and they are ignored, as upstream does.
pub const PLATFORM_FIELDS: [&str; 3] = ["mobile", "storeBuild", "aiAvailable"];

const MANAGED_BY_SERVER: &str = "That setting is managed by the server's administrator.";
const SET_BY_DM: &str = "Your Dungeon Master sets this group's house rules.";
const RULES_IN_GROUPS: &str = "House rules belong to a group. Open the group to change them.";
const DM_ONLY: &str = "Only Dungeon Masters have a content library.";

/// Who owns a workspace, and so whose account settings it carries.
fn owner(key: &WsKey) -> (&str, Role) {
    match key {
        WsKey::Personal { player } | WsKey::GroupMember { player, .. } => (player, Role::Player),
        WsKey::Library { dm } | WsKey::GroupDm { dm, .. } => (dm, Role::Dm),
    }
}

fn account_fields(role: Role) -> impl Iterator<Item = &'static str> {
    let dm: &[&str] = if role == Role::Dm { &DM_FIELDS } else { &[] };
    USER_FIELDS.iter().chain(dm).copied()
}

fn to_value(settings: &AppSettings) -> ApiResult<Value> {
    serde_json::to_value(settings).map_err(|e| ApiError::Internal(format!("settings: {e}")))
}

/// The settings as a client of this server sees them: no server folders, no AI, no update checks.
pub fn shown(settings: &AppSettings, role: Role) -> ApiResult<Value> {
    let mut value = to_value(settings)?;
    let ai = serde_json::to_value(AiSettings::default())
        .map_err(|e| ApiError::Internal(format!("settings: {e}")))?;
    for field in SERVER_FIELDS {
        value[field] = match field {
            "checkAppUpdates" => json!(false),
            "ai" => ai.clone(),
            _ => Value::Null,
        };
    }
    // Upstream's flags for "no native file pickers, no updater, no AI CLI": true of every client of a server.
    value["mobile"] = json!(true);
    value["storeBuild"] = json!(true);
    value["aiAvailable"] = json!(false);
    if role == Role::Player {
        value["autoContentUpdate"] = json!(false);
        value["showImportNotice"] = json!(false);
    }
    Ok(value)
}

/// The account's own fields out of a full set of settings.
fn account_part(settings: &AppSettings, role: Role) -> ApiResult<Map<String, Value>> {
    let value = to_value(settings)?;
    Ok(account_fields(role)
        .filter_map(|f| Some((f.to_string(), value.get(f)?.clone())))
        .collect())
}

/// Every account field (a player's workspace gets the defaults for a DM's), from what the account saved and the
/// defaults for the rest. Covering every field means nothing of them is left from the workspace's file.
fn overlay(saved: &Map<String, Value>, role: Role) -> ApiResult<Map<String, Value>> {
    let defaults = to_value(&AppSettings::default())?;
    Ok(account_fields(Role::Dm)
        .filter_map(|f| {
            let own = role == Role::Dm || !DM_FIELDS.contains(&f);
            let value = own
                .then(|| saved.get(f))
                .flatten()
                .or_else(|| defaults.get(f))?;
            Some((f.to_string(), value.clone()))
        })
        .collect())
}

async fn load(state: &AppState, user: &str) -> ApiResult<Option<Map<String, Value>>> {
    let user = user.to_string();
    let text: Option<String> = state
        .db
        .run(move |c| {
            Ok(c.query_row(
                "SELECT settings FROM user_settings WHERE user_id = ?1",
                params![user],
                |r| r.get(0),
            )
            .optional()?)
        })
        .await?;
    Ok(text.map(|t| match serde_json::from_str::<Value>(&t) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }))
}

/// Write an account's settings; `replace` false keeps a row that is already there.
async fn store(
    state: &AppState,
    user: &str,
    settings: &Map<String, Value>,
    replace: bool,
) -> ApiResult<()> {
    let user = user.to_string();
    let text = Value::Object(settings.clone()).to_string();
    let sql = if replace {
        "INSERT INTO user_settings (user_id, settings) VALUES (?1, ?2) \
         ON CONFLICT (user_id) DO UPDATE SET settings = excluded.settings"
    } else {
        "INSERT OR IGNORE INTO user_settings (user_id, settings) VALUES (?1, ?2)"
    };
    state
        .db
        .run(move |c| {
            c.execute(sql, params![user, text])?;
            Ok(())
        })
        .await
}

async fn current(workspace: &Workspace) -> ApiResult<AppSettings> {
    let value = workspace.call("get_settings", Map::new()).await?;
    serde_json::from_value(value).map_err(|e| ApiError::Internal(format!("settings: {e}")))
}

/// Lay the owner's account settings over the workspace's own. Every request into a workspace starts with this, so a
/// change made in one workspace reaches the account's others before they next run a command.
pub async fn sync(state: &AppState, key: &WsKey, workspace: &Workspace) -> ApiResult<()> {
    let (user, role) = owner(key);
    let saved = match load(state, user).await? {
        Some(saved) => saved,
        // Settings from before accounts had their own come from the first workspace the account opens. A member's
        // file is the DM's, so it is no source for the player's.
        None if !matches!(key, WsKey::GroupMember { .. }) => {
            let seed = account_part(&current(workspace).await?, role)?;
            store(state, user, &seed, false).await?;
            load(state, user).await?.unwrap_or(seed)
        }
        None => Map::new(),
    };
    workspace.overlay_settings(&overlay(&saved, role)?)
}

/// Where a changed field goes, or why it can't change here.
enum Place {
    Account,
    Workspace,
}

fn place(key: &WsKey, role: Role, field: &str) -> Result<Place, &'static str> {
    if USER_FIELDS.contains(&field) {
        return Ok(Place::Account);
    }
    if DM_FIELDS.contains(&field) {
        return if role == Role::Dm {
            Ok(Place::Account)
        } else {
            Err(DM_ONLY)
        };
    }
    if RULE_FIELDS.contains(&field) {
        return match key {
            WsKey::Personal { .. } | WsKey::GroupDm { .. } => Ok(Place::Workspace),
            WsKey::GroupMember { .. } => Err(SET_BY_DM),
            WsKey::Library { .. } => Err(RULES_IN_GROUPS),
        };
    }
    Err(MANAGED_BY_SERVER)
}

fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(t), Value::Object(p)) => {
            for (k, v) in p {
                match t.get_mut(k) {
                    Some(existing) if existing.is_object() && v.is_object() => merge(existing, v),
                    _ => {
                        t.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (t, p) => *t = p.clone(),
    }
}

/// Apply a patch. The settings window sends every field back, so a field equal to what the caller was shown is not a
/// change. With `strict`, a change the caller may not make is refused; without, it is skipped (Restore defaults).
async fn save(
    state: &AppState,
    key: &WsKey,
    role: Role,
    workspace: &Workspace,
    patch: &Value,
    strict: bool,
) -> ApiResult<Value> {
    let patch = patch
        .as_object()
        .ok_or_else(|| ApiError::Invalid("The settings patch must be an object.".into()))?;
    let before = current(workspace).await?;
    let seen = shown(&before, role)?;
    let mut account = Map::new();
    let mut rules = Map::new();
    for (field, value) in patch {
        if PLATFORM_FIELDS.contains(&field.as_str()) {
            continue;
        }
        if let Some(old) = seen.get(field) {
            let mut merged = old.clone();
            merge(&mut merged, value);
            if merged == *old {
                continue;
            }
        }
        match place(key, role, field) {
            Ok(Place::Account) => account.insert(field.clone(), value.clone()),
            Ok(Place::Workspace) => rules.insert(field.clone(), value.clone()),
            Err(reason) if strict => return Err(ApiError::Forbidden(reason.into())),
            Err(_) => None,
        };
    }
    let mut both = account.clone();
    both.extend(rules.clone());
    let next = before
        .patched(&Value::Object(both))
        .map_err(|e| ApiError::Invalid(e.message))?;
    if !rules.is_empty() {
        let mut args = Map::new();
        args.insert("patch".into(), Value::Object(rules));
        workspace.call("save_settings", args).await?;
        if let WsKey::GroupDm { group, .. } = key {
            let group = group.clone();
            state.workspaces.reload_where(
                "house-rules-changed",
                move |k| matches!(k, WsKey::GroupMember { group: g, .. } if *g == group),
            );
        }
    }
    if !account.is_empty() {
        let (user, _) = owner(key);
        let saved = account_part(&next, role)?;
        store(state, user, &saved, true).await?;
        workspace.overlay_settings(&overlay(&saved, role)?)?;
    }
    shown(&current(workspace).await?, role)
}

/// The settings commands, answered here rather than by the workspace alone. Also brings the workspace's account
/// settings up to date before any other command. `None`: not a settings command, run it as usual.
pub async fn handle(
    state: &AppState,
    user: &User,
    key: &WsKey,
    workspace: &Workspace,
    command: &str,
    args: &Map<String, Value>,
) -> ApiResult<Option<Value>> {
    sync(state, key, workspace).await?;
    let role = user.role;
    let answer = match command {
        "get_settings" => shown(&current(workspace).await?, role)?,
        "save_settings" => {
            let patch = args.get("patch").cloned().unwrap_or(Value::Null);
            save(state, key, role, workspace, &patch, true).await?
        }
        "restore_default_settings" => {
            let defaults = shown(&current(workspace).await?.restored_defaults(), role)?;
            save(state, key, role, workspace, &defaults, false).await?
        }
        _ => return Ok(None),
    };
    Ok(Some(answer))
}
