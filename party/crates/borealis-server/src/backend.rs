//! The Borealis backend as the server uses it.
//!
//! A workspace is one [`AppHandle`] holding one [`AppState`] over one account's data folder, so the desktop command
//! code runs unchanged, once per account. Only the commands listed here can be reached over the network. Everything
//! tied to one machine (native pickers, folders on disk, the AI CLIs, device transfer, the updater) is left out on
//! purpose, and each listed command takes only JSON, base64 or backend-issued tokens, never a path from the client.
//!
//! `.borealis` bundles are files, not device transfer: [`stage_bundle`] writes an uploaded bundle into the account's
//! own `sync/incoming` folder and hands back a pick token, and upstream's `bundle_open`, `bundle_accept` and
//! `bundle_discard` preview and apply it there ([`bundle_accept`] also follows the indexes the bundle's packs came
//! from). [`bundle_export`] builds one with upstream's builder and sends its
//! bytes back. Sharing bundles from the server's machine (`bundle_share`), the device link (`link_*`, paired devices)
//! and the OS inbox (`take_opened_*`, one queue for the whole process) stay out.
//!
//! A bundle from another player is received safely ([`bundle_open`], [`bundle_accept`]): a pack or character this
//! account already has with different content is refused, never replaced or renamed, and the rest comes in.
//!
//! Every address the server downloads from passes [`crate::net_guard`] first: [`add_index`] and the indexes
//! [`bundle_accept`] follows are checked before they are saved, and [`update_indexes`], [`stage_5etools_url`] and
//! [`update_5etools_url`] check each file they fetch, so no account can make the server reach its own network.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use borealis_bundle::{
    scan_characters, scan_packs, Bundle, BundleMeta, BundleWriter, Item, ItemKind, Limits, Manifest,
};
use borealis_lib::sync_dto::{BundlePreview, LinkSummary};

use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{AppHandle, Invocation, Manager};

use borealis_lib::commands_content::{SearchEvents, TauriSink};
use borealis_lib::commands_sync::TauriSyncEvents;
use borealis_lib::content_dto::{
    ContentIndex, FiveToolsImportReport, FiveToolsImportRequest, UpdateIndexesResult,
};
use borealis_lib::content_fivetools_url::FiveToolsUrlUpdate;
use borealis_lib::content_index::HttpsFetcher;
use borealis_lib::device_sync::inbox::is_bundle_name;
use borealis_lib::device_sync::{incoming_dir, DeviceSync, SyncConfig};
use borealis_lib::dto::ExportResult;
use borealis_lib::dto_ext::SheetOptionsDto;
use borealis_lib::error::{ApiError, ApiResult};
use borealis_lib::picked_files::BUNDLE_EXTENSION;
use borealis_lib::picked_paths::PickPurpose;
use borealis_lib::picked_paths::PickedPathDto;
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

/// The answer to [`bundle_export`]: upstream's `BundleExport` without a path, plus the file's bytes.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BundleFile {
    file_name: String,
    characters: usize,
    packs: usize,
    bytes: u64,
    data_base64: String,
}

/// File > Export to bundle...: upstream's own builder, as desktop Borealis uses it. Upstream's version opens a native
/// save dialog; the server sends the bytes to the client instead, which saves them as a download. With no characters
/// and every pack, the file is the account's whole library (imported homebrew and packs from indexes alike).
#[tauri::command(async)]
fn bundle_export(
    sync: tauri::State<'_, Arc<DeviceSync>>,
    selection: borealis_lib::sync_dto::LinkSelection,
) -> ApiResult<BundleFile> {
    use base64::Engine as _;
    let built = sync.build_bundle_file(&selection)?;
    let info = borealis_lib::device_sync::export_dto(&built, None);
    let bytes =
        std::fs::read(built.file.path()).map_err(|e| ApiError::io("prepare the bundle", e))?;
    let file_name = if info.characters == 0 {
        format!("Borealis content.{BUNDLE_EXTENSION}")
    } else {
        info.file_name
    };
    Ok(BundleFile {
        file_name,
        characters: info.characters,
        packs: info.packs,
        bytes: info.bytes,
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Each pack in the library with the index it was downloaded from: its id, that index's address and its whole
/// `pack-meta.json`, which changes whenever the pack is installed again.
fn index_packs(state: &AppState) -> std::collections::BTreeMap<String, (String, String)> {
    use borealis_lib::content_packs::PACK_META_FILE;
    std::fs::read_dir(state.paths.library())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let text = std::fs::read_to_string(entry.path().join(PACK_META_FILE)).ok()?;
            let meta: Value = serde_json::from_str(&text).ok()?;
            let url = meta["indexUrl"].as_str()?.to_string();
            Some((
                entry.file_name().to_string_lossy().into_owned(),
                (url, text),
            ))
        })
        .collect()
}

/// Accept, as upstream does, then keep the packs that came from indexes up to date: a pack this bundle added or
/// replaced records the index it was downloaded from (`indexUrl` in its `pack-meta.json`, which travels in the
/// bundle), and this account now follows that index too, as the one who exported it does. Packs the bundle left as
/// they were (identical, or newer here) add nothing, so an index this account removed stays removed unless the
/// bundle brings its content back. An index that can't be added (not HTTPS) leaves the pack as a snapshot.
///
/// A bundle from another player (made by another account, or `from_another_player` when the user says so) goes
/// through [`accept_foreign`] instead, so nothing this account already has is replaced.
#[tauri::command(async)]
fn bundle_accept(
    state: tauri::State<'_, AppState>,
    sync: tauri::State<'_, Arc<DeviceSync>>,
    received: tauri::State<'_, Arc<Received>>,
    bundle_id: String,
    from_another_player: Option<bool>,
) -> ApiResult<LinkSummary> {
    let opened = lock(&received.open).remove(&bundle_id);
    let foreign = from_another_player == Some(true) || opened.as_ref().is_some_and(|o| o.foreign);
    let before = index_packs(&state);
    let summary = match (foreign, opened) {
        (false, _) => sync.accept_bundle(&bundle_id)?,
        (true, Some(opened)) => accept_foreign(&state, &sync, &bundle_id, &opened.path)?,
        (true, None) => {
            sync.discard_bundle(&bundle_id);
            return Err(ApiError::invalid(
                "This bundle could not be checked against what you already have, so nothing was changed. Open it \
                 again.",
            ));
        }
    };
    let urls: std::collections::BTreeSet<String> = index_packs(&state)
        .into_iter()
        .filter(|(id, now)| before.get(id) != Some(now))
        .map(|(_, (url, _))| url)
        .collect();
    for url in urls {
        // The address comes from someone else's file: it is checked as if this account had typed it.
        if let Err(e) = guarded_add_index(&state, &url) {
            tracing::warn!("bundle: index {url} not added: {}", e.message);
        }
    }
    Ok(summary)
}

/// Upstream's `add_index` once [`crate::net_guard::check_url`] allows the address.
fn guarded_add_index(state: &AppState, url: &str) -> ApiResult<Vec<ContentIndex>> {
    crate::net_guard::check_url(&borealis_lib::content_index::raw_file_url(url))
        .map_err(ApiError::invalid)?;
    state.add_index(url)
}

/// Remember an index address: upstream's, for addresses on the internet only.
#[tauri::command(async)]
fn add_index(state: tauri::State<'_, AppState>, url: String) -> ApiResult<Vec<ContentIndex>> {
    guarded_add_index(&state, &url)
}

/// Run `f` on a blocking thread, as upstream's private `commands_content::blocking` does.
async fn blocking<T, F>(app: AppHandle, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&AppState, &TauriSink) -> ApiResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || {
        let sink = TauriSink(app.clone());
        let state = app.state::<AppState>();
        f(&state, &sink)
    })
    .await
    .map_err(ApiError::internal)?
}

/// Upstream's, with every index and file address checked before it is fetched, the ones saved before this check
/// existed and the files an index lists included. A refused one is reported like any failed download.
#[tauri::command]
async fn update_indexes(app: AppHandle) -> ApiResult<UpdateIndexesResult> {
    blocking(app, |state, sink| {
        state.update_indexes(&crate::net_guard::Guarded::new(HttpsFetcher), sink)
    })
    .await
}

/// Upstream's, with the address and every file under it checked before it is fetched.
#[tauri::command]
async fn stage_5etools_url(app: AppHandle, url: String) -> ApiResult<PickedPathDto> {
    crate::net_guard::check_url(&url).map_err(ApiError::invalid)?;
    blocking(app, move |state, sink| {
        state.stage_5etools_url(&crate::net_guard::Guarded::new(HttpsFetcher), &url, sink)
    })
    .await
}

/// Upstream's, with every file checked before it is fetched. When the address changed and its books were imported
/// again, the creature details the import drops are read from the new download ([`crate::bestiary::capture`]).
#[tauri::command]
async fn update_5etools_url(app: AppHandle, url: String) -> ApiResult<FiveToolsUrlUpdate> {
    blocking(app, move |state, sink| {
        let update =
            state.update_5etools_url(&crate::net_guard::Guarded::new(HttpsFetcher), &url, sink)?;
        if let (Some(report), Some(dir)) = (
            &update.report,
            crate::bestiary::staged_download(&state.paths.data),
        ) {
            crate::bestiary::capture_logged(state, &dir, report);
        }
        Ok(update)
    })
    .await
}

/// Upstream's; then the creature details the import drops are read from the same 5etools folder
/// ([`crate::bestiary::capture`]). A failed capture is logged and never fails the import.
#[tauri::command]
async fn import_5etools(
    app: AppHandle,
    request: FiveToolsImportRequest,
) -> ApiResult<FiveToolsImportReport> {
    blocking(app, move |state, sink| {
        let folder = state
            .picked
            .resolve(&request.folder, PickPurpose::FivetoolsFolder)
            .ok();
        let report = state.import_5etools(&request, sink)?;
        if let Some(dir) = folder {
            crate::bestiary::capture_logged(state, &dir, &report);
        }
        Ok(report)
    })
    .await
}

/// The start of the source device in every bundle a server account exports. The account's folder name (its id)
/// follows, so a bundle tells whether this account or another one made it.
const PARTY_SOURCE: &str = "Borealis Party server";

/// The source device of the bundles the account in `data` exports.
fn party_source(data: &Path) -> String {
    data.file_name().map_or_else(
        || PARTY_SOURCE.to_string(),
        |id| format!("{PARTY_SOURCE} · {}", id.to_string_lossy()),
    )
}

/// What the preview names as the source of a bundle that another account made.
const FROM_ANOTHER_ACCOUNT: &str = "another Borealis Party account";
/// What the preview names as the source of a bundle that this account made.
const FROM_THIS_ACCOUNT: &str = "this account";

/// The bundles of one account that [`stage_bundle`] kept, so [`bundle_accept`] can read them again.
pub(crate) struct Received {
    /// This account's own source device ([`party_source`]).
    own: String,
    /// Staged files by pick token, until [`bundle_open`] takes the token.
    staged: Mutex<HashMap<String, PathBuf>>,
    /// Open previews by bundle id, until accepted or discarded.
    open: Mutex<HashMap<String, Opened>>,
}

struct Opened {
    path: PathBuf,
    /// Made by another account (or an older server that named no account).
    foreign: bool,
}

impl Received {
    fn new(own: String) -> Self {
        Self {
            own,
            staged: Mutex::default(),
            open: Mutex::default(),
        }
    }

    /// Another account made the bundle: any party source but this account's own. A bundle from before sources named
    /// the account counts as another's, since it may be.
    fn is_foreign(&self, source: &str) -> bool {
        source != self.own && source.starts_with(PARTY_SOURCE)
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the preview row of a refused item says.
fn refusal(kind: &str, label: &str) -> String {
    if kind == "pack" {
        format!("You already have a pack called {label}; it was not replaced. Rename one before sharing.")
    } else {
        format!(
            "You already have a different copy of {label}; it was not replaced. To take this one, delete yours first."
        )
    }
}

/// Upstream's preview, with every pack or character of another player's bundle that differs from this account's
/// copy shown as refused: its row says why and that this account's copy is kept.
#[tauri::command(async)]
fn bundle_open(
    sync: tauri::State<'_, Arc<DeviceSync>>,
    received: tauri::State<'_, Arc<Received>>,
    token: String,
) -> ApiResult<BundlePreview> {
    let path = lock(&received.staged).remove(&token);
    let mut preview = sync.open_bundle(&token)?;
    let Some(path) = path else {
        return Ok(preview);
    };
    let foreign = received.is_foreign(&preview.source_device);
    if foreign {
        preview.source_device = FROM_ANOTHER_ACCOUNT.to_string();
        for item in &mut preview.preview.items {
            if matches!(item.action.as_str(), "replace" | "keep_local") {
                item.label = refusal(&item.kind, &item.label);
                item.action = "keep_local".to_string();
            }
        }
    } else if preview.source_device == received.own {
        preview.source_device = FROM_THIS_ACCOUNT.to_string();
    }
    let mut open = lock(&received.open);
    open.retain(|_, o| o.path.exists());
    open.insert(preview.bundle_id.clone(), Opened { path, foreign });
    Ok(preview)
}

/// Accept another player's bundle without replacing anything: every pack or character this account already has with
/// different content is left out. When nothing is left out the upload is accepted as it is; otherwise the rest is
/// written as a bundle of its own next to the upload and goes through upstream's own open and accept. Those left out
/// count as kept.
fn accept_foreign(
    state: &AppState,
    sync: &DeviceSync,
    bundle_id: &str,
    path: &Path,
) -> ApiResult<LinkSummary> {
    let kept = path.with_file_name(format!(
        "kept-{}",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let written = without_clashes(state, path, &kept);
    if matches!(written, Ok(0)) {
        let _ = std::fs::remove_file(&kept);
        return sync.accept_bundle(bundle_id);
    }
    // The upload is no longer needed: only what was written from it, if anything, goes on.
    sync.discard_bundle(bundle_id);
    let refused = match written {
        Ok(n) if kept.is_file() => n,
        Ok(n) => {
            let _ = path.parent().map(std::fs::remove_dir);
            return Ok(LinkSummary {
                added: 0,
                replaced: 0,
                kept_local: n,
                identical: 0,
            });
        }
        Err(e) => {
            let _ = std::fs::remove_file(&kept);
            let _ = path.parent().map(std::fs::remove_dir);
            return Err(e);
        }
    };
    let token = state.picked.grant(kept, PickPurpose::BundleFile)?.token;
    let preview = sync.open_bundle(&token)?;
    let mut summary = sync.accept_bundle(&preview.bundle_id)?;
    summary.kept_local += refused;
    Ok(summary)
}

/// How many items of the bundle at `path` this account already has with different content. When there are some and
/// not every item is one, the others are written to `out`: each read and verified, then written with its own files,
/// edit stamp and required packs.
fn without_clashes(state: &AppState, path: &Path, out: &Path) -> ApiResult<usize> {
    let unusable = |e: borealis_bundle::BundleError| {
        ApiError::invalid(format!(
            "This bundle cannot be used: {e}. Nothing was changed."
        ))
    };
    let file = std::fs::File::open(path).map_err(|e| ApiError::io("read the bundle", e))?;
    let mut bundle = Bundle::open_with(file, Limits::default()).map_err(unusable)?;
    let manifest = bundle.manifest().clone();
    let mut local = scan_characters(&state.characters_dir()).map_err(unusable)?;
    local.extend(scan_packs(&state.paths.library(), &Limits::default()).map_err(unusable)?);
    let held: HashMap<(ItemKind, &str), &str> = local
        .iter()
        .map(|l| ((l.kind, l.key.as_str()), l.sha256.as_str()))
        .collect();
    let (clash, keep): (Vec<&Item>, Vec<&Item>) = manifest.items.iter().partition(|item| {
        held.get(&(item.kind, item.key.as_str()))
            .is_some_and(|sha| *sha != item.sha256)
    });
    for item in &clash {
        tracing::info!(
            "bundle: {:?} {} kept; another player's copy differs",
            item.kind,
            item.key
        );
    }
    if !clash.is_empty() && !keep.is_empty() {
        write_items(&mut bundle, &manifest, &keep, out).map_err(unusable)?;
    }
    Ok(clash.len())
}

/// A new bundle at `out` with the `items` of `bundle`, under the same manifest details.
fn write_items(
    bundle: &mut Bundle<std::fs::File>,
    manifest: &Manifest,
    items: &[&Item],
    out: &Path,
) -> borealis_bundle::Result<()> {
    let meta = BundleMeta {
        app_version: manifest.app_version.clone(),
        source_device: manifest.source_device.clone(),
        created_ms: manifest.created_ms,
    };
    let mut writer = BundleWriter::with_limits(std::fs::File::create(out)?, meta, bundle.limits());
    for item in items {
        let data = bundle.read_item(item.kind, &item.key)?;
        match item.kind {
            ItemKind::Character => {
                let Some((_, bytes)) = data.files.first() else {
                    return Err(borealis_bundle::BundleError::Inconsistent(format!(
                        "character `{}` has no file",
                        item.key
                    )));
                };
                writer.add_character(
                    &item.key,
                    bytes,
                    item.edited_at_ms,
                    item.required_packs.clone(),
                )?;
            }
            ItemKind::Pack => {
                let prefix = format!("packs/{}/", item.key);
                let files = data
                    .files
                    .into_iter()
                    .map(|(name, bytes)| match name.strip_prefix(&prefix) {
                        Some(rel) => Ok((rel.to_string(), bytes)),
                        None => Err(borealis_bundle::BundleError::Inconsistent(format!(
                            "`{name}` is outside pack `{}`",
                            item.key
                        ))),
                    })
                    .collect::<borealis_bundle::Result<Vec<_>>>()?;
                writer.add_pack(&item.key, &files, item.edited_at_ms)?;
            }
        }
    }
    if let Some(settings) = bundle.read_settings()? {
        writer.set_portable_settings(&settings)?;
    }
    writer.finish()?;
    Ok(())
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
    received: tauri::State<'_, Arc<Received>>,
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
    let picked = state.picked.grant(path.clone(), PickPurpose::BundleFile)?;
    let mut staged = lock(&received.staged);
    staged.retain(|_, p| p.exists());
    staged.insert(picked.token.clone(), path);
    drop(staged);
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
        add_index,
        commands_content::remove_index,
        update_indexes,
        commands_content::clear_downloaded_content,
        commands_settings::clear_caches,
        commands_content::scan_5etools_folder,
        commands_content::stage_5etools_zip,
        commands_content::stage_5etools_files,
        stage_5etools_url,
        commands_content::remove_5etools_url,
        update_5etools_url,
        import_5etools,
        // Bundle files, made from and previewed and applied in the account's own folder.
        bundle_export,
        stage_bundle,
        bundle_open,
        bundle_accept,
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
    let source = party_source(&state.paths.data);
    handle.manage(Arc::new(Received::new(source.clone())));
    // Not `commands_sync::install`, which turns discovery on and installs the system keychain: here the link can
    // only ever listen on localhost and has no keychain, should a `link_*` command ever be offered by mistake.
    let sync = DeviceSync::new(
        Arc::new(handle.clone()),
        Arc::new(TauriSyncEvents(TauriSink(handle.clone()))),
        SyncConfig {
            bind_ip: Some(Ipv4Addr::LOCALHOST.into()),
            device_name: source,
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
