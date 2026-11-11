//! The Borealis backend as the server uses it.
//!
//! A workspace is one [`AppHandle`] holding one [`AppState`] over its own data folder, so the desktop command code runs
//! unchanged, once per account and group. Only the commands listed here can be reached over the network. Everything
//! tied to one machine (native pickers, folders on disk, the AI CLIs, device transfer, the updater) is left out on
//! purpose, and each listed command takes only JSON, base64 or backend-issued tokens, never a path from the client.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use serde_json::{Map, Value};
use tauri::{AppHandle, Invocation, Manager};

use borealis_lib::commands_content::{SearchEvents, TauriSink};
use borealis_lib::content_prefs::ContentPrefs;
use borealis_lib::dto::{AbilityMethodDto, CharacterSnapshot, ExportResult};
use borealis_lib::dto_ext::SheetOptionsDto;
use borealis_lib::error::ApiResult;
use borealis_lib::settings::AppSettings;
use borealis_lib::state::{AppPaths, AppState, ContentHub};
use borealis_lib::{
    commands, commands_added, commands_content, commands_gear, commands_hp, commands_play,
    commands_settings, commands_tracker,
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
    ])(name, invocation)
}

fn character_commands(name: &str, invocation: &Invocation) -> Option<Result<Value, Value>> {
    (tauri::generate_handler![
        take_group_notes,
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
        commands_settings::clear_caches,
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

/// Every file under `dir` with its size and change time, so an updated pack gets a new snapshot.
fn stamp_tree(dir: &Path, key: &mut String) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            stamp_tree(&path, key);
        } else if let Ok(meta) = path.metadata() {
            let _ = writeln!(
                key,
                "{} {} {:?}",
                path.display(),
                meta.len(),
                meta.modified().ok()
            );
        }
    }
}

fn hub_key(paths: &AppPaths, settings: &AppSettings, prefs: &ContentPrefs) -> Vec<u8> {
    let mut key = format!(
        "{:?}|{:?}|{:?}\n",
        settings.engine_settings(),
        prefs.disabled_packs,
        prefs.source_editions()
    );
    // The library is reached through a per-workspace symlink: compare where it really is.
    for dir in AppState::party_packs_at(paths) {
        stamp_tree(&dir.canonicalize().unwrap_or(dir), &mut key);
    }
    key.into_bytes()
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
        Ok(hub) => AppState::party_with_shared_hub(paths, hub),
        Err(reason) => {
            tracing::error!("starting a workspace without content: {reason}");
            AppState::new_degraded(paths, &reason)
        }
    }
    .map_err(|e| e.message)?;
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
    use borealis_lib::content_packs::EventSink;
    let state = app.state::<AppState>();
    let settings = rules_from_file(&state)?;
    let prefs = ContentPrefs::load(&state.paths.prefs_file());
    let hub = shared_hub(&state.paths, &settings, &prefs)?;
    state.party_set_shared_hub(hub).map_err(|e| e.message)?;
    // Only now, so nobody sees the new rules before the engine that applies them.
    state
        .set_settings(rules_from_file(&state)?)
        .map_err(|e| e.message)?;
    if let Err(e) = state.recheck_missing_content() {
        tracing::warn!("re-checking imported characters failed: {e}");
    }
    TauriSink(app.clone()).content_reloaded(reason);
    Ok(())
}

/// The settings in memory with the house rules from the file, which a group member's workspace shares with the
/// group's Dungeon Master.
fn rules_from_file(state: &AppState) -> Result<AppSettings, String> {
    let file = serde_json::to_value(AppSettings::load(&state.paths.settings_file()))
        .map_err(|e| e.to_string())?;
    let mut settings = serde_json::to_value(state.settings().map_err(|e| e.message)?)
        .map_err(|e| e.to_string())?;
    for field in crate::settings::RULE_FIELDS {
        settings[field] = file[field].clone();
    }
    serde_json::from_value(settings).map_err(|e| e.to_string())
}

/// Replace some settings in memory only, leaving the file alone (an account's own settings, see `settings.rs`).
///
/// # Errors
/// When a value does not fit its field.
pub fn overlay_settings(app: &AppHandle, fields: &Map<String, Value>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let current = state.settings().map_err(|e| e.message)?;
    let mut value = serde_json::to_value(&current).map_err(|e| e.to_string())?;
    for (field, v) in fields {
        value[field] = v.clone();
    }
    let next: AppSettings = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if next == current {
        return Ok(());
    }
    let search_changed = next.search != current.search;
    state.set_settings(next).map_err(|e| e.message)?;
    if search_changed {
        state.refresh_semantic_index();
    }
    Ok(())
}

/// The character options that are house rules. In a group they are the Dungeon Master's, so a member's character keeps
/// the defaults (see `Workspace::call`).
pub const HOUSE_RULE_OPTIONS: [&str; 4] = [
    "pointBuyUnrestricted",
    "attunementLimit",
    "ignoreFeatPrerequisites",
    "ignoreMulticlassPrerequisites",
];

fn has_house_rules(character: &borealis_lib::state::Session) -> bool {
    let rules = &character.character.build.house_rules;
    rules.use_default_ability_maximum.is_some()
        || rules.attunement_limit.is_some()
        || rules.point_buy_budget.is_some()
        || rules.ignore_feat_prerequisites
        || rules.ignore_multiclass_prerequisites
        || borealis_lib::extras::ui_extras(&character.character).point_buy_unrestricted
}

fn clear_own_house_rules(state: &AppState, id: &str) -> ApiResult<()> {
    state
        .mutate(id, |_, draft| {
            let character = &mut draft.character;
            let rules = &mut character.build.house_rules;
            rules.use_default_ability_maximum = None;
            rules.attunement_limit = None;
            rules.point_buy_budget = None;
            rules.ignore_feat_prerequisites = false;
            rules.ignore_multiclass_prerequisites = false;
            let mut extras = borealis_lib::extras::ui_extras(character);
            extras.point_buy_unrestricted = false;
            borealis_lib::extras::set_ui_extras(character, &extras);
            Ok(())
        })
        .map(|_| ())
}

fn method_text(method: AbilityMethodDto, budget: i32) -> String {
    match method {
        AbilityMethodDto::PointBuy => format!("point buy with {budget} points"),
        AbilityMethodDto::StandardArray => "the standard array".into(),
        AbilityMethodDto::Roll3d6 => "rolling 3d6".into(),
        AbilityMethodDto::Roll4d6 => "rolling 4d6 and dropping the lowest".into(),
        AbilityMethodDto::Manual => "scores entered by hand".into(),
    }
}

/// True when the group's method, budget or score range doesn't allow a character's ability scores.
fn ability_problem(snapshot: &CharacterSnapshot, settings: &AppSettings) -> bool {
    let abilities = &snapshot.abilities;
    let base = &abilities.base;
    let scores = [base.str, base.dex, base.con, base.int, base.wis, base.cha];
    abilities.method != settings.ability_method
        || abilities
            .point_buy
            .as_ref()
            .is_some_and(|p| p.remaining < 0)
        || scores
            .iter()
            .any(|s| *s < settings.ability_min_base || *s > settings.ability_max_base)
}

/// Bring an open character in a group's workspace in line with the group's rules, which its Dungeon Master sets: its
/// own house rules go, picks only those rules allowed are taken out, and ability scores the group's method, budget or
/// range don't allow are reset. What changed is saved, and kept for the player to read (`take_group_notes`).
/// `None` when nothing had to change.
///
/// # Errors
/// When the character is not open or cannot be saved.
pub fn apply_group_rules(app: &AppHandle, id: &str) -> Result<Option<Value>, String> {
    let state = app.state::<AppState>();
    let message = |e: borealis_lib::error::ApiError| e.message;
    let settings = state.settings().map_err(message)?;
    let mut notes = Vec::new();
    if state
        .read(id, |_, session| Ok(has_house_rules(session)))
        .map_err(message)?
    {
        clear_own_house_rules(&state, id).map_err(message)?;
        notes.push(
            "This character's own house rules are off. In this group your Dungeon Master sets the rules."
                .to_string(),
        );
    }
    // A cleared pick can take others with it, so look again after each one.
    for _ in 0..64 {
        let snapshot = state.get_snapshot(id).map_err(message)?;
        let Some(slot) = snapshot
            .slots
            .iter()
            .find(|s| s.prerequisites_unmet && s.chosen.is_some())
        else {
            break;
        };
        let name = slot.chosen.as_ref().map_or("", |c| c.name.as_str());
        notes.push(format!(
            "{}: {name} doesn't meet its prerequisites, so it was taken out. Choose again.",
            slot.title
        ));
        state.clear_selection(id, &slot.id).map_err(message)?;
    }
    let before = state.get_snapshot(id).map_err(message)?;
    if ability_problem(&before, &settings) {
        state
            .set_ability_method(id, settings.ability_method)
            .map_err(message)?;
        let after = state.reset_abilities(id).map_err(message)?;
        if after.abilities != before.abilities {
            notes.push(format!(
                "Ability scores were reset: this group uses {}. Set them again.",
                method_text(settings.ability_method, settings.point_buy_budget)
            ));
        }
    }
    if notes.is_empty() {
        return Ok(None);
    }
    let snapshot = state.save_character(id).map_err(message)?;
    add_group_notes(&state.paths, id, notes)?;
    serde_json::to_value(snapshot)
        .map(Some)
        .map_err(|e| e.to_string())
}

fn notes_file(paths: &AppPaths, id: &str) -> Option<PathBuf> {
    let safe = !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    safe.then(|| paths.data.join("party-notes").join(format!("{id}.json")))
}

fn read_notes(file: &Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn add_group_notes(paths: &AppPaths, id: &str, notes: Vec<String>) -> Result<(), String> {
    let Some(file) = notes_file(paths, id) else {
        return Ok(());
    };
    let mut all = read_notes(&file);
    for note in notes {
        if !all.contains(&note) {
            all.push(note);
        }
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string(&all).map_err(|e| e.to_string())?;
    std::fs::write(&file, text).map_err(|e| e.to_string())
}

/// What the group's rules changed on a character since its player last looked, once: reading clears it.
#[tauri::command(async)]
fn take_group_notes(
    state: tauri::State<'_, AppState>,
    character_id: String,
) -> ApiResult<Vec<String>> {
    let Some(file) = notes_file(&state.paths, &character_id) else {
        return Ok(Vec::new());
    };
    let notes = read_notes(&file);
    let _ = std::fs::remove_file(&file);
    Ok(notes)
}

/// The ability score method of the workspace's rules (a group's DM's, in a member's workspace).
#[must_use]
pub fn group_ability_method(app: &AppHandle) -> Value {
    app.state::<AppState>()
        .settings()
        .ok()
        .and_then(|s| serde_json::to_value(s.ability_method).ok())
        .unwrap_or(Value::Null)
}

/// The house-rule options a `set_character_options` patch would change on an open character.
///
/// # Errors
/// When the character is not open.
pub fn changed_house_rules(
    app: &AppHandle,
    id: &str,
    options: &Value,
) -> Result<Vec<String>, String> {
    let snapshot = app
        .state::<AppState>()
        .get_snapshot(id)
        .map_err(|e| e.message)?;
    let current = serde_json::to_value(snapshot.options).map_err(|e| e.to_string())?;
    Ok(HOUSE_RULE_OPTIONS
        .iter()
        .filter(|key| {
            options
                .get(**key)
                .is_some_and(|v| !v.is_null() && Some(v) != current.get(**key))
        })
        .map(|key| (*key).to_string())
        .collect())
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
