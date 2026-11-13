//! The default 5etools source: an address the admin chooses, which the Import screen offers in one tap.
//!
//! The party ships no 5etools address (upstream doesn't either: "only use content you are entitled to use"). A server's
//! admin may set one on Admin › Settings, or `import.default_5etools_url` in `config.toml`
//! (`BOREALIS__IMPORT__DEFAULT_5ETOOLS_URL`); the admin page wins. Without one, the Import screen offers only the
//! account's own address.
//!
//! Routes:
//! - `GET /api/import/default-5etools` (any account): `{ url }`, `null` when there is none;
//! - `GET /api/admin/default-5etools` (admins): `{ url, source, configFile }`;
//! - `POST /api/admin/default-5etools` (admins) `{ url }`: an address to offer, `""` for none (winning over the
//!   config file), or `null` to go back to the config file. An address must be one the 5etools importer reads and
//!   one the server may download from ([`crate::net_guard`]).

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::admin::{record, Admin};
use crate::auth::User;
use crate::db::Activity;
use crate::error::{ApiError, ApiResult};
use crate::AppState;

/// Key in `server_settings`. A saved `""` means "none", winning over the config file.
pub const KEY: &str = "default_5etools_source";

/// Where the default in force comes from.
fn source_of(saved: Option<&str>, file: Option<&str>) -> &'static str {
    match (saved, file) {
        (Some(_), _) => "admin",
        (None, Some(_)) => "config",
        (None, None) => "none",
    }
}

/// The config file's address, blank treated as none.
fn config_url() -> Option<String> {
    crate::config::import()
        .default_5etools_url
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
}

/// The default in force: the admin page's, else the config file's. `None` when there is none.
async fn in_force(state: &AppState) -> ApiResult<(Option<String>, &'static str)> {
    let saved = state.db.setting(KEY).await?;
    let file = config_url();
    let source = source_of(saved.as_deref(), file.as_deref());
    let url = match saved {
        Some(saved) => Some(saved).filter(|u| !u.is_empty()),
        None => file,
    };
    Ok((url, source))
}

/// Check an address the admin typed: one the importer reads and the server may download from. Answers it trimmed.
///
/// # Errors
/// [`ApiError::Invalid`] in words for the admin page.
pub fn checked(url: &str) -> ApiResult<String> {
    let url = url.trim().to_string();
    borealis_lib::content_fivetools_remote::parse_location(&url).map_err(ApiError::Invalid)?;
    crate::net_guard::check_url(&url).map_err(ApiError::Invalid)?;
    Ok(url)
}

async fn for_accounts(State(state): State<AppState>, _: User) -> ApiResult<Json<Value>> {
    let (url, _) = in_force(&state).await?;
    Ok(Json(json!({ "url": url })))
}

async fn admin_json(state: &AppState) -> ApiResult<Value> {
    let (url, source) = in_force(state).await?;
    Ok(json!({ "url": url, "source": source, "configFile": config_url() }))
}

async fn for_admins(State(state): State<AppState>, _: Admin) -> ApiResult<Json<Value>> {
    Ok(Json(admin_json(&state).await?))
}

/// The admin form: an address, `""` for none, or `null` for the config file's.
#[derive(Deserialize)]
struct Form {
    url: Option<String>,
}

async fn save(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(form): Json<Form>,
) -> ApiResult<Json<Value>> {
    let detail = match form.url.map(|u| u.trim().to_string()) {
        None => {
            state.db.delete_setting(KEY).await?;
            format!("{KEY} = as in config.toml")
        }
        Some(url) if url.is_empty() => {
            state.db.set_setting(KEY, "").await?;
            format!("{KEY} = none")
        }
        Some(url) => {
            // The check may look the host up: off the async threads.
            let url = tokio::task::spawn_blocking(move || checked(&url))
                .await
                .map_err(|e| ApiError::Internal(format!("address check: {e}")))??;
            state.db.set_setting(KEY, &url).await?;
            format!("{KEY} = {url}")
        }
    };
    record(
        &state,
        Activity::new("setting_changed").by(&admin).detail(detail),
    )
    .await;
    Ok(Json(admin_json(&state).await?))
}

pub fn routes() -> axum::Router<AppState> {
    use axum::routing::get;
    axum::Router::new()
        .route("/api/import/default-5etools", get(for_accounts))
        .route("/api/admin/default-5etools", get(for_admins).post(save))
}

#[cfg(test)]
mod unit {
    use super::source_of;

    #[test]
    fn the_admin_page_wins_over_the_config_file() {
        assert_eq!(
            source_of(
                Some("https://a.example/data/"),
                Some("https://b.example/data/")
            ),
            "admin"
        );
        assert_eq!(
            source_of(Some(""), Some("https://b.example/data/")),
            "admin"
        );
        assert_eq!(source_of(None, Some("https://b.example/data/")), "config");
        assert_eq!(source_of(None, None), "none");
    }
}
