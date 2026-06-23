// Reproduce serenity's get_bot_gateway call through the proxy to see the real error.
#[tokio::main]
async fn main() {
    let proxy = std::env::var("HUB_PROXY").unwrap_or_else(|_| "http://127.0.0.1:19097".into());
    let token = "MTIzNDU.fake.token";

    let http = serenity::http::HttpBuilder::new(token)
        .proxy(&proxy)
        .ratelimiter_disabled(true)
        .build();

    println!("calling get_gateway (what client uses) via proxy {proxy} ...");
    match http.get_gateway().await {
        Ok(g) => println!("OK url={}", g.url),
        Err(e) => println!("ERR (debug): {e:?}"),
    }

    println!("\ncalling get_bot_gateway via proxy {proxy} ...");
    match http.get_bot_gateway().await {
        Ok(g) => println!("OK url={} shards={}", g.url, g.shards),
        Err(e) => println!("ERR (debug): {e:?}"),
    }

    // Send a message, then create a thread from it, then rename the thread —
    // this exercises serenity's GuildChannel + Message deserialization on our responses.
    use serenity::all::{ChannelId, MessageId, CreateMessage, CreateThread, EditChannel, AutoArchiveDuration};
    let ch = ChannelId::new(100);

    println!("\nsend_message via serenity ...");
    let msg = match ch.send_message(&http, CreateMessage::new().content("probe trigger")).await {
        Ok(m) => { println!("OK msg id={}", m.id); m }
        Err(e) => { println!("ERR (debug): {e:?}"); return; }
    };

    println!("\ncreate_thread_from_message via serenity ...");
    match ch.create_thread_from_message(&http, MessageId::new(msg.id.get()),
        CreateThread::new("probe thread").auto_archive_duration(AutoArchiveDuration::OneDay)).await {
        Ok(t) => println!("OK thread id={} name={} kind={:?}", t.id, t.name, t.kind),
        Err(e) => println!("ERR (debug): {e:?}"),
    }

    println!("\nedit channel name (rename thread) via serenity ...");
    match ch.edit(&http, EditChannel::new().name("renamed-probe")).await {
        Ok(c) => println!("OK channel id={} name={}", c.id, c.name),
        Err(e) => println!("ERR (debug): {e:?}"),
    }

    println!("\nget_messages via serenity ...");
    match ch.messages(&http, serenity::all::GetMessages::new().limit(5)).await {
        Ok(ms) => println!("OK got {} messages", ms.len()),
        Err(e) => println!("ERR (debug): {e:?}"),
    }
}
