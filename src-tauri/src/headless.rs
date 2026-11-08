//! Entry points for the multi-user server (`crates/borealis-server`, feature `headless`).
//!
//! A workspace is one [`AppHandle`] holding one [`AppState`] over its own data folder, so the desktop command code runs
//! unchanged, once per account and group. Only the commands listed here can be reached over the network. Everything
//! tied to one machine (native pickers, folders on disk, the AI CLIs, device transfer, the updater) is left out on
//! purpose, and each listed command takes only JSON, base64 or backend-issued tokens, never a path from the client.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use serde_json::{Map, Value};
use tauri::{AppHandle, Invocation, Manager};

use crate::commands_content::{SearchEvents, TauriSink};
use crate::content_cache::CacheKey;
use crate::content_prefs::ContentPrefs;
use crate::settings::AppSettings;
use crate::state::{AppPaths, AppState, ContentHub};
use crate::{
    commands, commands_added, commands_content, commands_gear, commands_hp, commands_play,
    commands_tracker,
};

/// Who may run a command (the server combines this with the caller's role and scope).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Reads content or a character, or changes only the caller's own view of the workspace.
    Read,
    /// Changes a character: players only, on their own.
    Character,
    /// Changes the content library: Dungeon Masters only.
    Library,
}

type Handler = fn(&str, &Invocation) -> Option<Result<Value, Value>>;

fn read_commands(name: &str, invocation: &Invocation) -> Option<Result<Value, Value>> {
    (tauri::generate_handler![
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
        commands_gear::export_sheet,
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
        crate::commands_settings::content_update_status,
        commands_play::preview_convert_to_2024,
    ])(name, invocation)
}

fn character_commands(name: &str, invocation: &Invocation) -> Option<Result<Value, Value>> {
    (tauri::generate_handler![
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
    ])(name, invocation)
}

fn library_commands(name: &str, invocation: &Invocation) -> Option<Result<Value, Value>> {
    (tauri::generate_handler![
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
        commands_content::scan_5etools_folder,
        commands_content::stage_5etools_zip,
        commands_content::stage_5etools_url,
        commands_content::remove_5etools_url,
        commands_content::update_5etools_url,
        commands_content::import_5etools,
    ])(name, invocation)
}

const TABLES: [(Access, Handler); 3] = [
    (Access::Read, read_commands),
    (Access::Character, character_commands),
    (Access::Library, library_commands),
];

/// Content snapshots in use. Members of a group, and everyone with only the bundled packs, have identical content, so
/// they share one engine however many workspaces are open.
static HUBS: LazyLock<Mutex<HashMap<Vec<u8>, Weak<ContentHub>>>> = LazyLock::new(Mutex::default);

fn hub_key(paths: &AppPaths, settings: &AppSettings, prefs: &ContentPrefs) -> Vec<u8> {
    // The library is reached through a per-workspace symlink: compare where it really is.
    let packs: Vec<PathBuf> = crate::state::user_pack_dirs(&paths.library())
        .into_iter()
        .map(|dir| dir.canonicalize().unwrap_or(dir))
        .collect();
    let editions = prefs.source_editions();
    let mut key = CacheKey::new(&[], &packs, &prefs.disabled_packs, &editions)
        .as_bytes()
        .to_vec();
    key.extend_from_slice(format!("{:?}", settings.engine_settings()).as_bytes());
    key
}

fn shared_hub(
    paths: &AppPaths,
    settings: &AppSettings,
    prefs: &ContentPrefs,
) -> Result<Arc<ContentHub>, String> {
    let key = hub_key(paths, settings, prefs);
    // Held across the load, so a burst of opens builds one snapshot rather than many.
    let mut hubs = HUBS
        .lock()
        .map_err(|_| "content lock poisoned".to_string())?;
    hubs.retain(|_, hub| hub.strong_count() > 0);
    if let Some(hub) = hubs.get(&key).and_then(Weak::upgrade) {
        return Ok(hub);
    }
    let hub = Arc::new(ContentHub::load_with(paths, settings, prefs).map_err(|e| e.message)?);
    hubs.insert(key, Arc::downgrade(&hub));
    Ok(hub)
}

/// Open a workspace over `data`, using the bundled packs in `content_root`.
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
    for dir in [paths.characters(), paths.library()] {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    let settings = AppSettings::load(&paths.settings_file());
    let prefs = ContentPrefs::load(&paths.prefs_file());
    let state = match shared_hub(&paths, &settings, &prefs) {
        Ok(hub) => AppState::from_shared_hub(paths, settings, hub),
        Err(reason) => {
            tracing::error!("starting a workspace without content: {reason}");
            AppState::new_degraded(paths, &reason).map_err(|e| e.message)?
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
    Ok(handle.clone())
}

/// What a command requires, or `None` for anything not exposed to the network.
#[must_use]
pub fn access_of(app: &AppHandle, command: &str) -> Option<Access> {
    let probe = Invocation {
        app: app.clone(),
        args: Map::new(),
        probe: true,
    };
    TABLES
        .iter()
        .find(|(_, table)| table(command, &probe).is_some())
        .map(|(access, _)| *access)
}

/// Run one command in a workspace. Call it from a blocking thread.
#[must_use]
pub fn invoke(
    app: &AppHandle,
    command: &str,
    args: Map<String, Value>,
) -> Option<Result<Value, Value>> {
    let invocation = Invocation {
        app: app.clone(),
        args,
        probe: false,
    };
    TABLES
        .iter()
        .find_map(|(_, table)| table(command, &invocation))
}

/// Pick up a change to the library or the enabled packs that was made from another workspace.
///
/// Unlike `AppState::reload_content` this never writes the preferences file, which a group member's workspace shares
/// with the group's Dungeon Master.
///
/// # Errors
/// When the content cannot be loaded; the workspace keeps what it had.
pub fn reload_shared_content(app: &AppHandle, reason: &str) -> Result<(), String> {
    use crate::content_packs::EventSink;
    let state = app.state::<AppState>();
    let settings = state.settings().map_err(|e| e.message)?;
    let prefs = ContentPrefs::load(&state.paths.prefs_file());
    let hub = shared_hub(&state.paths, &settings, &prefs)?;
    state.set_shared_hub(hub).map_err(|e| e.message)?;
    if let Err(e) = state.recheck_missing_content() {
        tracing::warn!("re-checking imported characters failed: {e}");
    }
    TauriSink(app.clone()).content_reloaded(reason);
    Ok(())
}

/// True when the workspace holds unsaved edits and must stay open.
#[must_use]
pub fn has_unsaved_changes(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .sessions()
        .map_or(true, |sessions| sessions.values().any(|s| s.dirty))
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
