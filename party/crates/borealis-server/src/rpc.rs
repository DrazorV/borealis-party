//! The Borealis commands over HTTP. A signed-in account runs them in its own workspace, and in no other.

use std::convert::Infallible;

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use serde_json::{json, Map, Value};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

use crate::auth::User;
use crate::error::{ApiError, ApiResult};
use crate::AppState;
use crate::{settings, sync};

/// Server paths are nobody's business: the commands report them for the desktop app's benefit.
fn scrub_paths(value: &mut Value) {
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

/// `POST /api/rpc/{command}`.
pub async fn command(
    State(state): State<AppState>,
    user: User,
    Path(command): Path<String>,
    Json(mut args): Json<Map<String, Value>>,
) -> ApiResult<Json<Value>> {
    if command == "save_character_as" && args.get("name").is_none_or(Value::is_null) {
        // Without a name upstream asks a native dialog where to write, and a server has no such thing.
        return Err(ApiError::Invalid("Choose a name for the copy.".into()));
    }
    let workspace = state.workspaces.get(&user.id).await?;
    let mut result = match command.as_str() {
        "restore_default_settings" => settings::restore_defaults(&workspace).await?,
        "save_settings" => {
            if let Some(patch) = args.get_mut("patch") {
                settings::strip(patch);
            }
            workspace.call(&command, args).await?
        }
        name if sync::COMMANDS.contains(&name) => sync::call(&workspace, name, &args).await?,
        _ => sync::call_upstream(&workspace, &command, args).await?,
    };
    if settings::COMMANDS.contains(&command.as_str()) {
        result = settings::shown(result);
    }
    scrub_paths(&mut result);
    Ok(Json(result))
}

/// `GET /api/events`: the events of the account's workspace, as server-sent events.
pub async fn events(
    State(state): State<AppState>,
    user: User,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let workspace = state.workspaces.get(&user.id).await?;
    let stream = BroadcastStream::new(workspace.events.subscribe()).filter_map(|event| {
        let event = event.ok()?;
        Some(Ok(Event::default().data(
            json!({ "event": event.name, "payload": event.payload }).to_string(),
        )))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// The Borealis commands and the account's event stream.
pub fn routes() -> axum::Router<AppState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/rpc/{command}", post(command))
        .route("/api/events", get(events))
}
