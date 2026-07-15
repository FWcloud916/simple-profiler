use std::{fmt::Write as _, net::Ipv4Addr, path::PathBuf, process::Command, sync::Arc};

use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    extract::{Path, Query, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::{net::TcpListener, sync::Semaphore};

use crate::{report::resolve_range, storage::Storage};

const MAX_CONCURRENT_QUERIES: usize = 4;
const INDEX_HTML: &str = include_str!("dashboard/index.html");
const APP_CSS: &str = include_str!("dashboard/app.css");
const APP_JS: &str = include_str!("dashboard/app.js");

#[derive(Clone)]
struct DashboardState {
    database_path: Arc<PathBuf>,
    query_slots: Arc<Semaphore>,
    expected_host: Arc<str>,
}

#[derive(Debug, Deserialize)]
struct RangeQuery {
    last: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

#[derive(Debug, Serialize)]
struct ApiErrorBody {
    error: String,
}

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: error.to_string(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}

pub async fn serve(database_path: PathBuf, port: u16, open: bool) -> Result<()> {
    Storage::open_read_only(&database_path)?;
    let token = session_token()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .await
        .context("failed to bind the local dashboard")?;
    let address = listener.local_addr()?;
    let expected_host: Arc<str> = format!("{}:{}", address.ip(), address.port()).into();
    let state = DashboardState {
        database_path: Arc::new(database_path),
        query_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_QUERIES)),
        expected_host,
    };
    let session = Router::new()
        .route("/assets/app.css", get(styles))
        .route("/assets/app.js", get(script))
        .route("/api/v1/snapshot", get(snapshot))
        .route("/api/v1/status", get(status))
        .route("/api/v1/events/{id}", get(event));
    let app = Router::new()
        .route(&format!("/session/{token}/"), get(index))
        .nest(&format!("/session/{token}"), session)
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            secure_response,
        ))
        .with_state(state);
    let url = format!("http://{address}/session/{token}/");
    println!("dashboard: {url}");
    println!("press Ctrl-C to stop the dashboard; background collection is unaffected");
    if open {
        open_url(&url)?;
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("dashboard server stopped unexpectedly")?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn styles() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS)
}

async fn script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        APP_JS,
    )
}

async fn not_found() -> ApiError {
    ApiError {
        status: StatusCode::NOT_FOUND,
        message: "dashboard session was not found".to_owned(),
    }
}

async fn snapshot(
    State(state): State<DashboardState>,
    Query(query): Query<RangeQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let range = resolve_range(
        query.last.as_deref(),
        query.from.as_deref(),
        query.to.as_deref(),
        Utc::now().timestamp_millis(),
    )
    .map_err(ApiError::bad_request)?;
    let database_path = state.database_path.clone();
    let _permit = state
        .query_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::unavailable("too many dashboard queries are running"))?;
    let data = tokio::task::spawn_blocking(move || {
        let storage = Storage::open_read_only(&database_path)?;
        storage.dashboard_snapshot(range)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?;
    Ok(Json(data))
}

async fn status(State(state): State<DashboardState>) -> Result<impl IntoResponse, ApiError> {
    let database_path = state.database_path.clone();
    let _permit = state
        .query_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::unavailable("too many dashboard queries are running"))?;
    let data = tokio::task::spawn_blocking(move || {
        let storage = Storage::open_read_only(&database_path)?;
        storage.status()
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?;
    Ok(Json(data))
}

async fn event(
    State(state): State<DashboardState>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    if id <= 0 {
        return Err(ApiError::bad_request("event ID must be positive"));
    }
    let database_path = state.database_path.clone();
    let _permit = state
        .query_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::unavailable("too many dashboard queries are running"))?;
    let data = tokio::task::spawn_blocking(move || {
        let storage = Storage::open_read_only(&database_path)?;
        storage.event(id)
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError {
        status: StatusCode::NOT_FOUND,
        message: format!("anomaly event #{id} was not found"),
    })?;
    Ok(Json(data))
}

async fn secure_response(
    State(state): State<DashboardState>,
    request: Request,
    next: Next,
) -> Response {
    let allowed = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|host| host == state.expected_host.as_ref());
    let mut response = if allowed {
        next.run(request).await
    } else {
        ApiError {
            status: StatusCode::FORBIDDEN,
            message: "dashboard accepts only its loopback origin".to_owned(),
        }
        .into_response()
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; font-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    response
}

fn session_token() -> Result<String> {
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random)
        .map_err(|error| anyhow::anyhow!("failed to create a dashboard session token: {error}"))?;
    let mut token = String::with_capacity(random.len() * 2);
    for byte in random {
        let _ = write!(token, "{byte:02x}");
    }
    Ok(token)
}

fn open_url(url: &str) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("--open is currently supported only on macOS");
    }
    let status = Command::new("/usr/bin/open")
        .arg(url)
        .status()
        .context("failed to launch /usr/bin/open")?;
    if !status.success() {
        bail!("could not open the local dashboard");
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to install SIGTERM handler");
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                let _ = result;
            }
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_tokens_are_random_hex_values() {
        let first = session_token().expect("first token");
        let second = session_token().expect("second token");
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|character| character.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn embedded_assets_have_no_remote_dependencies() {
        assert!(!INDEX_HTML.contains("src=\"http"));
        assert!(!INDEX_HTML.contains("href=\"http"));
        assert!(!APP_CSS.contains("url(http"));
        assert!(!APP_CSS.contains("@import"));
        assert!(!APP_JS.contains("fetch(\"http"));
        assert!(!APP_JS.contains("new URL(\"http"));
        assert!(INDEX_HTML.contains("Simple Profiler"));
        assert!(APP_JS.contains("api/v1/snapshot"));
    }

    #[test]
    fn embedded_dashboard_exposes_bounded_timeline_navigation() {
        assert!(INDEX_HTML.contains("id=\"timelineSlider\""));
        assert!(INDEX_HTML.contains("id=\"timelineLive\""));
        assert!(APP_JS.contains("navigateToStart"));
        assert!(APP_JS.contains("pointerdown"));
        assert!(APP_JS.contains("ArrowLeft"));
        assert!(APP_JS.contains("window.setTimeout(refresh, 180)"));
        assert!(APP_CSS.contains("touch-action: pan-y"));
    }

    #[test]
    fn embedded_dashboard_exposes_hover_values_and_ranked_process_lines() {
        assert!(APP_JS.contains("enableChartTooltip"));
        assert!(APP_JS.contains("min ${formatValue(point.min_value"));
        assert!(APP_JS.contains("processSeriesForMetric"));
        assert!(APP_JS.contains("process-rank-${rank}"));
        assert!(APP_CSS.contains(".chart-tooltip"));
        assert!(APP_CSS.contains(".process-line.process-rank-2"));
        assert!(APP_CSS.contains(".process-line.process-rank-3"));
    }
}
