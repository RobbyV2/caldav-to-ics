use std::time::Duration;

use axum::http::{HeaderName, Method, header};
use axum::middleware;
use caldav_ics_sync::api::AppState;
use caldav_ics_sync::config::AppConfig;
use caldav_ics_sync::server::auth::{AuthConfig, basic_auth_middleware};
use caldav_ics_sync::server::build_router;
use tokio_retry2::strategy::ExponentialBackoff;
use tokio_retry2::{Retry, RetryError};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::from_filename(".env.local");
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = AppConfig::load()?;

    std::fs::create_dir_all(&cfg.data_dir)?;
    let db_path = format!("{}/caldav-sync.db", cfg.data_dir);
    let conn = rusqlite::Connection::open(&db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    caldav_ics_sync::db::init_db(&conn)?;
    info!("Database initialized at {}", db_path);

    let proxy_url = cfg.proxy_url();

    let app_state = AppState {
        db: std::sync::Arc::new(std::sync::Mutex::new(conn)),
        start_time: std::time::Instant::now(),
    };

    start_auto_sync(app_state.clone());

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            header::UPGRADE,
            header::CONNECTION,
            HeaderName::from_static("sec-websocket-key"),
            HeaderName::from_static("sec-websocket-version"),
            HeaderName::from_static("sec-websocket-protocol"),
        ])
        .allow_credentials(true);

    let auth_config = AuthConfig::from_config(&cfg);
    match &auth_config {
        AuthConfig::Disabled => {
            info!("HTTP Basic Auth disabled (AUTH_USERNAME not set or no password configured)");
        }
        AuthConfig::PlainText { username, .. } => {
            info!(
                "HTTP Basic Auth enabled for user '{}' (plain text)",
                username
            );
        }
        AuthConfig::Hashed { username, .. } => {
            info!(
                "HTTP Basic Auth enabled for user '{}' (argon2 hash)",
                username
            );
        }
    }

    let app = build_router(app_state, &proxy_url)
        .await
        .layer(middleware::from_fn(basic_auth_middleware))
        .layer(axum::Extension(auth_config))
        .layer(cors);

    let addr = format!("{}:{}", cfg.server_host, cfg.server_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!("Starting server");
    info!("Listening on http://{}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Server shutdown complete");

    Ok(())
}

const RETRY_BASE_MS: u64 = 30_000;
const RETRY_MAX_MS: u64 = 300_000;
const MAX_RETRIES: usize = 5;

#[derive(Debug)]
struct SyncError {
    inner: anyhow::Error,
    permanent: bool,
}

impl SyncError {
    fn transient(e: impl Into<anyhow::Error>) -> Self {
        Self {
            inner: e.into(),
            permanent: false,
        }
    }

    fn permanent(e: impl Into<anyhow::Error>) -> Self {
        Self {
            inner: e.into(),
            permanent: true,
        }
    }
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

enum SyncKind {
    Source(i64),
    Destination(i64),
}

impl SyncKind {
    fn write_error_status(&self, state: &AppState, msg: &str) {
        let Ok(db) = state.db.lock() else {
            tracing::error!("Failed to acquire DB lock for error status update");
            return;
        };
        match self {
            SyncKind::Source(id) => {
                let _ = caldav_ics_sync::db::update_sync_status(&db, *id, "error", Some(msg));
            }
            SyncKind::Destination(id) => {
                let _ = caldav_ics_sync::db::update_destination_sync_status(
                    &db,
                    *id,
                    "error",
                    Some(msg),
                );
            }
        }
    }
}

fn spawn_auto_sync<F, Fut>(
    name: &str,
    interval_secs: u64,
    kind: SyncKind,
    state: AppState,
    sync_fn: F,
) where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<String, RetryError<SyncError>>> + Send,
{
    let display_name = name.to_owned();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            let strategy = ExponentialBackoff::from_millis(RETRY_BASE_MS)
                .max_delay(Duration::from_millis(RETRY_MAX_MS))
                .take(MAX_RETRIES);

            match Retry::spawn(strategy, &sync_fn).await {
                Ok(msg) => info!("{}", msg),
                Err(e) if e.permanent => {
                    tracing::error!("Auto-sync '{}' stopping: {}", display_name, e);
                    break;
                }
                Err(e) => {
                    let msg = e.to_string();
                    tracing::error!(
                        "Auto-sync '{}' failed after {} retries: {}",
                        display_name,
                        MAX_RETRIES,
                        msg
                    );
                    kind.write_error_status(&state, &msg);
                }
            }
        }
    });
    info!("Auto-sync enabled for {} (every {}s)", name, interval_secs);
}

fn start_auto_sync(state: AppState) {
    // Auto-sync sources (CalDAV -> ICS)
    let sources = {
        let db = state.db.lock().unwrap();
        caldav_ics_sync::db::list_sources(&db).unwrap_or_default()
    };

    for source in sources {
        if source.sync_interval_secs > 0 {
            let state = state.clone();
            let id = source.id;
            spawn_auto_sync(
                &source.name,
                source.sync_interval_secs as u64,
                SyncKind::Source(id),
                state.clone(),
                move || {
                    let state = state.clone();
                    async move {
                        let (url, user, pass) = {
                            let db = state.db.lock().unwrap();
                            match caldav_ics_sync::db::get_source(&db, id) {
                                Ok(Some(s)) => (s.caldav_url, s.username, s.password),
                                _ => {
                                    return Err(RetryError::permanent(SyncError::permanent(
                                        anyhow::anyhow!("Source {} no longer exists", id),
                                    )));
                                }
                            }
                        };
                        let (events, calendars, ics_data) =
                            caldav_ics_sync::api::sync::run_sync(&url, &user, &pass)
                                .await
                                .map_err(|e| RetryError::transient(SyncError::transient(e)))?;
                        let db = state.db.lock().unwrap();
                        caldav_ics_sync::db::save_ics_data(&db, id, &ics_data)
                            .map_err(|e| RetryError::transient(SyncError::transient(e)))?;
                        caldav_ics_sync::db::update_last_synced(&db, id)
                            .map_err(|e| RetryError::transient(SyncError::transient(e)))?;
                        caldav_ics_sync::db::update_sync_status(&db, id, "ok", None)
                            .map_err(|e| RetryError::transient(SyncError::transient(e)))?;
                        Ok(format!(
                            "Auto-sync source {}: {} events from {} calendars",
                            id, events, calendars
                        ))
                    }
                },
            );
        }
    }

    // Auto-sync destinations (ICS -> CalDAV)
    let destinations = {
        let db = state.db.lock().unwrap();
        caldav_ics_sync::db::list_destinations(&db).unwrap_or_default()
    };

    for dest in destinations {
        if dest.sync_interval_secs > 0 {
            let state = state.clone();
            let id = dest.id;
            spawn_auto_sync(
                &dest.name,
                dest.sync_interval_secs as u64,
                SyncKind::Destination(id),
                state.clone(),
                move || {
                    let state = state.clone();
                    async move {
                        let d = {
                            let db = state.db.lock().unwrap();
                            match caldav_ics_sync::db::get_destination(&db, id) {
                                Ok(Some(d)) => d,
                                _ => {
                                    return Err(RetryError::permanent(SyncError::permanent(
                                        anyhow::anyhow!("Destination {} no longer exists", id),
                                    )));
                                }
                            }
                        };
                        let (uploaded, total) =
                            caldav_ics_sync::api::reverse_sync::run_reverse_sync(
                                &d.ics_url,
                                &d.caldav_url,
                                &d.calendar_name,
                                &d.username,
                                &d.password,
                                d.sync_all,
                                d.keep_local,
                            )
                            .await
                            .map_err(|e| RetryError::transient(SyncError::transient(e)))?;
                        let db = state.db.lock().unwrap();
                        caldav_ics_sync::db::update_destination_sync_status(&db, id, "ok", None)
                            .map_err(|e| RetryError::transient(SyncError::transient(e)))?;
                        Ok(format!(
                            "Auto-sync destination {}: uploaded {} of {} events",
                            id, uploaded, total
                        ))
                    }
                },
            );
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!("Received Ctrl+C signal, initiating graceful shutdown...");
        },
        _ = terminate => {
            info!("Received terminate signal, initiating graceful shutdown...");
        },
    }
}
