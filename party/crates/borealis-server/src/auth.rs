//! Accounts and sessions. Clients send `Authorization: Bearer <token>`: the phone app and the browser work the same
//! way, and a request from another site carries no credentials by itself.
//!
//! Every account can run groups and play in them; what someone may do in a group comes from the group. New accounts
//! wait for an admin (unless the admin page says otherwise), and with email set up they confirm their address first.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Argon2, PasswordHash};
use axum::extract::{ConnectInfo, FromRef, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap};
use axum::response::Html;
use axum::Json;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::admin::{record, runtime_settings};
use crate::db::{now, Activity, Db, InsertOutcome, NewAccount};
use crate::error::{ApiError, ApiResult};
use crate::AppState;

const SESSION_SECONDS: i64 = 90 * 24 * 60 * 60;
/// How long the emailed link works.
const CONFIRM_SECONDS: i64 = 48 * 60 * 60;
/// Failed sign-ins allowed per client and username in [`FAILED_LOGIN_WINDOW`].
pub const FAILED_LOGINS_ALLOWED: usize = 10;
/// See [`FAILED_LOGINS_ALLOWED`].
pub const FAILED_LOGIN_WINDOW: Duration = Duration::from_secs(5 * 60);
/// Sign-up attempts allowed per client address in [`SIGNUP_WINDOW`].
pub const SIGNUPS_ALLOWED: usize = 10;
/// See [`SIGNUPS_ALLOWED`].
pub const SIGNUP_WINDOW: Duration = Duration::from_secs(60 * 60);

/// Requests a user whose password has to be changed may still make.
const WHILE_CHANGING_PASSWORD: [&str; 2] = ["/api/me", "/api/me/password"];

/// Argon2 takes tens of megabytes per hash: only a few run at once.
static HASHING: Semaphore = Semaphore::const_new(4);

/// Verified against when the username is unknown, so both cases take the same time.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| hash_now("borealis-dummy-password"));

static COMMON_PASSWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    include_str!("common-passwords.txt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
});

/// Counts attempts per key in a sliding window: failed sign-ins per client and username, sign-ups per client.
#[derive(Clone)]
pub struct RateLimiter {
    attempts: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    allowed: usize,
    window: Duration,
}

impl RateLimiter {
    /// `allowed` attempts per key in `window`.
    pub fn new(allowed: usize, window: Duration) -> Self {
        Self {
            attempts: Arc::default(),
            allowed,
            window,
        }
    }

    /// The limiter for failed sign-ins.
    pub fn logins() -> Self {
        Self::new(FAILED_LOGINS_ALLOWED, FAILED_LOGIN_WINDOW)
    }

    /// The limiter for sign-ups.
    pub fn signups() -> Self {
        Self::new(SIGNUPS_ALLOWED, SIGNUP_WINDOW)
    }

    fn blocked(&self, key: &str) -> bool {
        let Ok(mut map) = self.attempts.lock() else {
            return true;
        };
        let cutoff = Instant::now().checked_sub(self.window);
        map.retain(|_, times| {
            times.retain(|t| cutoff.is_none_or(|c| *t > c));
            !times.is_empty()
        });
        map.get(key).is_some_and(|t| t.len() >= self.allowed)
    }

    fn count(&self, key: &str) {
        if let Ok(mut map) = self.attempts.lock() {
            map.entry(key.to_string()).or_default().push(Instant::now());
        }
    }

    fn clear(&self, key: &str) {
        if let Ok(mut map) = self.attempts.lock() {
            map.remove(key);
        }
    }
}

/// Whether an account may sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Waiting for an admin, or for its email address to be confirmed.
    Pending,
    /// In use.
    Active,
    /// Turned down by an admin.
    Rejected,
    /// Switched off by an admin.
    Disabled,
}

impl Status {
    /// As stored.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Rejected => "rejected",
            Self::Disabled => "disabled",
        }
    }

    /// From storage; anything unknown keeps the account out.
    pub fn parse(text: &str) -> Self {
        match text {
            "active" => Self::Active,
            "rejected" => Self::Rejected,
            "pending" => Self::Pending,
            _ => Self::Disabled,
        }
    }

    /// Why sign-in is refused, or `None` when it isn't.
    fn refusal(self, awaiting_email: bool) -> Option<ApiError> {
        let (code, message) = match self {
            Self::Active => return None,
            Self::Pending if awaiting_email => (
                "email_unconfirmed",
                "Confirm your email address first: open the link in the email we sent you.",
            ),
            Self::Pending => (
                "account_pending",
                "Your account is waiting for an admin to approve it.",
            ),
            Self::Rejected => (
                "account_rejected",
                "An admin turned down this account request.",
            ),
            Self::Disabled => ("account_disabled", "This account has been disabled."),
        };
        Some(ApiError::Refused(code, message.into()))
    }
}

/// The signed-in user.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    /// Account id.
    pub id: String,
    /// Sign-in and display name.
    pub username: String,
    /// Email address; accounts from before email was asked for may have none.
    pub email: Option<String>,
    /// Runs the server: the admin page.
    pub admin: bool,
    /// Has to choose a new password before doing anything else.
    pub must_change_password: bool,
}

/// The sign-in form.
#[derive(Deserialize)]
pub struct Credentials {
    /// Username or email address.
    username: String,
    password: String,
}

/// The sign-up form.
#[derive(Deserialize)]
pub struct Registration {
    username: String,
    email: String,
    password: String,
}

/// A successful sign-in.
#[derive(Serialize)]
pub struct SessionOut {
    token: String,
    user: User,
}

fn hash_now(password: &str) -> String {
    let mut random = [0u8; 16];
    rand::rng().fill_bytes(&mut random);
    let salt = SaltString::encode_b64(&random).expect("encoding a salt");
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .expect("hashing a password")
}

async fn hashing<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    let _permit = HASHING
        .acquire()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))
}

/// Hash a password off the async threads.
pub async fn hash_password(password: &str) -> ApiResult<String> {
    let password = password.to_string();
    hashing(move || hash_now(&password)).await
}

async fn verify_password(password: String, hash: Option<String>) -> ApiResult<bool> {
    hashing(move || {
        let hash = hash.unwrap_or_else(|| DUMMY_HASH.clone());
        PasswordHash::new(&hash).is_ok_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
    })
    .await
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex(&bytes)
}

fn validate_username(username: &str) -> ApiResult<()> {
    let valid_name = (2..=40).contains(&username.chars().count())
        && username
            .chars()
            .all(|c| c.is_alphanumeric() || "._-".contains(c));
    if valid_name {
        Ok(())
    } else {
        Err(ApiError::Invalid(
            "Usernames are 2 to 40 letters, digits, '.', '_' or '-'.".into(),
        ))
    }
}

/// Check a new password.
///
/// # Errors
/// When it is too short, too long or too easy to guess.
pub fn validate_password(username: &str, password: &str) -> ApiResult<()> {
    if password.len() < 8 || password.len() > 1024 {
        return Err(ApiError::Invalid(
            "Passwords are 8 to 1024 characters long.".into(),
        ));
    }
    let lower = password.to_lowercase();
    if COMMON_PASSWORDS.contains(lower.as_str()) || lower == username.to_lowercase() {
        return Err(ApiError::Invalid(
            "That password is too easy to guess. Choose another.".into(),
        ));
    }
    Ok(())
}

/// An email address as typed, trimmed, or an error when it can't be one.
///
/// # Errors
/// When it isn't an address.
pub fn validate_email(email: &str) -> ApiResult<String> {
    let email = email.trim();
    let valid = email.len() <= 254
        && !email.contains(['<', '>', ',', ' '])
        && email.parse::<lettre::Address>().is_ok();
    if valid {
        Ok(email.to_string())
    } else {
        Err(ApiError::Invalid("Enter a valid email address.".into()))
    }
}

/// An account to create.
pub struct AccountSpec<'a> {
    /// Username.
    pub username: &'a str,
    /// Email address, if any.
    pub email: Option<&'a str>,
    /// Password.
    pub password: &'a str,
    /// Where it starts; the first account on a server is always an active admin.
    pub status: Status,
    /// Whether it is an admin.
    pub admin: bool,
    /// Hash of the confirmation link's token, when one is emailed.
    pub confirm_token_hash: Option<&'a str>,
}

/// Create an account.
///
/// # Errors
/// When the details aren't acceptable or are taken.
pub async fn create_account(db: &Db, spec: AccountSpec<'_>) -> ApiResult<(User, Status)> {
    let username = spec.username.trim().to_string();
    validate_username(&username)?;
    validate_password(&username, spec.password)?;
    let email = spec.email.map(validate_email).transpose()?;
    let hash = hash_password(spec.password).await?;
    let first = db.user_count().await? == 0;
    let (status, admin) = if first {
        (Status::Active, true)
    } else {
        (spec.status, spec.admin)
    };
    let confirm = spec.confirm_token_hash.filter(|_| !first);
    let id = uuid::Uuid::new_v4().to_string();
    let added = db
        .insert_user(&NewAccount {
            id: &id,
            username: &username,
            email: email.as_deref(),
            password_hash: &hash,
            status,
            admin,
            verify_token_hash: confirm,
            verify_expires_at: confirm.map(|_| now() + CONFIRM_SECONDS),
        })
        .await?;
    match added {
        InsertOutcome::Added => Ok((
            User {
                id,
                username,
                email,
                admin,
                must_change_password: false,
            },
            status,
        )),
        InsertOutcome::UsernameTaken => Err(ApiError::Conflict("That username is taken.".into())),
        InsertOutcome::EmailTaken => Err(ApiError::Conflict(
            "An account with that email address exists already.".into(),
        )),
    }
}

async fn start_session(db: &Db, user: User) -> ApiResult<SessionOut> {
    let token = new_token();
    db.insert_session(&token_hash(&token), &user.id, now() + SESSION_SECONDS)
        .await?;
    Ok(SessionOut { token, user })
}

/// Email every admin about a request waiting for them, in the background.
pub fn tell_admins(state: &AppState, username: &str, email: &str) {
    let (Some(mailer), Some(base)) = (state.mailer.clone(), state.config.public_url.clone()) else {
        return;
    };
    let db = state.db.clone();
    let (username, email) = (username.to_string(), email.to_string());
    tokio::spawn(async move {
        let admins = match db.admin_emails().await {
            Ok(admins) => admins,
            Err(e) => {
                tracing::warn!("finding admins to email: {e:?}");
                return;
            }
        };
        let (subject, body) =
            crate::mail::approval_needed(&username, &email, &format!("{base}/#/admin"));
        for admin in admins {
            if let Err(e) = mailer.send(&admin, &subject, body.clone()).await {
                tracing::warn!("emailing an admin about {username}: {e}");
            }
        }
    });
}

/// `POST /api/auth/register`: ask for an account. Active straight away, it comes back signed in; otherwise it says
/// what it is waiting for.
pub async fn register(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(form): Json<Registration>,
) -> ApiResult<Json<Value>> {
    let ip = peer.ip().to_string();
    if state.signup_limiter.blocked(&ip) {
        return Err(ApiError::TooMany);
    }
    state.signup_limiter.count(&ip);
    let settings = runtime_settings(&state).await?;
    // Held across the count and the insert, so only one account can be the first.
    let _guard = state.accounts.lock().await;
    let first = state.db.user_count().await? == 0;
    if !settings.open_registration && !first {
        return Err(ApiError::Forbidden(
            "Sign-up is closed on this server.".into(),
        ));
    }
    let email = validate_email(&form.email)?;
    let token = state.mailer.as_ref().map(|_| new_token());
    let hash = token.as_deref().map(token_hash);
    let status = if token.is_some() || settings.require_approval {
        Status::Pending
    } else {
        Status::Active
    };
    let (user, status) = create_account(
        &state.db,
        AccountSpec {
            username: &form.username,
            email: Some(&email),
            password: &form.password,
            status,
            admin: false,
            confirm_token_hash: hash.as_deref(),
        },
    )
    .await?;
    drop(_guard);
    record(
        &state,
        Activity::new("sign_up")
            .by(&user)
            .client(&ip)
            .detail(if user.admin {
                "first account: admin".to_string()
            } else {
                status.as_str().to_string()
            }),
    )
    .await;
    if status == Status::Active {
        let session = start_session(&state.db, user).await?;
        return Ok(Json(
            json!({ "status": "active", "token": session.token, "user": session.user }),
        ));
    }
    if let (Some(token), Some(mailer), Some(base)) = (
        token,
        state.mailer.as_ref(),
        state.config.public_url.as_ref(),
    ) {
        let link = format!("{base}/api/auth/confirm/{token}");
        let (subject, body) = crate::mail::confirmation(&user.username, &link);
        if let Err(e) = mailer.send(&email, &subject, body).await {
            tracing::warn!("emailing {} their confirmation link: {e}", user.username);
            return Ok(Json(json!({
                "status": "pending",
                "awaitingEmail": true,
                "message": "Your account was created, but the confirmation email could not be sent. Ask an admin to approve it.",
            })));
        }
        return Ok(Json(json!({
            "status": "pending",
            "awaitingEmail": true,
            "message": format!("We sent a link to {email}. Open it to confirm your address{}.",
                if settings.require_approval { "; then an admin will approve your account" } else { "" }),
        })));
    }
    tell_admins(&state, &user.username, &email);
    Ok(Json(json!({
        "status": "pending",
        "awaitingEmail": false,
        "message": "Your account is waiting for an admin to approve it. Try signing in later.",
    })))
}

fn page(title: &str, text: &str) -> Html<String> {
    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" \
         content=\"width=device-width, initial-scale=1\"><title>{title}</title></head>\
         <body style=\"font-family: system-ui, sans-serif; max-width: 32rem; margin: 4rem auto; padding: 0 16px; \
         line-height: 1.5\"><h1 style=\"font-size: 1.4rem\">{title}</h1><p>{text}</p>\
         <p><a href=\"/\">Open Borealis Party</a></p></body></html>"
    ))
}

/// `GET /api/auth/confirm/{token}`: the link in the confirmation email.
pub async fn confirm(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> ApiResult<Html<String>> {
    let Some(account) = state.db.confirm_email(&token_hash(&token)).await? else {
        return Ok(page(
            "That link doesn't work",
            "It may have been used already, or be more than 48 hours old. Try signing in; if your address is \
             still unconfirmed, ask an admin to approve your account.",
        ));
    };
    record(&state, Activity::new("email_confirmed").by(&account.user)).await;
    if account.status != Status::Pending {
        return Ok(page("Email address confirmed", "You can sign in."));
    }
    if runtime_settings(&state).await?.require_approval {
        tell_admins(
            &state,
            &account.user.username,
            account.user.email.as_deref().unwrap_or_default(),
        );
        return Ok(page(
            "Email address confirmed",
            "Thank you. An admin will now look at your request; you can sign in once it is approved.",
        ));
    }
    state
        .db
        .set_status(&account.user.id, Status::Active)
        .await?;
    Ok(page(
        "Email address confirmed",
        "Your account is ready: you can sign in.",
    ))
}

/// `POST /api/auth/login`.
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(form): Json<Credentials>,
) -> ApiResult<Json<SessionOut>> {
    let name = form.username.trim().to_string();
    let ip = peer.ip().to_string();
    let key = format!("{ip}|{}", name.to_lowercase());
    if state.login_limiter.blocked(&key) {
        return Err(ApiError::TooMany);
    }
    let found = state.db.find_login(&name).await?;
    let hash = found.as_ref().map(|f| f.password_hash.clone());
    let matches = verify_password(form.password, hash).await?;
    match found {
        Some(login) if matches => {
            state.login_limiter.clear(&key);
            if let Some(refused) = login.status.refusal(login.awaiting_email) {
                return Err(refused);
            }
            record(&state, Activity::new("sign_in").by(&login.user).client(&ip)).await;
            Ok(Json(start_session(&state.db, login.user).await?))
        }
        found => {
            state.login_limiter.count(&key);
            let mut entry = Activity::new("sign_in_failed").client(&ip);
            entry = match &found {
                Some(login) => entry.on(&login.user.id, &login.user.username),
                None => entry.detail(format!("unknown name {name}")),
            };
            record(&state, entry).await;
            Err(ApiError::Unauthorized(
                "That username and password don't match.".into(),
            ))
        }
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// `POST /api/auth/logout`.
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<()> {
    if let Some(token) = bearer(&headers) {
        state.db.delete_session(&token_hash(token)).await?;
    }
    Ok(())
}

/// `GET /api/me`.
pub async fn me(user: User) -> Json<User> {
    Json(user)
}

/// The form for changing one's own details.
#[derive(Deserialize)]
pub struct MeForm {
    email: String,
}

/// `PATCH /api/me`: change one's email address.
pub async fn update_me(
    State(state): State<AppState>,
    user: User,
    Json(form): Json<MeForm>,
) -> ApiResult<Json<User>> {
    let email = validate_email(&form.email)?;
    if !state.db.set_email(&user.id, Some(&email)).await? {
        return Err(ApiError::Conflict(
            "An account with that email address exists already.".into(),
        ));
    }
    record(&state, Activity::new("email_changed").by(&user)).await;
    Ok(Json(User {
        email: Some(email),
        ..user
    }))
}

/// The password form.
#[derive(Deserialize)]
pub struct PasswordForm {
    current: String,
    password: String,
}

/// `POST /api/me/password`: choose a new password; every other session of the account ends.
pub async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    user: User,
    Json(form): Json<PasswordForm>,
) -> ApiResult<Json<User>> {
    let hash = state.db.password_hash(&user.id).await?;
    if !verify_password(form.current, hash).await? {
        return Err(ApiError::Invalid(
            "Your current password isn't right.".into(),
        ));
    }
    validate_password(&user.username, &form.password)?;
    let hash = hash_password(&form.password).await?;
    state.db.set_password(&user.id, Some(&hash), false).await?;
    if let Some(token) = bearer(&headers) {
        state
            .db
            .delete_other_sessions(&user.id, &token_hash(token))
            .await?;
    }
    record(&state, Activity::new("password_changed").by(&user)).await;
    Ok(Json(User {
        must_change_password: false,
        ..user
    }))
}

impl<S> FromRequestParts<S> for User
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        let state = AppState::from_ref(state);
        let unauthorized = || ApiError::Unauthorized("Please sign in.".into());
        let token = bearer(&parts.headers).ok_or_else(unauthorized)?;
        let user = state
            .db
            .session_user(&token_hash(token))
            .await?
            .ok_or_else(unauthorized)?;
        if user.must_change_password && !WHILE_CHANGING_PASSWORD.contains(&parts.uri.path()) {
            return Err(ApiError::Refused(
                "password_change_required",
                "Choose a new password first.".into(),
            ));
        }
        Ok(user)
    }
}
