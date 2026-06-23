use serenity::all::ClientBuilder;
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::model::gateway::Ready;
use serenity::prelude::*;

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        println!("[MESSAGE] #{} @{}: {}", msg.channel_id, msg.author.name, msg.content);

        if msg.content.contains("ping") {
            if let Err(e) = msg.channel_id.say(&ctx.http, "pong!").await {
                eprintln!("Error sending message: {e}");
            }
        }
    }

    async fn ready(&self, _ctx: Context, ready: Ready) {
        println!("[READY] Connected as {} (id: {})", ready.user.name, ready.user.id);
    }
}

#[tokio::main]
async fn main() {
    let token = std::env::var("BOT_TOKEN").expect("BOT_TOKEN env var required");
    let proxy = std::env::var("HUB_PROXY").expect("HUB_PROXY env var required");

    println!("Connecting to hub via proxy: {proxy}");

    let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

    let http = serenity::http::HttpBuilder::new(&token)
        .proxy(&proxy)
        .ratelimiter_disabled(true)
        .build();

    let mut client = ClientBuilder::new_with_http(http, intents)
        .event_handler(Handler)
        .await
        .expect("Error creating client");

    println!("Starting bot...");
    if let Err(e) = client.start().await {
        eprintln!("Client error: {e}");
    }
}
