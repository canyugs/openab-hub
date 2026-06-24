# openab-hub

A headless, Discord-compatible server that implements the subset of the Discord
API that [serenity](https://github.com/serenity-rs/serenity)/OpenAB actually use.
Point an OpenAB bot at it via serenity's `proxy` setting and it behaves exactly
like Discord — no Discord app, no bot tokens to manage, no rate limits — while
letting bots talk to each other as a self-hosted message bus.

Built for the OpenAB multi-bot PR review panel ("公審會"), where several agent
bots discuss a change in a thread. openab-hub replaces Discord as the transport
so the panel runs on your own infrastructure.

## Why

The panel ran on Discord purely as a message bus between bots. That meant N
Discord bot tokens to manage, Discord rate limits and gateway reconnects in the
hot path, and bot-to-bot delivery at the mercy of a third party. openab-hub is
the same wire protocol, self-hosted: bots connect out to it exactly as they
connect to Discord.

## How it works

serenity has built-in proxy support — every `https://discord.com` REST call is
rewritten to the proxy URL, and the gateway WebSocket URL is fetched through it
too. So an OpenAB bot needs **one config line** to talk to openab-hub instead of
Discord:

```toml
[discord]
bot_token = "..."                       # unchanged — real Discord tokens work too
proxy = "http://openab-hub.internal:8080" # ← the only change
```

```
                      openab-hub
                  ┌────────────────────┐
   OpenAB bots ──▶│  REST  /api/v10/... │  ← serenity HTTP (proxy mode)
   (serenity,     │  Gateway WS /gateway │  ← serenity gateway connection
    proxy mode)   │  SQLite persistence  │
                  │  /bot-config/{id}    │  ← serve bot config remotely
                  │  /webhook  /health   │
                  └────────────────────┘
```

Everything the panel needs is preserved: bot user IDs (decoded from tokens just
like Discord), channel IDs, thread IDs, `trusted_bot_ids` — all unchanged.

## Discord API subset implemented

**Gateway (6 opcodes):** HELLO, IDENTIFY, READY, GUILD_CREATE, HEARTBEAT/ACK,
DISPATCH (MESSAGE_CREATE / MESSAGE_UPDATE).

**REST (11 endpoints):** get gateway, send/edit/delete message, get messages,
get channel, create thread, rename thread, add/remove reaction, register slash
commands.

**Behaviors:** threads (with Discord's one-thread-per-message rule + `thread_metadata`),
status reactions, streaming edits, mention parsing (`<@id>` → `mentions`), bot-to-bot
fanout. Verified end-to-end against a real OpenAB bot.

## Remote config (no file mounts)

The hub can serve a bot's config so deployments need no mounted/baked config file:

- `GET /bot-config` — generic config; token via `${DISCORD_BOT_TOKEN}` env.
- `GET /bot-config/{id}` — **fleet-aware**: token derived from the id, and
  `trusted_bot_ids` auto-filled with every other registered bot. A bot then needs
  only `OPENAB_CONFIG=http://openab-hub.internal:8080/bot-config/<id>`. Add a bot
  to `HUB_BOTS` and every other bot trusts it automatically.

## Configuration (env)

| Var | Purpose |
|-----|---------|
| `HUB_LISTEN` | listen address (default `0.0.0.0:8080`) |
| `HUB_DB` | SQLite path (default `hub.db`; `:memory:` for ephemeral) |
| `HUB_GUILD_ID` | pre-seeded guild id |
| `HUB_CHANNELS` | comma-separated `id:name` channels to pre-seed |
| `HUB_BOTS` | comma-separated `id:name` bots to pre-seed (drives fleet trust lists) |
| `HUB_PUBLIC_URL` | base WS URL the gateway returns (e.g. `ws://openab-hub.internal:8080`) — **must be bot-reachable**, see note below |
| `HUB_CONFIG_PROXY` | proxy URL embedded in served configs |
| `HUB_ACCESS_LOG` | `1` to log every REST request |

## Quickstart

```bash
HUB_GUILD_ID=1 HUB_CHANNELS="100:general" cargo run

# in another shell — drive it like Discord
curl localhost:8080/health
curl -X POST localhost:8080/webhook -H 'Content-Type: application/json' \
  -d '{"channel_id":100,"content":"hello","username":"github"}'
```

`cargo test` runs the integration suite (gateway handshake, message fanout,
threads, reactions, remote config). The `examples/` dir has a serenity `test-bot`
and a `proxy-probe` for verifying compatibility.

## OpenAB integration

OpenAB needs a tiny opt-in `proxy` field on `[discord]` config — see the
`feat/discord-api-proxy` branch of
[canyugs/openab](https://github.com/canyugs/openab/tree/feat/discord-api-proxy).
When unset, OpenAB behaves exactly as before.

## Notes / gotchas (serenity 0.12)

- **Gateway WS does not go through the HTTP proxy.** serenity fetches the gateway
  URL via the proxied REST call, then connects the WS directly to that URL. So set
  `HUB_PUBLIC_URL` to a bot-reachable address, or the hub returns
  `ws://localhost:8080` and the bot gets ConnectionRefused. serenity caches the
  URL at startup — restart the bot after changing it.
- **proxy requires the ratelimiter disabled.** serenity only applies the proxy on
  its ratelimiter-disabled code path (the ratelimiter builds requests with
  `proxy=None`). OpenAB disables the limiter automatically when `proxy` is set.
- **Real Discord token format works as-is** — the hub decodes the user id from the
  base64 first segment, exactly like Discord. Existing bots keep their tokens.
- IDs are `NonZeroU64`; messages omit `discriminator` (use `global_name`);
  `GUILD_CREATE` must follow READY; unimplemented routes return valid JSON
  (`[]`/`{}`), never an empty body.

## Status

Implemented and verified end-to-end: a 3-bot panel of real Claude agents held a
multi-turn technical discussion entirely through openab-hub — zero Discord. See
[docs/design.md](docs/design.md) for the full design.
