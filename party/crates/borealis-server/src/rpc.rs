//! The Borealis commands over HTTP. A signed-in account runs them in its own workspace, and in no other.

use std::convert::Infallible;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::auth::{token_hash, User};
use crate::error::{ApiError, ApiResult};
use crate::hub::{Subscription, RESYNC_EVENT};
use crate::workspaces::WorkspaceEvent;
use crate::AppState;
use crate::{jobs, parties, settings, sync};

/// Server paths are nobody's business: the commands report them for the desktop app's benefit.
pub(crate) fn scrub_paths(value: &mut Value) {
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
    // Which party cards the command may change, read before it runs (it consumes its arguments).
    let target = parties::card_target(&command, &args);
    let mut result = match command.as_str() {
        "restore_default_settings" => settings::restore_defaults(&workspace).await?,
        "save_settings" => {
            if let Some(patch) = args.get_mut("patch") {
                settings::strip(patch);
            }
            workspace.call(&command, args).await?
        }
        name if sync::COMMANDS.contains(&name) => sync::call(&workspace, name, &args).await?,
        name if jobs::COMMANDS.contains(&name) => {
            jobs::call(state.workspaces.jobs(), &user.id, &workspace, name, &args).await?
        }
        _ => sync::call_upstream(&workspace, &command, args).await?,
    };
    parties::after_command(&state, &user.id, &workspace, target).await;
    if settings::COMMANDS.contains(&command.as_str()) {
        result = settings::shown(result);
    }
    scrub_paths(&mut result);
    Ok(Json(result))
}

/// How often an open stream checks that its session still exists. Revocation closes it at once; this is the
/// backstop.
const SESSION_CHECK: Duration = Duration::from_secs(60);

/// `?after=<id>`: the last event id the client saw on an earlier stream.
#[derive(Debug, Default, Deserialize)]
pub struct EventsQuery {
    #[serde(default)]
    after: Option<u64>,
}

fn data(event: &str, payload: &Value) -> Event {
    Event::default().data(json!({ "event": event, "payload": payload }).to_string())
}

/// A resync, with the account's last event id, so a stream opened with `?after=` that id needs none.
fn resync(state: &AppState, party: &Subscription) -> Event {
    data(RESYNC_EVENT, &json!({})).id(state.hub.last_id(party.user()).to_string())
}

/// `GET /api/events`: the events of the account's workspace and of its parties, as server-sent events.
///
/// Party events carry ids (`id:`), counted per account. Workspace events dropped because the stream fell behind, and
/// party events the client missed (`?after=`), become one `party://resync`. The stream ends when its session is
/// signed out, the password changes, or the account is disabled or deleted.
pub async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: User,
    Query(query): Query<EventsQuery>,
) -> ApiResult<impl IntoResponse> {
    let workspace = state.workspaces.get(&user.id).await?;
    let session = crate::auth::bearer(&headers)
        .map(token_hash)
        .unwrap_or_default();
    let subscription = state.hub.subscribe(&user.id, &session, query.after);
    let (out, stream) = mpsc::channel::<Event>(64);
    tokio::spawn(forward(
        state.clone(),
        session,
        workspace.events.subscribe(),
        subscription,
        out,
    ));
    let sse = Sse::new(ReceiverStream::new(stream).map(Ok::<_, Infallible>))
        .keep_alive(KeepAlive::default());
    Ok((
        [(
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        )],
        sse,
    ))
}

/// Feed one stream until the client goes or its session is revoked.
async fn forward(
    state: AppState,
    session: String,
    mut workspace: broadcast::Receiver<WorkspaceEvent>,
    mut party: Subscription,
    out: mpsc::Sender<Event>,
) {
    if party.resync && out.send(resync(&state, &party)).await.is_err() {
        return;
    }
    let mut check = tokio::time::interval(SESSION_CHECK);
    check.tick().await;
    loop {
        let event = tokio::select! {
            biased;
            _ = party.closed.changed() => break,
            () = out.closed() => break,
            message = party.rx.recv() => match message {
                Some(m) => data(m.event, &m.payload).id(m.id.to_string()),
                None => break,
            },
            received = workspace.recv() => match received {
                Ok(e) => data(&e.name, &e.payload),
                Err(broadcast::error::RecvError::Lagged(_)) => resync(&state, &party),
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = check.tick() => match state.db.session_user(&session).await {
                Ok(Some(_)) | Err(_) => continue,
                Ok(None) => break,
            },
        };
        if out.send(event).await.is_err() {
            break;
        }
        if party.lagged.swap(false, Ordering::SeqCst)
            && out.send(resync(&state, &party)).await.is_err()
        {
            break;
        }
    }
}

/// The Borealis commands and the account's event stream.
pub fn routes() -> axum::Router<AppState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/rpc/{command}", post(command))
        .route("/api/events", get(events))
}
