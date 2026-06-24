# openab-hub: Headless Discord-compatible Server

A lightweight, self-hosted server that implements the subset of Discord API used by OpenAB. Bots connect via serenity's existing `proxy` setting — one-line OpenAB change, all existing config (bot tokens, channel IDs, trusted_bot_ids) works as-is.

Status: **implemented & verified**. Repo: https://github.com/canyugs/openab-hub

## Verification (real OpenAB bot, end-to-end)

A real OpenAB bot connected through `proxy = "http://openab-hub:8080"` and ran a
complete review cycle against openab-hub — **no Discord involved**. The hub
access log (`HUB_ACCESS_LOG=1`) confirms every Discord API call OpenAB makes is
served correctly:

| Behavior | Call | Status |
|---|---|---|
| Get gateway URL | `GET /api/v10/gateway` | 200 |
| Gateway WebSocket | `GET /gateway` (upgrade) | 101 |
| Register global commands | `PUT /applications/{id}/commands` | 200 |
| Clear guild commands | `PUT /applications/{id}/guilds/{gid}/commands` | 200 |
| Channel info (thread detect) | `GET /channels/{id}` | 200 |
| Create thread from trigger | `POST /channels/{id}/messages/{mid}/threads` | 200 |
| Status reactions (👀🤔🆗💪) | `PUT`/`DELETE .../reactions/{emoji}/@me` | 204 |
| Initial reply | `POST /channels/{thread}/messages` | 200 |
| Streaming progressive edits | `PATCH /channels/{thread}/messages/{mid}` | 200 |

Observed behavior matched Discord exactly: user `@mention` in a channel →
bot creates a thread from the message → adds 👀 → posts a reply → edits it
progressively as the agent streams → cycles status reactions → finalizes.

### Multi-bot panel (公審會) verified

Three OpenAB bots (Gandalf chair, Aragorn + Gimli reviewers) ran a council
entirely through openab-hub: the human `@mentions` the chair → chair opens a
thread and `@mentions` the reviewers → reviewers (admitted via `trusted_bot_ids`
+ `allow_bot_messages = "mentions"`) post their verdicts **in the same thread**.

The hub does fanout only (every bot sees every message, like Discord's
guild-wide visibility); the trust restriction lives entirely in OpenAB's
per-bot `trusted_bot_ids` gating — unchanged from Discord. Friendly bot names
are pre-seeded with `HUB_BOTS="1001:Gandalf,1002:Aragorn,..."`.

**Thread fidelity (the fix that made the panel converge on one thread):**
threads must report `thread_metadata` + `owner_id` on `GET /channels/{id}` —
this is how serenity and OpenAB's `detect_thread` recognize a channel as a
thread. Without them, a reviewer replying inside the thread thought it was a
top-level channel and created a *nested* thread (3 bots → 20 threads). The hub
also enforces Discord's **one-thread-per-message** rule (second create →
error `160004`) and embeds `message.thread`, so a multi-bot race converges to a
single shared thread.

### serenity-compatibility gotchas (all fixed)
- **All snowflake IDs are `NonZeroU64`** — webhook author id `0` fails to
  deserialize. Use a non-zero sentinel (hub uses `1`).
- **No `discriminator: "0000"`** — serenity's discriminator is
  `Option<NonZeroU16>` and `"0000"` fails. Omit it; send `global_name` instead.
- **`GUILD_CREATE` is required after `READY`** — serenity won't dispatch guild
  message events until it receives the guild.
- **Client uses `get_gateway` (`/gateway`), not `get_bot_gateway`** — both must
  exist; the plain one is what `Client::start()` calls.
- **Empty body breaks error decode** — unimplemented routes must return valid
  JSON (`[]`/`{}`), not an empty body, or serenity's JSON decode errors.
- **Populate `mentions`/`mention_roles`** by parsing `<@id>`/`<@!id>`/`<@&id>`
  from content, mirroring Discord's server-side behavior.

### Deferred (not applicable to headless operation)
- **Slash command interaction callbacks** (`create_response`,
  `POST /interactions/{id}/{token}/callback`) — only fire when a user invokes a
  slash command via the Discord UI; there is no UI in the headless hub.

## Problem

The current PR review panel runs 8 OpenAB bots on Discord. Pain points:
- 8 Discord bot tokens to manage (OAuth/device-auth, filesystem persistence)
- Discord API rate limits and gateway reconnects add latency
- Bot-to-bot communication depends on Discord's delivery semantics
- Discord is a UI layer being used as a message bus

## Architecture

openab-hub is a single Rust binary (axum) that speaks enough Discord API for serenity to work. Bots think they're talking to Discord.

```
                      openab-hub
                  ┌────────────────────┐
                  │  REST API           │  ← serenity HTTP calls (proxy mode)
                  │  /api/v10/...       │
                  │                    │
                  │  Gateway WebSocket  │  ← serenity gateway connection
                  │  /gateway           │
                  │                    │
                  │  SQLite             │  ← message persistence
                  │                    │
                  │  POST /webhook      │  ← GitHub webhook trigger
                  │  GET  /threads      │  ← web viewer (optional)
                  └────────────────────┘
                     ▲  ▲  ▲  ▲  ▲
                     │  │  │  │  │  WebSocket + HTTP
                  OpenAB bots (8 pods, serenity proxy mode)
```

### How it connects

serenity has built-in proxy support. All `https://discord.com` calls get rewritten to the proxy URL:

```rust
// request.rs:78
path = path.replace("https://discord.com", proxy.trim_end_matches('/'));
```

Gateway URL is also fetched via the proxy (`GET /api/v10/gateway/bot`), so openab-hub controls where the WebSocket connects.

OpenAB change: add `proxy` field to `[discord]` config, wire it into `HttpBuilder::proxy()`. One line in `main.rs`.

```toml
# Bot pod config — only change is adding proxy
[discord]
bot_token = "MTQ5MzkyMDc..."           # same token
proxy = "http://openab-hub:8080"        # ← NEW
allow_bot_messages = "mentions"         # same
trusted_bot_ids = [1493950480559247481]  # same
allowed_channels = [1493817398997029006] # same
```

### What openab-hub preserves

All existing IDs work unchanged:
- **Bot tokens**: openab-hub parses base64 prefix to extract bot user ID (same as Discord)
- **Bot user IDs**: decoded from tokens, used in trusted_bot_ids — identical
- **Channel IDs**: pre-configured in openab-hub, same snowflake values
- **Thread IDs**: dynamically generated snowflake-format IDs
- **Message IDs**: dynamically generated snowflake-format IDs

## Discord API subset to implement

### Gateway WebSocket (6 opcodes)

| Opcode | Direction | Purpose |
|---|---|---|
| 10 HELLO | hub → bot | send `heartbeat_interval` |
| 2 IDENTIFY | bot → hub | bot token + intents |
| 0 DISPATCH (READY) | hub → bot | bot user info, guild list |
| 1 HEARTBEAT | bot → hub | keepalive ping |
| 11 HEARTBEAT_ACK | hub → bot | keepalive pong |
| 0 DISPATCH (MESSAGE_CREATE) | hub → bot | message event — the core |

### REST API (11 endpoints)

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/v10/gateway/bot` | return gateway WebSocket URL |
| POST | `/api/v10/channels/{id}/messages` | send message (with optional `message_reference` for replies) |
| PATCH | `/api/v10/channels/{id}/messages/{id}` | edit message (streaming) |
| DELETE | `/api/v10/channels/{id}/messages/{id}` | delete message |
| GET | `/api/v10/channels/{id}/messages` | get message history (for bot turn cap check) |
| GET | `/api/v10/channels/{id}` | get channel info (thread detection) |
| POST | `/api/v10/channels/{id}/messages/{id}/threads` | create thread from message |
| PATCH | `/api/v10/channels/{id}` | rename thread |
| PUT | `/api/v10/channels/{id}/messages/{id}/reactions/{emoji}/@me` | add reaction |
| DELETE | `/api/v10/channels/{id}/messages/{id}/reactions/{emoji}/@me` | remove reaction |
| PUT | `/api/v10/applications/{id}/commands` | register slash commands (can no-op) |

## Message flow

```
1. GitHub webhook POST /webhook
   → openab-hub creates channel message with PR info
   → dispatches MESSAGE_CREATE to all connected bots via Gateway WebSocket

2. Bot processes message, calls POST /channels/{id}/messages (via proxy)
   → openab-hub stores message in SQLite
   → dispatches MESSAGE_CREATE to all OTHER connected bots
     (Gandalf sees reviewer replies, reviewers see Gandalf)

3. Gandalf detects quorum, posts verdict
   → openab-hub detects verdict (optional) → posts to GitHub API
   → or Gandalf posts to GitHub directly via gh CLI (existing behavior)
```

## Storage (SQLite)

```sql
CREATE TABLE guilds (
    id       INTEGER PRIMARY KEY,  -- snowflake
    name     TEXT NOT NULL
);

CREATE TABLE channels (
    id        INTEGER PRIMARY KEY,  -- snowflake, same values as current Discord config
    guild_id  INTEGER NOT NULL REFERENCES guilds(id),
    name      TEXT NOT NULL,
    type      INTEGER NOT NULL DEFAULT 0  -- 0=text, 11=public_thread
);

CREATE TABLE messages (
    id         INTEGER PRIMARY KEY,  -- snowflake
    channel_id INTEGER NOT NULL REFERENCES channels(id),
    author_id  INTEGER NOT NULL,
    author_name TEXT NOT NULL,
    is_bot     BOOLEAN NOT NULL,
    content    TEXT NOT NULL,
    timestamp  TEXT NOT NULL,
    reference_id INTEGER  -- reply-to message ID
);

CREATE TABLE reactions (
    message_id INTEGER NOT NULL REFERENCES messages(id),
    user_id    INTEGER NOT NULL,
    emoji      TEXT NOT NULL,
    PRIMARY KEY (message_id, user_id, emoji)
);

CREATE TABLE bots (
    user_id    INTEGER PRIMARY KEY,  -- decoded from token
    username   TEXT NOT NULL,
    token_hash TEXT NOT NULL,  -- for auth validation
    connected  BOOLEAN DEFAULT FALSE,
    last_seen  TEXT
);
```

Pre-seed `guilds`, `channels` with the same IDs from current Discord config.

## Additional endpoints (non-Discord)

| Method | Path | Purpose |
|---|---|---|
| POST | `/webhook` | GitHub webhook → create message + fanout |
| GET | `/threads` | list threads (web viewer) |
| GET | `/threads/{id}` | thread messages (web viewer) |
| GET | `/health` | connected bot count, uptime |

## Config (env vars)

| Var | Purpose |
|---|---|
| `HUB_LISTEN` | listen address (default `0.0.0.0:8080`) |
| `HUB_DB` | SQLite path (default `./hub.db`) |
| `HUB_GUILD_ID` | pre-configured guild ID |
| `HUB_CHANNELS` | comma-separated `id:name` pairs to pre-seed |
| `GITHUB_TOKEN` | for posting commit status + PR comments (optional) |
| `GITHUB_WEBHOOK_SECRET` | webhook signature validation |

## Crate dependencies

```toml
[dependencies]
axum = { version = "0.8", features = ["ws"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
rusqlite = { version = "0.34", features = ["bundled"] }
uuid = { version = "1", features = ["v4"] }
tracing = "0.1"
tracing-subscriber = "0.3"
base64 = "0.22"
flate2 = "1"            # gateway zlib-stream compression (serenity uses it)
```

## OpenAB changes required

One field added to `[discord]` config:

```rust
// src/config.rs — DiscordConfig
pub struct DiscordConfig {
    pub bot_token: String,
    pub proxy: Option<String>,  // ← NEW
    // ... existing fields unchanged
}

// src/main.rs — Client builder
let mut builder = Client::builder(&discord_cfg.bot_token, intents);
if let Some(ref proxy) = discord_cfg.proxy {
    builder = builder.http_settings(|b| b.proxy(proxy).ratelimiter_disabled(true));
}
let mut client = builder.event_handler(handler).await?;
```

That's it. All other config (`trusted_bot_ids`, `allow_bot_messages`, `allowed_channels`, etc.) works unchanged because serenity's behavior is identical — it just talks to a different server.

## Migration path

1. Build openab-hub, deploy in `agent-team` project
2. Pre-seed guild + channels with current Discord IDs
3. Register bot tokens (same tokens, openab-hub just needs to know them)
4. Add `proxy = "http://openab-hub:8080"` to one bot's config (Gimli — simplest)
5. Test: webhook trigger → Gimli receives → Gimli replies → other bots see reply
6. Roll out to remaining 7 bots one by one
7. Once stable: bots only talk to openab-hub, Discord tokens become optional
8. Optional: openab-hub posts thread summaries to Discord webhook for human visibility

## Open questions

- **zlib compression**: serenity sends `compress: true` in IDENTIFY and expects zlib-stream on the gateway. openab-hub needs to support this (flate2 crate) or tell serenity not to compress. Check if serenity respects server not offering compression.
- **Slash commands**: `PUT /applications/{id}/commands` can return empty list (no-op). Slash command interactions (`/reset`, `/cancel`) can be handled as regular messages or skipped initially.
- **Rate limiting**: openab-hub can return standard Discord rate limit headers or disable rate limiting (serenity supports `ratelimiter_disabled(true)` alongside proxy).
- **GitHub output**: Keep Gandalf posting via `gh` CLI (works today), or move to openab-hub detecting verdict and posting. Decide during implementation.
