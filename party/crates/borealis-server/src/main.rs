//! The Borealis party server: the Borealis backend for many accounts, plus groups.
//!
//! See `party/README.md`.

use std::io::BufRead;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderName, HeaderValue, Method};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

mod auth;
mod backend;
mod db;
mod error;
mod groups;
mod rpc;
mod settings;
#[cfg(test)]
mod tests;
mod workspaces;

use auth::{LoginLimiter, Role};
use db::Db;
use workspaces::Workspaces;

const CSP: &str = "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; \
                   object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

#[derive(Parser)]
#[command(
    name = "borealis-server",
    version,
    about = "Multi-user Borealis server"
)]
struct Args {
    /// Address to listen on.
    #[arg(long, env = "BOREALIS_BIND", default_value = "0.0.0.0:8787")]
    bind: SocketAddr,
    /// Where accounts, characters and content live.
    #[arg(
        long,
        env = "BOREALIS_DATA_DIR",
        default_value = "/var/lib/borealis-party"
    )]
    data_dir: PathBuf,
    /// The bundled content packs (the repository's `content` folder).
    #[arg(
        long,
        env = "BOREALIS_CONTENT_DIR",
        default_value = "/opt/borealis-party/content"
    )]
    content_dir: PathBuf,
    /// The built web app (`ui/dist`). The API works without it.
    #[arg(
        long,
        env = "BOREALIS_WEB_DIR",
        default_value = "/opt/borealis-party/web"
    )]
    web_dir: PathBuf,
    /// Let anyone reach the sign-up form. When off, accounts come from `add-user`.
    #[arg(long, env = "BOREALIS_OPEN_REGISTRATION", default_value_t = true, action = clap::ArgAction::Set)]
    open_registration: bool,
    /// Largest request, in megabytes (content imports arrive as base64).
    #[arg(long, env = "BOREALIS_MAX_UPLOAD_MB", default_value_t = 512)]
    max_upload_mb: usize,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create an account; the password is read from standard input.
    AddUser {
        /// The username.
        username: String,
        /// `dm` or `player`.
        #[arg(value_parser = parse_role)]
        role: Role,
    },
}

fn parse_role(text: &str) -> Result<Role, String> {
    match text {
        "dm" => Ok(Role::Dm),
        "player" => Ok(Role::Player),
        _ => Err("use dm or player".into()),
    }
}

/// Server settings the routes need.
pub struct Config {
    open_registration: bool,
}

/// Shared by every route.
#[derive(Clone)]
pub struct AppState {
    db: Db,
    workspaces: Arc<Workspaces>,
    config: Arc<Config>,
    login_limiter: LoginLimiter,
    /// Serialises anything that moves characters in or out of groups.
    membership: Arc<tokio::sync::Mutex<()>>,
}

async fn server_info(axum::extract::State(state): axum::extract::State<AppState>) -> Json<Value> {
    Json(json!({
        "app": "borealis-party",
        "version": env!("CARGO_PKG_VERSION"),
        "openRegistration": state.config.open_registration,
    }))
}

/// Every route under `/api`.
pub fn api_routes(state: AppState, max_upload: usize) -> Router {
    Router::new()
        .route("/api/server", get(server_info))
        .route("/api/auth/register", post(auth::register))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/api/groups", get(groups::list).post(groups::create))
        .route(
            "/api/groups/{id}",
            get(groups::detail)
                .patch(groups::rename)
                .delete(groups::delete),
        )
        .route("/api/groups/{id}/party", get(groups::party))
        .route("/api/groups/{id}/invites", post(groups::invite))
        .route(
            "/api/groups/{id}/invites/{invite}",
            delete(groups::cancel_invite),
        )
        .route(
            "/api/groups/{id}/members/{player}",
            delete(groups::remove_member),
        )
        .route(
            "/api/groups/{id}/members/{player}/view/{command}",
            post(groups::view),
        )
        .route(
            "/api/groups/{id}/character",
            post(groups::set_character).delete(groups::clear_character),
        )
        .route("/api/invites", get(groups::my_invites))
        .route("/api/invites/{id}/accept", post(groups::accept))
        .route("/api/invites/{id}/decline", post(groups::decline))
        .route("/api/characters", get(groups::my_characters))
        .route("/api/rpc/{scope}/{command}", post(rpc::command))
        .route("/api/events/{scope}", get(rpc::events))
        .layer(DefaultBodyLimit::max(max_upload))
        .with_state(state)
}

fn app(state: AppState, web_dir: &std::path::Path, max_upload: usize) -> Router {
    let web = ServeDir::new(web_dir).fallback(ServeFile::new(web_dir.join("index.html")));
    // The API is authorised by a header no other site can add, so any origin (the phone app's included) may call it.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    api_routes(state, max_upload)
        .fallback_service(web)
        .layer(cors)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
}

async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(e) => {
                tracing::error!("cannot listen for SIGTERM: {e}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tantivy=warn".into()),
        )
        .init();
    let args = Args::parse();
    std::fs::create_dir_all(&args.data_dir)?;
    let db = Db::open(&args.data_dir.join("party.db"))?;
    if let Some(Command::AddUser { username, role }) = args.command {
        let mut password = String::new();
        std::io::stdin().lock().read_line(&mut password)?;
        let user = auth::create_user(
            &db,
            &username,
            password.trim_end_matches(['\r', '\n']),
            role,
        )
        .await
        .map_err(|e| format!("{e:?}"))?;
        println!("Created {} ({}).", user.username, role.as_str());
        return Ok(());
    }
    if !args.content_dir.join("core").join("pack.json").is_file() {
        return Err(format!(
            "no bundled content in {}: set BOREALIS_CONTENT_DIR",
            args.content_dir.display()
        )
        .into());
    }
    let workspaces = Arc::new(Workspaces::new(args.data_dir.clone(), args.content_dir));
    tokio::spawn(Arc::clone(&workspaces).evict_loop());
    let state = AppState {
        db,
        workspaces: Arc::clone(&workspaces),
        config: Arc::new(Config {
            open_registration: args.open_registration,
        }),
        login_limiter: LoginLimiter::default(),
        membership: Arc::new(tokio::sync::Mutex::new(())),
    };
    let router = app(
        state,
        &args.web_dir,
        args.max_upload_mb.saturating_mul(1024 * 1024),
    );
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    tracing::info!("listening on {}", args.bind);
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    tokio::task::spawn_blocking(move || workspaces.save_all()).await?;
    Ok(())
}
