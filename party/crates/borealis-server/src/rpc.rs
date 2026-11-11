//! The Borealis commands over HTTP, with who-may-do-what enforced here.

use std::convert::Infallible;
use std::sync::Arc;

use crate::backend::Access;
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use serde_json::{json, Map, Value};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use crate::auth::{Role, User};
use crate::db::Db;
use crate::error::{ApiError, ApiResult};
use crate::workspaces::{Workspace, WsKey};
use crate::AppState;

/// In a group, a DM may choose packs; everything else about content happens in their Library.
const GROUP_LIBRARY_COMMANDS: [&str; 2] = ["set_pack_enabled", "set_source_edition"];

/// Commands after which other workspaces have to pick up new content.
const CONTENT_CHANGING: [&str; 9] = [
    "import_pack",
    "install_import_preview",
    "remove_pack",
    "update_indexes",
    "clear_downloaded_content",
    "import_5etools",
    "update_5etools_url",
    "set_pack_enabled",
    "set_source_edition",
];

/// Commands answering with the settings.
const SETTINGS_RESULT: [&str; 2] = ["get_settings", "save_settings"];

/// Commands that make a new character.
const CREATING: [&str; 2] = ["create_character", "import_character"];

/// Commands that copy a character, which a group's one slot has no room for.
const COPYING: [&str; 2] = ["duplicate_character", "save_character_as"];

/// A group as seen by a caller: who runs it and whether the caller has a character in it.
pub struct GroupRole {
    /// The group's DM.
    pub dm: String,
    /// The caller's character, when the caller is a player with one.
    pub character: Option<String>,
}

/// The caller's place in a group; `None` when they have none (reported as "no such group").
pub async fn group_role(db: &Db, user: &User, group: &str) -> ApiResult<Option<GroupRole>> {
    let Some(dm) = db.group_dm(group).await? else {
        return Ok(None);
    };
    if user.role == Role::Dm {
        return Ok((dm == user.id).then_some(GroupRole {
            dm,
            character: None,
        }));
    }
    Ok(db
        .membership(group, &user.id)
        .await?
        .map(|character| GroupRole { dm, character }))
}

async fn resolve_scope(state: &AppState, user: &User, scope: &str) -> ApiResult<(WsKey, Scope)> {
    match (scope, user.role) {
        ("personal", Role::Player) => Ok((
            WsKey::Personal {
                player: user.id.clone(),
            },
            Scope::Personal,
        )),
        ("library", Role::Dm) => Ok((
            WsKey::Library {
                dm: user.id.clone(),
            },
            Scope::Library,
        )),
        ("personal", Role::Dm) => Err(ApiError::Forbidden(
            "Only players have personal characters.".into(),
        )),
        ("library", Role::Player) => Err(ApiError::Forbidden(
            "Only Dungeon Masters have a content library.".into(),
        )),
        _ => {
            let group = scope
                .strip_prefix("group-")
                .ok_or_else(|| ApiError::NotFound("No such place.".into()))?;
            let role = group_role(&state.db, user, group)
                .await?
                .ok_or_else(ApiError::group_not_found)?;
            let key = match user.role {
                Role::Dm => WsKey::GroupDm {
                    group: group.to_string(),
                    dm: role.dm,
                },
                Role::Player => WsKey::GroupMember {
                    group: group.to_string(),
                    dm: role.dm,
                    player: user.id.clone(),
                },
            };
            Ok((key, Scope::Group(group.to_string())))
        }
    }
}

enum Scope {
    Personal,
    Library,
    Group(String),
}

fn check_access(role: Role, scope: &Scope, access: Access, command: &str) -> ApiResult<()> {
    match access {
        Access::Read => Ok(()),
        Access::Character if role == Role::Player => Ok(()),
        Access::Character => Err(ApiError::Forbidden(
            "Dungeon Masters can look at characters but not change them.".into(),
        )),
        Access::Library if role == Role::Dm => match scope {
            Scope::Group(_) if !GROUP_LIBRARY_COMMANDS.contains(&command) => {
                Err(ApiError::Forbidden(
                    "Add content from your Library, then choose which packs each group uses."
                        .into(),
                ))
            }
            _ => Ok(()),
        },
        Access::Library => Err(ApiError::Forbidden(
            "Your Dungeon Master manages the content.".into(),
        )),
    }
}

/// Server paths are nobody's business: the commands report them for the desktop app's benefit.
pub fn scrub_paths(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("displayPath");
            if map.contains_key("updatedAt") {
                map.remove("path");
            }
            map.values_mut().for_each(scrub_paths);
        }
        Value::Array(items) => items.iter_mut().for_each(scrub_paths),
        _ => {}
    }
}

fn created_id(command: &str, result: &Value) -> Option<String> {
    let id = if command == "import_character" {
        result.get("summary")?.get("id")?
    } else {
        result.get("id")?
    };
    id.as_str().map(str::to_string)
}

async fn set_character(
    db: &Db,
    group: String,
    player: String,
    character: Option<String>,
) -> ApiResult<()> {
    db.set_member_character(character.as_deref(), &group, &player)
        .await
}

async fn call_in_group(
    state: &AppState,
    user: &User,
    workspace: &Arc<Workspace>,
    group: String,
    command: &str,
    args: Map<String, Value>,
) -> ApiResult<Value> {
    if COPYING.contains(&command) && args.get("name").is_none_or(|n| !n.is_null()) {
        return Err(ApiError::Conflict(
            "A group holds one character per player, so it can't hold a copy.".into(),
        ));
    }
    if CREATING.contains(&command) {
        // Held across the create so two devices can't both fill the one slot.
        let _guard = state.membership.lock().await;
        let role = group_role(&state.db, user, &group)
            .await?
            .ok_or_else(ApiError::group_not_found)?;
        if role.character.is_some() {
            return Err(ApiError::Conflict(
                "You already have a character in this group.".into(),
            ));
        }
        let result = workspace.call(command, args).await?;
        let id = created_id(command, &result)
            .ok_or_else(|| ApiError::Internal("a created character had no id".into()))?;
        set_character(&state.db, group, user.id.clone(), Some(id)).await?;
        return Ok(result);
    }
    let deleted = (command == "delete_character")
        .then(|| args.get("characterId").and_then(Value::as_str))
        .flatten()
        .map(str::to_string);
    let result = workspace.call(command, args).await?;
    if let Some(deleted) = deleted {
        let _guard = state.membership.lock().await;
        let matches = state
            .db
            .membership(&group, &user.id)
            .await?
            .flatten()
            .is_some_and(|current| current == deleted);
        if matches {
            set_character(&state.db, group, user.id.clone(), None).await?;
        }
    }
    Ok(result)
}

/// `POST /api/rpc/{scope}/{command}`.
pub async fn command(
    State(state): State<AppState>,
    user: User,
    Path((scope, command)): Path<(String, String)>,
    Json(args): Json<Map<String, Value>>,
) -> ApiResult<Json<Value>> {
    if command == "save_character_as" && args.get("name").is_none_or(Value::is_null) {
        // Without a name upstream asks a native dialog where to write, and a server has no such thing.
        return Err(ApiError::Invalid("Choose a name for the copy.".into()));
    }
    let (key, resolved) = resolve_scope(&state, &user, &scope).await?;
    let workspace = state.workspaces.get(&key).await?;
    let access = workspace
        .access_of(&command)
        .ok_or_else(|| ApiError::NotFound("The server does not offer that.".into()))?;
    check_access(user.role, &resolved, access, &command)?;
    let mut result = match resolved {
        Scope::Group(id) if user.role == Role::Player => {
            call_in_group(&state, &user, &workspace, id, &command, args).await?
        }
        _ => workspace.call(&command, args).await?,
    };
    if CONTENT_CHANGING.contains(&command.as_str()) {
        reload_others(&state, &key);
    }
    if SETTINGS_RESULT.contains(&command.as_str()) {
        // Upstream's flag for "no native file pickers": every client of the server is in that position.
        result["mobile"] = json!(true);
    }
    scrub_paths(&mut result);
    Ok(Json(result))
}

fn reload_others(state: &AppState, origin: &WsKey) {
    let reason = "library-changed";
    match origin.clone() {
        WsKey::Library { dm } => state.workspaces.reload_where(reason, move |key| {
            matches!(key, WsKey::GroupDm { dm: d, .. } | WsKey::GroupMember { dm: d, .. } if *d == dm)
        }),
        WsKey::GroupDm { group, .. } => state.workspaces.reload_where(
            reason,
            move |key| matches!(key, WsKey::GroupMember { group: g, .. } if *g == group),
        ),
        _ => {}
    }
}

/// `GET /api/events/{scope}`: the workspace's events as server-sent events.
pub async fn events(
    State(state): State<AppState>,
    user: User,
    Path(scope): Path<String>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let (key, _) = resolve_scope(&state, &user, &scope).await?;
    let workspace = state.workspaces.get(&key).await?;
    let stream = BroadcastStream::new(workspace.events.subscribe()).filter_map(|event| {
        let event = event.ok()?;
        Some(Ok(Event::default().data(
            json!({ "event": event.name, "payload": event.payload }).to_string(),
        )))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
