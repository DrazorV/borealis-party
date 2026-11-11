//! Instance settings: `config.toml` in the config folder, overridden by `BOREALIS__<SECTION>__<KEY>` variables.
//!
//! Secrets such as the Postgres password belong in variables (a Docker `environment:` entry, a systemd
//! `EnvironmentFile`), not in the file. The file is written with every setting documented on first start.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::db::Location;

/// Prefix of the variables that override `config.toml`.
pub const ENV_PREFIX: &str = "BOREALIS__";

/// The file written on first start. Keep it in step with the defaults below (a test checks).
pub const DEFAULT_FILE: &str = r#"# Borealis Party settings.
#
# Any setting can also come from an environment variable named BOREALIS__<SECTION>__<KEY>, which wins over this file.
# Put passwords there rather than here, for example BOREALIS__POSTGRES__PASSWORD.
# Restart the server after editing.

[server]
# Address and port inside the container (or on the host for a direct install).
bind = "0.0.0.0:8787"
# true: anyone who can reach the server can create an account, Dungeon Masters included (a DM can make the
# server download content from https addresses). false: accounts come from `borealis-server add-user`, or the
# [initial_dm] section below.
open_registration = true
# Largest upload in megabytes (a zip of a 5etools data folder can be large).
max_upload_mb = 512

[database]
# "sqlite" (a file in this folder) or "postgres" (your own server, see [postgres]).
# Switching to postgres: on its first start with an empty PostgreSQL database, the server copies the accounts and
# groups from the SQLite file below, and leaves the file as it was.
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

[initial_dm]
# Creates this Dungeon Master account on a server with no accounts yet. Useful with open_registration = false.
# Set BOREALIS__INITIAL_DM__USERNAME and BOREALIS__INITIAL_DM__PASSWORD.

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
    /// The first account.
    pub initial_dm: InitialDm,
    /// Logging.
    pub log: LogSettings,
}

/// `[server]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ServerSettings {
    /// Listen address.
    pub bind: SocketAddr,
    /// Anyone may sign up.
    pub open_registration: bool,
    /// Largest request in megabytes.
    pub max_upload_mb: usize,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], 8787)),
            open_registration: true,
            max_upload_mb: 512,
        }
    }
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

/// `[initial_dm]`.
#[derive(Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct InitialDm {
    /// Username.
    pub username: Option<String>,
    /// Password.
    pub password: Option<String>,
}

impl std::fmt::Debug for InitialDm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitialDm")
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
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
        Self::parse(&text, std::env::vars())
            .map_err(|e| ConfigError(format!("{}: {e}", file.display())))
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
        if !["disable", "prefer", "require"].contains(&self.postgres.sslmode.as_str()) {
            return Err(ConfigError(
                "postgres.sslmode must be disable, prefer or require".into(),
            ));
        }
        if self.initial_dm.username.is_some() != self.initial_dm.password.is_some() {
            return Err(ConfigError(
                "initial_dm needs both a username and a password".into(),
            ));
        }
        Ok(())
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
}
