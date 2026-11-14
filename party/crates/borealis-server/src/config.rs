//! Instance settings: `config.toml` in the config folder, overridden by `BOREALIS__<SECTION>__<KEY>` variables.
//!
//! Secrets such as the Postgres password belong in variables (a Docker `environment:` entry, a systemd
//! `EnvironmentFile`), not in the file. The file is written with every setting documented on first start.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::db::Location;
use crate::net_guard::Cidr;

/// Prefix of the variables that override `config.toml`.
pub const ENV_PREFIX: &str = "BOREALIS__";

/// Where the apps come from unless `server.app_download` says otherwise: the fork's latest release.
pub const DEFAULT_APP_DOWNLOAD: &str =
    "https://github.com/fortress-soft/borealis-connect/releases/latest/download";

/// The file written on first start. Keep it in step with the defaults below (a test checks).
pub const DEFAULT_FILE: &str = r#"# Borealis Party settings.
#
# Any setting can also come from an environment variable named BOREALIS__<SECTION>__<KEY>, which wins over this file.
# Put passwords there rather than here, for example BOREALIS__POSTGRES__PASSWORD.
# Restart the server after editing. Sign-up and approval can also be changed on the admin page without a restart;
# what is saved there wins over this file.

[server]
# Address and port inside the container (or on the host for a direct install).
bind = "0.0.0.0:8787"
# The address people open the server at, for links in emails, for example "https://party.example.com".
# public_url = ""
# true: anyone who can reach the server can ask for an account (and an account can make the server download
# content from https addresses). false: accounts come from
# `borealis-server add-user`, or the [initial_admin] section below.
open_registration = true
# true: a new account waits until an admin approves it on the admin page.
require_approval = true
# Largest upload in megabytes (a zip of a 5etools data folder can be large).
max_upload_mb = 512
# Largest upload in megabytes that gets through the public address. A proxy in front of the server can have a lower
# cap than max_upload_mb: Cloudflare's free plan stops requests over 100 MB. Files travel as base64, a third larger,
# so 95 lets about 71 MB of file through. The app checks a file against this before it sends it, when it is connected
# by an address outside the home network.
public_upload_mb = 95
# Where visitors of the website are offered the app for their system: a folder URL that holds the release files
# (borealis-party-arm64.apk, borealis-party-windows-x64-setup.exe, ...). By default the latest release on GitHub; "off" offers no download.
# app_download = "https://github.com/fortress-soft/borealis-connect/releases/latest/download"
# The reverse proxies in front of the server, as addresses or ranges ("172.20.20.100", "172.18.0.0/16"). Only a
# request from one of these may say who the client is (CF-Connecting-IP, else the right-most X-Forwarded-For entry
# that is not one of these). Sign-up and sign-in limits count per client, so behind a proxy list it here, or every
# client looks like the proxy: one sign-up limit for everyone, and failed sign-ins lock a name for everyone.
# Empty: the headers are ignored and the connecting address is the client. As a variable, separate with commas.
trusted_proxies = []

[database]
# "sqlite" (a file in this folder) or "postgres" (your own server, see [postgres]).
# Switching to postgres: on its first start with an empty PostgreSQL database, the server copies the accounts
# from the SQLite file below, and leaves the file as it was.
type = "sqlite"

[sqlite]
# Relative paths are inside this folder.
path = "party.db"

[postgres]
host = "localhost"
port = 5432
user = "borealis"
# password: set BOREALIS__POSTGRES__PASSWORD.
database = "borealis_party"
# disable, prefer or require.
sslmode = "prefer"

[paths]
# Characters, content libraries and caches. Relative paths are inside this folder.
data = "data"
# The bundled rules and the web app; the defaults are the folders next to the program.
# content = "/app/content"
# web = "/app/web"

[smtp]
# Email for sign-up. With a host set, a new account confirms its email address with a link before it waits for
# approval, and admins get an email about each request. Without one, nobody is emailed and approval is the check.
# Needs server.public_url.
# host = "smtp.example.com"
port = 587
# user = "borealis@example.com"
# password: set BOREALIS__SMTP__PASSWORD.
# from = "Borealis Party <borealis@example.com>"
# "starttls" (usually port 587), "tls" (usually port 465) or "none" (a relay on a trusted network only).
tls = "starttls"

[initial_admin]
# Creates this admin account on a server with no accounts yet. Useful with open_registration = false.
# Set BOREALIS__INITIAL_ADMIN__USERNAME and BOREALIS__INITIAL_ADMIN__PASSWORD, and optionally
# BOREALIS__INITIAL_ADMIN__EMAIL.

[import]
# A 5etools-format source the Import screen offers in one tap ("Default 5etools data"), for example a GitHub
# repository of 5etools data your group is entitled to use. None by default. The admin page's setting wins over this.
# default_5etools_url = ""

[backup]
# Before a new version first starts, or whenever database steps are waiting, the server writes a backup of the
# database here (pg_dump --format=custom for PostgreSQL, VACUUM INTO for SQLite) and only then updates the tables.
# If the backup fails, the server stops without touching the database. Relative paths are inside this folder.
# Copy the files somewhere off this host as well.
dir = "backups"
# Automatic backups kept; older ones are deleted (only files the server wrote itself). 0 keeps every one.
keep = 10
# "auto", or "skip" to start without a backup in an emergency (set BOREALIS__BACKUP__ON_UPGRADE=skip once).
on_upgrade = "auto"
# The folder holding pg_dump and pg_restore, when they are not on the PATH. Their major version must be at least
# the PostgreSQL server's.
# postgres_bin = "/usr/lib/postgresql/18/bin"

[features]
# Emergency switches, for this file only (no admin page toggles them). A feature switched off answers 404 on all its
# routes and keeps its data; switch it back on and restart.
parties = true
# AI apps: personal API tokens and the OAuth server ChatGPT and Claude connect through (tokens.rs, oauth.rs, mcp.rs).
ai = true

[log]
# error, warn, info, debug or trace. RUST_LOG, when set, wins.
level = "info"
"#;

/// Everything the server is configured with.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Network and sign-up.
    pub server: ServerSettings,
    /// Which database.
    pub database: DatabaseSettings,
    /// SQLite.
    pub sqlite: SqliteSettings,
    /// PostgreSQL.
    pub postgres: PostgresSettings,
    /// Folders.
    pub paths: PathSettings,
    /// Email.
    pub smtp: SmtpSettings,
    /// The first account.
    pub initial_admin: InitialAdmin,
    /// Imports (`import_source.rs`).
    pub import: ImportSettings,
    /// The backup before an upgrade (`backup.rs`).
    pub backup: BackupSettings,
    /// Emergency kill switches.
    pub features: FeatureSettings,
    /// `[ai]`: unused since 0.3.3, still accepted (see [`AiSettings`]).
    pub ai: AiSettings,
    /// Logging.
    pub log: LogSettings,
}

/// `[backup]`: the backup taken before the tables are updated (`backup.rs`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BackupSettings {
    /// The folder, relative to the config folder unless absolute.
    pub dir: PathBuf,
    /// Automatic backups kept; 0 keeps all.
    pub keep: usize,
    /// Whether to take one.
    pub on_upgrade: OnUpgrade,
    /// Where `pg_dump` and `pg_restore` are, when not on the PATH.
    pub postgres_bin: Option<PathBuf>,
}

impl Default for BackupSettings {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("backups"),
            keep: 10,
            on_upgrade: OnUpgrade::Auto,
            postgres_bin: None,
        }
    }
}

/// `backup.on_upgrade`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OnUpgrade {
    /// Back up before an upgrade, and refuse to upgrade when that fails.
    #[default]
    Auto,
    /// Upgrade without a backup (emergencies only; logged loudly).
    Skip,
}

/// `[features]`: emergency kill switches, read once at start.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct FeatureSettings {
    /// Parties (`parties.rs`).
    pub parties: bool,
    /// AI apps: personal API tokens, the OAuth server and the MCP endpoint (`tokens.rs`, `oauth.rs`, `mcp.rs`).
    /// `BOREALIS__FEATURES__AI=false` turns them off: their routes answer 404 and no token is accepted.
    pub ai: bool,
}

impl Default for FeatureSettings {
    fn default() -> Self {
        Self {
            parties: true,
            ai: true,
        }
    }
}

/// `[ai]`: the in-app assistant's settings, unused since 0.3.3 (Borealis Party never calls an AI service itself; AI
/// apps connect to it over MCP). Still accepted, so a config file or a `BOREALIS__AI__*` variable from 0.3.1 or 0.3.2
/// never stops the server from starting; main.rs logs once that they are no longer used.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AiSettings {
    /// Whatever the section held (`secret_key`, `requests_per_minute`, `tokens_per_day`), of any type, unread.
    #[serde(flatten)]
    pub unused: toml::Table,
}

impl AiSettings {
    /// The old settings' names, sorted, when any is set.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.unused.keys().cloned().collect();
        names.sort();
        names
    }
}

/// The values are never printed: `secret_key` was a key.
impl std::fmt::Debug for AiSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiSettings")
            .field("unused", &self.names())
            .finish()
    }
}

static FEATURES: std::sync::OnceLock<FeatureSettings> = std::sync::OnceLock::new();

/// `[import]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ImportSettings {
    /// The 5etools-format source offered as the default on the Import screen; blank or absent is none.
    pub default_5etools_url: Option<String>,
}

static IMPORT: std::sync::OnceLock<ImportSettings> = std::sync::OnceLock::new();

/// The `[import]` settings [`Settings::load`] read at start, or none (tests that build the routes by hand).
#[must_use]
pub fn import() -> ImportSettings {
    IMPORT.get().cloned().unwrap_or_default()
}

/// The switches [`Settings::load`] read at start, or all on (tests that build the routes by hand).
#[must_use]
pub fn features() -> FeatureSettings {
    FEATURES.get().copied().unwrap_or_default()
}

/// `[server]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ServerSettings {
    /// Listen address.
    pub bind: SocketAddr,
    /// Where people reach the server, for links in emails.
    pub public_url: Option<String>,
    /// Anyone may sign up.
    pub open_registration: bool,
    /// New accounts wait for an admin.
    pub require_approval: bool,
    /// Largest request in megabytes.
    pub max_upload_mb: usize,
    /// Largest request in megabytes through the public address (a proxy's cap, such as Cloudflare's 100 MB).
    pub public_upload_mb: usize,
    /// Where the apps are downloaded from: unset for [`DEFAULT_APP_DOWNLOAD`], `off` for nowhere.
    pub app_download: Option<String>,
    /// Reverse proxies whose client headers are believed ([`crate::auth::ClientIp`]).
    pub trusted_proxies: Vec<String>,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], 8787)),
            public_url: None,
            open_registration: true,
            require_approval: true,
            max_upload_mb: 512,
            public_upload_mb: 95,
            app_download: None,
            trusted_proxies: Vec::new(),
        }
    }
}

impl ServerSettings {
    /// What `/api/server` tells the app about uploads. The public limit is never above the server's own.
    #[must_use]
    pub fn upload_limits(&self) -> UploadLimits {
        UploadLimits {
            max_upload_mb: self.max_upload_mb,
            public_upload_mb: self.public_upload_mb.min(self.max_upload_mb),
        }
    }
}

/// The largest request the server takes, and the largest that gets through its public address, in megabytes (MiB).
/// The app checks a file against these before reading it (`ui/party/api/overrides/errors.ts`).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadLimits {
    /// `server.max_upload_mb`: on the home network.
    pub max_upload_mb: usize,
    /// `server.public_upload_mb`, at most `max_upload_mb`: through the public address.
    pub public_upload_mb: usize,
}

/// What the network code needs from the settings: who may name the client, and the server's own names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Network {
    /// `server.trusted_proxies`, parsed.
    pub trusted_proxies: Vec<Cidr>,
    /// The host of `server.public_url`, lower case: the server never downloads from itself.
    pub own_hosts: Vec<String>,
}

static NETWORK: std::sync::OnceLock<Network> = std::sync::OnceLock::new();

/// The network settings [`Settings::load`] read at start, or none (tests that build the routes by hand): no proxy is
/// trusted, and the server has no public name.
#[must_use]
pub fn network() -> &'static Network {
    static NONE: std::sync::OnceLock<Network> = std::sync::OnceLock::new();
    NETWORK
        .get()
        .unwrap_or_else(|| NONE.get_or_init(Network::default))
}

static UPLOAD_LIMITS: std::sync::OnceLock<UploadLimits> = std::sync::OnceLock::new();

/// Sets what [`upload_limits`] answers, once, at start. Later calls are ignored.
pub fn set_upload_limits(limits: UploadLimits) {
    let _ = UPLOAD_LIMITS.set(limits);
}

/// The limits set at start, or the defaults when none were (tests that build the routes by hand).
#[must_use]
pub fn upload_limits() -> UploadLimits {
    UPLOAD_LIMITS
        .get()
        .copied()
        .unwrap_or_else(|| ServerSettings::default().upload_limits())
}

/// `[database]`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseSettings {
    /// Which kind.
    #[serde(rename = "type")]
    pub kind: DatabaseKind,
}

/// A database kind.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseKind {
    /// A file in the config folder.
    #[default]
    Sqlite,
    /// A PostgreSQL server.
    Postgres,
}

/// `[sqlite]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SqliteSettings {
    /// The database file.
    pub path: PathBuf,
}

impl Default for SqliteSettings {
    fn default() -> Self {
        Self {
            path: PathBuf::from("party.db"),
        }
    }
}

/// `[postgres]`.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PostgresSettings {
    /// Host name or address.
    pub host: String,
    /// Port.
    pub port: u16,
    /// Role.
    pub user: String,
    /// Password, best given as `BOREALIS__POSTGRES__PASSWORD`.
    pub password: Option<String>,
    /// Database name.
    pub database: String,
    /// `disable`, `prefer` or `require`.
    pub sslmode: String,
}

impl std::fmt::Debug for PostgresSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresSettings")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("database", &self.database)
            .field("sslmode", &self.sslmode)
            .finish()
    }
}

impl Default for PostgresSettings {
    fn default() -> Self {
        Self {
            host: "localhost".into(),
            port: 5432,
            user: "borealis".into(),
            password: None,
            database: "borealis_party".into(),
            sslmode: "prefer".into(),
        }
    }
}

/// `[paths]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PathSettings {
    /// Workspaces and caches.
    pub data: PathBuf,
    /// Bundled rules.
    pub content: Option<PathBuf>,
    /// The web app.
    pub web: Option<PathBuf>,
}

impl Default for PathSettings {
    fn default() -> Self {
        Self {
            data: PathBuf::from("data"),
            content: None,
            web: None,
        }
    }
}

/// How the SMTP connection is secured.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SmtpTls {
    /// Plain connection upgraded with STARTTLS (required).
    #[default]
    Starttls,
    /// TLS from the start.
    Tls,
    /// No encryption.
    None,
}

/// `[smtp]`.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SmtpSettings {
    /// Server; no host means no email.
    pub host: Option<String>,
    /// Port.
    pub port: u16,
    /// Sign-in name, if the server wants one.
    pub user: Option<String>,
    /// Password, best given as `BOREALIS__SMTP__PASSWORD`.
    pub password: Option<String>,
    /// The sender, `Name <address>` or an address.
    pub from: Option<String>,
    /// Encryption.
    pub tls: SmtpTls,
}

impl SmtpSettings {
    /// Whether email is set up.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.host.as_deref().is_some_and(|h| !h.trim().is_empty())
    }
}

impl Default for SmtpSettings {
    fn default() -> Self {
        Self {
            host: None,
            port: 587,
            user: None,
            password: None,
            from: None,
            tls: SmtpTls::Starttls,
        }
    }
}

impl std::fmt::Debug for SmtpSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpSettings")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("from", &self.from)
            .field("tls", &self.tls)
            .finish()
    }
}

/// `[initial_admin]`.
#[derive(Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct InitialAdmin {
    /// Username.
    pub username: Option<String>,
    /// Password.
    pub password: Option<String>,
    /// Email address.
    pub email: Option<String>,
}

impl std::fmt::Debug for InitialAdmin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitialAdmin")
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("email", &self.email)
            .finish()
    }
}

/// `[log]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LogSettings {
    /// Level filter.
    pub level: String,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            level: "info".into(),
        }
    }
}

/// A setting that could not be read.
#[derive(Debug)]
pub struct ConfigError(String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// Turn a variable's text into the type the setting has by default, so `PORT=5432` is a number and
/// `PASSWORD=1234` stays text.
fn typed(value: &str, like: Option<&toml::Value>) -> toml::Value {
    match like {
        Some(toml::Value::Boolean(_)) => match value.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => toml::Value::Boolean(true),
            "false" | "0" | "no" | "off" => toml::Value::Boolean(false),
            _ => toml::Value::String(value.into()),
        },
        Some(toml::Value::Integer(_)) => value
            .parse::<i64>()
            .map_or_else(|_| toml::Value::String(value.into()), toml::Value::Integer),
        Some(toml::Value::Array(_)) => toml::Value::Array(
            value
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| toml::Value::String(v.into()))
                .collect(),
        ),
        _ => toml::Value::String(value.into()),
    }
}

/// Apply `BOREALIS__SECTION__KEY=value` pairs on top of `table`.
fn overlay(
    table: &mut toml::Table,
    defaults: &toml::Table,
    vars: impl IntoIterator<Item = (String, String)>,
) -> Result<(), ConfigError> {
    for (name, value) in vars {
        let Some(path) = name.strip_prefix(ENV_PREFIX) else {
            continue;
        };
        let parts: Vec<String> = path.split("__").map(str::to_ascii_lowercase).collect();
        let [section, key] = parts.as_slice() else {
            return Err(ConfigError(format!(
                "{name}: use {ENV_PREFIX}<SECTION>__<KEY>, for example BOREALIS__POSTGRES__HOST"
            )));
        };
        let like = defaults
            .get(section)
            .and_then(toml::Value::as_table)
            .and_then(|t| t.get(key));
        let entry = table
            .entry(section.clone())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let toml::Value::Table(section_table) = entry else {
            return Err(ConfigError(format!("{name}: [{section}] is not a section")));
        };
        section_table.insert(key.clone(), typed(&value, like));
    }
    Ok(())
}

impl Settings {
    /// Read `config.toml` from `dir` (writing the documented default when there is none) and apply variables.
    ///
    /// # Errors
    /// When the file or a variable doesn't make sense.
    pub fn load(dir: &Path) -> Result<Self, ConfigError> {
        let file = dir.join("config.toml");
        if !file.exists() {
            std::fs::create_dir_all(dir)
                .and_then(|()| std::fs::write(&file, DEFAULT_FILE))
                .map_err(|e| ConfigError(format!("cannot write {}: {e}", file.display())))?;
        }
        let text = std::fs::read_to_string(&file)
            .map_err(|e| ConfigError(format!("cannot read {}: {e}", file.display())))?;
        let settings = Self::parse(&text, std::env::vars())
            .map_err(|e| ConfigError(format!("{}: {e}", file.display())))?;
        // Read by the sign-in routes and the download check, which have no settings of their own. Once per process.
        let _ = NETWORK.set(settings.network());
        let _ = FEATURES.set(settings.features);
        let _ = IMPORT.set(settings.import.clone());
        Ok(settings)
    }

    /// What [`network`] answers after [`Settings::load`].
    #[must_use]
    pub fn network(&self) -> Network {
        Network {
            trusted_proxies: self
                .server
                .trusted_proxies
                .iter()
                .filter_map(|p| Cidr::parse(p).ok())
                .collect(),
            own_hosts: self
                .public_url()
                .and_then(|u| url::Url::parse(&u).ok())
                .and_then(|u| {
                    u.host_str()
                        .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
                })
                .into_iter()
                .collect(),
        }
    }

    /// Settings from file text and variables.
    ///
    /// # Errors
    /// When either doesn't make sense.
    pub fn parse(
        text: &str,
        vars: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let mut table: toml::Table = text.parse().map_err(|e| ConfigError(format!("{e}")))?;
        let defaults = toml::Table::try_from(Self::default())
            .map_err(|e| ConfigError(format!("defaults: {e}")))?;
        overlay(&mut table, &defaults, vars)?;
        let settings: Self = table
            .try_into()
            .map_err(|e: toml::de::Error| ConfigError(e.message().to_string()))?;
        settings.check()?;
        Ok(settings)
    }

    fn check(&self) -> Result<(), ConfigError> {
        for proxy in &self.server.trusted_proxies {
            Cidr::parse(proxy).map_err(|e| ConfigError(format!("server.trusted_proxies: {e}")))?;
        }
        if let Some(url) = self.import.default_5etools_url.as_deref().map(str::trim) {
            if !url.is_empty() && !url.starts_with("https://") {
                return Err(ConfigError(
                    "import.default_5etools_url must start with https://".into(),
                ));
            }
        }
        if !["disable", "prefer", "require"].contains(&self.postgres.sslmode.as_str()) {
            return Err(ConfigError(
                "postgres.sslmode must be disable, prefer or require".into(),
            ));
        }
        if self.initial_admin.username.is_some() != self.initial_admin.password.is_some() {
            return Err(ConfigError(
                "initial_admin needs both a username and a password".into(),
            ));
        }
        if let Some(url) = self.public_url() {
            let parsed = url::Url::parse(&url)
                .map_err(|e| ConfigError(format!("server.public_url: {e}")))?;
            if !["http", "https"].contains(&parsed.scheme()) {
                return Err(ConfigError(
                    "server.public_url must start with http:// or https://".into(),
                ));
            }
        }
        if let Some(url) = self.app_download() {
            let parsed = url::Url::parse(&url)
                .map_err(|e| ConfigError(format!("server.app_download: {e}")))?;
            if !["http", "https"].contains(&parsed.scheme()) {
                return Err(ConfigError(
                    "server.app_download must start with http:// or https://, or be off".into(),
                ));
            }
        }
        if self.smtp.enabled() {
            if self.public_url().is_none() {
                return Err(ConfigError(
                    "smtp needs server.public_url, for the link in the email".into(),
                ));
            }
            let from = self.smtp.from.as_deref().unwrap_or_default();
            if from.parse::<lettre::message::Mailbox>().is_err() {
                return Err(ConfigError(
                    "smtp.from must be an address, or Name <address>".into(),
                ));
            }
        }
        Ok(())
    }

    /// `server.public_url` without a trailing slash, when set.
    #[must_use]
    pub fn public_url(&self) -> Option<String> {
        self.server
            .public_url
            .as_deref()
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
    }

    /// The folder URL the app downloads are in, without a trailing slash; `None` when switched off.
    #[must_use]
    pub fn app_download(&self) -> Option<String> {
        let Some(set) = self.server.app_download.as_deref() else {
            return Some(DEFAULT_APP_DOWNLOAD.to_string());
        };
        let url = set.trim().trim_end_matches('/');
        if url.is_empty()
            || ["off", "none", "false", "no"].contains(&url.to_ascii_lowercase().as_str())
        {
            return None;
        }
        Some(url.to_string())
    }

    /// A path relative to the config folder, unless absolute.
    #[must_use]
    pub fn resolve(dir: &Path, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            dir.join(path)
        }
    }

    /// Where the database is.
    ///
    /// # Errors
    /// When Postgres is chosen without a password.
    pub fn database(&self, dir: &Path) -> Result<Location, ConfigError> {
        match self.database.kind {
            DatabaseKind::Sqlite => Ok(Location::Sqlite(Self::resolve(dir, &self.sqlite.path))),
            DatabaseKind::Postgres => {
                let pg = &self.postgres;
                let password = pg.password.as_deref().ok_or_else(|| {
                    ConfigError(
                        "database.type is postgres: set BOREALIS__POSTGRES__PASSWORD".into(),
                    )
                })?;
                let mut url = url::Url::parse("postgres://localhost")
                    .map_err(|e| ConfigError(e.to_string()))?;
                let bad = |what: &str| ConfigError(format!("postgres.{what} is not valid"));
                url.set_host(Some(&pg.host)).map_err(|_| bad("host"))?;
                url.set_port(Some(pg.port)).map_err(|()| bad("port"))?;
                url.set_username(&pg.user).map_err(|()| bad("user"))?;
                url.set_password(Some(password))
                    .map_err(|()| bad("password"))?;
                url.set_path(&pg.database);
                url.query_pairs_mut().append_pair("sslmode", &pg.sslmode);
                Ok(Location::Postgres(url.into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn the_written_file_means_the_defaults() {
        assert_eq!(
            Settings::parse(DEFAULT_FILE, []).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn upload_limits_are_exposed_and_the_public_one_is_capped() {
        let defaults = Settings::parse(DEFAULT_FILE, [])
            .unwrap()
            .server
            .upload_limits();
        assert_eq!(
            defaults,
            UploadLimits {
                max_upload_mb: 512,
                public_upload_mb: 95
            }
        );
        assert_eq!(
            serde_json::to_value(defaults).unwrap(),
            serde_json::json!({ "maxUploadMb": 512, "publicUploadMb": 95 })
        );
        let set = Settings::parse(
            DEFAULT_FILE,
            vars(&[
                ("BOREALIS__SERVER__MAX_UPLOAD_MB", "64"),
                ("BOREALIS__SERVER__PUBLIC_UPLOAD_MB", "200"),
            ]),
        )
        .unwrap();
        assert_eq!(
            set.server.upload_limits(),
            UploadLimits {
                max_upload_mb: 64,
                public_upload_mb: 64
            }
        );
        // Nothing set at start (as in the route tests): the defaults.
        assert_eq!(upload_limits().public_upload_mb, 95);
    }

    #[test]
    fn variables_win_and_keep_their_types() {
        let s = Settings::parse(
            DEFAULT_FILE,
            vars(&[
                ("BOREALIS__DATABASE__TYPE", "postgres"),
                ("BOREALIS__POSTGRES__HOST", "172.20.20.247"),
                ("BOREALIS__POSTGRES__PORT", "5433"),
                ("BOREALIS__POSTGRES__PASSWORD", "1234"),
                ("BOREALIS__SERVER__OPEN_REGISTRATION", "false"),
                ("PATH", "/usr/bin"),
            ]),
        )
        .unwrap();
        assert_eq!(s.database.kind, DatabaseKind::Postgres);
        assert_eq!(s.postgres.port, 5433);
        assert_eq!(s.postgres.password.as_deref(), Some("1234"));
        assert!(!s.server.open_registration);
        let Location::Postgres(url) = s.database(Path::new("/config")).unwrap() else {
            panic!("postgres");
        };
        assert_eq!(
            url,
            "postgres://borealis:1234@172.20.20.247:5433/borealis_party?sslmode=prefer"
        );
        assert!(
            !format!("{s:?}").contains("1234"),
            "passwords stay out of logs"
        );
    }

    #[test]
    fn trusted_proxies_are_ranges_from_the_file_or_a_variable() {
        assert!(Settings::default().network().trusted_proxies.is_empty());
        let s = Settings::parse(
            DEFAULT_FILE,
            vars(&[
                (
                    "BOREALIS__SERVER__TRUSTED_PROXIES",
                    "172.20.20.100, 172.18.0.0/16",
                ),
                ("BOREALIS__SERVER__PUBLIC_URL", "https://Party.Example.com/"),
            ]),
        )
        .unwrap();
        assert_eq!(s.server.trusted_proxies, ["172.20.20.100", "172.18.0.0/16"]);
        let net = s.network();
        assert_eq!(net.trusted_proxies.len(), 2);
        assert!(net.trusted_proxies[1].contains("172.18.4.2".parse().unwrap()));
        assert_eq!(net.own_hosts, ["party.example.com"]);
        let file = Settings::parse(
            "[server]
trusted_proxies = [\"::1\"]",
            [],
        )
        .unwrap();
        assert_eq!(file.network().trusted_proxies.len(), 1);
        assert!(Settings::parse(
            "[server]
trusted_proxies = [\"npm\"]",
            []
        )
        .is_err());
        assert!(Settings::parse(
            "",
            vars(&[("BOREALIS__SERVER__TRUSTED_PROXIES", "10.0.0.0/40")])
        )
        .is_err());
        assert!(
            network().trusted_proxies.is_empty(),
            "nothing loaded in tests"
        );
    }

    #[test]
    fn passwords_are_escaped_in_the_url() {
        let s = Settings::parse(
            "[database]\ntype = \"postgres\"",
            vars(&[("BOREALIS__POSTGRES__PASSWORD", "p@ss/w:rd")]),
        )
        .unwrap();
        let Location::Postgres(url) = s.database(Path::new("/config")).unwrap() else {
            panic!("postgres");
        };
        assert!(url.contains("p%40ss%2Fw%3Ard@"), "{url}");
    }

    #[test]
    fn smtp_needs_a_public_address_and_a_sender() {
        let smtp = [
            ("BOREALIS__SMTP__HOST", "smtp.example.com"),
            ("BOREALIS__SMTP__PASSWORD", "mail-secret"),
            ("BOREALIS__SMTP__FROM", "Party <party@example.com>"),
        ];
        assert!(Settings::parse(DEFAULT_FILE, vars(&smtp)).is_err());
        let mut with_url = smtp.to_vec();
        with_url.push(("BOREALIS__SERVER__PUBLIC_URL", "https://party.example.com/"));
        let s = Settings::parse(DEFAULT_FILE, vars(&with_url)).unwrap();
        assert!(s.smtp.enabled());
        assert_eq!(s.smtp.port, 587);
        assert_eq!(s.public_url().as_deref(), Some("https://party.example.com"));
        assert!(!format!("{s:?}").contains("mail-secret"));
        let mut bad_from = with_url.clone();
        bad_from[2] = ("BOREALIS__SMTP__FROM", "nobody");
        assert!(Settings::parse(DEFAULT_FILE, vars(&bad_from)).is_err());
        assert!(!Settings::default().smtp.enabled());
    }

    #[test]
    fn mistakes_are_reported() {
        assert!(
            Settings::parse("[server]\nprot = 1", []).is_err(),
            "unknown key"
        );
        assert!(Settings::parse("", vars(&[("BOREALIS__SERVER__BIND", "nowhere")])).is_err());
        assert!(Settings::parse("", vars(&[("BOREALIS__BIND", "0.0.0.0:1")])).is_err());
        assert!(
            Settings::parse("[database]\ntype = \"postgres\"", [])
                .unwrap()
                .database(Path::new("/c"))
                .is_err(),
            "postgres without a password"
        );
    }

    #[test]
    fn the_app_download_has_a_default_and_can_be_moved_or_turned_off() {
        let get = |value: &str| {
            Settings::parse(
                DEFAULT_FILE,
                vars(&[("BOREALIS__SERVER__APP_DOWNLOAD", value)]),
            )
            .map(|s| s.app_download())
        };
        assert_eq!(
            Settings::default().app_download().as_deref(),
            Some(DEFAULT_APP_DOWNLOAD)
        );
        assert_eq!(
            get("https://files.example.com/apps/").unwrap().as_deref(),
            Some("https://files.example.com/apps")
        );
        assert_eq!(get("off").unwrap(), None);
        assert_eq!(get("").unwrap(), None);
        assert!(get("ftp://files.example.com").is_err());
        assert!(get("not a url").is_err());
    }

    #[test]
    fn relative_paths_are_inside_the_config_folder() {
        let s = Settings::default();
        let Location::Sqlite(path) = s.database(Path::new("/config")).unwrap() else {
            panic!("sqlite");
        };
        assert_eq!(path, Path::new("/config/party.db"));
        assert_eq!(
            Settings::resolve(Path::new("/config"), Path::new("/data")),
            Path::new("/data")
        );
    }

    #[test]
    fn the_old_ai_settings_still_parse_and_are_never_printed() {
        // A 0.3.1/0.3.2 config.toml and stack env: accepted and unread since 0.3.3.
        let old = format!(
            "{DEFAULT_FILE}
[ai]
requests_per_minute = 20
tokens_per_day = 200000
"
        );
        let s = Settings::parse(
            &old,
            vars(&[
                (
                    "BOREALIS__AI__SECRET_KEY",
                    "c2VjcmV0LXNlY3JldC1zZWNyZXQtc2VjcmV0LXNlY3I=",
                ),
                ("BOREALIS__AI__REQUESTS_PER_MINUTE", "30"),
            ]),
        )
        .unwrap();
        assert_eq!(
            s.ai.names(),
            ["requests_per_minute", "secret_key", "tokens_per_day"]
        );
        let shown = format!("{s:?}");
        assert!(!shown.contains("c2VjcmV0"), "{shown}");
        assert!(Settings::parse(DEFAULT_FILE, [])
            .unwrap()
            .ai
            .names()
            .is_empty());
        // The kill switch still parses.
        let off =
            Settings::parse(DEFAULT_FILE, vars(&[("BOREALIS__FEATURES__AI", "false")])).unwrap();
        assert!(!off.features.ai);
    }
}
