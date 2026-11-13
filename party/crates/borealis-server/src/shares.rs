//! Share codes (plan E4 and E6; decisions D12, D14, D37): a player shares characters and content packs from any device,
//! Android included, and another player types the code (or opens its link) to get the usual bundle preview.
//!
//! - **Sending.** The server builds the bundle from the sender's own folder with upstream's builder (as File > Export
//!   to bundle does) and keeps it in `data/users/<sender>/sync/outgoing/share-<id>.borealis`. The sender gets a code of
//!   80 random bits; only its hash is stored. A code works once and for 7 days, for anyone signed in, or only for the
//!   account the sender named (which the answer never confirms exists).
//! - **Links first (D14).** Packs that came from an index or a 5etools address travel as that address, not as files: the
//!   recipient adds them again through the server, and every download passes [`crate::net_guard`]. 5etools packs
//!   imported from files, and packs whose address can't travel (a query string, a password, a private address), are
//!   left out. "Include the files" sends all of them as files instead; the app shows a licence notice first.
//! - **Receiving.** The code is redeemed into the recipient's own `sync/incoming` through `stage_bundle`, so the 0.1.4
//!   safe receive applies (the bundle names the sender's account, so nothing the recipient has is replaced). The app
//!   then opens upstream's preview with the token, and says afterwards whether it was accepted or declined. A
//!   recipient can decline and block a sender.
//! - **Limits (D37).** 20 open codes per sender; 8 redeemed and waiting per recipient from any one sender; a share at
//!   most `server.public_upload_mb`; outgoing and received files of one account at most twice that; creating and
//!   failed redeems rate-limited per account. Expired shares and their files go at start-up and every hour.
//! - **Kill switch.** `BOREALIS_SHARES=off` in the server's environment: every share route answers as if the server
//!   didn't offer it. There is no admin switch; the admin page shows counts and bytes only (D40).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::{Path as UrlPath, State};
use axum::Json;
use borealis_bundle::{Bundle, BundleMeta, BundleWriter, Item, ItemKind, Limits, Manifest};
use borealis_lib::device_sync::DeviceSync;
use borealis_lib::sync_dto::{LinkPackChoice, LinkSelection};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::Row;
use tauri::{AppHandle, Manager};

use crate::auth::User;
use crate::db::{now, Db};
use crate::error::{ApiError, ApiResult};
use crate::workspaces::Workspaces;
use crate::AppState;

/// How long a code works.
pub const SHARE_SECONDS: i64 = 7 * 24 * 60 * 60;
/// Codes one account may have open (not yet redeemed, or redeemed and not yet accepted or declined).
pub const OPEN_PER_SENDER: i64 = 20;
/// Shares from one sender that one recipient may have redeemed and not yet accepted or declined.
pub const WAITING_PER_SENDER: i64 = 8;
/// Ended shares are kept this long for the sender's list and the counts, then deleted.
const KEEP_ENDED_SECONDS: i64 = 30 * 24 * 60 * 60;
/// How often expired shares are cleared.
const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);
/// Codes one account may make per [`CREATE_WINDOW`].
pub const CREATES_ALLOWED: usize = 30;
const CREATE_WINDOW: Duration = Duration::from_secs(60 * 60);
/// Codes one account may get wrong per [`REDEEM_WINDOW`].
pub const FAILED_REDEEMS_ALLOWED: usize = 10;
const REDEEM_WINDOW: Duration = Duration::from_secs(15 * 60);

/// The one answer to a code that is unknown, used, expired or meant for another account.
pub const NO_SUCH_CODE: &str =
    "That code doesn't work. Check it, or ask for a new one: a code works once and for 7 days.";
const NOT_OFFERED: &str = "The server does not offer that.";

/// A share's states: `pending` (made, not redeemed), `claimed` (redeemed, waiting for accept or decline), then one of
/// `done`, `declined`, `cancelled` or `expired`.
const OPEN: &str = "('pending', 'claimed')";

/// `BOREALIS_SHARES=off` (or `0`, `false`, `no`) turns share codes off.
fn off(value: Option<&str>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no"
        )
    })
}

static DISABLED: LazyLock<bool> =
    LazyLock::new(|| off(std::env::var("BOREALIS_SHARES").ok().as_deref()));

fn offered() -> ApiResult<()> {
    if *DISABLED {
        Err(ApiError::NotFound(NOT_OFFERED.into()))
    } else {
        Ok(())
    }
}

/// Attempts per account in a sliding window.
struct Limiter {
    allowed: usize,
    window: Duration,
    hits: Mutex<HashMap<String, Vec<Instant>>>,
}

impl Limiter {
    fn new(allowed: usize, window: Duration) -> Self {
        Self {
            allowed,
            window,
            hits: Mutex::default(),
        }
    }

    fn blocked(&self, key: &str) -> bool {
        let mut map = self.hits.lock().unwrap_or_else(PoisonError::into_inner);
        let cutoff = Instant::now().checked_sub(self.window);
        map.retain(|_, times| {
            times.retain(|t| cutoff.is_none_or(|c| *t > c));
            !times.is_empty()
        });
        map.get(key).is_some_and(|t| t.len() >= self.allowed)
    }

    fn hit(&self, key: &str) {
        self.hits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key.to_string())
            .or_default()
            .push(Instant::now());
    }
}

static CREATES: LazyLock<Limiter> = LazyLock::new(|| Limiter::new(CREATES_ALLOWED, CREATE_WINDOW));
static FAILED_REDEEMS: LazyLock<Limiter> =
    LazyLock::new(|| Limiter::new(FAILED_REDEEMS_ALLOWED, REDEEM_WINDOW));

// Codes.

/// Crockford's base 32: no I, L, O or U, so a code read aloud or typed from a phone survives.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Symbols in a code: 16 × 5 = 80 bits.
const CODE_LEN: usize = 16;

/// A new code, as stored (no dashes).
fn new_code() -> String {
    let mut bytes = [0u8; 10];
    rand::rng().fill_bytes(&mut bytes);
    let mut value = bytes.iter().fold(0u128, |v, b| (v << 8) | u128::from(*b));
    let mut out = [0u8; CODE_LEN];
    for slot in out.iter_mut().rev() {
        *slot = ALPHABET[usize::try_from(value & 31).unwrap_or(0)];
        value >>= 5;
    }
    out.iter().map(|b| char::from(*b)).collect()
}

/// `ABCD-EFGH-JKMN-PQRS`.
fn shown(code: &str) -> String {
    code.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// A typed code as stored: any case, dashes and spaces ignored, O read as 0 and I or L as 1. `None` when it can't be
/// one.
fn normalized(typed: &str) -> Option<String> {
    let code: String = typed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect();
    (code.len() == CODE_LEN && code.bytes().all(|b| ALPHABET.contains(&b))).then_some(code)
}

fn code_hash(code: &str) -> String {
    crate::auth::token_hash(&format!("share:{code}"))
}

// Files.

fn sync_dir(workspaces: &Workspaces, account: &str) -> PathBuf {
    workspaces.dir(account).join("sync")
}

fn outgoing_dir(workspaces: &Workspaces, account: &str) -> PathBuf {
    sync_dir(workspaces, account).join("outgoing")
}

fn share_file(workspaces: &Workspaces, account: &str, id: &str) -> PathBuf {
    outgoing_dir(workspaces, account).join(format!("share-{id}.borealis"))
}

/// Bytes of the files under `dir`, links not followed.
fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}

/// What one account's shared and received bundles take: its outgoing files and its staged uploads.
async fn held_bytes(workspaces: &Workspaces, account: &str) -> ApiResult<u64> {
    let sync = sync_dir(workspaces, account);
    tokio::task::spawn_blocking(move || {
        dir_bytes(&sync.join("outgoing")) + dir_bytes(&sync.join("incoming"))
    })
    .await
    .map_err(|e| ApiError::Internal(format!("disk task: {e}")))
}

/// The largest share: the public upload limit.
fn max_share_bytes() -> u64 {
    u64::try_from(crate::config::upload_limits().public_upload_mb)
        .unwrap_or(u64::MAX / 4)
        .saturating_mul(1024 * 1024)
}

/// What one account's outgoing and staged files may take together.
fn disk_cap() -> u64 {
    max_share_bytes().saturating_mul(2)
}

#[allow(clippy::cast_precision_loss)]
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
}

fn too_full(held: u64) -> ApiError {
    ApiError::Conflict(format!(
        "Your shared and received bundles already take {} of the {} this server allows each account. Cancel a share \
         code, or accept or decline what you received, then try again.",
        megabytes(held),
        megabytes(disk_cap())
    ))
}

// Links first.

/// A source address that stands in for packs: the recipient adds it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    /// `index` or `5etools`.
    pub kind: String,
    /// The address.
    pub url: String,
    /// The names of the packs it brings.
    pub packs: Vec<String>,
}

/// Where a pack in the library came from, when that keeps it out of a share's file.
enum Origin {
    /// An index or a 5etools address that can travel.
    Link(&'static str, String),
    /// Licensed content with no address that can travel: left out unless the files are included.
    Licensed,
}

/// An address another account may be sent to: HTTPS with no password, query or fragment (which could carry a token),
/// on the internet ([`crate::net_guard::check_url`]).
pub fn linkable(url: &str, index: bool) -> bool {
    let Ok(parsed) = url::Url::parse(url.trim()) else {
        return false;
    };
    let plain = parsed.scheme() == "https"
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none();
    let checked = if index {
        borealis_lib::content_index::raw_file_url(url)
    } else {
        url.trim().to_string()
    };
    plain && crate::net_guard::check_url(&checked).is_ok()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The library's packs that travel as links or not at all, by pack id.
fn pack_origins(state: &borealis_lib::state::AppState) -> HashMap<String, Origin> {
    let mut out = HashMap::new();
    for record in state.list_5etools_urls() {
        let link = linkable(&record.url, false);
        for pack in record.packs {
            let origin = if link {
                Origin::Link("5etools", record.url.clone())
            } else {
                Origin::Licensed
            };
            out.insert(pack, origin);
        }
    }
    for entry in std::fs::read_dir(state.paths.library())
        .into_iter()
        .flatten()
        .flatten()
    {
        let id = entry.file_name().to_string_lossy().into_owned();
        let dir = entry.path();
        let index = read_json(&dir.join(borealis_lib::content_packs::PACK_META_FILE))
            .and_then(|m| m["indexUrl"].as_str().map(str::to_string));
        if let Some(url) = index {
            let origin = if linkable(&url, true) {
                Origin::Link("index", url)
            } else {
                Origin::Licensed
            };
            out.insert(id, origin);
        } else if read_json(&dir.join("import.json"))
            .is_some_and(|v| v["origin"] == "5etools-import")
        {
            out.entry(id).or_insert(Origin::Licensed);
        }
    }
    out
}

/// A share's file as built.
struct Built {
    file_name: String,
    characters: usize,
    packs: usize,
    bytes: u64,
    links: Vec<Link>,
    /// Names of the packs left out with no link.
    left_out: Vec<String>,
    /// False when every item travels as a link, so there is no file.
    written: bool,
}

fn command_error(e: &borealis_lib::error::ApiError) -> ApiError {
    serde_json::to_value(e)
        .map_or_else(|_| ApiError::Internal(e.message.clone()), ApiError::Command)
}

/// Build the share's file at `out` from the account's folder: upstream's bundle, less the packs that travel as links
/// (and the licensed ones), unless `include_files`.
fn build(
    handle: &AppHandle,
    selection: &LinkSelection,
    include_files: bool,
    out: &Path,
) -> ApiResult<Built> {
    let sync = handle.state::<Arc<DeviceSync>>();
    let state = handle.state::<borealis_lib::state::AppState>();
    let built = sync
        .build_bundle_file(selection)
        .map_err(|e| command_error(&e))?;
    let info = borealis_lib::device_sync::export_dto(&built, None);
    let names: HashMap<String, String> = state
        .list_packs()
        .map(|packs| packs.into_iter().map(|p| (p.id, p.name)).collect())
        .unwrap_or_default();
    let name_of = |id: &str| {
        names
            .get(id)
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or_else(|| id.to_string())
    };
    let origins = if include_files {
        HashMap::new()
    } else {
        pack_origins(&state)
    };
    let mut links: BTreeMap<(&'static str, String), Vec<String>> = BTreeMap::new();
    let mut left_out = Vec::new();
    let mut keep: Vec<&Item> = Vec::new();
    for item in &built.manifest.items {
        if item.kind == ItemKind::Pack {
            match origins.get(&item.key) {
                Some(Origin::Link(kind, url)) => {
                    links
                        .entry((kind, url.clone()))
                        .or_default()
                        .push(name_of(&item.key));
                    continue;
                }
                Some(Origin::Licensed) => {
                    left_out.push(name_of(&item.key));
                    continue;
                }
                None => {}
            }
        }
        keep.push(item);
    }
    let bad = |e: borealis_bundle::BundleError| ApiError::Internal(format!("share bundle: {e}"));
    let written = if keep.is_empty() {
        false
    } else if keep.len() == built.manifest.items.len() {
        std::fs::copy(built.file.path(), out)
            .map_err(|e| ApiError::Internal(format!("share copy: {e}")))?;
        true
    } else {
        let file = std::fs::File::open(built.file.path())
            .map_err(|e| ApiError::Internal(format!("share read: {e}")))?;
        let mut bundle = Bundle::open_with(file, Limits::default()).map_err(bad)?;
        write_items(&mut bundle, &built.manifest, &keep, out).map_err(bad)?;
        true
    };
    let count = |kind: ItemKind| keep.iter().filter(|i| i.kind == kind).count();
    let characters = count(ItemKind::Character);
    Ok(Built {
        file_name: if characters == 0 {
            format!(
                "Borealis content.{}",
                borealis_lib::picked_files::BUNDLE_EXTENSION
            )
        } else {
            info.file_name
        },
        characters,
        packs: count(ItemKind::Pack),
        bytes: if written {
            std::fs::metadata(out).map_or(0, |m| m.len())
        } else {
            0
        },
        links: links
            .into_iter()
            .map(|((kind, url), packs)| Link {
                kind: kind.to_string(),
                url,
                packs,
            })
            .collect(),
        left_out,
        written,
    })
}

/// A new bundle at `out` with the `items` of `bundle`, under the same manifest details (as `backend::write_items`).
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

// The database.

/// A share as stored, with the sender's and recipient's usernames.
struct Share {
    id: String,
    from_id: String,
    from_name: String,
    to_id: Option<String>,
    to_name: Option<String>,
    claimed_by: Option<String>,
    file_name: String,
    characters: i64,
    packs: i64,
    links: Vec<Link>,
    bytes: i64,
    created_at: i64,
    expires_at: i64,
    status: String,
}

const SELECT: &str = "SELECT s.id, s.from_id, u.username, s.to_id, s.to_name, t.username, s.file_name, s.characters, \
     s.packs, s.links, s.bytes, s.created_at, s.expires_at, s.status \
     FROM shares s JOIN users u ON u.id = s.from_id LEFT JOIN users t ON t.id = s.to_id";

fn share_from(row: &sqlx::any::AnyRow) -> sqlx::Result<Share> {
    Ok(Share {
        id: row.try_get(0)?,
        from_id: row.try_get(1)?,
        from_name: row.try_get(2)?,
        to_id: row.try_get(3)?,
        to_name: row.try_get(4)?,
        claimed_by: row.try_get(5)?,
        file_name: row.try_get(6)?,
        characters: row.try_get(7)?,
        packs: row.try_get(8)?,
        links: serde_json::from_str(&row.try_get::<String, _>(9)?).unwrap_or_default(),
        bytes: row.try_get(10)?,
        created_at: row.try_get(11)?,
        expires_at: row.try_get(12)?,
        status: row.try_get(13)?,
    })
}

impl Share {
    fn is_open(&self) -> bool {
        matches!(self.status.as_str(), "pending" | "claimed") && self.expires_at > now()
    }

    /// The status, with an open share past its end shown as expired before the sweep gets to it.
    fn shown_status(&self) -> &str {
        let open = matches!(self.status.as_str(), "pending" | "claimed");
        if open && !self.is_open() {
            "expired"
        } else {
            &self.status
        }
    }

    /// What the sender sees.
    fn for_sender(&self) -> Value {
        json!({
            "id": self.id,
            "fileName": self.file_name,
            "characters": self.characters,
            "packs": self.packs,
            "links": self.links,
            "bytes": self.bytes,
            "createdAt": self.created_at,
            "expiresAt": self.expires_at,
            "status": self.shown_status(),
            "forUsername": self.to_name,
            "claimedBy": self.claimed_by,
        })
    }

    /// What the recipient sees.
    fn for_recipient(&self) -> Value {
        json!({
            "id": self.id,
            "from": self.from_name,
            "fromId": self.from_id,
            "fileName": self.file_name,
            "characters": self.characters,
            "packs": self.packs,
            "links": self.links,
            "bytes": self.bytes,
            "expiresAt": self.expires_at,
            "status": self.status,
        })
    }
}

async fn share_where(db: &Db, condition: &str, binds: &[&str]) -> ApiResult<Option<Share>> {
    let sql = format!("{SELECT} WHERE {condition}");
    let mut query = sqlx::query(&sql);
    for bind in binds {
        query = query.bind(*bind);
    }
    let row = query.fetch_optional(db.pool()).await?;
    Ok(row.map(|r| share_from(&r)).transpose()?)
}

async fn count(db: &Db, sql: &str, a: &str, b: Option<&str>) -> ApiResult<i64> {
    let mut query = sqlx::query(sql).bind(a.to_string());
    if let Some(b) = b {
        query = query.bind(b.to_string());
    }
    Ok(query.bind(now()).fetch_one(db.pool()).await?.try_get(0)?)
}

/// End a share that is still open; true when this call ended it. Its file goes too.
async fn end(state: &AppState, share: &Share, status: &str, who: (&str, &str)) -> ApiResult<bool> {
    let done = sqlx::query(&format!(
        "UPDATE shares SET status = $1, ended_at = $2 WHERE id = $3 AND {} = $4 AND status IN {OPEN}",
        who.0
    ))
    .bind(status)
    .bind(now())
    .bind(&share.id)
    .bind(who.1)
    .execute(state.db.pool())
    .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    let _ = tokio::fs::remove_file(share_file(&state.workspaces, &share.from_id, &share.id)).await;
    Ok(true)
}

fn not_found() -> ApiError {
    ApiError::NotFound("That share is not there any more.".into())
}

// Routes.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateForm {
    #[serde(default)]
    character_ids: Vec<String>,
    #[serde(default)]
    packs: LinkPackChoice,
    /// Send the packs that would travel as links (or not at all) as files: the app shows the licence notice first.
    #[serde(default)]
    include_files: bool,
    /// Only this account can redeem the code.
    #[serde(default)]
    for_username: Option<String>,
}

/// `POST /api/shares`: build a share and hand back its code.
async fn create(
    State(state): State<AppState>,
    user: User,
    Json(form): Json<CreateForm>,
) -> ApiResult<Json<Value>> {
    offered()?;
    if CREATES.blocked(&user.id) {
        return Err(ApiError::TooMany);
    }
    if form.character_ids.is_empty() && matches!(form.packs, LinkPackChoice::None) {
        return Err(ApiError::Invalid(
            "Choose at least one character, or content packs, to share.".into(),
        ));
    }
    if form.character_ids.len() > 500 || form.character_ids.iter().any(|id| id.len() > 200) {
        return Err(ApiError::Invalid(
            "That is too many characters for one share.".into(),
        ));
    }
    let for_name = form
        .for_username
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_lowercase);
    if let Some(name) = &for_name {
        if name.chars().count() > 40 {
            return Err(ApiError::Invalid("That is not a username.".into()));
        }
        if *name == user.username.to_lowercase() {
            return Err(ApiError::Invalid("That is your own account.".into()));
        }
    }
    CREATES.hit(&user.id);
    let open = count(
        &state.db,
        &format!("SELECT COUNT(*) FROM shares WHERE from_id = $1 AND status IN {OPEN} AND expires_at > $2"),
        &user.id,
        None,
    )
    .await?;
    if open >= OPEN_PER_SENDER {
        return Err(ApiError::Conflict(format!(
            "You have {OPEN_PER_SENDER} share codes open. Cancel one, or wait until one is used or expires."
        )));
    }
    let held = held_bytes(&state.workspaces, &user.id).await?;
    if held >= disk_cap() {
        return Err(too_full(held));
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let out = share_file(&state.workspaces, &user.id, &id);
    let part = out.with_extension("part");
    let workspace = state.workspaces.get(&user.id).await?;
    let selection = LinkSelection {
        character_ids: form.character_ids,
        packs: form.packs,
    };
    let include = form.include_files;
    let target = part.clone();
    let built = workspace
        .blocking(move |handle| {
            if let Some(dir) = target.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| ApiError::Internal(format!("outgoing folder: {e}")))?;
            }
            build(handle, &selection, include, &target)
        })
        .await;
    let fail = |e: ApiError| {
        let _ = std::fs::remove_file(&part);
        Err(e)
    };
    let built = match built {
        Ok(built) => built,
        Err(e) => return fail(e),
    };
    if !built.written && built.links.is_empty() {
        return fail(ApiError::Invalid(format!(
            "Nothing is left to share: {} can't travel as a link. Turn on \"Include the files\" to send {} as files.",
            built.left_out.join(", "),
            if built.left_out.len() == 1 { "it" } else { "them" }
        )));
    }
    if built.bytes > max_share_bytes() {
        return fail(ApiError::Invalid(format!(
            "This share is {}; at most {} can be shared at once. Choose fewer characters, or \"No content packs\".",
            megabytes(built.bytes),
            megabytes(max_share_bytes())
        )));
    }
    if held + built.bytes > disk_cap() {
        return fail(too_full(held));
    }
    if built.written {
        if let Err(e) = std::fs::rename(&part, &out) {
            return fail(ApiError::Internal(format!("share file: {e}")));
        }
    }
    let code = new_code();
    let created = now();
    let inserted = sqlx::query(
        "INSERT INTO shares (id, from_id, code_hash, to_id, to_name, file_name, characters, packs, links, bytes,
            created_at, expires_at, claimed_at, ended_at, status)
         VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, $8, $9, $10, $11, NULL, NULL, 'pending')",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(code_hash(&code))
    .bind(for_name.clone())
    .bind(&built.file_name)
    .bind(i64::try_from(built.characters).unwrap_or(i64::MAX))
    .bind(i64::try_from(built.packs).unwrap_or(i64::MAX))
    .bind(serde_json::to_string(&built.links).unwrap_or_else(|_| "[]".into()))
    .bind(i64::try_from(built.bytes).unwrap_or(i64::MAX))
    .bind(created)
    .bind(created + SHARE_SECONDS)
    .execute(state.db.pool())
    .await;
    if let Err(e) = inserted {
        let _ = std::fs::remove_file(&out);
        return Err(e.into());
    }
    let share = share_where(&state.db, "s.id = $1", &[&id])
        .await?
        .ok_or_else(not_found)?;
    let link = state
        .config
        .public_url
        .as_ref()
        .map(|base| format!("{base}/#/share/receive/{}", shown(&code)));
    Ok(Json(json!({
        "share": share.for_sender(),
        "code": shown(&code),
        "link": link,
        "leftOut": built.left_out,
    })))
}

/// `GET /api/shares`: this account's codes, what it redeemed and has yet to accept or decline, and whom it blocked.
async fn list(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    offered()?;
    let since = now() - SHARE_SECONDS;
    let sent = sqlx::query(&format!(
        "{SELECT} WHERE s.from_id = $1 AND (s.status IN {OPEN} OR s.ended_at > $2) ORDER BY s.created_at DESC"
    ))
    .bind(&user.id)
    .bind(since)
    .fetch_all(state.db.pool())
    .await?;
    let received = sqlx::query(&format!(
        "{SELECT} WHERE s.to_id = $1 AND s.status = 'claimed' AND s.expires_at > $2 ORDER BY s.claimed_at DESC"
    ))
    .bind(&user.id)
    .bind(now())
    .fetch_all(state.db.pool())
    .await?;
    let blocked = sqlx::query(
        "SELECT b.blocked_id, u.username FROM share_blocks b JOIN users u ON u.id = b.blocked_id
         WHERE b.user_id = $1 ORDER BY lower(u.username)",
    )
    .bind(&user.id)
    .fetch_all(state.db.pool())
    .await?;
    let rows =
        |rows: &[sqlx::any::AnyRow], view: fn(&Share) -> Value| -> sqlx::Result<Vec<Value>> {
            rows.iter().map(|r| Ok(view(&share_from(r)?))).collect()
        };
    let held = held_bytes(&state.workspaces, &user.id).await?;
    Ok(Json(json!({
        "sent": rows(&sent, Share::for_sender)?,
        "received": rows(&received, Share::for_recipient)?,
        "blocked": blocked.iter().map(|r| Ok(json!({ "id": r.try_get::<String, _>(0)?, "username": r.try_get::<String, _>(1)? }))).collect::<sqlx::Result<Vec<_>>>()?,
        "limits": {
            "openPerSender": OPEN_PER_SENDER,
            "waitingPerSender": WAITING_PER_SENDER,
            "days": SHARE_SECONDS / 86_400,
            "maxBytes": max_share_bytes(),
            "diskBytes": disk_cap(),
            "heldBytes": held,
        },
    })))
}

/// `DELETE /api/shares/{id}`: the sender takes a code back. A copy the recipient already opened stays theirs to decide on.
async fn cancel(
    State(state): State<AppState>,
    user: User,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    offered()?;
    let share = share_where(&state.db, "s.id = $1 AND s.from_id = $2", &[&id, &user.id])
        .await?
        .ok_or_else(not_found)?;
    if !end(&state, &share, "cancelled", ("from_id", &user.id)).await? {
        return Err(not_found());
    }
    Ok(Json(json!({ "ok": true })))
}

/// Put the share's file into the recipient's own `sync/incoming` through `stage_bundle`, the 0.1.4 safe receive path,
/// and hand back its token for the preview. `None` when every item travels as a link.
async fn stage(state: &AppState, share: &Share, user: &User) -> ApiResult<Option<Value>> {
    if share.bytes == 0 {
        return Ok(None);
    }
    let path = share_file(&state.workspaces, &share.from_id, &share.id);
    let bytes = tokio::fs::read(&path).await.map_err(|_| {
        ApiError::NotFound(
            "The shared file is not on the server any more. Ask for a new code.".into(),
        )
    })?;
    let encoded = tokio::task::spawn_blocking(move || {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    })
    .await
    .map_err(|e| ApiError::Internal(format!("encode task: {e}")))?;
    let workspace = state.workspaces.get(&user.id).await?;
    let mut args = Map::new();
    args.insert("fileName".into(), json!(share.file_name));
    args.insert("contentBase64".into(), Value::String(encoded));
    let staged = workspace.call("stage_bundle", args).await?;
    Ok(Some(
        json!({ "token": staged["token"], "name": staged["name"] }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RedeemForm {
    code: String,
}

async fn blocked_by(db: &Db, user: &str, sender: &str) -> ApiResult<bool> {
    Ok(
        sqlx::query("SELECT 1 FROM share_blocks WHERE user_id = $1 AND blocked_id = $2")
            .bind(user)
            .bind(sender)
            .fetch_optional(db.pool())
            .await?
            .is_some(),
    )
}

/// `POST /api/shares/redeem`: take a code. The share is this account's from now on (no one else can redeem it), and
/// its file is staged for the preview.
async fn redeem(
    State(state): State<AppState>,
    user: User,
    Json(form): Json<RedeemForm>,
) -> ApiResult<Json<Value>> {
    offered()?;
    if FAILED_REDEEMS.blocked(&user.id) {
        return Err(ApiError::TooMany);
    }
    let refused = || {
        FAILED_REDEEMS.hit(&user.id);
        ApiError::NotFound(NO_SUCH_CODE.into())
    };
    let Some(code) = normalized(&form.code) else {
        return Err(refused());
    };
    let Some(share) = share_where(&state.db, "s.code_hash = $1", &[&code_hash(&code)]).await?
    else {
        return Err(refused());
    };
    if share.from_id == user.id {
        return Err(ApiError::Invalid(
            "This is your own share code. Send it to the other player.".into(),
        ));
    }
    let mine = share.to_id.as_deref() == Some(user.id.as_str());
    // Redeemed already by this account: the same share again, as from the list.
    let again = mine && share.status == "claimed";
    if !share.is_open() || (share.status == "claimed" && !mine) {
        return Err(refused());
    }
    if share
        .to_name
        .as_ref()
        .is_some_and(|n| *n != user.username.to_lowercase())
    {
        return Err(refused());
    }
    if !again {
        if blocked_by(&state.db, &user.id, &share.from_id).await? {
            return Err(ApiError::Forbidden(format!(
                "You blocked {}. Unblock them under Share codes to receive this.",
                share.from_name
            )));
        }
        let waiting = count(
            &state.db,
            "SELECT COUNT(*) FROM shares WHERE to_id = $1 AND from_id = $2 AND status = 'claimed' AND expires_at > $3",
            &user.id,
            Some(&share.from_id),
        )
        .await?;
        if waiting >= WAITING_PER_SENDER {
            return Err(ApiError::Conflict(format!(
                "You have {WAITING_PER_SENDER} shares from {} waiting. Accept or decline some of them first.",
                share.from_name
            )));
        }
        let held = held_bytes(&state.workspaces, &user.id).await?;
        if held + u64::try_from(share.bytes).unwrap_or(u64::MAX) > disk_cap() {
            return Err(too_full(held));
        }
        let claimed = sqlx::query(
            "UPDATE shares SET status = 'claimed', to_id = $1, claimed_at = $2
             WHERE id = $3 AND status = 'pending' AND expires_at > $2",
        )
        .bind(&user.id)
        .bind(now())
        .bind(&share.id)
        .execute(state.db.pool())
        .await?;
        if claimed.rows_affected() == 0 {
            return Err(refused());
        }
    }
    let staged = stage(&state, &share, &user).await?;
    let mut out = share.for_recipient();
    out["status"] = json!("claimed");
    Ok(Json(json!({ "share": out, "bundle": staged })))
}

/// A share this account redeemed, by id.
async fn received(state: &AppState, user: &User, id: &str) -> ApiResult<Share> {
    share_where(&state.db, "s.id = $1 AND s.to_id = $2", &[id, &user.id])
        .await?
        .ok_or_else(not_found)
}

/// `POST /api/shares/{id}/open`: stage a redeemed share again, for a preview that was closed without a choice.
async fn reopen(
    State(state): State<AppState>,
    user: User,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    offered()?;
    let share = received(&state, &user, &id).await?;
    if share.status != "claimed" || !share.is_open() {
        return Err(not_found());
    }
    let staged = stage(&state, &share, &user).await?;
    Ok(Json(
        json!({ "share": share.for_recipient(), "bundle": staged }),
    ))
}

/// `POST /api/shares/{id}/accepted`: the recipient applied it; the file is no longer needed.
async fn accepted(
    State(state): State<AppState>,
    user: User,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    offered()?;
    let share = received(&state, &user, &id).await?;
    if !end(&state, &share, "done", ("to_id", &user.id)).await? {
        return Err(not_found());
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclineForm {
    #[serde(default)]
    block: bool,
}

/// `POST /api/shares/{id}/decline`: the recipient doesn't want it, and with `block` wants nothing more from its sender
/// (every other share of theirs waiting here is declined too).
async fn decline(
    State(state): State<AppState>,
    user: User,
    UrlPath(id): UrlPath<String>,
    Json(form): Json<DeclineForm>,
) -> ApiResult<Json<Value>> {
    offered()?;
    let share = received(&state, &user, &id).await?;
    let ended = end(&state, &share, "declined", ("to_id", &user.id)).await?;
    if !ended && !form.block {
        return Err(not_found());
    }
    if form.block {
        sqlx::query(
            "INSERT INTO share_blocks (user_id, blocked_id, created_at) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(&user.id)
        .bind(&share.from_id)
        .bind(now())
        .execute(state.db.pool())
        .await?;
        let others = sqlx::query(&format!(
            "{SELECT} WHERE s.to_id = $1 AND s.from_id = $2 AND s.status = 'claimed'"
        ))
        .bind(&user.id)
        .bind(&share.from_id)
        .fetch_all(state.db.pool())
        .await?;
        for row in &others {
            end(&state, &share_from(row)?, "declined", ("to_id", &user.id)).await?;
        }
    }
    Ok(Json(json!({ "ok": true, "blocked": form.block })))
}

/// `POST /api/shares/{id}/links`: add the share's index links to this account (each checked by `net_guard`, as if
/// typed here), and say which 5etools links may be imported. The app then updates the indexes and imports the 5etools
/// addresses through the usual commands, which check every download again.
async fn links(
    State(state): State<AppState>,
    user: User,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    offered()?;
    let share = received(&state, &user, &id).await?;
    if !matches!(share.status.as_str(), "claimed" | "done") {
        return Err(not_found());
    }
    let workspace = state.workspaces.get(&user.id).await?;
    let followed: HashSet<String> = workspace
        .call("list_indexes", Map::new())
        .await?
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|i| i["url"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut indexes = Vec::new();
    let mut fivetools = Vec::new();
    for link in &share.links {
        if link.kind == "index" {
            if followed.contains(&link.url) {
                indexes.push(
                    json!({ "url": link.url, "packs": link.packs, "added": true, "already": true }),
                );
                continue;
            }
            let mut args = Map::new();
            args.insert("url".into(), json!(link.url));
            // The party's add_index (backend.rs): refused unless net_guard allows the address.
            match workspace.call("add_index", args).await {
                Ok(_) => indexes.push(json!({ "url": link.url, "packs": link.packs, "added": true })),
                Err(e) => indexes.push(json!({ "url": link.url, "packs": link.packs, "added": false, "message": message_of(e) })),
            }
        } else {
            let url = link.url.clone();
            let allowed = tokio::task::spawn_blocking(move || crate::net_guard::check_url(&url))
                .await
                .map_err(|e| ApiError::Internal(format!("check task: {e}")))?;
            fivetools.push(match allowed {
                Ok(()) => json!({ "url": link.url, "packs": link.packs, "allowed": true }),
                Err(message) => json!({ "url": link.url, "packs": link.packs, "allowed": false, "message": message }),
            });
        }
    }
    if indexes
        .iter()
        .any(|i| i["added"] == json!(true) && i.get("already").is_none())
    {
        crate::content_events::changed(&state.hub, &user.id, "indexes", "add_share_links");
    }
    Ok(Json(json!({ "indexes": indexes, "fivetools": fivetools })))
}

fn message_of(e: ApiError) -> String {
    match e {
        ApiError::Command(v) => v["message"].as_str().unwrap_or("Not added.").to_string(),
        ApiError::Invalid(m)
        | ApiError::NotFound(m)
        | ApiError::Conflict(m)
        | ApiError::Forbidden(m) => m,
        _ => "Not added.".into(),
    }
}

/// `DELETE /api/shares/blocks/{id}`: take a block back.
async fn unblock(
    State(state): State<AppState>,
    user: User,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    offered()?;
    sqlx::query("DELETE FROM share_blocks WHERE user_id = $1 AND blocked_id = $2")
        .bind(&user.id)
        .bind(&id)
        .execute(state.db.pool())
        .await?;
    Ok(Json(json!({ "ok": true })))
}

/// The share routes.
pub fn routes() -> axum::Router<AppState> {
    use axum::routing::{delete, get, post};
    axum::Router::new()
        .route("/api/shares", get(list).post(create))
        .route("/api/shares/redeem", post(redeem))
        .route("/api/shares/blocks/{id}", delete(unblock))
        .route("/api/shares/{id}", delete(cancel))
        .route("/api/shares/{id}/open", post(reopen))
        .route("/api/shares/{id}/accepted", post(accepted))
        .route("/api/shares/{id}/decline", post(decline))
        .route("/api/shares/{id}/links", post(links))
}

// Clean-up and counts.

/// Every account folder's `sync` folder.
fn sync_dirs(workspaces: &Workspaces) -> Vec<(String, PathBuf)> {
    let users = workspaces
        .dir("_")
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    std::fs::read_dir(users)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                e.path().join("sync"),
            )
        })
        .collect()
}

/// End expired shares, delete their files and any file no open share names, and forget shares ended long ago. Returns
/// how many files went.
///
/// # Errors
/// When the database can't be read.
pub async fn sweep(db: &Db, workspaces: &Arc<Workspaces>) -> ApiResult<usize> {
    let at = now();
    sqlx::query(&format!(
        "UPDATE shares SET status = 'expired', ended_at = $1 WHERE status IN {OPEN} AND expires_at <= $1"
    ))
    .bind(at)
    .execute(db.pool())
    .await?;
    sqlx::query("DELETE FROM shares WHERE ended_at IS NOT NULL AND ended_at < $1")
        .bind(at - KEEP_ENDED_SECONDS)
        .execute(db.pool())
        .await?;
    let open: HashSet<String> =
        sqlx::query(&format!("SELECT id FROM shares WHERE status IN {OPEN}"))
            .fetch_all(db.pool())
            .await?
            .iter()
            .map(|r| r.try_get(0))
            .collect::<sqlx::Result<_>>()?;
    let workspaces = Arc::clone(workspaces);
    tokio::task::spawn_blocking(move || {
        let mut removed = 0;
        for (_, sync) in sync_dirs(&workspaces) {
            for entry in std::fs::read_dir(sync.join("outgoing"))
                .into_iter()
                .flatten()
                .flatten()
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                let id = name.strip_prefix("share-").and_then(|n| {
                    n.strip_suffix(".borealis")
                        .or_else(|| n.strip_suffix(".part"))
                });
                // A file being written right now has no row yet: only `.part` files older than an hour go.
                let fresh_part = name.ends_with(".part")
                    && entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .is_ok_and(|t| t.elapsed().is_ok_and(|age| age < SWEEP_EVERY));
                if id.is_some_and(|id| !open.contains(id) || name.ends_with(".part"))
                    && !fresh_part
                    && std::fs::remove_file(entry.path()).is_ok()
                {
                    removed += 1;
                }
            }
        }
        removed
    })
    .await
    .map_err(|e| ApiError::Internal(format!("sweep task: {e}")))
}

/// [`sweep`] now and every hour.
pub async fn sweep_loop(db: Db, workspaces: Arc<Workspaces>) {
    loop {
        match sweep(&db, &workspaces).await {
            Ok(0) => {}
            Ok(n) => tracing::info!("shares: removed {n} files of expired or ended shares"),
            Err(e) => tracing::warn!("shares: clean-up failed: {e:?}"),
        }
        tokio::time::sleep(SWEEP_EVERY).await;
    }
}

/// Counts and bytes for the admin page, never what was shared or by whom.
///
/// # Errors
/// When the database can't be read.
pub async fn stats(db: &Db, workspaces: &Arc<Workspaces>) -> ApiResult<Value> {
    let rows = sqlx::query(
        "SELECT status, COUNT(*), CAST(COALESCE(SUM(bytes), 0) AS BIGINT) FROM shares WHERE expires_at > $1 OR status \
         NOT IN ('pending', 'claimed') GROUP BY status",
    )
    .bind(now())
    .fetch_all(db.pool())
    .await?;
    let mut by_status = json!({ "pending": 0, "claimed": 0, "done": 0, "declined": 0, "cancelled": 0, "expired": 0 });
    for row in &rows {
        let status: String = row.try_get(0)?;
        by_status[status] = json!(row.try_get::<i64, _>(1)?);
    }
    let blocks: i64 = sqlx::query("SELECT COUNT(*) FROM share_blocks")
        .fetch_one(db.pool())
        .await?
        .try_get(0)?;
    let workspaces = Arc::clone(workspaces);
    let (outgoing, staged) = tokio::task::spawn_blocking(move || {
        sync_dirs(&workspaces)
            .iter()
            .fold((0, 0), |(o, s), (_, sync)| {
                (
                    o + dir_bytes(&sync.join("outgoing")),
                    s + dir_bytes(&sync.join("incoming")),
                )
            })
    })
    .await
    .map_err(|e| ApiError::Internal(format!("disk task: {e}")))?;
    Ok(json!({
        "enabled": !*DISABLED,
        "byStatus": by_status,
        "blocks": blocks,
        "outgoingBytes": outgoing,
        "stagedBytes": staged,
        "maxBytes": max_share_bytes(),
        "diskBytesPerAccount": disk_cap(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_have_80_bits_and_survive_typing() {
        let mut seen = HashSet::new();
        for _ in 0..2000 {
            let code = new_code();
            assert_eq!(code.len(), CODE_LEN);
            assert!(code.bytes().all(|b| ALPHABET.contains(&b)), "{code}");
            assert!(seen.insert(code.clone()), "repeated {code}");
            let typed = shown(&code);
            assert_eq!(typed.len(), CODE_LEN + 3);
            assert_eq!(normalized(&typed).as_deref(), Some(code.as_str()));
            assert_eq!(
                normalized(&format!("  {} ", typed.to_lowercase())).as_deref(),
                Some(code.as_str())
            );
        }
        // O for 0 and I or L for 1, as people misread them.
        assert_eq!(
            normalized("OIL0-1111-2222-3333").as_deref(),
            Some("0110111122223333")
        );
        for bad in [
            "",
            "ABCD",
            "ABCD-EFGH-JKMN-PQRSX",
            "ABCD-EFGH-JKMN-PQRU",
            "ABCD/EFGH/JKMN/PQRS",
        ] {
            assert_eq!(normalized(bad), None, "{bad}");
        }
        // 16 symbols of 32: 80 bits, over the plan's 64.
        const { assert!(CODE_LEN * 5 >= 64) };
        assert_ne!(code_hash("0110111122223333"), "0110111122223333");
        assert_eq!(code_hash("0110111122223333").len(), 64);
    }

    #[test]
    fn the_kill_switch_reads_the_environment_value() {
        for value in ["off", "OFF", " 0 ", "false", "no"] {
            assert!(off(Some(value)), "{value}");
        }
        for value in [None, Some(""), Some("on"), Some("1"), Some("true")] {
            assert!(!off(value), "{value:?}");
        }
    }

    #[test]
    fn only_plain_internet_addresses_travel_as_links() {
        assert!(linkable("https://content.example/homebrew.index", true));
        assert!(linkable("https://github.com/example/srd-data", false));
        for url in [
            "http://content.example/homebrew.index",
            "https://me:pw@content.example/homebrew.index",
            "https://content.example/homebrew.index?token=1",
            "https://content.example/homebrew.index?v=2",
            "https://content.example/homebrew.index#x",
            "https://lan.example.invalid/homebrew.index",
            "https://127.0.0.1/homebrew.index",
            "not a url",
        ] {
            assert!(!linkable(url, true), "{url}");
        }
    }
}
