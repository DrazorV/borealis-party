//! The Borealis Connect server: the Borealis backend for many accounts, each with its own characters and content.
//!
//! See `party/README.md`.

use std::io::BufRead;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderName, HeaderValue, Method};
use axum::routing::{any, get};
use axum::{Json, Router};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;

mod admin;
mod auth;
mod backend;
mod backup;
mod bestiary;
mod combat;
mod config;
mod content_events;
mod content_rules;
mod content_update;
mod db;
mod encounters;
mod error;
mod health;
mod homebrew;
mod hub;
mod import_source;
mod index_subs;
mod item_tools;
mod jobs;
mod mail;
mod mcp;
mod migrate;
mod net_guard;
mod oauth;
mod pack_removal;
mod parties;
mod release;
#[cfg(test)]
mod route_tests;
mod rpc;
mod settings;
mod shares;
mod simulate;
mod simulate_tools;
mod spell_tools;
mod spells;
mod sync;
#[cfg(test)]
mod tests;
mod tokens;
mod tools_backend;
mod workspaces;

use auth::{RateLimiter, Status};
use config::Settings;
use db::Db;
use workspaces::Workspaces;

// The page calls only this server: Borealis Connect never calls an AI service itself (AI apps connect to it over MCP).
const CSP: &str = "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; \
                   connect-src 'self'; \
                   object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

#[derive(Parser)]
#[command(
    name = "borealis-server",
    version,
    about = "Multi-user Borealis server. Settings are in <config dir>/config.toml and BOREALIS__<SECTION>__<KEY> variables."
)]
struct Args {
    /// The folder holding config.toml and, by default, the database and data.
    #[arg(long, env = "BOREALIS_CONFIG_DIR", default_value = "/config")]
    config_dir: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create an active account; the password is read from standard input. The first account on a server is always
    /// an admin.
    AddUser {
        /// The username.
        username: String,
        /// The email address.
        #[arg(long)]
        email: Option<String>,
        /// Make it an admin.
        #[arg(long)]
        admin: bool,
    },
    /// Exit 0 when the running server answers (for container health checks).
    Health,
    /// Copy the accounts from a SQLite file into the configured PostgreSQL database, which must have none yet.
    /// The server does this by itself on its first start with PostgreSQL; this is for doing it by hand.
    MigrateSqlite {
        /// The SQLite file; by default the one `[sqlite] path` names.
        #[arg(long)]
        from: Option<PathBuf>,
    },
}

/// Server settings the routes need, from `config.toml`. The admin page can override sign-up, approval and the
/// trusted proxies.
pub struct Config {
    open_registration: bool,
    require_approval: bool,
    /// `server.public_url`, without a trailing slash.
    public_url: Option<String>,
    /// `server.app_download`: the folder the apps are downloaded from, if any.
    app_download: Option<String>,
    /// `server.trusted_proxies`, as written. A list saved on the admin page wins ([`admin::trusted_proxies`]).
    trusted_proxies: Vec<String>,
}

/// Shared by every route.
#[derive(Clone)]
pub struct AppState {
    db: Db,
    workspaces: Arc<Workspaces>,
    config: Arc<Config>,
    login_limiter: RateLimiter,
    signup_limiter: RateLimiter,
    /// Sends sign-up email, when `[smtp]` is set up.
    mailer: Option<Arc<dyn mail::Mailer>>,
    /// Serialises sign-ups, so only one can be the first account.
    accounts: Arc<tokio::sync::Mutex<()>>,
    /// When the server started.
    started: std::time::Instant,
    /// Live party events and roster cards ([`hub`]).
    hub: Arc<hub::PartyHub>,
}

async fn server_info(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> error::ApiResult<Json<Value>> {
    let settings = admin::runtime_settings(&state).await?;
    Ok(Json(json!({
        "app": "borealis-party",
        "version": release::version(),
        "openRegistration": settings.open_registration,
        "requireApproval": settings.require_approval,
        "email": state.mailer.is_some(),
        "appDownload": state.config.app_download,
        // The app checks a file's size against these before reading it (ui/party/api/overrides/errors.ts).
        "uploads": config::upload_limits(),
    })))
}

/// Any other path under `/api`, so a client gets JSON and not the web app's page.
async fn no_such_route() -> error::ApiError {
    error::ApiError::NotFound("The server does not offer that.".into())
}

/// Every route under `/api`. Each feature module has a `routes()` and adds one `.merge` line here.
pub fn api_routes(state: AppState, max_upload: usize) -> Router {
    Router::new()
        .route("/api/server", get(server_info))
        .merge(auth::routes())
        .merge(admin::routes())
        .merge(rpc::routes())
        .merge(shares::routes())
        .merge(parties::routes())
        .merge(encounters::routes())
        .merge(combat::routes())
        .merge(import_source::routes())
        .merge(pack_removal::routes())
        .merge(bestiary::routes())
        .merge(tokens::routes())
        .merge(oauth::routes())
        .merge(mcp::routes())
        .merge(homebrew::routes())
        .merge(spells::routes())
        .merge(health::routes())
        .route("/api/{*rest}", any(no_such_route))
        .layer(axum::middleware::from_fn(health::count))
        .layer(DefaultBodyLimit::max(max_upload))
        .with_state(state)
}

fn app(state: AppState, web_dir: &std::path::Path, max_upload: usize) -> Router {
    let web = ServeDir::new(web_dir).fallback(ServeFile::new(web_dir.join("index.html")));
    // The API is authorised by a header no other site can add, so any origin (the phone app's included) may call it.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static("mcp-protocol-version"),
            HeaderName::from_static("mcp-method"),
            HeaderName::from_static("mcp-name"),
        ])
        .expose_headers([header::WWW_AUTHENTICATE]);
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

/// How long requests under way get to finish once the server is told to stop.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Serve until `stop` resolves, then let requests under way finish, for a moment only: an event stream never ends by
/// itself.
async fn serve(
    listener: tokio::net::TcpListener,
    router: Router,
    stop: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    let (closing, closed) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = closed.await;
        })
        .await
    });
    tokio::select! {
        ended = &mut server => return ended.map_err(std::io::Error::other)?,
        () = stop => {}
    }
    drop(closing);
    let _ = tokio::time::timeout(STOP_GRACE, server).await;
    Ok(())
}

/// The folder next to the program, where the packaged content and web app sit.
fn beside_program(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Ask the server on this machine for `/api/server`.
fn health(bind: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    let ip = if bind.ip().is_unspecified() {
        std::net::IpAddr::from([127, 0, 0, 1])
    } else {
        bind.ip()
    };
    let timeout = std::time::Duration::from_secs(5);
    let mut stream =
        std::net::TcpStream::connect_timeout(&SocketAddr::new(ip, bind.port()), timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.write_all(b"GET /api/server HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    if reply.starts_with("HTTP/1.0 200") || reply.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        Err("the server did not answer 200".into())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let settings = Settings::load(&args.config_dir)?;
    if matches!(args.command, Some(Command::Health)) {
        return health(settings.server.bind);
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("{},tantivy=warn", settings.log.level).into()),
        )
        .init();
    let dir = &args.config_dir;
    // AI app tokens are hashed with a key kept beside config.toml (tokens.rs).
    if let Err(e) = tokens::load_pepper(dir) {
        tracing::error!(
            "{e}: API tokens and connected AI apps will stop working when the server restarts"
        );
    }
    // 0.3.1 and 0.3.2 had an in-app assistant that called AI services; its `[ai]` settings are accepted and unread.
    if !settings.ai.names().is_empty() {
        tracing::info!(
            "[ai] {} (BOREALIS__AI__*) is no longer used: Borealis Connect never calls an AI service itself since 0.3.3; the setting can be removed",
            settings.ai.names().join(", ")
        );
    }
    let location = settings.database(dir)?;
    tracing::info!(
        "config {}, database {}",
        dir.join("config.toml").display(),
        match &location {
            db::Location::Postgres(_) => format!(
                "postgres at {}:{}/{}",
                settings.postgres.host, settings.postgres.port, settings.postgres.database
            ),
            other => format!("{other:?}"),
        }
    );
    let db = Db::connect(&location).await.map_err(|e| e.to_string())?;
    // Before any table changes: a failed backup stops here, with the database as it was.
    backup::upgrade_with_backup(&db, &location, &settings.backup, dir, release::version()).await?;
    backup::set_dir(Settings::resolve(dir, &settings.backup.dir));
    health::set_config_dir(dir.clone());
    let sqlite_file = Settings::resolve(dir, &settings.sqlite.path);
    if let Some(Command::MigrateSqlite { from }) = &args.command {
        if !matches!(location, db::Location::Postgres(_)) {
            return Err(
                "set database.type = postgres (BOREALIS__DATABASE__TYPE=postgres) first".into(),
            );
        }
        let file = from.clone().unwrap_or(sqlite_file);
        if !file.is_file() {
            return Err(format!("no SQLite database at {}", file.display()).into());
        }
        let source = Db::open(&db::Location::Sqlite(file))
            .await
            .map_err(|e| e.to_string())?;
        let report = migrate::copy(&source, &db).await?;
        println!("Copied {}.", migrate::describe(&report));
        return Ok(());
    }
    if matches!(location, db::Location::Postgres(_)) {
        // Switched from SQLite: bring the accounts along, once.
        if let Some(report) = migrate::adopt_sqlite(&db, &sqlite_file).await? {
            tracing::info!(
                "moved to PostgreSQL: {}. {} was left as it was",
                migrate::describe(&report),
                sqlite_file.display()
            );
        }
    }
    if let Some(Command::AddUser {
        username,
        email,
        admin,
    }) = args.command
    {
        let mut password = String::new();
        std::io::stdin().lock().read_line(&mut password)?;
        let (user, _) = auth::create_account(
            &db,
            auth::AccountSpec {
                username: &username,
                email: email.as_deref(),
                password: password.trim_end_matches(['\r', '\n']),
                status: Status::Active,
                admin,
                confirm_token_hash: None,
            },
        )
        .await
        .map_err(|e| format!("{e:?}"))?;
        let kind = if user.admin { "admin" } else { "account" };
        println!("Created the {kind} {}.", user.username);
        return Ok(());
    }
    let initial = &settings.initial_admin;
    if let (Some(username), Some(password)) = (&initial.username, &initial.password) {
        if db.user_count().await.map_err(|e| format!("{e:?}"))? == 0 {
            auth::create_account(
                &db,
                auth::AccountSpec {
                    username,
                    email: initial.email.as_deref(),
                    password,
                    status: Status::Active,
                    admin: true,
                    confirm_token_hash: None,
                },
            )
            .await
            .map_err(|e| format!("initial_admin: {e:?}"))?;
            tracing::info!("created the initial admin account {username}");
        }
    }
    let mailer = if settings.smtp.enabled() {
        let mailer = mail::SmtpMailer::shared(&settings.smtp)?;
        tracing::info!(
            "sign-up email through {}:{}",
            settings.smtp.host.as_deref().unwrap_or_default(),
            settings.smtp.port
        );
        Some(mailer)
    } else {
        None
    };
    let content = settings
        .paths
        .content
        .clone()
        .unwrap_or_else(|| beside_program("content"));
    let web = settings
        .paths
        .web
        .clone()
        .unwrap_or_else(|| beside_program("web"));
    if !content.join("core").join("pack.json").is_file() {
        return Err(format!(
            "no bundled content in {}: set paths.content (BOREALIS__PATHS__CONTENT)",
            content.display()
        )
        .into());
    }
    let data = Settings::resolve(dir, &settings.paths.data);
    std::fs::create_dir_all(&data)?;
    let cards = data.join("party-cards");
    // Open simulations saved by the previous run (simulate.rs): an update doesn't end them.
    simulate::install(data.join("sims"));
    let workspaces = Arc::new(Workspaces::new(data, content));
    tokio::spawn(Arc::clone(&workspaces).evict_loop());
    tokio::spawn(shares::sweep_loop(db.clone(), Arc::clone(&workspaces)));
    tokio::spawn(combat::startup(db.clone()));
    config::set_upload_limits(settings.server.upload_limits());
    let state = AppState {
        db,
        workspaces: Arc::clone(&workspaces),
        config: Arc::new(Config {
            open_registration: settings.server.open_registration,
            require_approval: settings.server.require_approval,
            public_url: settings.public_url(),
            app_download: settings.app_download(),
            trusted_proxies: settings.server.trusted_proxies.clone(),
        }),
        login_limiter: RateLimiter::logins(),
        signup_limiter: RateLimiter::signups(),
        mailer,
        accounts: Arc::new(tokio::sync::Mutex::new(())),
        started: std::time::Instant::now(),
        hub: Arc::new(hub::PartyHub::new(cards)),
    };
    // The automatic content update when an account's workspace opens (content_update.rs).
    content_update::install(Arc::clone(&state.hub));
    let router = app(
        state,
        &web,
        settings.server.max_upload_mb.saturating_mul(1024 * 1024),
    );
    let listener = tokio::net::TcpListener::bind(settings.server.bind).await?;
    tracing::info!("listening on {}", settings.server.bind);
    serve(listener, router, shutdown_signal()).await?;
    tokio::task::spawn_blocking(move || workspaces.save_all()).await?;
    Ok(())
}
