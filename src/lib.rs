pub mod db;
pub mod gateway;
pub mod rest;
pub mod snowflake;

use axum::response::IntoResponse;
use std::sync::Arc;
use tokio::sync::broadcast;

pub struct AppState {
    pub db: db::Db,
    pub event_tx: broadcast::Sender<String>,
    pub gateway_url: String,
}

pub fn build_app(state: Arc<AppState>) -> axum::Router {
    // No rate-limit headers needed: serenity only honours the proxy with its
    // ratelimiter disabled, and the disabled path never reads response rate
    // headers. OpenAB disables the limiter automatically when proxy is set.
    let mut app = rest::router(state.clone()).merge(gateway::router(state));
    // Public read-only guard: external requests (Host not *.internal) may only
    // GET the viewer + thread/health endpoints. Bots reach the full API over the
    // Zeabur internal hostname. Enabled with HUB_DEMO_MODE=1.
    if std::env::var("HUB_DEMO_MODE").is_ok() {
        app = app.layer(axum::middleware::from_fn(public_readonly_guard));
    }
    // Opt-in access log: HUB_ACCESS_LOG=1 logs every REST method+path. Handy for
    // verifying exactly which Discord endpoints a connected bot exercises.
    if std::env::var("HUB_ACCESS_LOG").is_ok() {
        app.layer(axum::middleware::from_fn(access_log))
    } else {
        app
    }
}

/// Restrict external (non-internal) traffic to a read-only allowlist so a bound
/// public domain can't trigger bots (`/webhook`), impersonate them (REST writes),
/// or leak config (`/bot-config`). Internal requests (`*.internal`) pass through.
async fn public_readonly_guard(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::http::{Method, StatusCode};
    let host = req.headers().get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok()).unwrap_or("");
    let internal = host.contains(".internal") || host.starts_with("127.0.0.1") || host.starts_with("localhost");
    if !internal {
        let path = req.uri().path();
        let allowed = req.method() == Method::GET
            && (path == "/" || path == "/health" || path == "/threads" || path.starts_with("/threads/"));
        if !allowed {
            return (StatusCode::FORBIDDEN, "read-only demo endpoint").into_response();
        }
    }
    next.run(req).await
}

async fn access_log(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let resp = next.run(req).await;
    tracing::info!(%method, path, status = resp.status().as_u16(), "request");
    resp
}
