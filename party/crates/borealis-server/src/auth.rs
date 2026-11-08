//! Accounts and sessions. Clients send `Authorization: Bearer <token>`: the phone app and the browser work the same
//! way, and a request from another site carries no credentials by itself.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use argon2::password_hash::{PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Argon2, PasswordHash};
use axum::extract::{ConnectInfo, FromRef, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap};
use axum::Json;
use rand::RngCore;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::db::{now, Db};
use crate::error::{ApiError, ApiResult};
use crate::AppState;

const SESSION_SECONDS: i64 = 90 * 24 * 60 * 60;
const FAILED_LOGINS_ALLOWED: usize = 10;
const FAILED_LOGIN_WINDOW: Duration = Duration::from_secs(5 * 60);

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

/// Failed sign-ins by client and username.
#[derive(Clone, Default)]
pub struct LoginLimiter(Arc<Mutex<HashMap<String, Vec<Instant>>>>);

impl LoginLimiter {
    fn blocked(&self, key: &str) -> bool {
        let Ok(mut map) = self.0.lock() else {
            return true;
        };
        let cutoff = Instant::now() - FAILED_LOGIN_WINDOW;
        map.retain(|_, times| {
            times.retain(|t| *t > cutoff);
            !times.is_empty()
        });
        map.get(key)
            .is_some_and(|t| t.len() >= FAILED_LOGINS_ALLOWED)
    }

    fn failed(&self, key: &str) {
        if let Ok(mut map) = self.0.lock() {
            map.entry(key.to_string()).or_default().push(Instant::now());
        }
    }

    fn clear(&self, key: &str) {
        if let Ok(mut map) = self.0.lock() {
            map.remove(key);
        }
    }
}

/// Account type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Runs groups and owns the content library.
    Dm,
    /// Owns characters.
    Player,
}

impl Role {
    /// As stored.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dm => "dm",
            Self::Player => "player",
        }
    }

    fn parse(text: &str) -> Self {
        if text == "dm" {
            Self::Dm
        } else {
            Self::Player
        }
    }
}

/// The signed-in user.
#[derive(Debug, Clone, Serialize)]
pub struct User {
    /// Account id.
    pub id: String,
    /// Sign-in and display name.
    pub username: String,
    /// Account type.
    pub role: Role,
}

/// The sign-in form.
#[derive(Deserialize)]
pub struct Credentials {
    username: String,
    password: String,
}

/// The sign-up form.
#[derive(Deserialize)]
pub struct Registration {
    username: String,
    password: String,
    role: Role,
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

fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn validate(username: &str, password: &str) -> ApiResult<()> {
    let valid_name = (2..=40).contains(&username.chars().count())
        && username
            .chars()
            .all(|c| c.is_alphanumeric() || "._-".contains(c));
    if !valid_name {
        return Err(ApiError::Invalid(
            "Usernames are 2 to 40 letters, digits, '.', '_' or '-'.".into(),
        ));
    }
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

/// Create an account.
pub async fn create_user(db: &Db, username: &str, password: &str, role: Role) -> ApiResult<User> {
    let username = username.trim().to_string();
    validate(&username, password)?;
    let password = password.to_string();
    let hash = hashing(move || hash_now(&password)).await?;
    let user = User {
        id: uuid::Uuid::new_v4().to_string(),
        username,
        role,
    };
    let out = user.clone();
    db.run(move |c| {
        let inserted = c.execute(
            "INSERT OR IGNORE INTO users (id, username, password_hash, role, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![user.id, user.username, hash, user.role.as_str(), now()],
        )?;
        if inserted == 0 {
            return Err(ApiError::Conflict("That username is taken.".into()));
        }
        Ok(())
    })
    .await?;
    Ok(out)
}

async fn start_session(db: &Db, user: User) -> ApiResult<SessionOut> {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let hash = token_hash(&token);
    let user_id = user.id.clone();
    db.run(move |c| {
        c.execute("DELETE FROM sessions WHERE expires_at < ?1", params![now()])?;
        c.execute(
            "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)",
            params![hash, user_id, now() + SESSION_SECONDS],
        )?;
        Ok(())
    })
    .await?;
    Ok(SessionOut { token, user })
}

/// `POST /api/auth/register`.
pub async fn register(
    State(state): State<AppState>,
    Json(form): Json<Registration>,
) -> ApiResult<Json<SessionOut>> {
    if !state.config.open_registration {
        return Err(ApiError::Forbidden(
            "Sign-up is closed on this server.".into(),
        ));
    }
    let user = create_user(&state.db, &form.username, &form.password, form.role).await?;
    Ok(Json(start_session(&state.db, user).await?))
}

/// `POST /api/auth/login`.
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(form): Json<Credentials>,
) -> ApiResult<Json<SessionOut>> {
    let username = form.username.trim().to_string();
    let key = format!("{}|{}", peer.ip(), username.to_lowercase());
    if state.login_limiter.blocked(&key) {
        return Err(ApiError::TooMany);
    }
    let lookup = username.clone();
    let found = state
        .db
        .run(move |c| {
            Ok(c.query_row(
                "SELECT id, username, role, password_hash FROM users WHERE username = ?1",
                params![lookup],
                |r| {
                    Ok((
                        User {
                            id: r.get(0)?,
                            username: r.get(1)?,
                            role: Role::parse(&r.get::<_, String>(2)?),
                        },
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?)
        })
        .await?;
    let (user, hash) = found.map_or((None, None), |(u, h)| (Some(u), Some(h)));
    let password = form.password;
    let matches = hashing(move || {
        let hash = hash.unwrap_or_else(|| DUMMY_HASH.clone());
        PasswordHash::new(&hash).is_ok_and(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
    })
    .await?;
    match user {
        Some(user) if matches => {
            state.login_limiter.clear(&key);
            Ok(Json(start_session(&state.db, user).await?))
        }
        _ => {
            state.login_limiter.failed(&key);
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
        let hash = token_hash(token);
        state
            .db
            .run(move |c| {
                c.execute("DELETE FROM sessions WHERE token_hash = ?1", params![hash])?;
                Ok(())
            })
            .await?;
    }
    Ok(())
}

/// `GET /api/me`.
pub async fn me(user: User) -> Json<User> {
    Json(user)
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
        let hash = token_hash(token);
        state
            .db
            .run(move |c| {
                Ok(c.query_row(
                    "SELECT u.id, u.username, u.role FROM sessions s JOIN users u ON u.id = s.user_id
                     WHERE s.token_hash = ?1 AND s.expires_at > ?2",
                    params![hash, now()],
                    |r| {
                        Ok(User {
                            id: r.get(0)?,
                            username: r.get(1)?,
                            role: Role::parse(&r.get::<_, String>(2)?),
                        })
                    },
                )
                .optional()?)
            })
            .await?
            .ok_or_else(unauthorized)
    }
}
