use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use openab_hub::{db::Db, AppState, build_app};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const BOT_TOKEN: &str = "MTIzNDU.fake.token"; // base64("12345") = MTIzNDU → user_id 12345
const BOT2_TOKEN: &str = "Njc4OTA.fake.token"; // base64("67890") = Njc4OTA → user_id 67890
const CHANNEL_ID: u64 = 100;
const GUILD_ID: u64 = 1;

async fn start_server() -> (String, Arc<AppState>) {
    let db = Db::open(":memory:").unwrap();
    db.ensure_guild(GUILD_ID, "test");
    db.ensure_channel(CHANNEL_ID, GUILD_ID, "general", 0);

    let (event_tx, _) = broadcast::channel::<String>(256);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");

    let state = Arc::new(AppState {
        db,
        event_tx,
        gateway_url: format!("ws://127.0.0.1:{port}/gateway"),
    });

    let app = build_app(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // Give server a moment to bind
    tokio::time::sleep(Duration::from_millis(50)).await;
    (base, state)
}

async fn ws_identify(base: &str, token: &str) -> (futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, Message>, futures_util::stream::SplitStream<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>>) {
    let ws_url = base.replace("http://", "ws://") + "/gateway";
    let (ws, _) = connect_async(&ws_url).await.expect("ws connect failed");
    let (mut write, mut read) = ws.split();

    // Expect HELLO
    let hello = read.next().await.unwrap().unwrap();
    let hello: Value = serde_json::from_str(hello.to_text().unwrap()).unwrap();
    assert_eq!(hello["op"], 10);

    // Send IDENTIFY
    let identify = json!({
        "op": 2,
        "d": { "token": token, "intents": 33281 }
    });
    write.send(Message::Text(serde_json::to_string(&identify).unwrap().into())).await.unwrap();

    // Expect READY
    let ready = read.next().await.unwrap().unwrap();
    let ready: Value = serde_json::from_str(ready.to_text().unwrap()).unwrap();
    assert_eq!(ready["t"], "READY");
    assert_eq!(ready["d"]["user"]["bot"], true);

    // Expect GUILD_CREATE
    let guild = read.next().await.unwrap().unwrap();
    let guild: Value = serde_json::from_str(guild.to_text().unwrap()).unwrap();
    assert_eq!(guild["t"], "GUILD_CREATE");

    (write, read)
}

#[tokio::test]
async fn test_health() {
    let (base, _state) = start_server().await;
    let resp: Value = reqwest::get(format!("{base}/health"))
        .await.unwrap().json().await.unwrap();
    assert_eq!(resp["status"], "ok");
}

#[tokio::test]
async fn test_gateway_bot() {
    let (base, _state) = start_server().await;
    let resp: Value = reqwest::get(format!("{base}/api/v10/gateway/bot"))
        .await.unwrap().json().await.unwrap();
    assert!(resp["url"].as_str().unwrap().contains("/gateway"));
    assert_eq!(resp["shards"], 1);
}

#[tokio::test]
async fn test_gateway_identify_and_heartbeat() {
    let (base, _state) = start_server().await;
    let (mut write, mut read) = ws_identify(&base, BOT_TOKEN).await;

    // Send HEARTBEAT
    let hb = json!({ "op": 1, "d": null });
    write.send(Message::Text(serde_json::to_string(&hb).unwrap().into())).await.unwrap();

    // Expect HEARTBEAT_ACK
    let ack = tokio::time::timeout(Duration::from_secs(2), read.next()).await.unwrap().unwrap().unwrap();
    let ack: Value = serde_json::from_str(ack.to_text().unwrap()).unwrap();
    assert_eq!(ack["op"], 11);
}

#[tokio::test]
async fn test_webhook_message_dispatched_to_bot() {
    let (base, _state) = start_server().await;
    let (_write, mut read) = ws_identify(&base, BOT_TOKEN).await;

    // POST webhook message
    let client = reqwest::Client::new();
    let resp = client.post(format!("{base}/webhook"))
        .json(&json!({ "channel_id": CHANNEL_ID, "content": "PR #42 opened", "username": "github" }))
        .send().await.unwrap();
    assert!(resp.status().is_success());

    // Bot should receive MESSAGE_CREATE
    let event = tokio::time::timeout(Duration::from_secs(2), read.next()).await.unwrap().unwrap().unwrap();
    let event: Value = serde_json::from_str(event.to_text().unwrap()).unwrap();
    assert_eq!(event["t"], "MESSAGE_CREATE");
    assert_eq!(event["d"]["content"], "PR #42 opened");
    assert_eq!(event["d"]["author"]["username"], "github");
}

#[tokio::test]
async fn test_bot_sends_message_other_bot_receives() {
    let (base, _state) = start_server().await;

    // Connect bot1 and bot2
    let (_w1, mut read1) = ws_identify(&base, BOT_TOKEN).await;
    let (_w2, mut read2) = ws_identify(&base, BOT2_TOKEN).await;

    // Bot1 sends a message via REST
    let client = reqwest::Client::new();
    let resp = client.post(format!("{base}/api/v10/channels/{CHANNEL_ID}/messages"))
        .header("Authorization", format!("Bot {BOT_TOKEN}"))
        .json(&json!({ "content": "LGTM" }))
        .send().await.unwrap();
    assert!(resp.status().is_success());
    let msg: Value = resp.json().await.unwrap();
    assert_eq!(msg["content"], "LGTM");
    assert_eq!(msg["author"]["id"], "12345");

    // Bot2 should receive it
    let event = tokio::time::timeout(Duration::from_secs(2), read2.next()).await.unwrap().unwrap().unwrap();
    let event: Value = serde_json::from_str(event.to_text().unwrap()).unwrap();
    assert_eq!(event["t"], "MESSAGE_CREATE");
    assert_eq!(event["d"]["content"], "LGTM");

    // Bot1 should NOT receive its own message
    let timeout = tokio::time::timeout(Duration::from_millis(200), read1.next()).await;
    assert!(timeout.is_err(), "bot1 should not receive its own message");
}

#[tokio::test]
async fn test_edit_message() {
    let (base, _state) = start_server().await;

    // Connect bot, send message
    let (_w, mut read) = ws_identify(&base, BOT_TOKEN).await;

    let client = reqwest::Client::new();
    let msg: Value = client.post(format!("{base}/api/v10/channels/{CHANNEL_ID}/messages"))
        .header("Authorization", format!("Bot {BOT_TOKEN}"))
        .json(&json!({ "content": "draft" }))
        .send().await.unwrap().json().await.unwrap();
    let msg_id = msg["id"].as_str().unwrap();

    // Drain the MESSAGE_CREATE from another bot's perspective won't apply here
    // Just edit it
    let edited: Value = client.patch(format!("{base}/api/v10/channels/{CHANNEL_ID}/messages/{msg_id}"))
        .header("Authorization", format!("Bot {BOT_TOKEN}"))
        .json(&json!({ "content": "final review" }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(edited["content"], "final review");
}

#[tokio::test]
async fn test_create_thread() {
    let (base, _state) = start_server().await;

    let client = reqwest::Client::new();

    // Send a message first
    let (_w, _read) = ws_identify(&base, BOT_TOKEN).await;
    let msg: Value = client.post(format!("{base}/api/v10/channels/{CHANNEL_ID}/messages"))
        .header("Authorization", format!("Bot {BOT_TOKEN}"))
        .json(&json!({ "content": "review trigger" }))
        .send().await.unwrap().json().await.unwrap();
    let msg_id = msg["id"].as_str().unwrap();

    // Create thread from message
    let thread: Value = client.post(format!("{base}/api/v10/channels/{CHANNEL_ID}/messages/{msg_id}/threads"))
        .header("Authorization", format!("Bot {BOT_TOKEN}"))
        .json(&json!({ "name": "PR #42 review" }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(thread["name"], "PR #42 review");
    assert_eq!(thread["type"], 11);
    assert_eq!(thread["parent_id"], CHANNEL_ID.to_string());

    // Verify thread appears in /threads
    let threads: Vec<Value> = client.get(format!("{base}/threads"))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0]["name"], "PR #42 review");
}

#[tokio::test]
async fn test_get_channel_messages() {
    let (base, _state) = start_server().await;
    let (_w, _read) = ws_identify(&base, BOT_TOKEN).await;

    let client = reqwest::Client::new();

    // Send 3 messages
    for i in 0..3 {
        client.post(format!("{base}/api/v10/channels/{CHANNEL_ID}/messages"))
            .header("Authorization", format!("Bot {BOT_TOKEN}"))
            .json(&json!({ "content": format!("msg {i}") }))
            .send().await.unwrap();
    }

    // Fetch messages
    let msgs: Vec<Value> = client.get(format!("{base}/api/v10/channels/{CHANNEL_ID}/messages?limit=10"))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(msgs.len(), 3);
}

#[tokio::test]
async fn test_reactions() {
    let (base, _state) = start_server().await;
    let (_w, _read) = ws_identify(&base, BOT_TOKEN).await;

    let client = reqwest::Client::new();
    let msg: Value = client.post(format!("{base}/api/v10/channels/{CHANNEL_ID}/messages"))
        .header("Authorization", format!("Bot {BOT_TOKEN}"))
        .json(&json!({ "content": "vote" }))
        .send().await.unwrap().json().await.unwrap();
    let msg_id = msg["id"].as_str().unwrap();

    // Add reaction
    let status = client.put(format!("{base}/api/v10/channels/{CHANNEL_ID}/messages/{msg_id}/reactions/👍/@me"))
        .header("Authorization", format!("Bot {BOT_TOKEN}"))
        .send().await.unwrap().status();
    assert_eq!(status, 204);

    // Remove reaction
    let status = client.delete(format!("{base}/api/v10/channels/{CHANNEL_ID}/messages/{msg_id}/reactions/👍/@me"))
        .header("Authorization", format!("Bot {BOT_TOKEN}"))
        .send().await.unwrap().status();
    assert_eq!(status, 204);
}
