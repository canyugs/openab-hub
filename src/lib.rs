pub mod db;
pub mod gateway;
pub mod rest;
pub mod snowflake;

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
    let app = rest::router(state.clone()).merge(gateway::router(state));
    // Opt-in access log: HUB_ACCESS_LOG=1 logs every REST method+path. Handy for
    // verifying exactly which Discord endpoints a connected bot exercises.
    if std::env::var("HUB_ACCESS_LOG").is_ok() {
        app.layer(axum::middleware::from_fn(access_log))
    } else {
        app
    }
}

async fn access_log(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let resp = next.run(req).await;
    tracing::info!(%method, path, status = resp.status().as_u16(), "request");
    resp
}
