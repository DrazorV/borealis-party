//! Credentials for AI apps (0.3.1, plan 4.5a part A): personal API tokens (`bpt_…`) for CLI and desktop MCP clients,
//! and [`TokenAuth`], the extractor the MCP server (and later the HTTP API) uses for both those and the OAuth access
//! tokens of `oauth.rs` (`bpa_…`). See `party/docs/ai-auth.md`.
//!
//! - Every secret is stored only as HMAC-SHA256 under a server pepper (`<config dir>/ai-token-pepper`), so neither a
//!   database dump nor a log holds anything that works. The plaintext is shown once, at creation.
//! - A token carries scopes, never a role: every call re-reads the account (it must be active), and the party code
//!   re-checks DM and membership as for a session. So a token can never do more than its account.
//! - Session tokens are refused here, and these tokens are refused everywhere a session is asked for (they are not
//!   sessions), so `bpt_`/`bpa_` reach only the routes that take [`TokenAuth`].
//! - `BOREALIS__FEATURES__AI=false` (server config, D40) turns all of it off: 404 everywhere.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{LazyLock, Mutex, OnceLock};

use axum::extract::{FromRef, FromRequestParts, Path as UrlPath, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::Engine as _;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::admin::record;
use crate::auth::{bearer, User};
use crate::db::{now, Activity};
use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// Prefix of a personal API token.
pub const PERSONAL_PREFIX: &str = "bpt_";
/// Prefix of an OAuth access token (`oauth.rs`).
pub const ACCESS_PREFIX: &str = "bpa_";
/// Prefix of an OAuth refresh token (`oauth.rs`).
pub const REFRESH_PREFIX: &str = "bpr_";

/// Days a new personal token lasts unless the account picks otherwise (not sliding).
pub const DEFAULT_DAYS: i64 = 90;
/// The longest a personal token may last.
pub const MAX_DAYS: i64 = 365;
/// Live personal tokens an account may hold.
pub const MAX_TOKENS: i64 = 25;
/// Calls per token (or per connected app) per [`CALL_WINDOW`] seconds.
pub const CALLS_PER_MINUTE: usize = 60;
/// See [`CALLS_PER_MINUTE`].
pub const CALL_WINDOW: i64 = 60;
/// `last_used_at` is written at most this often per token, so most calls write nothing.
const TOUCH_SECONDS: i64 = 60;
const DAY: i64 = 24 * 60 * 60;

// Scopes.

/// What a token may be used for. The role checks still apply on top: `encounters:write` lets a token act only where
/// its account is the party's DM. There is no scope for live combat (D25: AI apps get no access to active combats
/// in 0.3.1) and none for publishing (D47: the app never hosts or publishes indexes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// Search and read the account's own bestiary.
    BestiaryRead,
    /// Encounters: the DM view in the account's own parties, the player projection elsewhere.
    EncountersRead,
    /// Create and edit encounter drafts in parties the account runs as DM.
    EncountersWrite,
    /// Dry-run simulations of an encounter the account runs as DM (D26): on copies, never writing.
    EncountersSimulate,
    /// The account's own homebrew content.
    HomebrewRead,
    /// Write the account's own homebrew content.
    HomebrewWrite,
    /// The account's own characters, and the party rosters it may see.
    CharactersRead,
}

impl Scope {
    /// Every scope, in display order.
    pub const ALL: [Self; 7] = [
        Self::BestiaryRead,
        Self::EncountersRead,
        Self::EncountersWrite,
        Self::EncountersSimulate,
        Self::HomebrewRead,
        Self::HomebrewWrite,
        Self::CharactersRead,
    ];

    /// As written in tokens and on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BestiaryRead => "bestiary:read",
            Self::EncountersRead => "encounters:read",
            Self::EncountersWrite => "encounters:write",
            Self::EncountersSimulate => "encounters:simulate",
            Self::HomebrewRead => "homebrew:read",
            Self::HomebrewWrite => "homebrew:write",
            Self::CharactersRead => "characters:read",
        }
    }

    /// The scope this one includes: a write scope includes its read scope.
    #[must_use]
    pub fn implies(self) -> Option<Self> {
        match self {
            Self::EncountersWrite => Some(Self::EncountersRead),
            Self::HomebrewWrite => Some(Self::HomebrewRead),
            _ => None,
        }
    }

    /// From its name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == text)
    }

    /// For the consent screen and the Account page.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::BestiaryRead => "Search and read your bestiary",
            Self::EncountersRead => "Read encounters (players see only what the DM shows them)",
            Self::EncountersWrite => "Create and edit encounter drafts in parties you run",
            Self::EncountersSimulate => {
                "Simulate encounters in parties you run (on copies: nothing changes)"
            }
            Self::HomebrewRead => "Read your homebrew content",
            Self::HomebrewWrite => "Create and edit your homebrew content",
            Self::CharactersRead => "Read your characters and the party rosters you can see",
        }
    }

    /// Ticked by default for new tokens and on the consent screen. Every scope is today; the flag stays so a
    /// riskier scope added later can start unticked.
    #[must_use]
    pub fn default_on(self) -> bool {
        true
    }
}

/// A set of scopes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scopes(BTreeSet<Scope>);

impl Scopes {
    /// The default for new tokens and grants: the scopes that are [`Scope::default_on`].
    #[must_use]
    pub fn defaults() -> Self {
        Self(Scope::ALL.into_iter().filter(|s| s.default_on()).collect())
    }

    /// Every scope.
    #[must_use]
    pub fn all() -> Self {
        Self(Scope::ALL.into_iter().collect())
    }

    /// Whether the set grants `scope`, itself or through a scope that includes it (write includes read).
    #[must_use]
    pub fn has(&self, scope: Scope) -> bool {
        self.0
            .iter()
            .any(|s| *s == scope || s.implies() == Some(scope))
    }

    /// Whether `other` grants every scope of `self` (write includes read).
    #[must_use]
    pub fn within(&self, other: &Self) -> bool {
        self.0.iter().all(|s| other.has(*s))
    }

    /// True when empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The scopes, in display order.
    pub fn iter(&self) -> impl Iterator<Item = Scope> + '_ {
        self.0.iter().copied()
    }

    /// Space-separated, as stored and as OAuth writes them.
    #[must_use]
    pub fn to_text(&self) -> String {
        self.iter().map(Scope::as_str).collect::<Vec<_>>().join(" ")
    }

    /// From a space-separated list. Names in `ignored` are skipped (OAuth's `offline_access`, say); any other unknown
    /// name is an error naming it.
    ///
    /// # Errors
    /// The first unknown name.
    pub fn parse(text: &str, ignored: &[&str]) -> Result<Self, String> {
        let mut set = BTreeSet::new();
        for word in text.split_ascii_whitespace() {
            if ignored.contains(&word) {
                continue;
            }
            match Scope::parse(word) {
                Some(s) => {
                    set.insert(s);
                }
                None => return Err(word.chars().take(64).collect()),
            }
        }
        Ok(Self(set))
    }

    /// As stored; anything unknown (a scope from a newer version after a rollback) is dropped.
    #[must_use]
    pub fn stored(text: &str) -> Self {
        Self(
            text.split_ascii_whitespace()
                .filter_map(Scope::parse)
                .collect(),
        )
    }

    /// From a JSON list of names.
    ///
    /// # Errors
    /// An unknown name.
    pub fn from_names(names: &[String]) -> ApiResult<Self> {
        let mut set = BTreeSet::new();
        for name in names {
            let scope = Scope::parse(name)
                .ok_or_else(|| ApiError::Invalid(format!("There is no scope {name:?}.")))?;
            set.insert(scope);
        }
        Ok(Self(set))
    }

    /// As a JSON list of names.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.iter().map(Scope::as_str).collect()
    }
}

/// The scope catalog for the UI.
#[must_use]
pub fn catalog() -> Value {
    Value::Array(
        Scope::ALL
            .iter()
            .map(|s| json!({ "id": s.as_str(), "label": s.label(), "default": s.default_on() }))
            .collect(),
    )
}

// Secrets and hashes.

static PEPPER: OnceLock<[u8; 32]> = OnceLock::new();

/// The file under the config folder that holds the pepper.
pub const PEPPER_FILE: &str = "ai-token-pepper";

/// Read the pepper from `<config dir>/ai-token-pepper`, making it on first start. Without it every token stops
/// working, so the folder must persist (it does: it holds `config.toml`). Called once at start-up.
///
/// # Errors
/// When the file exists but can't be read or is malformed, or can't be created.
pub fn load_pepper(config_dir: &Path) -> Result<(), String> {
    let path = config_dir.join(PEPPER_FILE);
    let bytes = match std::fs::read_to_string(&path) {
        Ok(text) => base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .map_err(|_| format!("{} is not base64", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut fresh = [0u8; 32];
            rand::rng().fill_bytes(&mut fresh);
            write_private(
                &path,
                &base64::engine::general_purpose::STANDARD.encode(fresh),
            )
            .map_err(|e| format!("creating {}: {e}", path.display()))?;
            tracing::info!(
                "created {} (the key API tokens are hashed with; keep it with config.toml)",
                path.display()
            );
            fresh.to_vec()
        }
        Err(e) => return Err(format!("reading {}: {e}", path.display())),
    };
    let key: [u8; 32] = bytes
        .try_into()
        .map_err(|_| format!("{} must hold 32 bytes", path.display()))?;
    let _ = PEPPER.set(key);
    Ok(())
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

fn pepper() -> &'static [u8; 32] {
    // Tests build the routes without start-up: a key for this process only.
    PEPPER.get_or_init(|| {
        let mut fresh = [0u8; 32];
        rand::rng().fill_bytes(&mut fresh);
        fresh
    })
}

/// HMAC-SHA256 (RFC 2104) of `message` under `key`.
fn hmac_sha256(key: &[u8; 32], message: &[u8]) -> [u8; 32] {
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for (i, k) in key.iter().enumerate() {
        ipad[i] ^= k;
        opad[i] ^= k;
    }
    let inner = Sha256::new()
        .chain_update(ipad)
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(opad)
        .chain_update(inner)
        .finalize()
        .into()
}

/// The stored form of a secret (a token, code, client secret or consent id): hex HMAC-SHA256 under the pepper.
#[must_use]
pub fn secret_hash(secret: &str) -> String {
    hmac_sha256(pepper(), secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A new secret: `prefix` and 32 random bytes, base64url without padding (43 characters).
#[must_use]
pub fn new_secret(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!(
        "{prefix}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

/// Equal without an early exit, so the time taken says nothing about where two values differ.
#[must_use]
pub fn same(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// Limits.

static WINDOWS: LazyLock<Mutex<HashMap<String, Vec<i64>>>> = LazyLock::new(Mutex::default);

/// Count a call under `key`; false (and not counted) when `allowed` were made in the last `window` seconds. Shared
/// with `oauth.rs` for its per-client-address limits.
pub(crate) fn take(key: &str, allowed: usize, window: i64) -> bool {
    let Ok(mut map) = WINDOWS.lock() else {
        return false;
    };
    let at = now();
    if map.len() > 10_000 {
        map.retain(|_, times| {
            times.retain(|t| *t > at - DAY);
            !times.is_empty()
        });
    }
    let times = map.entry(key.to_string()).or_default();
    times.retain(|t| *t > at - window.max(1));
    if times.len() >= allowed {
        return false;
    }
    times.push(at);
    true
}

// The MCP resource.

/// The MCP endpoint's canonical URL (the OAuth resource and the tokens' audience), or `None` without
/// `server.public_url`.
#[must_use]
pub fn mcp_resource(state: &AppState) -> Option<String> {
    state
        .config
        .public_url
        .as_ref()
        .map(|base| format!("{base}/api/mcp"))
}

/// Where the MCP endpoint's protected-resource metadata is (RFC 9728, path form).
#[must_use]
pub fn resource_metadata_url(state: &AppState) -> Option<String> {
    state
        .config
        .public_url
        .as_ref()
        .map(|base| format!("{base}/.well-known/oauth-protected-resource/api/mcp"))
}

// The extractor.

/// Which credential a [`TokenAuth`] came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenClient {
    /// A personal API token from the Account page.
    Personal {
        /// The token's id (not the secret).
        token_id: String,
        /// The name the account gave it.
        name: String,
    },
    /// An OAuth app the account connected (ChatGPT, claude.ai, …).
    OAuth {
        /// The connected app (grant) id.
        grant_id: String,
        /// The registered client id.
        client_id: String,
        /// The name the app registered with (not verified).
        client_name: String,
    },
}

impl TokenClient {
    /// A short label for logs and activity: never a secret.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Personal { token_id, .. } => format!("api token {token_id}"),
            Self::OAuth { grant_id, .. } => format!("app {grant_id}"),
        }
    }

    /// The token's id, or the connected app's grant id: stable across an app's token refreshes, never a secret.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Personal { token_id, .. } => token_id,
            Self::OAuth { grant_id, .. } => grant_id,
        }
    }
}

/// A request authorised by a personal API token or an OAuth access token.
///
/// `account` is a plain [`User`]: pass it to the same handlers and permission code the session routes use, so the
/// role checks (DM of the party, member, owner) and the player projections apply unchanged. Then check the scope the
/// action needs with [`TokenAuth::require`] before acting.
#[derive(Debug, Clone)]
pub struct TokenAuth {
    /// The active account the token belongs to, read from the database on this request.
    pub account: User,
    /// What the token may be used for.
    pub scopes: Scopes,
    /// Which token or app.
    pub client: TokenClient,
}

impl TokenAuth {
    /// The scope names the token was given, e.g. `["bestiary:read", "encounters:write"]`.
    #[must_use]
    pub fn scope_names(&self) -> Vec<&'static str> {
        self.scopes.names()
    }

    /// Whether the token has `scope` (write includes read).
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "/api/mcp checks scopes in party-tools; these serve token routes that check one scope themselves"
        )
    )]
    #[must_use]
    pub fn has(&self, scope: Scope) -> bool {
        self.scopes.has(scope)
    }

    /// `Ok` when the token has `scope`; otherwise a 403 `insufficient_scope` naming it.
    ///
    /// # Errors
    /// The refusal to return.
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "/api/mcp checks scopes in party-tools; these serve token routes that check one scope themselves"
        )
    )]
    pub fn require(&self, scope: Scope) -> Result<(), TokenRejection> {
        if self.has(scope) {
            Ok(())
        } else {
            Err(TokenRejection {
                status: StatusCode::FORBIDDEN,
                error: "insufficient_scope",
                description: format!("This token does not have the {} scope.", scope.as_str()),
                scope: Some(scope.as_str().to_string()),
                metadata: None,
            })
        }
    }
}

/// Why a request was refused, in the shape MCP clients expect (RFC 6750): a `WWW-Authenticate: Bearer …` header that
/// points at the protected-resource metadata, and an OAuth-style JSON body.
#[derive(Debug, Clone)]
pub struct TokenRejection {
    /// 401, 403, 404, 429 or 500.
    pub status: StatusCode,
    /// `invalid_token`, `insufficient_scope`, `not_found`, `too_many_requests`, `server_error`.
    pub error: &'static str,
    /// For people.
    pub description: String,
    /// The scope to ask for.
    pub scope: Option<String>,
    /// The protected-resource metadata URL.
    pub metadata: Option<String>,
}

impl TokenRejection {
    fn new(status: StatusCode, error: &'static str, description: &str) -> Self {
        Self {
            status,
            error,
            description: description.to_string(),
            scope: None,
            metadata: None,
        }
    }

    fn invalid(state: &AppState, description: &str) -> Self {
        Self {
            scope: Some(Scopes::defaults().to_text()),
            metadata: resource_metadata_url(state),
            ..Self::new(StatusCode::UNAUTHORIZED, "invalid_token", description)
        }
    }
}

impl From<ApiError> for TokenRejection {
    fn from(e: ApiError) -> Self {
        // Logged by ApiError's own conversion; the detail is never sent.
        let _ = e.into_response();
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            "Something went wrong on the server.",
        )
    }
}

impl From<sqlx::Error> for TokenRejection {
    fn from(e: sqlx::Error) -> Self {
        ApiError::from(e).into()
    }
}

impl IntoResponse for TokenRejection {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(json!({ "error": self.error, "error_description": self.description })),
        )
            .into_response();
        if matches!(
            self.status,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            let mut value = format!("Bearer error=\"{}\"", self.error);
            if let Some(scope) = &self.scope {
                value.push_str(&format!(", scope=\"{scope}\""));
            }
            if let Some(url) = &self.metadata {
                value.push_str(&format!(", resource_metadata=\"{url}\""));
            }
            if let Ok(v) = HeaderValue::from_str(&value) {
                response.headers_mut().insert(header::WWW_AUTHENTICATE, v);
            }
        }
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

/// The account columns read with a token, from `users u`; [`user_at`] reads them from `first`.
const USER_COLUMNS: &str = "u.id, u.username, u.email, u.is_admin, u.must_change_password";

fn user_at(row: &sqlx::any::AnyRow, first: usize) -> sqlx::Result<User> {
    Ok(User {
        id: row.try_get(first)?,
        username: row.try_get(first + 1)?,
        email: row.try_get(first + 2)?,
        admin: row.try_get::<i64, _>(first + 3)? != 0,
        must_change_password: row.try_get::<i64, _>(first + 4)? != 0,
    })
}

/// Look `token` up: the account, scopes, client, rate-limit key, and the row to touch.
async fn authenticate(state: &AppState, token: &str) -> Result<TokenAuth, TokenRejection> {
    let invalid = || TokenRejection::invalid(state, "The token is unknown, expired or revoked.");
    let at = now();
    let hash = secret_hash(token);
    let (auth, key) = if token.starts_with(PERSONAL_PREFIX) {
        let row = sqlx::query(&format!(
            "SELECT t.id, t.name, t.scopes, t.last_used_at, {USER_COLUMNS} FROM api_tokens t
             JOIN users u ON u.id = t.user_id
             WHERE t.token_hash = $1 AND t.expires_at > $2 AND u.status = 'active'"
        ))
        .bind(&hash)
        .bind(at)
        .fetch_optional(state.db.pool())
        .await?
        .ok_or_else(invalid)?;
        let id: String = row.try_get(0)?;
        let last: Option<i64> = row.try_get(3)?;
        if last.is_none_or(|l| l < at - TOUCH_SECONDS) {
            sqlx::query("UPDATE api_tokens SET last_used_at = $1 WHERE id = $2")
                .bind(at)
                .bind(&id)
                .execute(state.db.pool())
                .await?;
        }
        let auth = TokenAuth {
            account: user_at(&row, 4)?,
            scopes: Scopes::stored(&row.try_get::<String, _>(2)?),
            client: TokenClient::Personal {
                token_id: id.clone(),
                name: row.try_get(1)?,
            },
        };
        (auth, format!("t:{id}"))
    } else if token.starts_with(ACCESS_PREFIX) {
        let row = sqlx::query(&format!(
            "SELECT g.id, g.client_id, c.name, k.scopes, g.resource, g.last_used_at, {USER_COLUMNS}
             FROM oauth_tokens k JOIN oauth_grants g ON g.id = k.grant_id
             JOIN oauth_clients c ON c.id = g.client_id JOIN users u ON u.id = g.user_id
             WHERE k.token_hash = $1 AND k.kind = 'access' AND k.expires_at > $2 AND u.status = 'active'"
        ))
        .bind(&hash)
        .bind(at)
        .fetch_optional(state.db.pool())
        .await?
        .ok_or_else(invalid)?;
        // The audience (RFC 8707): a token issued for another resource is not one for this server's MCP endpoint.
        let resource: String = row.try_get(4)?;
        if mcp_resource(state).as_deref() != Some(resource.as_str()) {
            return Err(invalid());
        }
        let grant: String = row.try_get(0)?;
        let last: Option<i64> = row.try_get(5)?;
        if last.is_none_or(|l| l < at - TOUCH_SECONDS) {
            sqlx::query("UPDATE oauth_grants SET last_used_at = $1 WHERE id = $2")
                .bind(at)
                .bind(&grant)
                .execute(state.db.pool())
                .await?;
        }
        let auth = TokenAuth {
            account: user_at(&row, 6)?,
            scopes: Scopes::stored(&row.try_get::<String, _>(3)?),
            client: TokenClient::OAuth {
                grant_id: grant.clone(),
                client_id: row.try_get(1)?,
                client_name: row.try_get(2)?,
            },
        };
        (auth, format!("g:{grant}"))
    } else {
        return Err(invalid());
    };
    if auth.account.must_change_password {
        return Err(TokenRejection::new(
            StatusCode::FORBIDDEN,
            "password_change_required",
            "The account has to choose a new password first.",
        ));
    }
    if !take(&key, CALLS_PER_MINUTE, CALL_WINDOW) {
        return Err(TokenRejection::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_requests",
            "Too many calls with this token. Wait a minute and try again.",
        ));
    }
    Ok(auth)
}

impl<S> FromRequestParts<S> for TokenAuth
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = TokenRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, TokenRejection> {
        let state = AppState::from_ref(state);
        if !crate::config::features().ai {
            return Err(TokenRejection::new(
                StatusCode::NOT_FOUND,
                "not_found",
                "The server does not offer that.",
            ));
        }
        let Some(token) = bearer(&parts.headers) else {
            return Err(TokenRejection::invalid(
                &state,
                "Send an API token or an OAuth access token as Authorization: Bearer.",
            ));
        };
        authenticate(&state, token.trim()).await
    }
}

// Personal tokens: the Account page.

/// The routes, unless `BOREALIS__FEATURES__AI=false`.
pub fn routes() -> axum::Router<AppState> {
    routes_if(crate::config::features().ai)
}

/// The routes when `on`; none otherwise (404).
pub(crate) fn routes_if(on: bool) -> axum::Router<AppState> {
    use axum::routing::{delete, get};
    if !on {
        return axum::Router::new();
    }
    axum::Router::new()
        .route("/api/me/ai", get(overview))
        .route("/api/me/ai/tokens", get(list).post(create))
        .route("/api/me/ai/tokens/{token}", delete(revoke))
}

/// `GET /api/me/ai`: what the Account page needs to explain and set things up.
pub async fn overview(State(state): State<AppState>, _user: User) -> Json<Value> {
    Json(json!({
        "mcpUrl": mcp_resource(&state),
        "oauth": state.config.public_url.is_some(),
        "scopes": catalog(),
        "defaultDays": DEFAULT_DAYS,
        "maxDays": MAX_DAYS,
        "maxTokens": MAX_TOKENS,
        "callsPerMinute": CALLS_PER_MINUTE,
        // The instructions connected apps get with the MCP handshake, to show and copy (ChatGPT projects).
        "instructions": party_tools::mcp::INSTRUCTIONS,
    }))
}

fn token_json(row: &sqlx::any::AnyRow) -> sqlx::Result<Value> {
    Ok(json!({
        "id": row.try_get::<String, _>(0)?,
        "name": row.try_get::<String, _>(1)?,
        "hint": row.try_get::<String, _>(2)?,
        "scopes": Scopes::stored(&row.try_get::<String, _>(3)?).names(),
        "createdAt": row.try_get::<i64, _>(4)?,
        "expiresAt": row.try_get::<i64, _>(5)?,
        "lastUsedAt": row.try_get::<Option<i64>, _>(6)?,
    }))
}

/// `GET /api/me/ai/tokens`: the account's live tokens (never their secrets).
pub async fn list(State(state): State<AppState>, user: User) -> ApiResult<Json<Value>> {
    let rows = sqlx::query(
        "SELECT id, name, hint, scopes, created_at, expires_at, last_used_at FROM api_tokens
         WHERE user_id = $1 AND expires_at > $2 ORDER BY created_at DESC, id",
    )
    .bind(&user.id)
    .bind(now())
    .fetch_all(state.db.pool())
    .await?;
    let tokens = rows
        .iter()
        .map(token_json)
        .collect::<sqlx::Result<Vec<_>>>()?;
    Ok(Json(json!({ "tokens": tokens })))
}

/// The new-token form.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenForm {
    name: String,
    /// Scope names; the defaults when absent.
    #[serde(default)]
    scopes: Option<Vec<String>>,
    /// 1 to [`MAX_DAYS`]; [`DEFAULT_DAYS`] when absent.
    #[serde(default)]
    expires_in_days: Option<i64>,
}

/// `POST /api/me/ai/tokens`: a new token. The answer is the only time its secret is shown.
pub async fn create(
    State(state): State<AppState>,
    user: User,
    Json(form): Json<TokenForm>,
) -> ApiResult<Json<Value>> {
    let name = form.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err(ApiError::Invalid(
            "Give the token a name of up to 60 characters.".into(),
        ));
    }
    let scopes = match &form.scopes {
        Some(names) => Scopes::from_names(names)?,
        None => Scopes::defaults(),
    };
    if scopes.is_empty() {
        return Err(ApiError::Invalid("Choose at least one scope.".into()));
    }
    let days = form.expires_in_days.unwrap_or(DEFAULT_DAYS);
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(ApiError::Invalid(format!(
            "A token lasts 1 to {MAX_DAYS} days."
        )));
    }
    if !take(&format!("new-token:{}", user.id), 20, 60 * 60) {
        return Err(ApiError::TooMany);
    }
    let at = now();
    let live: i64 =
        sqlx::query("SELECT COUNT(*) FROM api_tokens WHERE user_id = $1 AND expires_at > $2")
            .bind(&user.id)
            .bind(at)
            .fetch_one(state.db.pool())
            .await?
            .try_get(0)?;
    if live >= MAX_TOKENS {
        return Err(ApiError::Conflict(format!(
            "You have {MAX_TOKENS} tokens already. Revoke one you no longer use first."
        )));
    }
    // Expired ones are only clutter.
    sqlx::query("DELETE FROM api_tokens WHERE user_id = $1 AND expires_at <= $2")
        .bind(&user.id)
        .bind(at)
        .execute(state.db.pool())
        .await?;
    let secret = new_secret(PERSONAL_PREFIX);
    let hint = format!("{}…", &secret[..PERSONAL_PREFIX.len() + 4]);
    let id = uuid::Uuid::new_v4().to_string();
    let expires_at = at + days * DAY;
    sqlx::query(
        "INSERT INTO api_tokens (id, user_id, name, token_hash, hint, scopes, created_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(&id)
    .bind(&user.id)
    .bind(&name)
    .bind(secret_hash(&secret))
    .bind(&hint)
    .bind(scopes.to_text())
    .bind(at)
    .bind(expires_at)
    .execute(state.db.pool())
    .await?;
    record(
        &state,
        Activity::new("api_token_created")
            .by(&user)
            .detail(scopes.to_text()),
    )
    .await;
    Ok(Json(json!({
        "token": secret,
        "id": id,
        "name": name,
        "hint": hint,
        "scopes": scopes.names(),
        "createdAt": at,
        "expiresAt": expires_at,
        "lastUsedAt": Value::Null,
    })))
}

/// `DELETE /api/me/ai/tokens/{token}`: revoke one of the account's tokens, at once.
pub async fn revoke(
    State(state): State<AppState>,
    user: User,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    let done = sqlx::query("DELETE FROM api_tokens WHERE id = $1 AND user_id = $2")
        .bind(&id)
        .bind(&user.id)
        .execute(state.db.pool())
        .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::NotFound("There is no such token here.".into()));
    }
    record(&state, Activity::new("api_token_revoked").by(&user)).await;
    Ok(Json(json!({ "revoked": true })))
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231_case_2_shape() {
        // Same key and message hash the same; another key does not.
        let a = hmac_sha256(&[1; 32], b"what do ya want for nothing?");
        let b = hmac_sha256(&[1; 32], b"what do ya want for nothing?");
        let c = hmac_sha256(&[2; 32], b"what do ya want for nothing?");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn hmac_known_answer() {
        // RFC 4231 test case 1 uses a 20-byte key; padded with zeros to 32 it is the same HMAC key block.
        let mut key = [0u8; 32];
        key[..20].copy_from_slice(&[0x0b; 20]);
        let mac = hmac_sha256(&key, b"Hi There");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn secrets_have_their_prefix_and_enough_randomness() {
        let s = new_secret(PERSONAL_PREFIX);
        assert!(s.starts_with("bpt_"));
        assert_eq!(s.len(), 4 + 43);
        assert_ne!(s, new_secret(PERSONAL_PREFIX));
    }

    #[test]
    fn scopes_parse_and_defaults() {
        let d = Scopes::defaults();
        assert_eq!(d, Scopes::all());
        assert_eq!(d.iter().count(), 7);
        let p = Scopes::parse(
            "bestiary:read offline_access encounters:write",
            &["offline_access"],
        )
        .unwrap();
        assert_eq!(p.to_text(), "bestiary:read encounters:write");
        // Write includes read, never the other way round.
        assert!(p.has(Scope::EncountersRead) && !p.has(Scope::HomebrewRead));
        let read = Scopes::parse("encounters:read", &[]).unwrap();
        assert!(read.within(&p) && !p.within(&read));
        assert!(!read.has(Scope::EncountersWrite));
        for gone in ["admin", "combat:read", "combat:run", "index:publish"] {
            assert_eq!(Scopes::parse(gone, &[]).unwrap_err(), gone);
        }
        assert!(same(b"abc", b"abc") && !same(b"abc", b"abd") && !same(b"abc", b"ab"));
    }
}
