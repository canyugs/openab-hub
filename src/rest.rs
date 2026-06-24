use std::sync::Arc;
use axum::{
    Router, Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, patch, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::AppState;
use crate::db;
use crate::snowflake;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        // Discord-compatible REST API
        .route("/api/v10/gateway/bot", get(get_gateway_bot))
        .route("/api/v10/gateway", get(get_gateway))
        .route("/api/v10/channels/{channel_id}/messages", post(create_message))
        .route("/api/v10/channels/{channel_id}/messages", get(get_messages))
        .route("/api/v10/channels/{channel_id}/messages/{message_id}", patch(edit_message))
        .route("/api/v10/channels/{channel_id}/messages/{message_id}", delete(delete_message))
        .route("/api/v10/channels/{channel_id}", get(get_channel))
        .route("/api/v10/channels/{channel_id}", patch(modify_channel))
        .route("/api/v10/channels/{channel_id}/messages/{message_id}/threads", post(create_thread))
        .route("/api/v10/channels/{channel_id}/messages/{message_id}/reactions/{emoji}/@me", put(add_reaction))
        .route("/api/v10/channels/{channel_id}/messages/{message_id}/reactions/{emoji}/@me", delete(remove_reaction))
        .route("/api/v10/applications/{app_id}/commands", put(register_commands))
        .route("/api/v10/applications/{app_id}/commands", get(register_commands))
        .route("/api/v10/applications/{app_id}/guilds/{guild_id}/commands", put(register_guild_commands))
        .route("/api/v10/applications/{app_id}/guilds/{guild_id}/commands", get(register_guild_commands))
        // Non-Discord endpoints
        .route("/webhook", post(github_webhook))
        .route("/threads", get(list_threads))
        .route("/threads/{thread_id}", get(get_thread_messages))
        .route("/health", get(health))
        // Serve an OpenAB bot config so bots can `-c http://hub/bot-config`
        // instead of mounting/baking a file.
        //   /bot-config        → token via ${DISCORD_BOT_TOKEN} env placeholder
        //   /bot-config/{id}    → fully self-contained: token derived from id,
        //                         trusted_bot_ids auto-filled with every other
        //                         registered bot (fleet-aware, no env needed)
        .route("/bot-config", get(bot_config))
        .route("/bot-config/{id}", get(bot_config_for))
        .with_state(state)
}

fn hub_proxy() -> String {
    std::env::var("HUB_CONFIG_PROXY")
        .unwrap_or_else(|_| "http://openab-hub.zeabur.internal:8080".into())
}

fn render_config(bot_token: &str, proxy: &str, trusted_bot_ids: &[u64]) -> String {
    let trusted = trusted_bot_ids.iter()
        .map(|id| format!("\"{id}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"[discord]
bot_token = "{bot_token}"
proxy = "{proxy}"
allow_all_channels = true
allow_all_users = true
allow_bot_messages = "mentions"
trusted_bot_ids = [{trusted}]

[agent]
command = "claude-agent-acp"
working_dir = "/home/node"

[pool]
max_sessions = 1
session_ttl_hours = 2
"#
    )
}

async fn bot_config() -> impl IntoResponse {
    let toml = render_config("${DISCORD_BOT_TOKEN}", &hub_proxy(), &[]);
    ([(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")], toml)
}

async fn bot_config_for(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    // Derive a hub token from the id (base64 of the id, same scheme the gateway
    // decodes). No real secret — the hub model identifies bots by id.
    use base64::Engine;
    let token = format!(
        "{}.hub.gen",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(id.to_string())
    );
    // Trust every other registered bot in the fleet.
    let trusted: Vec<u64> = state.db.get_all_bot_ids().into_iter().filter(|b| *b != id).collect();
    let toml = render_config(&token, &hub_proxy(), &trusted);
    ([(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")], toml)
}

fn extract_bot_from_header(headers: &HeaderMap, state: &AppState) -> Option<db::Bot> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    let token = auth.strip_prefix("Bot ").unwrap_or(auth);
    state.db.get_bot_by_token(token)
}

/// Parse Discord mention tokens from content, mirroring what Discord's API does
/// server-side: `<@id>`/`<@!id>` → user mentions, `<@&id>` → role mentions.
/// Returns (user_ids, role_ids).
fn parse_mentions(content: &str) -> (Vec<u64>, Vec<u64>) {
    let mut users = Vec::new();
    let mut roles = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' && i + 1 < bytes.len() && bytes[i + 1] == b'@' {
            let mut j = i + 2;
            let is_role = j < bytes.len() && bytes[j] == b'&';
            if is_role {
                j += 1;
            } else if j < bytes.len() && bytes[j] == b'!' {
                j += 1; // <@!id> nickname mention
            }
            let start = j;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > start && j < bytes.len() && bytes[j] == b'>' {
                if let Ok(id) = content[start..j].parse::<u64>() {
                    if is_role { roles.push(id) } else { users.push(id) }
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    (users, roles)
}

fn message_to_json(msg: &db::Message, state: &AppState) -> Value {
    let bot_info = state.db.get_bot(msg.author_id);
    let username = bot_info.as_ref().map(|b| b.username.as_str()).unwrap_or(&msg.author_name);
    let guild_id = state.db.get_guild_id().unwrap_or(1);

    let (mention_users, mention_roles) = parse_mentions(&msg.content);
    let mentions_json: Vec<Value> = mention_users.iter().map(|uid| {
        let name = state.db.get_bot(*uid).map(|b| b.username).unwrap_or_else(|| format!("user-{uid}"));
        json!({
            "id": uid.to_string(),
            "username": name,
            "global_name": name,
            "avatar": null,
            "bot": state.db.get_bot(*uid).is_some()
        })
    }).collect();
    let mention_roles_json: Vec<String> = mention_roles.iter().map(|r| r.to_string()).collect();

    let mut j = json!({
        "id": msg.id.to_string(),
        "channel_id": msg.channel_id.to_string(),
        "guild_id": guild_id.to_string(),
        "content": msg.content,
        "timestamp": msg.timestamp,
        "edited_timestamp": null,
        "tts": false,
        "mention_everyone": false,
        "mentions": mentions_json,
        "mention_roles": mention_roles_json,
        "attachments": [],
        "embeds": [],
        "pinned": false,
        "type": 0,
        "author": {
            "id": msg.author_id.to_string(),
            "username": username,
            "global_name": username,
            "avatar": null,
            "bot": msg.is_bot
        }
    });
    if let Some(ref_id) = msg.reference_id {
        j["message_reference"] = json!({
            "message_id": ref_id.to_string(),
            "channel_id": msg.channel_id.to_string()
        });
    }
    // If a thread was started from this message, embed it — OpenAB reads
    // `message.thread` to recover after the one-thread-per-message (160004) error.
    if let Some(thread) = state.db.get_thread_by_source_message(msg.id) {
        j["thread"] = channel_to_json(&thread);
    }
    j
}

fn dispatch_message_create(state: &AppState, msg_json: &Value) {
    let event = json!({
        "op": 0,
        "t": "MESSAGE_CREATE",
        "d": msg_json
    });
    let _ = state.event_tx.send(serde_json::to_string(&event).unwrap());
}

// --- Discord REST ---

async fn get_gateway_bot(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "url": state.gateway_url,
        "shards": 1,
        "session_start_limit": {
            "total": 1000,
            "remaining": 999,
            "reset_after": 0,
            "max_concurrency": 1
        }
    }))
}

async fn get_gateway(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({ "url": state.gateway_url }))
}

#[derive(Deserialize)]
struct CreateMessageBody {
    content: Option<String>,
    message_reference: Option<MessageReference>,
}

#[derive(Deserialize)]
struct MessageReference {
    message_id: Option<String>,
}

async fn create_message(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(channel_id): Path<u64>,
    Json(body): Json<CreateMessageBody>,
) -> impl IntoResponse {
    let bot = match extract_bot_from_header(&headers, &state) {
        Some(b) => b,
        None => return (StatusCode::UNAUTHORIZED, Json(json!({"message": "unauthorized"}))),
    };

    let content = body.content.unwrap_or_default();
    let reference_id = body.message_reference
        .and_then(|r| r.message_id)
        .and_then(|id| id.parse::<u64>().ok());

    let now = chrono_now();
    let msg_id = snowflake::generate();

    let msg = db::Message {
        id: msg_id,
        channel_id,
        author_id: bot.user_id,
        author_name: bot.username.clone(),
        is_bot: true,
        content,
        timestamp: now,
        reference_id,
    };

    state.db.insert_message(&msg);
    let msg_json = message_to_json(&msg, &state);
    dispatch_message_create(&state, &msg_json);

    (StatusCode::OK, Json(msg_json))
}

#[derive(Deserialize)]
struct GetMessagesQuery {
    limit: Option<u32>,
}

async fn get_messages(
    State(state): State<Arc<AppState>>,
    Path(channel_id): Path<u64>,
    Query(q): Query<GetMessagesQuery>,
) -> Json<Value> {
    let limit = q.limit.unwrap_or(50).min(100);
    let messages = state.db.get_channel_messages(channel_id, limit);
    let json_msgs: Vec<Value> = messages.iter().map(|m| message_to_json(m, &state)).collect();
    Json(json!(json_msgs))
}

#[derive(Deserialize)]
struct EditMessageBody {
    content: Option<String>,
}

async fn edit_message(
    State(state): State<Arc<AppState>>,
    Path((_channel_id, message_id)): Path<(u64, u64)>,
    Json(body): Json<EditMessageBody>,
) -> impl IntoResponse {
    if let Some(content) = body.content {
        state.db.update_message_content(message_id, &content);
    }
    match state.db.get_message(message_id) {
        Some(msg) => {
            let j = message_to_json(&msg, &state);
            // Dispatch MESSAGE_UPDATE for streaming edits
            let event = json!({ "op": 0, "t": "MESSAGE_UPDATE", "d": &j });
            let _ = state.event_tx.send(serde_json::to_string(&event).unwrap());
            (StatusCode::OK, Json(j))
        }
        None => (StatusCode::NOT_FOUND, Json(json!({"message": "Unknown Message"}))),
    }
}

async fn delete_message(
    State(state): State<Arc<AppState>>,
    Path((_channel_id, message_id)): Path<(u64, u64)>,
) -> StatusCode {
    state.db.delete_message(message_id);
    StatusCode::NO_CONTENT
}

async fn get_channel(
    State(state): State<Arc<AppState>>,
    Path(channel_id): Path<u64>,
) -> impl IntoResponse {
    match state.db.get_channel(channel_id) {
        Some(ch) => (StatusCode::OK, Json(channel_to_json(&ch))),
        None => (StatusCode::NOT_FOUND, Json(json!({"message": "Unknown Channel"}))),
    }
}

fn channel_to_json(ch: &db::Channel) -> Value {
    let mut j = json!({
        "id": ch.id.to_string(),
        "guild_id": ch.guild_id.to_string(),
        "name": ch.name,
        "type": ch.channel_type,
        "position": 0,
        "permission_overwrites": [],
        "topic": null,
        "nsfw": false,
        "last_message_id": null
    });
    if let Some(pid) = ch.parent_id {
        j["parent_id"] = json!(pid.to_string());
    }
    // Threads (type 11) carry thread_metadata + owner_id — this is how serenity
    // (and OpenAB's detect_thread) recognize a channel as a thread. Without it,
    // a bot replying in a thread thinks it's a top-level channel and creates a
    // nested thread, matching Discord behavior exactly.
    if ch.channel_type == 11 {
        j["thread_metadata"] = json!({
            "archived": false,
            "auto_archive_duration": 1440,
            "archive_timestamp": null,
            "locked": false
        });
        if let Some(oid) = ch.owner_id {
            j["owner_id"] = json!(oid.to_string());
        }
    }
    j
}

#[derive(Deserialize)]
struct ModifyChannelBody {
    name: Option<String>,
}

async fn modify_channel(
    State(state): State<Arc<AppState>>,
    Path(channel_id): Path<u64>,
    Json(body): Json<ModifyChannelBody>,
) -> impl IntoResponse {
    if let Some(name) = body.name {
        state.db.rename_channel(channel_id, &name);
    }
    match state.db.get_channel(channel_id) {
        Some(ch) => (StatusCode::OK, Json(channel_to_json(&ch))),
        None => (StatusCode::NOT_FOUND, Json(json!({"message": "Unknown Channel"}))),
    }
}

#[derive(Deserialize)]
struct CreateThreadBody {
    name: String,
}

async fn create_thread(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((channel_id, message_id)): Path<(u64, u64)>,
    Json(body): Json<CreateThreadBody>,
) -> impl IntoResponse {
    // Discord rule: one thread per message. A second create from the same
    // message returns error 160004 — OpenAB catches it and joins the existing
    // thread (the multi-bot race resolves to a single shared thread).
    if state.db.get_thread_by_source_message(message_id).is_some() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"code": 160004, "message": "A thread has already been created for this message"})),
        );
    }

    let guild_id = state.db.get_guild_id().unwrap_or(1);
    let owner_id = extract_bot_from_header(&headers, &state).map(|b| b.user_id);
    let thread_id = snowflake::generate();
    state.db.create_channel(thread_id, guild_id, &body.name, 11, Some(channel_id), owner_id, Some(message_id));
    let ch = db::Channel {
        id: thread_id,
        guild_id,
        name: body.name,
        channel_type: 11,
        parent_id: Some(channel_id),
        owner_id,
        source_message_id: Some(message_id),
    };
    (StatusCode::OK, Json(channel_to_json(&ch)))
}

async fn add_reaction(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_channel_id, message_id, emoji)): Path<(u64, u64, String)>,
) -> StatusCode {
    let bot = match extract_bot_from_header(&headers, &state) {
        Some(b) => b,
        None => return StatusCode::UNAUTHORIZED,
    };
    state.db.add_reaction(message_id, bot.user_id, &emoji);
    StatusCode::NO_CONTENT
}

async fn remove_reaction(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_channel_id, message_id, emoji)): Path<(u64, u64, String)>,
) -> StatusCode {
    let bot = match extract_bot_from_header(&headers, &state) {
        Some(b) => b,
        None => return StatusCode::UNAUTHORIZED,
    };
    state.db.remove_reaction(message_id, bot.user_id, &emoji);
    StatusCode::NO_CONTENT
}

// No-op: accept any command registration, always report empty command set.
// Body is ignored (GET has none, PUT sends an array). Returning `[]` keeps
// serenity happy — it decodes a valid JSON array instead of an empty body.
async fn register_commands(Path(_app_id): Path<u64>) -> Json<Value> {
    Json(json!([]))
}

async fn register_guild_commands(Path((_app_id, _guild_id)): Path<(u64, u64)>) -> Json<Value> {
    Json(json!([]))
}

// --- Non-Discord endpoints ---

#[derive(Deserialize)]
struct WebhookBody {
    channel_id: u64,
    content: String,
    username: Option<String>,
    author_id: Option<u64>,
}

// Stable webhook author ID (non-zero, won't collide with bot snowflakes)
const WEBHOOK_AUTHOR_ID: u64 = 1;

async fn github_webhook(
    State(state): State<Arc<AppState>>,
    Json(body): Json<WebhookBody>,
) -> impl IntoResponse {
    let msg_id = snowflake::generate();
    let msg = db::Message {
        id: msg_id,
        channel_id: body.channel_id,
        author_id: body.author_id.unwrap_or(WEBHOOK_AUTHOR_ID),
        author_name: body.username.unwrap_or_else(|| "github".into()),
        is_bot: false,
        content: body.content,
        timestamp: chrono_now(),
        reference_id: None,
    };
    state.db.insert_message(&msg);
    let msg_json = message_to_json(&msg, &state);
    dispatch_message_create(&state, &msg_json);

    (StatusCode::OK, Json(msg_json))
}

#[derive(Serialize)]
struct ThreadSummary {
    id: String,
    name: String,
    parent_id: Option<String>,
}

async fn list_threads(State(state): State<Arc<AppState>>) -> Json<Vec<ThreadSummary>> {
    let threads = state.db.get_threads();
    Json(threads.iter().map(|t| ThreadSummary {
        id: t.id.to_string(),
        name: t.name.clone(),
        parent_id: t.parent_id.map(|id| id.to_string()),
    }).collect())
}

async fn get_thread_messages(
    State(state): State<Arc<AppState>>,
    Path(thread_id): Path<u64>,
) -> Json<Value> {
    let messages = state.db.get_channel_messages(thread_id, 100);
    let json_msgs: Vec<Value> = messages.iter().map(|m| message_to_json(m, &state)).collect();
    Json(json!(json_msgs))
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    connected_bots: usize,
}

async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        connected_bots: state.event_tx.receiver_count(),
    })
}

fn chrono_now() -> String {
    // ISO 8601 timestamp without pulling in the chrono crate
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;
    // Approximate date from days since epoch
    let (year, month, day) = days_to_date(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

fn days_to_date(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::parse_mentions;

    #[test]
    fn mentions_parse() {
        assert_eq!(parse_mentions("<@12345> hi"), (vec![12345], vec![]));
        assert_eq!(parse_mentions("<@!999> yo"), (vec![999], vec![]));
        assert_eq!(parse_mentions("ping <@&77> role"), (vec![], vec![77]));
        assert_eq!(parse_mentions("<@1> and <@&2> and <@3>"), (vec![1, 3], vec![2]));
        assert_eq!(parse_mentions("no mentions here"), (vec![], vec![]));
        assert_eq!(parse_mentions("<@notanumber> <@>"), (vec![], vec![]));
    }
}
