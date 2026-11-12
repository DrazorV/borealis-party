//! The Borealis backend as the server uses it.
//!
//! A workspace is one [`AppHandle`] holding one [`AppState`] over one account's data folder, so the desktop command
//! code runs unchanged, once per account. Only the commands listed here can be reached over the network. Everything
//! tied to one machine (native pickers, folders on disk, the AI CLIs, device transfer, the updater) is left out on
//! purpose, and each listed command takes only JSON, base64 or backend-issued tokens, never a path from the client.
//!
//! `.borealis` bundles are files, not device transfer: [`stage_bundle`] writes an uploaded bundle into the account's
//! own `sync/incoming` folder and hands back a pick token, and upstream's `bundle_open`, `bundle_accept` and
//! `bundle_discard` preview and apply it there. Exporting bundles, the device link (`link_*`, paired devices) and the
//! OS inbox (`take_opened_*`, one queue for the whole process) stay out.

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{AppHandle, Invocation, Manager};

use borealis_lib::commands_content::{SearchEvents, TauriSink};
use borealis_lib::commands_sync::TauriSyncEvents;
use borealis_lib::device_sync::inbox::is_bundle_name;
use borealis_lib::device_sync::{incoming_dir, DeviceSync, SyncConfig};
use borealis_lib::dto::ExportResult;
use borealis_lib::dto_ext::SheetOptionsDto;
use borealis_lib::error::{ApiError, ApiResult};
use borealis_lib::picked_files::BUNDLE_EXTENSION;
use borealis_lib::picked_paths::PickPurpose;
use borealis_lib::state::{AppPaths, AppState};
use borealis_lib::{
    commands, commands_added, commands_content, commands_gear, commands_hp, commands_play,
    commands_settings, commands_sync, commands_tracker,
};

/// Upstream's version opens a native save dialog on desktop; the server sends the bytes to the client instead.
#[tauri::command(async)]
fn export_sheet(
    state: tauri::State<'_, AppState>,
    character_id: String,
    options: Option<SheetOptionsDto>,
) -> ApiResult<ExportResult> {
    Ok(state
        .sheet_file(&character_id, &options.unwrap_or_default())?
        .into_base64())
}

/// Upstream's prefix of the per-open folders under `sync/incoming`; it is crate-private there.
const OPEN_DIR_PREFIX: &str = "open-";
/// Uploaded bundles kept at once; staging one more removes the oldest (a later Accept of it fails, writing nothing).
const MAX_STAGED_BUNDLES: usize = 8;

/// The answer to [`stage_bundle`]: never the path on the server.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StagedBundle {
    token: String,
    name: String,
}

/// Longest kept name before `.borealis`: upstream's 120 characters, and at most 200 bytes (ext4 allows 255).
const MAX_NAME_CHARS: usize = 120;
const MAX_NAME_BYTES: usize = 200;

/// One file name component ending in `.borealis`, whatever the client sent. The same rules as upstream's private
/// `device_sync::inbox::copy_name`, so a file reads the same here as on desktop or Android, plus a byte cap.
pub(crate) fn bundle_file_name(sent: &str) -> String {
    let last = sent.rsplit(['/', '\\']).next().unwrap_or_default();
    let stem = if is_bundle_name(last) {
        &last[..last.len() - BUNDLE_EXTENSION.len() - 1]
    } else {
        last
    };
    let mut bytes = 0;
    let clean: String = stem
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .take(MAX_NAME_CHARS)
        .take_while(|c| {
            bytes += c.len_utf8();
            bytes <= MAX_NAME_BYTES
        })
        .collect();
    let clean = clean.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let stem = if clean.is_empty() { "bundle" } else { clean };
    format!("{stem}.{BUNDLE_EXTENSION}")
}

/// The `open-*` folders under `incoming`, oldest first.
fn staged_folders(incoming: &Path) -> Vec<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(incoming)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(OPEN_DIR_PREFIX))
        .filter_map(|e| {
            let meta = e.path().symlink_metadata().ok().filter(|m| m.is_dir())?;
            Some((meta.modified().ok()?, e.path()))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, path)| path).collect()
}

/// Keep an uploaded `.borealis` file in the account's own `sync/incoming` folder and hand back a `bundle-file` token
/// for upstream's `bundle_open`. The browser has the bytes but no path, so this stands in for `pick_path`.
#[tauri::command(async)]
fn stage_bundle(
    state: tauri::State<'_, AppState>,
    file_name: String,
    content_base64: String,
) -> ApiResult<StagedBundle> {
    use base64::Engine as _;
    if !state.characters_dir().starts_with(&state.paths.data) {
        return Err(ApiError::invalid(
            "Bundles cannot be opened while characters are kept outside this account's folder.",
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(content_base64.trim())
        .map_err(|_| ApiError::invalid("The file content is not valid base64."))?;
    let incoming = incoming_dir(&state);
    let old = staged_folders(&incoming);
    for dir in old
        .iter()
        .take((old.len() + 1).saturating_sub(MAX_STAGED_BUNDLES))
    {
        let _ = std::fs::remove_dir_all(dir);
    }
    let name = bundle_file_name(&file_name);
    let dir = incoming.join(format!(
        "{OPEN_DIR_PREFIX}{}",
        uuid::Uuid::new_v4().simple()
    ));
    let path = dir.join(&name);
    let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &bytes));
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(ApiError::io("keep the bundle", e));
    }
    let picked = state.picked.grant(path, PickPurpose::BundleFile)?;
    Ok(StagedBundle {
        token: picked.token,
        name,
    })
}

/// Every command the server offers. An account runs them in its own data folder only.
fn offered(name: &str, invocation: &Invocation) -> Option<Result<Value, Value>> {
    (tauri::generate_handler![
        // Reading.
        commands::app_version,
        commands::get_settings,
        commands::save_settings,
        commands::list_characters,
        commands::load_character,
        commands::get_snapshot,
        commands::close_character,
        commands::export_character,
        commands::get_slot_options,
        commands::get_ruleset_profile,
        commands::list_character_options,
        commands::class_counts_by_edition,
        commands_gear::explain_stat,
        commands_gear::search_items,
        commands_gear::list_item_categories,
        commands_gear::list_base_items,
        commands_gear::render_sheet,
        export_sheet,
        commands_gear::search_compendium,
        commands_gear::get_element,
        commands_gear::compendium_facets,
        commands_gear::search,
        commands_gear::search_index_status,
        commands_gear::get_search_passage,
        commands_added::list_added_pick_options,
        commands_content::preview_source_restrictions,
        commands_content::get_compatibility_report,
        commands_content::list_packs,
        commands_content::list_sources,
        commands_content::list_indexes,
        commands_content::list_5etools_urls,
        commands_settings::content_update_status,
        commands_play::preview_convert_to_2024,
        // Characters.
        commands::create_character,
        commands::save_character,
        commands::save_character_as,
        commands::delete_character,
        commands::duplicate_character,
        commands::update_character_meta,
        commands::set_portrait,
        commands::import_character,
        commands::recheck_missing_content,
        commands::apply_selection,
        commands::clear_selection,
        commands::clear_missing_content,
        commands::switch_version,
        commands::set_xp,
        commands::level_up,
        commands::level_down,
        commands::add_multiclass,
        commands::split_multiclass,
        commands::set_class_levels,
        commands::set_hp_roll,
        commands::set_ability_method,
        commands::set_base_score,
        commands::roll_abilities,
        commands::assign_standard_array,
        commands::reset_abilities,
        commands::set_character_options,
        commands::start_over_in_edition,
        commands_gear::replace_spell,
        commands_gear::set_spell_prepared,
        commands_gear::add_item,
        commands_gear::remove_item,
        commands_gear::equip_item,
        commands_gear::unequip_item,
        commands_gear::set_item_requirements_override,
        commands_added::add_pick,
        commands_added::remove_pick,
        commands_gear::add_to_attacks,
        commands_gear::set_attuned,
        commands_gear::set_rolled_max,
        commands_gear::reroll_max,
        commands_gear::store_item,
        commands_gear::move_item,
        commands_gear::update_item_details,
        commands_gear::extract_pack,
        commands_gear::adjust_coins,
        commands_gear::convert_coins,
        commands_gear::set_storage_names,
        commands_content::update_details,
        commands_content::upsert_attack,
        commands_content::remove_attack,
        commands_content::move_attack,
        commands_content::update_companion,
        commands_content::add_companion,
        commands_content::remove_companion,
        commands_content::set_source_restrictions,
        commands_content::set_allow_cross_edition,
        commands_play::apply_convert_to_2024,
        commands_play::spend_resource,
        commands_play::restore_resource,
        commands_play::short_rest,
        commands_play::long_rest,
        commands_play::set_toggle,
        commands_play::set_exhaustion,
        commands_play::set_heroic_inspiration,
        commands_play::choose_weapon_masteries,
        commands_play::set_wild_shape_forms,
        commands_play::choose_equipment_package,
        commands_tracker::spend_spell_slot,
        commands_tracker::restore_spell_slot,
        commands_tracker::set_concentration,
        commands_tracker::add_tracker,
        commands_tracker::update_tracker,
        commands_tracker::remove_tracker,
        commands_tracker::spend_tracker,
        commands_tracker::restore_tracker,
        commands_hp::take_damage,
        commands_hp::regain_hit_points,
        commands_hp::set_temp_hit_points,
        commands_hp::spend_hit_die,
        commands_hp::restore_hit_die,
        // Content.
        commands_content::import_pack,
        commands_content::preview_import_pack,
        commands_content::install_import_preview,
        commands_content::cancel_import_preview,
        commands_content::set_pack_enabled,
        commands_content::remove_pack,
        commands_content::set_source_edition,
        commands_content::add_index,
        commands_content::remove_index,
        commands_content::update_indexes,
        commands_content::clear_downloaded_content,
        commands_settings::clear_caches,
        commands_content::scan_5etools_folder,
        commands_content::stage_5etools_zip,
        commands_content::stage_5etools_files,
        commands_content::stage_5etools_url,
        commands_content::remove_5etools_url,
        commands_content::update_5etools_url,
        commands_content::import_5etools,
        // Bundle files, previewed and applied in the account's own folder.
        stage_bundle,
        commands_sync::bundle_open,
        commands_sync::bundle_accept,
        commands_sync::bundle_discard,
    ])(name, invocation)
}

/// Open a workspace over the data folder `data`, using the bundled packs in `content_root`.
///
/// # Errors
/// When the data folder cannot be set up.
pub fn open_workspace(
    data: PathBuf,
    content_root: PathBuf,
    events: impl Fn(&str, Value) + Send + Sync + 'static,
) -> Result<AppHandle, String> {
    let handle = AppHandle::new(data.clone(), content_root.clone(), events);
    let paths = AppPaths { content_root, data };
    let state = match AppState::new(paths.clone()) {
        Ok(state) => state,
        Err(e) => {
            tracing::error!("starting a workspace without content: {}", e.message);
            AppState::new_degraded(paths, &e.message).map_err(|e| e.message)?
        }
    };
    handle.manage::<AppState>(state);
    let state = handle.state::<AppState>();
    state
        .search
        .set_sink(Arc::new(SearchEvents(handle.clone())));
    if let Err(e) = state.start_search_index() {
        tracing::warn!("search: indexing not started: {}", e.message);
    }
    // Uploads from before this workspace opened have no token any more.
    for dir in staged_folders(&incoming_dir(&state)) {
        let _ = std::fs::remove_dir_all(dir);
    }
    // Not `commands_sync::install`, which turns discovery on and installs the system keychain: here the link can
    // only ever listen on localhost and has no keychain, should a `link_*` command ever be offered by mistake.
    let sync = DeviceSync::new(
        Arc::new(handle.clone()),
        Arc::new(TauriSyncEvents(TauriSink(handle.clone()))),
        SyncConfig {
            bind_ip: Some(Ipv4Addr::LOCALHOST.into()),
            device_name: "Borealis Party server".into(),
            discovery: false,
        },
        || Err("Device transfer is not available on the server.".to_string()),
    );
    handle.manage(Arc::new(sync));
    Ok(handle.clone())
}

/// Run one command in a workspace; `None` for a command the server does not offer. Call it from a blocking thread.
#[must_use]
pub fn invoke(
    app: &AppHandle,
    command: &str,
    args: Map<String, Value>,
) -> Option<Result<Value, Value>> {
    let invocation = Invocation {
        app: app.clone(),
        args,
    };
    offered(command, &invocation)
}

/// True when the workspace holds unsaved edits and must stay open.
#[must_use]
pub fn has_unsaved_changes(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .sessions()
        .map_or(true, |sessions| sessions.values().any(|s| s.dirty))
}

/// True when the character is open with unsaved edits.
#[must_use]
pub fn is_dirty(app: &AppHandle, character: &str) -> bool {
    app.state::<AppState>()
        .sessions()
        .is_ok_and(|sessions| sessions.get(character).is_some_and(|s| s.dirty))
}

/// Write every unsaved character to its file.
///
/// # Errors
/// The first character that could not be saved.
pub fn save_all(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut sessions = state.sessions().map_err(|e| e.message)?;
    for session in sessions.values_mut().filter(|s| s.dirty) {
        session
            .character
            .save(&session.path)
            .map_err(|e| e.to_string())?;
        session.dirty = false;
    }
    Ok(())
}
