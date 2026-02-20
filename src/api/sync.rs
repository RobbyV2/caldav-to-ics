use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use chrono::Utc;
use reqwest::{Client, header};
use roxmltree::Document;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct AppState {
    pub ics_cache: Arc<RwLock<Option<String>>>,
    pub last_synced: Arc<RwLock<Option<chrono::DateTime<Utc>>>>,
}

#[derive(Debug, Serialize)]
pub struct SyncResponse {
    status: String,
    message: String,
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    last_synced: Option<chrono::DateTime<Utc>>,
}

async fn fetch_calendars(client: &Client, url: &str) -> anyhow::Result<Vec<String>> {
    let propfind_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
     <d:resourcetype />
     <d:displayname />
     <c:supported-calendar-component-set />
  </d:prop>
</d:propfind>"#;

    let res = client
        .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), url)
        .header("Depth", "1")
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(propfind_body)
        .send()
        .await?
        .error_for_status()?;

    let text = res.text().await?;
    let doc = Document::parse(&text)?;

    let mut calendar_urls = Vec::new();

    for node in doc.descendants() {
        if node.has_tag_name(("DAV:", "response")) {
            let mut is_calendar = false;
            let mut href = None;

            for child in node.children() {
                if child.has_tag_name(("DAV:", "href")) {
                    href = child.text();
                } else if child.has_tag_name(("DAV:", "propstat")) {
                    for propstat_child in child.children() {
                        if propstat_child.has_tag_name(("DAV:", "prop")) {
                            for prop in propstat_child.children() {
                                if prop.has_tag_name(("DAV:", "resourcetype")) {
                                    for rt_child in prop.children() {
                                        if rt_child.has_tag_name((
                                            "urn:ietf:params:xml:ns:caldav",
                                            "calendar",
                                        )) {
                                            is_calendar = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if is_calendar && let Some(h) = href {
                calendar_urls.push(h.to_string());
            }
        }
    }

    Ok(calendar_urls)
}

async fn fetch_events(
    client: &Client,
    base_url: &str,
    calendar_path: &str,
) -> anyhow::Result<Vec<String>> {
    let url = if calendar_path.starts_with("http") {
        calendar_path.to_string()
    } else {
        let parsed = reqwest::Url::parse(base_url)?;
        format!(
            "{}://{}{}",
            parsed.scheme(),
            parsed.host_str().unwrap_or(""),
            calendar_path
        )
    };

    let report_body = r#"<?xml version="1.0" encoding="utf-8" ?>
<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:prop>
    <d:getetag />
    <c:calendar-data />
  </d:prop>
  <c:filter>
    <c:comp-filter name="VCALENDAR">
      <c:comp-filter name="VEVENT" />
    </c:comp-filter>
  </c:filter>
</c:calendar-query>"#;

    let res = client
        .request(reqwest::Method::from_bytes(b"REPORT").unwrap(), &url)
        .header("Depth", "1")
        .header(header::CONTENT_TYPE, "application/xml; charset=utf-8")
        .body(report_body)
        .send()
        .await?;

    let text = res.text().await?;
    let doc = Document::parse(&text)?;

    let mut ics_events = Vec::new();

    for node in doc.descendants() {
        if node.has_tag_name(("urn:ietf:params:xml:ns:caldav", "calendar-data"))
            && let Some(data) = node.text()
        {
            ics_events.push(data.to_string());
        }
    }

    Ok(ics_events)
}

pub async fn run_sync() -> Result<(usize, usize, String)> {
    let caldav_url = std::env::var("CALDAV_URL").context("Missing CALDAV_URL")?;
    let caldav_username = std::env::var("CALDAV_USERNAME").context("Missing CALDAV_USERNAME")?;
    let caldav_password = std::env::var("CALDAV_PASSWORD").context("Missing CALDAV_PASSWORD")?;

    // Ignore default client since we define an authenticated one next

    // In HTTP Basic Auth, we can just prepend username:password to the URL if supported,
    // or set basic auth manually. It's safer to just set basic auth on each request, but reqwest Client
    // requires setting it per request. Let's create an authenticated request builder.
    // Wait, let's inject authorization headers manually via default_headers.
    let mut headers = header::HeaderMap::new();
    let auth = format!("{}:{}", caldav_username, caldav_password);
    let auth_header = format!(
        "Basic {}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &auth)
    );
    headers.insert(
        header::AUTHORIZATION,
        header::HeaderValue::from_str(&auth_header)?,
    );

    let client = Client::builder().default_headers(headers).build()?;

    let calendar_paths = fetch_calendars(&client, &caldav_url).await?;
    let calendar_count = calendar_paths.len();

    let mut combined_events = Vec::new();
    let mut event_count = 0;

    for path in &calendar_paths {
        if let Ok(events_data) = fetch_events(&client, &caldav_url, path).await {
            for ics_str in events_data {
                let mut in_vevent = false;
                let mut current_event = String::new();
                for line in ics_str.lines() {
                    if line.starts_with("BEGIN:VEVENT") {
                        in_vevent = true;
                    }
                    if in_vevent {
                        current_event.push_str(line);
                        current_event.push_str("\r\n");
                    }
                    if line.starts_with("END:VEVENT") {
                        in_vevent = false;
                        combined_events.push(current_event.clone());
                        current_event.clear();
                        event_count += 1;
                    }
                }
            }
        }
    }

    let mut output = String::new();
    output.push_str("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//CalDAV to ICS//EN\r\nCALSCALE:GREGORIAN\r\nMETHOD:PUBLISH\r\n");
    for ev in combined_events {
        output.push_str(&ev);
    }
    output.push_str("END:VCALENDAR\r\n");

    Ok((event_count, calendar_count, output))
}

async fn sync_handler(State(state): State<AppState>) -> impl IntoResponse {
    match run_sync().await {
        Ok((events, calendars, ical_data)) => {
            // Save to memory
            let mut cache = state.ics_cache.write().await;
            *cache = Some(ical_data.clone());

            let mut last_synced = state.last_synced.write().await;
            *last_synced = Some(Utc::now());

            // Handle disk storage
            let strategy =
                std::env::var("STORAGE_STRATEGY").unwrap_or_else(|_| "memory-only".to_string());
            if (strategy == "disk-only" || strategy == "memory-and-disk")
                && let Ok(path) = std::env::var("STORAGE_DISK_PATH")
                && let Err(e) = tokio::fs::write(&path, ical_data).await
            {
                tracing::error!("Failed to save to disk: {}", e);
            }

            if strategy == "disk-only" {
                // We keep memory empty if disk-only
                *cache = None;
            }

            (
                StatusCode::OK,
                Json(SyncResponse {
                    status: "success".into(),
                    message: format!(
                        "Successfully synchronised {} events from {} calendars",
                        events, calendars
                    ),
                }),
            )
        }
        Err(e) => {
            tracing::error!("Sync error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SyncResponse {
                    status: "error".into(),
                    message: e.to_string(),
                }),
            )
        }
    }
}

async fn status_handler(State(state): State<AppState>) -> impl IntoResponse {
    let last_synced = state.last_synced.read().await;
    (
        StatusCode::OK,
        Json(StatusResponse {
            last_synced: *last_synced,
        }),
    )
}

async fn download_ics(State(state): State<AppState>) -> impl IntoResponse {
    use axum::body::Body;
    use axum::response::Response;

    let strategy = std::env::var("STORAGE_STRATEGY").unwrap_or_else(|_| "memory-only".to_string());

    if strategy == "disk-only" {
        if let Ok(path) = std::env::var("STORAGE_DISK_PATH")
            && let Ok(file) = tokio::fs::File::open(&path).await
        {
            let stream = tokio_util::io::ReaderStream::new(file);
            let body = Body::from_stream(stream);

            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/calendar")
                .body(body)
                .unwrap();
        }
        return (StatusCode::NOT_FOUND, "No ICS file generated yet").into_response();
    }

    let cache = state.ics_cache.read().await;
    if let Some(ref data) = *cache {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/calendar")
            .body(Body::from(data.clone()))
            .unwrap()
    } else {
        (StatusCode::NOT_FOUND, "No ICS data generated yet").into_response()
    }
}

pub fn start_auto_sync(state: AppState) {
    let interval_str =
        std::env::var("AUTO_SYNC_INTERVAL_MINUTES").unwrap_or_else(|_| "0".to_string());
    if let Ok(minutes) = interval_str.parse::<u64>() {
        if minutes > 0 {
            tokio::spawn(async move {
                let mut interval =
                    tokio::time::interval(std::time::Duration::from_secs(minutes * 60));
                // Tick immediately once, or skip first tick if we want it to wait
                // interval.tick().await;

                loop {
                    interval.tick().await;
                    match run_sync().await {
                        Ok((events, calendars, ical_data)) => {
                            let mut cache = state.ics_cache.write().await;
                            *cache = Some(ical_data.clone());

                            let mut last_synced = state.last_synced.write().await;
                            *last_synced = Some(Utc::now());

                            let strategy = std::env::var("STORAGE_STRATEGY")
                                .unwrap_or_else(|_| "memory-only".to_string());
                            if (strategy == "disk-only" || strategy == "memory-and-disk")
                                && let Ok(path) = std::env::var("STORAGE_DISK_PATH")
                                && let Err(e) = tokio::fs::write(&path, ical_data).await
                            {
                                tracing::error!("Auto-sync: Failed to save to disk: {}", e);
                            }

                            if strategy == "disk-only" {
                                *cache = None;
                            }
                            tracing::info!(
                                "Auto-sync completed: {} events from {} calendars",
                                events,
                                calendars
                            );
                        }
                        Err(e) => {
                            tracing::error!("Auto-sync failed: {}", e);
                        }
                    }
                }
            });
            tracing::info!("Background auto-sync enabled (Every {} minutes)", minutes);
        } else {
            tracing::info!("Background auto-sync disabled (interval is 0)");
        }
    } else {
        tracing::info!("Background auto-sync disabled (invalid interval)");
    }
}

pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/sync", post(sync_handler))
        .route("/sync/status", get(status_handler))
        .route("/sync/ics", get(download_ics))
        .with_state(state)
}
