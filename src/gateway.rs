use std::sync::Arc;
use axum::{Router, extract::{State, WebSocketUpgrade, ws::{Message, WebSocket}}, response::IntoResponse, routing::get};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::AppState;
use crate::db::Bot;

const HEARTBEAT_INTERVAL_MS: u64 = 41250;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/gateway", get(ws_handler))
        .route("/gateway/", get(ws_handler))
        .with_state(state)
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_connection(socket, state))
}

async fn handle_connection(mut socket: WebSocket, state: Arc<AppState>) {
    // Step 1: Send HELLO (opcode 10)
    let hello = json!({
        "op": 10,
        "d": { "heartbeat_interval": HEARTBEAT_INTERVAL_MS }
    });
    if send_json(&mut socket, &hello).await.is_err() {
        return;
    }

    // Step 2: Wait for IDENTIFY (opcode 2)
    let bot = match wait_for_identify(&mut socket, &state).await {
        Some(b) => b,
        None => return,
    };

    info!(bot_id = bot.user_id, username = %bot.username, "bot connected");

    // Step 3: Send READY (opcode 0, t: READY)
    let guild_id = state.db.get_guild_id().unwrap_or(1);
    let ready = json!({
        "op": 0,
        "s": 1,
        "t": "READY",
        "d": {
            "v": 10,
            "user": {
                "id": bot.user_id.to_string(),
                "username": bot.username,
                "global_name": bot.username,
                "bot": true,
                "avatar": null
            },
            "guilds": [{ "id": guild_id.to_string(), "unavailable": false }],
            "session_id": format!("hub-{}", bot.user_id),
            "resume_gateway_url": state.gateway_url,
            "application": { "id": bot.user_id.to_string(), "flags": 0 }
        }
    });
    if send_json(&mut socket, &ready).await.is_err() {
        return;
    }

    // Step 3.5: Send GUILD_CREATE (serenity needs this before dispatching guild events)
    let channels = state.db.get_channels_by_guild(guild_id);
    let channels_json: Vec<serde_json::Value> = channels.iter().map(|ch| {
        json!({
            "id": ch.id.to_string(),
            "name": ch.name,
            "type": ch.channel_type,
            "position": 0,
            "permission_overwrites": [],
            "parent_id": ch.parent_id.map(|id| id.to_string()),
        })
    }).collect();

    let guild_create = json!({
        "op": 0,
        "s": 2,
        "t": "GUILD_CREATE",
        "d": {
            "id": guild_id.to_string(),
            "name": "openab-hub",
            "owner_id": bot.user_id.to_string(),
            "channels": channels_json,
            "members": [],
            "roles": [{
                "id": guild_id.to_string(),
                "name": "@everyone",
                "permissions": "2147483647",
                "position": 0,
                "color": 0,
                "hoist": false,
                "managed": false,
                "mentionable": false
            }],
            "presences": [],
            "threads": [],
            "voice_states": [],
            "emojis": [],
            "features": [],
            "unavailable": false
        }
    });
    if send_json(&mut socket, &guild_create).await.is_err() {
        return;
    }

    // Step 4: Event loop — forward broadcasts + handle heartbeats
    let mut event_rx = state.event_tx.subscribe();
    let mut seq: u64 = 3;

    loop {
        tokio::select! {
            // Incoming from bot
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(payload) = serde_json::from_str::<Value>(&text) {
                            match payload.get("op").and_then(|v| v.as_u64()) {
                                Some(1) => {
                                    // HEARTBEAT → ACK
                                    let ack = json!({ "op": 11 });
                                    if send_json(&mut socket, &ack).await.is_err() {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            // Outgoing events from broadcast
            event = event_rx.recv() => {
                match event {
                    Ok(json_str) => {
                        if let Ok(mut payload) = serde_json::from_str::<Value>(&json_str) {
                            // Skip events authored by this bot
                            if let Some(author_id) = payload.pointer("/d/author/id").and_then(|v| v.as_str()) {
                                if author_id == bot.user_id.to_string() {
                                    continue;
                                }
                            }
                            seq += 1;
                            payload["s"] = json!(seq);
                            if send_json(&mut socket, &payload).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(bot_id = bot.user_id, lagged = n, "bot lagged behind");
                    }
                    Err(_) => break,
                }
            }
        }
    }

    info!(bot_id = bot.user_id, "bot disconnected");
}

async fn wait_for_identify(socket: &mut WebSocket, state: &AppState) -> Option<Bot> {
    let timeout = tokio::time::sleep(std::time::Duration::from_secs(30));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(payload) = serde_json::from_str::<Value>(&text) {
                            if payload.get("op").and_then(|v| v.as_u64()) == Some(2) {
                                let token = payload.pointer("/d/token")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let clean_token = token.strip_prefix("Bot ").unwrap_or(token);

                                // Try to find existing bot by token
                                if let Some(bot) = state.db.get_bot_by_token(clean_token) {
                                    return Some(bot);
                                }

                                // Decode user ID from token (base64 of user ID is first segment)
                                if let Some(user_id) = decode_user_id_from_token(clean_token) {
                                    // Keep a pre-seeded friendly name if one exists.
                                    if let Some(bot) = state.db.get_bot(user_id) {
                                        state.db.set_bot_token(user_id, clean_token);
                                        return Some(bot);
                                    }
                                    let username = format!("bot-{}", user_id);
                                    state.db.register_bot(user_id, &username, clean_token);
                                    return Some(Bot { user_id, username });
                                }

                                warn!("failed to decode bot token");
                                return None;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return None,
                    _ => {}
                }
            }
            _ = &mut timeout => {
                warn!("identify timeout");
                return None;
            }
        }
    }
}

fn decode_user_id_from_token(token: &str) -> Option<u64> {
    let first_segment = token.split('.').next()?;
    // Discord tokens: first segment is base64-encoded user ID
    let decoded = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD_NO_PAD,
        first_segment,
    ).ok().or_else(|| {
        base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            first_segment,
        ).ok()
    })?;
    let id_str = String::from_utf8(decoded).ok()?;
    id_str.parse::<u64>().ok()
}

async fn send_json(socket: &mut WebSocket, value: &Value) -> Result<(), ()> {
    let text = serde_json::to_string(value).map_err(|_| ())?;
    socket.send(Message::Text(text.into())).await.map_err(|_| ())
}
