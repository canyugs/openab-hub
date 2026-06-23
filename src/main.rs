mod db;
mod gateway;
mod rest;
mod snowflake;

use std::{env, sync::Arc};
use tokio::sync::broadcast;
use tracing_subscriber::EnvFilter;

pub struct AppState {
    pub db: db::Db,
    pub event_tx: broadcast::Sender<String>,
    pub gateway_url: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("openab_hub=info".parse().unwrap()))
        .init();

    let listen = env::var("HUB_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let db_path = env::var("HUB_DB").unwrap_or_else(|_| "hub.db".into());
    let guild_id: u64 = env::var("HUB_GUILD_ID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let db = db::Db::open(&db_path).expect("failed to open database");
    db.ensure_guild(guild_id, "openab-hub");

    if let Ok(channels) = env::var("HUB_CHANNELS") {
        for pair in channels.split(',') {
            if let Some((id_str, name)) = pair.split_once(':') {
                if let Ok(id) = id_str.trim().parse::<u64>() {
                    db.ensure_channel(id, guild_id, name.trim(), 0);
                }
            }
        }
    }

    let (event_tx, _) = broadcast::channel::<String>(256);
    let public_url = env::var("HUB_PUBLIC_URL").unwrap_or_else(|_| format!("ws://localhost:{}", listen.split(':').last().unwrap_or("8080")));

    let state = Arc::new(AppState {
        db,
        event_tx,
        gateway_url: format!("{}/gateway", public_url),
    });

    let app = rest::router(state.clone()).merge(gateway::router(state.clone()));

    let listener = tokio::net::TcpListener::bind(&listen).await.expect("failed to bind");
    tracing::info!("listening on {listen}");
    axum::serve(listener, app).await.unwrap();
}
