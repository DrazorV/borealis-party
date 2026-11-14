//! The admin page: accounts waiting for approval, account management, statistics, runtime settings (sign-up and the
//! trusted proxies), the activity log and the trash of deleted characters (`admin_trash.rs`). Every route here answers
//! 403 to anyone but an active admin.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{hash_password, validate_password, Status, User};
use crate::db::{now, AccountRow, Activity};
use crate::error::{ApiError, ApiResult};
use crate::net_guard::Cidr;
use crate::AppState;

#[path = "admin_trash.rs"]
mod trash;

/// Keys in `server_settings`.
const OPEN_REGISTRATION: &str = "open_registration";
const REQUIRE_APPROVAL: &str = "require_approval";
/// A JSON array of addresses and ranges. No row: `server.trusted_proxies` applies.
const TRUSTED_PROXIES: &str = "trusted_proxies";

/// Most entries the trusted proxy list takes.
const MAX_PROXIES: usize = 32;

/// Largest page the lists hand out.
const MAX_PAGE: i64 = 200;

impl Activity {
    /// An entry of `kind`, now.
    pub fn new(kind: &str) -> Self {
        Self {
            at: now(),
            kind: kind.to_string(),
            ..Self::default()
        }
    }

    /// Done by `user`.
    #[must_use]
    pub fn by(mut self, user: &User) -> Self {
        self.actor_id = Some(user.id.clone());
        self.actor_name = Some(user.username.clone());
        self
    }

    /// Done to the account `id`, called `name`.
    #[must_use]
    pub fn on(mut self, id: &str, name: &str) -> Self {
        self.target_id = Some(id.to_string());
        self.target_name = Some(name.to_string());
        self
    }

    /// With more words.
    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// From this client address.
    #[must_use]
    pub fn client(mut self, ip: &str) -> Self {
        self.ip = Some(ip.to_string());
        self
    }
}

/// Add to the activity log. A failure is logged and otherwise ignored: the log never stops the thing it records.
pub async fn record(state: &AppState, entry: Activity) {
    if let Err(e) = state.db.log(&entry).await {
        tracing::warn!("activity log ({}): {e:?}", entry.kind);
    }
}

/// Sign-up settings: what the admin page saved, else `config.toml`.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSettings {
    /// Anyone may ask for an account.
    pub open_registration: bool,
    /// New accounts wait for an admin.
    pub require_approval: bool,
}

async fn saved_flag(state: &AppState, key: &str, default: bool) -> ApiResult<bool> {
    Ok(state
        .db
        .setting(key)
        .await?
        .map_or(default, |value| value == "true"))
}

/// The sign-up settings in force.
pub async fn runtime_settings(state: &AppState) -> ApiResult<RuntimeSettings> {
    Ok(RuntimeSettings {
        open_registration: saved_flag(state, OPEN_REGISTRATION, state.config.open_registration)
            .await?,
        require_approval: saved_flag(state, REQUIRE_APPROVAL, state.config.require_approval)
            .await?,
    })
}

/// An active admin, or 403.
pub struct Admin(pub User);

impl<S> axum::extract::FromRequestParts<S> for Admin
where
    S: Send + Sync,
    AppState: axum::extract::FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, ApiError> {
        let user = User::from_request_parts(parts, state).await?;
        if user.admin {
            Ok(Self(user))
        } else {
            Err(ApiError::Forbidden("Only admins can do that.".into()))
        }
    }
}

fn account_json(row: &AccountRow) -> Value {
    json!({
        "id": row.user.id,
        "username": row.user.username,
        "email": row.user.email,
        "admin": row.user.admin,
        "mustChangePassword": row.user.must_change_password,
        "status": row.status,
        "createdAt": row.created_at,
        "emailConfirmedAt": row.email_verified_at,
        "awaitingEmail": row.awaiting_email,
        "sessions": row.sessions,
    })
}

/// A page of a list.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageQuery {
    #[serde(default)]
    page: Option<i64>,
    #[serde(default)]
    per_page: Option<i64>,
}

impl PageQuery {
    fn limits(&self) -> (i64, i64) {
        let per = self.per_page.unwrap_or(50).clamp(1, MAX_PAGE);
        let page = self.page.unwrap_or(0).max(0);
        (per, page.saturating_mul(per))
    }
}

/// The account list's filters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuery {
    #[serde(default)]
    search: String,
    #[serde(default)]
    status: Option<Status>,
    // Not a flattened `PageQuery`: numbers in a flattened query-string struct don't parse.
    #[serde(default)]
    page: Option<i64>,
    #[serde(default)]
    per_page: Option<i64>,
}

/// `GET /api/admin/accounts`.
pub async fn accounts(
    State(state): State<AppState>,
    _: Admin,
    Query(query): Query<AccountQuery>,
) -> ApiResult<Json<Value>> {
    let (limit, offset) = PageQuery {
        page: query.page,
        per_page: query.per_page,
    }
    .limits();
    let (rows, total) = state
        .db
        .accounts(&query.search, query.status, limit, offset)
        .await?;
    let items: Vec<Value> = rows.iter().map(account_json).collect();
    Ok(Json(json!({ "items": items, "total": total })))
}

async fn target(state: &AppState, id: &str) -> ApiResult<AccountRow> {
    state
        .db
        .account(id)
        .await?
        .ok_or_else(|| ApiError::NotFound("There is no such account.".into()))
}

fn last_admin() -> ApiError {
    ApiError::Conflict(
        "The server needs at least one active admin. Make someone else an admin first.".into(),
    )
}

/// The status form.
#[derive(Deserialize)]
pub struct StatusForm {
    status: Status,
}

/// `POST /api/admin/accounts/{id}/status`: approve, reject, disable or re-enable an account.
pub async fn set_status(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    Json(form): Json<StatusForm>,
) -> ApiResult<Json<Value>> {
    let before = target(&state, &id).await?;
    let kind = match (before.status, form.status) {
        (Status::Pending, Status::Pending) => return Ok(Json(account_json(&before))),
        (_, Status::Pending) => {
            return Err(ApiError::Invalid(
                "An account can't go back to waiting for approval.".into(),
            ))
        }
        (Status::Pending, Status::Active) => "approve",
        (_, Status::Active) => "enable",
        (_, Status::Rejected) => "reject",
        (_, Status::Disabled) => "disable",
    };
    if !state.db.set_status(&id, form.status).await? {
        return Err(last_admin());
    }
    if form.status != Status::Active {
        state.db.delete_sessions_of(&id).await?;
        state.hub.close_user(&id);
    }
    record(
        &state,
        Activity::new(kind)
            .by(&admin)
            .on(&id, &before.user.username),
    )
    .await;
    Ok(Json(account_json(&target(&state, &id).await?)))
}

/// The admin flag form.
#[derive(Deserialize)]
pub struct AdminForm {
    admin: bool,
}

/// `POST /api/admin/accounts/{id}/admin`: make an account an admin, or not.
pub async fn set_admin(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    Json(form): Json<AdminForm>,
) -> ApiResult<Json<Value>> {
    let before = target(&state, &id).await?;
    if form.admin && before.status != Status::Active {
        return Err(ApiError::Invalid(
            "Only an active account can be an admin.".into(),
        ));
    }
    if !state.db.set_admin(&id, form.admin).await? {
        return Err(last_admin());
    }
    let kind = if form.admin {
        "admin_granted"
    } else {
        "admin_removed"
    };
    record(
        &state,
        Activity::new(kind)
            .by(&admin)
            .on(&id, &before.user.username),
    )
    .await;
    Ok(Json(account_json(&target(&state, &id).await?)))
}

/// The password reset form: with a password, that becomes a temporary one; without, the old one stays and has to
/// be changed at the next sign-in.
#[derive(Deserialize)]
pub struct ResetForm {
    #[serde(default)]
    password: Option<String>,
}

/// `POST /api/admin/accounts/{id}/password`: the account is signed out and must choose a new password when it next
/// signs in.
pub async fn reset_password(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    Json(form): Json<ResetForm>,
) -> ApiResult<Json<Value>> {
    let before = target(&state, &id).await?;
    let temporary = form.password.filter(|p| !p.is_empty());
    let hash = match &temporary {
        Some(password) => {
            validate_password(&before.user.username, password)?;
            Some(hash_password(password).await?)
        }
        None => None,
    };
    state.db.set_password(&id, hash.as_deref(), true).await?;
    state.db.delete_sessions_of(&id).await?;
    state.hub.close_user(&id);
    let revoked = crate::tokens::revoke_all(&state, &id).await?;
    let detail = if temporary.is_some() {
        "temporary password set"
    } else {
        "new password required at next sign-in"
    };
    record(
        &state,
        Activity::new("password_reset")
            .by(&admin)
            .on(&id, &before.user.username)
            .detail(format!("{detail}; {}", revoked.detail())),
    )
    .await;
    Ok(Json(account_json(&target(&state, &id).await?)))
}

/// `POST /api/admin/accounts/{id}/sign-out`: end every session, personal API token and connected AI app of the
/// account.
pub async fn sign_out_everywhere(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let before = target(&state, &id).await?;
    let ended = state.db.delete_sessions_of(&id).await?;
    state.hub.close_user(&id);
    let revoked = crate::tokens::revoke_all(&state, &id).await?;
    record(
        &state,
        Activity::new("signed_out_everywhere")
            .by(&admin)
            .on(&id, &before.user.username)
            .detail(format!("{ended} sessions, {}", revoked.detail())),
    )
    .await;
    Ok(Json(
        json!({ "ended": ended, "apiTokens": revoked.tokens, "apps": revoked.apps }),
    ))
}

/// `DELETE /api/admin/accounts/{id}`: the account goes, with its sessions, its characters and its content.
///
/// The database row goes first and the folder second. If the second step fails, a folder is left that no account can
/// reach (ids are never reused) and that the server's log names, which wastes disk and nothing else. The other order
/// could leave an account that still signs in after its characters were deleted, or delete the files of an account
/// the last-admin guard then refuses to remove.
pub async fn delete_account(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> ApiResult<()> {
    let before = target(&state, &id).await?;
    let only_admin = before.user.admin
        && before.status == Status::Active
        && state.db.active_admins().await? <= 1;
    if only_admin {
        return Err(last_admin());
    }
    // Its parties' players hear of it (ids only, read before the rows go with the account).
    let departing = crate::parties::before_account_delete(&state, &id).await?;
    if !state.db.delete_user(&id).await? {
        return Err(last_admin());
    }
    state.hub.close_user(&id);
    crate::parties::after_account_delete(&state, &id, departing).await;
    record(
        &state,
        Activity::new("account_deleted")
            .by(&admin)
            .on(&id, &before.user.username),
    )
    .await;
    state.workspaces.remove(&id).await
}

/// `GET /api/admin/stats`.
pub async fn stats(State(state): State<AppState>, _: Admin) -> ApiResult<Json<Value>> {
    let counts = state.db.counts().await?;
    let disk = state.workspaces.usage().await?;
    let mut accounts = json!({ "pending": 0, "active": 0, "rejected": 0, "disabled": 0 });
    let mut total = 0;
    for (status, count) in &counts.accounts {
        accounts[Status::parse(status).as_str()] = json!(count);
        total += count;
    }
    accounts["total"] = json!(total);
    Ok(Json(json!({
        "accounts": accounts,
        "admins": counts.admins,
        "characters": disk.characters,
        "contentPacks": disk.packs,
        "activeSessions": counts.sessions,
        "diskBytes": disk.bytes,
        "version": env!("CARGO_PKG_VERSION"),
        "database": state.db.kind(),
        "uptimeSeconds": state.started.elapsed().as_secs(),
        "email": state.mailer.is_some(),
        // Counts and bytes only (D40).
        "shares": crate::shares::stats(&state.db, &state.workspaces).await?,
        // The newest automatic backup's time and size, and how many are kept (backup.rs). No contents.
        "backup": crate::backup::status(),
    })))
}

/// `GET /api/admin/settings`.
pub async fn settings(State(state): State<AppState>, _: Admin) -> ApiResult<Json<Value>> {
    let current = runtime_settings(&state).await?;
    Ok(Json(json!({
        "openRegistration": current.open_registration,
        "requireApproval": current.require_approval,
        "configFile": {
            "openRegistration": state.config.open_registration,
            "requireApproval": state.config.require_approval,
        },
        "email": state.mailer.is_some(),
        "publicUrl": state.config.public_url,
    })))
}

/// Settings to change; missing ones stay.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsForm {
    #[serde(default)]
    open_registration: Option<bool>,
    #[serde(default)]
    require_approval: Option<bool>,
}

/// `PATCH /api/admin/settings`: saved in the database, in force at once, and winning over `config.toml`.
pub async fn save_settings(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(form): Json<SettingsForm>,
) -> ApiResult<Json<Value>> {
    for (key, value) in [
        (OPEN_REGISTRATION, form.open_registration),
        (REQUIRE_APPROVAL, form.require_approval),
    ] {
        let Some(value) = value else {
            continue;
        };
        state.db.set_setting(key, &value.to_string()).await?;
        record(
            &state,
            Activity::new("setting_changed")
                .by(&admin)
                .detail(format!("{key} = {value}")),
        )
        .await;
    }
    settings(State(state), Admin(admin)).await
}

/// Where the trusted proxy list in force comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxySource {
    /// Saved on the admin page.
    Admin,
    /// `server.trusted_proxies` in `config.toml` or `BOREALIS__SERVER__TRUSTED_PROXIES`.
    Config,
    /// Neither: no proxy is believed.
    None,
}

/// The trusted proxies in force: the admin page's list when one is saved, else the config file's.
#[derive(Debug, Clone)]
pub struct TrustedProxies {
    /// As written.
    pub entries: Vec<String>,
    /// See [`ProxySource`].
    pub source: ProxySource,
}

impl TrustedProxies {
    /// The entries, parsed. Saved ones were checked on the way in; anything that still doesn't parse is skipped.
    #[must_use]
    pub fn ranges(&self) -> Vec<Cidr> {
        self.entries
            .iter()
            .filter_map(|e| Cidr::parse(e).ok())
            .collect()
    }
}

/// The trusted proxies in force. Read for each sign-up and sign-in, so a change applies at once.
pub async fn trusted_proxies(state: &AppState) -> ApiResult<TrustedProxies> {
    if let Some(saved) = state.db.setting(TRUSTED_PROXIES).await? {
        let entries = serde_json::from_str::<Vec<String>>(&saved).unwrap_or_else(|e| {
            tracing::warn!(
                "server_settings.{TRUSTED_PROXIES} is not a list ({e}): trusting no proxy"
            );
            Vec::new()
        });
        return Ok(TrustedProxies {
            entries,
            source: ProxySource::Admin,
        });
    }
    let entries = state.config.trusted_proxies.clone();
    let source = if entries.is_empty() {
        ProxySource::None
    } else {
        ProxySource::Config
    };
    Ok(TrustedProxies { entries, source })
}

/// Check a list from the admin page: each entry an address or a range, none so wide that it trusts strangers.
/// Answers the list trimmed, without blanks and repeats.
///
/// # Errors
/// [`ApiError::Invalid`] naming the first bad entry.
pub fn checked_proxies(list: &[String]) -> ApiResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for entry in list {
        let entry = entry.trim();
        if entry.is_empty() || out.iter().any(|e| e == entry) {
            continue;
        }
        let range = Cidr::parse(entry).map_err(ApiError::Invalid)?;
        if range.bits() == 0 {
            return Err(ApiError::Invalid(format!(
                "`{entry}` would trust every address, so anyone could pretend to be anyone. List only the \
                 proxies you run, such as 172.20.20.100."
            )));
        }
        let widest = if range.is_ipv4() { 8 } else { 32 };
        if range.bits() < widest {
            return Err(ApiError::Invalid(format!(
                "`{entry}` is too wide a range to trust (/{widest} at most). List only the proxies you run."
            )));
        }
        out.push(entry.to_string());
    }
    if out.len() > MAX_PROXIES {
        return Err(ApiError::Invalid(format!(
            "At most {MAX_PROXIES} proxies: use a range for a group of them."
        )));
    }
    Ok(out)
}

fn header_text(headers: &HeaderMap, name: &str) -> Option<String> {
    let values: Vec<&str> = headers
        .get_all(name)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    (!values.is_empty()).then(|| values.join(", "))
}

/// What the admin page shows about the proxies: the list in force, where it comes from, and how the server sees the
/// admin's own request with it.
async fn proxies_json(state: &AppState, peer: SocketAddr, headers: &HeaderMap) -> ApiResult<Value> {
    let trusted = trusted_proxies(state).await?;
    let ranges = trusted.ranges();
    let peer_ip = crate::net_guard::canonical(peer.ip());
    Ok(json!({
        "proxies": trusted.entries,
        "source": trusted.source,
        "configFile": state.config.trusted_proxies,
        "peer": peer_ip.to_string(),
        "peerTrusted": ranges.iter().any(|r| r.contains(peer_ip)),
        "headers": {
            "cfConnectingIp": header_text(headers, "cf-connecting-ip"),
            "xForwardedFor": header_text(headers, "x-forwarded-for"),
        },
        "clientIp": crate::auth::client_ip(peer.ip(), headers, &ranges).to_string(),
    }))
}

/// `GET /api/admin/proxies`.
pub async fn proxies(
    State(state): State<AppState>,
    _: Admin,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    Ok(Json(proxies_json(&state, peer, &headers).await?))
}

/// The trusted proxy form: a list to save, or `null` to go back to `config.toml`.
#[derive(Deserialize)]
pub struct ProxiesForm {
    proxies: Option<Vec<String>>,
}

/// `POST /api/admin/proxies`: saved in the database, in force at once, and winning over `config.toml`. The answer
/// shows the admin's own request under the new list.
pub async fn save_proxies(
    State(state): State<AppState>,
    Admin(admin): Admin,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(form): Json<ProxiesForm>,
) -> ApiResult<Json<Value>> {
    let detail = match form.proxies {
        Some(list) => {
            let list = checked_proxies(&list)?;
            let text = serde_json::to_string(&list)
                .map_err(|e| ApiError::Internal(format!("trusted proxies: {e}")))?;
            state.db.set_setting(TRUSTED_PROXIES, &text).await?;
            if list.is_empty() {
                format!("{TRUSTED_PROXIES} = none")
            } else {
                format!("{TRUSTED_PROXIES} = {}", list.join(", "))
            }
        }
        None => {
            state.db.delete_setting(TRUSTED_PROXIES).await?;
            format!("{TRUSTED_PROXIES} = as in config.toml")
        }
    };
    record(
        &state,
        Activity::new("setting_changed").by(&admin).detail(detail),
    )
    .await;
    Ok(Json(proxies_json(&state, peer, &headers).await?))
}

/// `GET /api/admin/activity`: newest first.
pub async fn activity(
    State(state): State<AppState>,
    _: Admin,
    Query(query): Query<PageQuery>,
) -> ApiResult<Json<Value>> {
    let (limit, offset) = query.limits();
    let (rows, total) = state.db.activity(limit, offset).await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|a| {
            json!({
                "at": a.at,
                "kind": a.kind,
                "actorId": a.actor_id,
                "actorName": a.actor_name,
                "targetId": a.target_id,
                "targetName": a.target_name,
                "detail": a.detail,
                "ip": a.ip,
            })
        })
        .collect();
    Ok(Json(json!({ "items": items, "total": total })))
}

/// The admin page's routes. Each handler checks for an admin itself.
pub fn routes() -> axum::Router<AppState> {
    use axum::routing::{delete, get, post};
    axum::Router::new()
        .route("/api/admin/stats", get(stats))
        .route("/api/admin/settings", get(settings).patch(save_settings))
        .route("/api/admin/proxies", get(proxies).post(save_proxies))
        .route("/api/admin/activity", get(activity))
        .route("/api/admin/trash", get(trash::list))
        .route("/api/admin/trash/restore", post(trash::restore))
        .route("/api/admin/accounts", get(accounts))
        .route("/api/admin/accounts/{id}", delete(delete_account))
        .route("/api/admin/accounts/{id}/status", post(set_status))
        .route("/api/admin/accounts/{id}/admin", post(set_admin))
        .route("/api/admin/accounts/{id}/password", post(reset_password))
        .route(
            "/api/admin/accounts/{id}/sign-out",
            post(sign_out_everywhere),
        )
}
