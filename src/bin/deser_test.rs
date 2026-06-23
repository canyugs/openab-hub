use serenity::model::channel::Message;

fn main() {
    let json = r#"{
        "id": "1519028366437515264",
        "channel_id": "100",
        "guild_id": "1",
        "content": "ping from webhook",
        "timestamp": "2026-06-23T17:16:24Z",
        "edited_timestamp": null,
        "tts": false,
        "mention_everyone": false,
        "mentions": [],
        "mention_roles": [],
        "attachments": [],
        "embeds": [],
        "pinned": false,
        "type": 0,
        "author": {
            "id": "999999999999999999",
            "username": "tester",
            "global_name": "tester",
            "avatar": null,
            "bot": false
        }
    }"#;

    match serde_json::from_str::<Message>(json) {
        Ok(msg) => println!("OK: {} said: {}", msg.author.name, msg.content),
        Err(e) => println!("DESER ERROR: {e}"),
    }
}
