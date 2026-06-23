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
    rest::router(state.clone()).merge(gateway::router(state))
}
