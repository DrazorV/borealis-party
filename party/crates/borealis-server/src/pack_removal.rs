//! Removing content in bulk, for Additional Content: all packs of a category, or the packs that came from one source.
//!
//! Upstream removes one pack per command, and each removal reloads the account's whole content, so a phone removing a
//! hundred 5etools packs would wait for a hundred reloads. Here the account's packs are deleted together and the
//! content reloads once. Only the caller's own workspace is touched; built-in packs are never removed. Character files
//! are left alone: a character using removed content shows it as missing, as after any removal.
//!
//! Routes (any account, its own content only):
//! - `GET /api/content/pack-sources`: `{ packs: [{ id, indexUrl?, fiveToolsUrl? }] }`, where each pack came from;
//! - `POST /api/content/remove-packs` `{ packIds }`: `{ removed, skipped, packs }`, `packs` as `list_packs` answers.

use std::collections::{BTreeSet, HashMap};

use axum::extract::State;
use axum::Json;
use borealis_lib::commands_content::TauriSink;
use borealis_lib::content_packs::{InstalledPack, PACK_META_FILE};
use borealis_lib::state::AppState as Backend;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::Manager;

use crate::auth::User;
use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// Most packs one request removes.
pub const MAX_PACKS: usize = 2000;

/// The index a downloaded pack came from, from its `pack-meta.json`.
fn index_url(pack: &InstalledPack) -> Option<String> {
    let text = std::fs::read_to_string(pack.dir.join(PACK_META_FILE)).ok()?;
    let meta: Value = serde_json::from_str(&text).ok()?;
    meta["indexUrl"]
        .as_str()
        .map(str::to_string)
        .filter(|u| !u.is_empty())
}

/// Every installed pack that came from somewhere: an index or a 5etools address.
fn sources(backend: &Backend) -> Vec<Value> {
    let mut five: HashMap<String, String> = HashMap::new();
    for record in backend.list_5etools_urls() {
        for pack in record.packs {
            five.insert(pack, record.url.clone());
        }
    }
    backend
        .installed_packs()
        .iter()
        .filter(|p| !p.bundled)
        .filter_map(|p| {
            let id = p.manifest.id.clone();
            let index = index_url(p);
            let fivetools = five.get(&id).cloned();
            (index.is_some() || fivetools.is_some())
                .then(|| json!({ "id": id, "indexUrl": index, "fiveToolsUrl": fivetools }))
        })
        .collect()
}

/// Delete the packs `ids` of the account's content and reload it once. Answers how many went, and the ids that were
/// not removed (built-in or not installed).
fn remove(
    backend: &Backend,
    sink: &TauriSink,
    ids: &BTreeSet<String>,
) -> ApiResult<(usize, Vec<String>)> {
    let lock = |e: borealis_lib::error::ApiError| ApiError::Internal(e.message);
    let _guard = backend.content_lock().map_err(lock)?;
    let installed = backend.installed_packs();
    let mut prefs = backend.content_prefs();
    let mut removed = 0;
    let mut skipped: Vec<String> = Vec::new();
    for id in ids {
        match installed.iter().find(|p| &p.manifest.id == id) {
            Some(pack) if !pack.bundled => {
                std::fs::remove_dir_all(&pack.dir)
                    .map_err(|e| ApiError::Internal(format!("remove {id}: {e}")))?;
                prefs.disabled_packs.remove(id);
                removed += 1;
            }
            _ => skipped.push(id.clone()),
        }
    }
    if removed > 0 {
        backend
            .reload_content(&prefs, sink, "pack-removed")
            .map_err(|e| ApiError::Internal(e.message))?;
    }
    Ok((removed, skipped))
}

async fn pack_sources(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    let workspace = state.workspaces.get(&user.id).await?;
    let packs = workspace
        .blocking(|handle| Ok(sources(&handle.state::<Backend>())))
        .await?;
    Ok(Json(json!({ "packs": packs })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoveForm {
    pack_ids: Vec<String>,
}

async fn remove_packs(
    State(state): State<AppState>,
    user: User,
    Json(form): Json<RemoveForm>,
) -> ApiResult<Json<Value>> {
    let ids: BTreeSet<String> = form
        .pack_ids
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    if ids.is_empty() {
        return Err(ApiError::Invalid("Choose the packs to remove.".into()));
    }
    if ids.len() > MAX_PACKS {
        return Err(ApiError::Invalid(format!(
            "Remove at most {MAX_PACKS} packs at once."
        )));
    }
    let workspace = state.workspaces.get(&user.id).await?;
    let (removed, skipped, packs) = workspace
        .blocking(move |handle| {
            let backend = handle.state::<Backend>();
            let sink = TauriSink(handle.clone());
            let (removed, skipped) = remove(&backend, &sink, &ids)?;
            let packs = backend
                .list_packs()
                .map_err(|e| ApiError::Internal(e.message))?;
            Ok((removed, skipped, packs))
        })
        .await?;
    // Content reloaded: every open Additional Content page of the account fetches its lists again, and the account's
    // party cards may show other classes and numbers now.
    if removed > 0 {
        crate::content_events::changed(&state.hub, &user.id, "packs", "remove_packs");
        crate::parties::after_command(
            &state,
            &user.id,
            &workspace,
            crate::parties::CardTarget::All,
        )
        .await;
    }
    Ok(Json(
        json!({ "removed": removed, "skipped": skipped, "packs": packs }),
    ))
}

pub fn routes() -> axum::Router<AppState> {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/api/content/pack-sources", get(pack_sources))
        .route("/api/content/remove-packs", post(remove_packs))
}
