//! The admin page: accounts waiting for approval, account management, statistics, runtime settings and the activity
//! log. Every route here answers 403 to anyone but an active admin.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{hash_password, validate_password, Status, User};
use crate::db::{now, AccountRow, Activity};
use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// Keys in `server_settings`.
const OPEN_REGISTRATION: &str = "open_registration";
const REQUIRE_APPROVAL: &str = "require_approval";

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
            .detail(detail),
    )
    .await;
    Ok(Json(account_json(&target(&state, &id).await?)))
}

/// `POST /api/admin/accounts/{id}/sign-out`: end every session of the account.
pub async fn sign_out_everywhere(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let before = target(&state, &id).await?;
    let ended = state.db.delete_sessions_of(&id).await?;
    record(
        &state,
        Activity::new("signed_out_everywhere")
            .by(&admin)
            .on(&id, &before.user.username)
            .detail(format!("{ended} sessions")),
    )
    .await;
    Ok(Json(json!({ "ended": ended })))
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
    if !state.db.delete_user(&id).await? {
        return Err(last_admin());
    }
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
        .route("/api/admin/activity", get(activity))
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
