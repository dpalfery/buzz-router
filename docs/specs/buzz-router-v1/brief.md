---
id: specs/buzz-router-v1/brief
title: buzz-router v1 design brief
doc-type: spec
status: superseded
component: buzz-router
owner: dpalfery
last-reviewed: 2026-10-05
---

# buzz-router v1 design brief

The design David approved in the Buzz team thread on 2026-10-04. It's the input to the formal specification in this folder: `requirements.md`, then `design.md`, then `tasks.md`. Once those three are approved, they supersede this brief. Until then, this brief is the source of truth.

Replaces every per-bot Buzz listener (dp-, sf-, Hal, Zo, Claw) with one program.

## 0. Decisions already made

These are settled. Don't reopen them while building.

| Topic | Decision |
|---|---|
| One program | `buzz-router`, a single binary for macOS, Linux and Windows. One process per machine serves every bot on that machine. |
| Language | Rust. Buzz itself is Rust (github.com/block/buzz), so we reuse its `buzz-sdk` and `buzz-core` crates instead of shelling out to the `buzz` CLI. |
| Who decides who answers | The router, deterministically, before any model runs. The router never calls a model. |
| `@everyone` | Starts a **discussion**: every bot in the channel is woken, and bots are then woken by each other's posts in that thread. |
| Turn limit | **4 turns per bot per round.** A turn is one wake, whether the bot posts or passes. A round restarts only when David posts in the thread. |
| Confirmations | Reactions are visible in the Buzz client, so "seen" is a reaction, not a chat message. One text note only for slow, direct requests (section 10). |
| Stop | A short message from David containing "stop" halts bots immediately and blocks their posts until "resume" (section 8). |
| Rollout | **No shadow mode.** Cut over and tune the defaults. |
| Builder | A separate agent builds from this spec. |

## 1. Scope

In scope: listening to the relay, routing, turn limits and budgets, stop/resume, waking agents, posting their replies, confirmations, state and crash recovery, packaging for 3 OSes.

Out of scope for v1:
- Any model or LLM call inside the router.
- Shadow or dry-run mode against live traffic. (Offline replay of captured events for testing is in scope, section 13.)
- Forum posts, DMs, canvases, workflows. Channel messages only (kind 9, plus kind 40003 edits).
- Bots posting outside the thread they were woken for.
- Changing Buzz itself.

Why not reuse `buzz-acp` (Buzz's own agent harness): it only drives local ACP agents (goose, claude-agent-acp, etc.), wakes on `p`-tag mentions only, and has no cross-bot discussion, turn caps or quiet hours. Our bots include webhook-driven cloud routines. We borrow its relay code patterns (section 2) but build our own router.

## 2. Facts about Buzz the builder must rely on

Verified from the Buzz source at commit `f0eb5575ffc9d5f57af4ed3f574529d997c83a0d` (2026-10-04). Pin this commit for the git dependencies.

**Crates to depend on** (git dependency on `https://github.com/block/buzz`, pinned `rev`):
- `buzz-core`: no I/O. Use `buzz_core::nip10::parse_thread_markers` and `ThreadMarkers::resolve()` for threading, and `buzz_core::kind::*` for kind constants.
- `buzz-sdk`: no I/O. Use `builders::build_message`, `build_reaction`, `build_edit`; `mentions::extract_at_mentions_with_known`, `extract_nostr_uris`, `strip_code_regions`; `nip_oa` to verify owner-attestation tags.
- `nostr = "0.44"` (same as Buzz).

**Events:**
- Channel message: kind `9`, content is the text, `["h", <channel-uuid>]` tag.
- Edit: kind `40003`. The desktop client puts `p` tags only on mentions the edit *newly adds*.
- Reaction: kind `7`, content is the emoji, `["e", <target-id>]`.
- Typing indicator: kind `20002` (ephemeral). Build it exactly like `buzz-acp` does (`crates/buzz-acp/src/relay.rs`, near `KIND_TYPING_INDICATOR`).

**Threading (NIP-10 markers on `e` tags):**
- Direct reply to the thread root: one tag `["e", <root>, "", "reply"]`.
- Nested reply: `["e", <root>, "", "root"]` plus `["e", <parent>, "", "reply"]`.
- No `reply` marker means a top-level message (it's its own thread root).

**What the Buzz desktop client actually sends** (`desktop/src/features/messages/lib/threading.ts`, `MessageThreadPanel.tsx`):
- Every message carries `["p", <sender's own pubkey>]`, plus one `p` tag per mention picked or typed in the composer.
- It does **not** add a `p` tag for the author of the message being replied to.
- When David types in a thread panel without clicking "reply" on a specific message, the parent is the **thread root**. So the parent only points at a bot's message when David explicitly chose that message to reply to.
- Buzz has **no native `@everyone`**. It's plain text with no `p` tags.

**Identity:**
- Agents may carry a NIP-OA tag `["auth", <owner-pubkey>, <conditions>, <sig>]` on their events. `buzz_sdk::nip_oa` verifies it.
- Existing owner conventions in `buzz-acp`: content `!shutdown` (exit) and `!cancel` (cancel current turn), sent by the owner with a `p` tag for the agent. The router honours both (section 8).

**Relay access** (copy these patterns):
- `examples/countdown-bot/src/main.rs` (438 lines): NIP-42 auth (standalone or owner-attested), subscribe, publish kind 9. **Start from this file.**
- `crates/buzz-acp/src/relay.rs`: channel discovery (`discover_channels`: kind 39002 with `#p` = bot pubkey, then kind 39000 for names), `POST /query` and `POST /events` over HTTP with NIP-98 auth (`RestClient`), reconnect and backfill.
- `crates/buzz-acp/src/pool.rs` (around line 4407): thread context fetch with an `#e` filter.

## 3. Architecture

```
            one buzz-router process per machine
 ┌──────────────────────────────────────────────────────────┐
 │  relay connections (one WebSocket per local bot)         │
 │        │ events (kind 9, 40003)                          │
 │        ▼                                                 │
 │  dedupe by event id ──► store (SQLite)                   │
 │        ▼                                                 │
 │  router-core::route()   PURE: event + snapshot → decisions│
 │        ▼                                                 │
 │  control (stop/resume/cancel)   wake queue (per bot)     │
 │                                    │ debounce, coalesce  │
 │                                    ▼                     │
 │                         adapters: command | webhook      │
 │                                    │                     │
 │  loopback API  ◄──── agent posts / passes (wake token)   │
 │        ▼                                                 │
 │  publisher: signs with the bot's key, threads the reply  │
 └──────────────────────────────────────────────────────────┘
```

**What it is:** one process doing three jobs.
1. **Listener:** holds the relay connections and receives every message.
2. **Router:** decides deterministically which bot, if any, is woken.
3. **Gatekeeper:** holds the keys, publishes replies, and enforces stop and the limits.

**Where it runs:** every bot is served by exactly one router.
- A `command` bot (an agent started as a local process) is served by the router on the machine where that agent runs.
- A `webhook` bot (a cloud routine) can be served by any router on the tailnet. Pick one and list the bot only in that machine's `router.toml`.
- A machine with no command bots doesn't need a router unless it's chosen to host webhook bots.
- Never list the same bot on two routers: both would wake it and both would answer.

Key properties:
- **The router holds every local bot's signing key.** Agents never see it, so they can only post through the router. That's what makes stop and turn limits enforceable. Cutover must remove `BUZZ_PRIVATE_KEY` from each agent's environment (section 16).
- Each router decides only for its own local bots, using relay data plus its own state. No coordination between machines is needed: every decision is per bot, and each bot lives on exactly one machine.
- One WebSocket per bot, authenticated as that bot, because channel visibility depends on membership. Events arriving on several connections are deduplicated by event id.

**Repo layout:**
```
buzz-router/
  Cargo.toml                 workspace
  crates/router-core/        pure logic, no I/O, no tokio: config types, classify,
                             parse (mentions, @everyone, stop), route(), limits
  crates/buzz-router/        the binary: relay, store, wake engine, adapters,
                             loopback API, CLI, service install
  fixtures/conformance/      one JSON file per case in section 15
  roster.example.toml
  router.example.toml
  .github/workflows/ci.yml   build + test on macOS, Linux, Windows
  .github/workflows/release.yml
```

## 4. Configuration

Two files. Config dir comes from the `directories` crate: `~/Library/Application Support/buzz-router` (macOS), `~/.config/buzz-router` (Linux), `%APPDATA%\buzz-router` (Windows). Data dir (SQLite, admin token, logs) also comes from `directories`.

### 4.1 `roster.toml`: shared and identical on every machine

```toml
version = 1

[owner]
name = "David"
pubkeys = ["<david-key-1-hex>", "<david-key-2-hex>"]   # both of David's keys
timezone = "America/Chicago"                            # IANA zone, used for quiet hours

[limits]                      # defaults for every bot; per-bot overrides allowed
turns_per_round = 4
wakes_per_hour = 20
wakes_per_day = 100
quiet_hours = "23:00-07:00"   # owner timezone; "" disables
discussion_debounce_secs = 20
discussion_debounce_max_secs = 90
max_wake_minutes = 20
max_posts_per_wake = 3
status_note_after_secs = 20

[[channels]]
id = "<channel-uuid>"
name = "work-for-david"
default_bot = ""              # optional: bot that answers David's untagged top-level messages here

[[bots]]
name = "dp-grok-bot"          # exact Buzz display name
pubkey = "<hex>"
aliases = []                  # extra @names that address this bot, matched as whole words
channels = ["*"]              # "*" = every channel the bot is a member of, or a list of channel uuids
respond_to = "owner-only"     # owner-only | anyone
machine = "dp-box"            # informational only
# limits = { turns_per_round = 4 }   # optional override

# Repeat [[bots]] for: dp-kyber-bot, sf-GrokBot, sf-KyberWeaveIssueFixer,
# Hal.macbook-pro, Hal.zo, Claw, ARR Ride Guide, Briefing Auditor, Bike Mechanic.
```

`buzz-router roster check` validates the file and prints its SHA-256. `status` shows the same hash, so a stale copy on one machine is easy to spot.

### 4.2 `router.toml`: per machine

```toml
relay_url = "wss://<relay-host>"
api_bind = "127.0.0.1:47821"   # loopback API for agents and the CLI (always on)
tailnet_bind = ""              # e.g. "100.x.y.z:47821"; this machine's Tailscale IP, for remote agents
public_url = ""                # e.g. "http://dp-box.<tailnet>.ts.net:47821"; MagicDNS name sent to async webhook agents
roster_path = "roster.toml"    # relative to the config dir

[[bots]]
name = "dp-grok-bot"           # must exist in the roster
key = "keychain"               # OS keychain entry "buzz-router/<name>", or "file:<path>"
auth_tag = ""                  # optional NIP-OA tag JSON, as with BUZZ_AUTH_TAG
max_concurrent = 1

[bots.adapter]
type = "webhook"
url = "https://<routine-endpoint>"
secret_env = "DP_GROK_WEBHOOK_SECRET"  # HMAC secret, read from this env var
mode = "async"                         # async = agent calls back; sync = reply in HTTP response
cancel_url = ""                        # optional, best-effort cancel

[[bots]]
name = "dp-kyber-bot"
key = "keychain"

[bots.adapter]
type = "command"
command = ["claude", "-p", "--output-format", "text"]
cwd = "~/git/personal/kyber-weave"
env = {}
prompt_mode = "stdin"          # stdin | file (path passed in BUZZ_ROUTER_PROMPT_FILE)
reply_mode = "stdout"          # stdout = router posts what the command prints; api = agent calls `buzz-router post`
prompt_template = ""           # optional path; empty = built-in template (section 9.4)
```

## 5. Classifying authors

Each event's author falls into exactly one class:

| Class | Rule |
|---|---|
| `owner` | Pubkey is in `owner.pubkeys`. |
| `bot` | Pubkey is a roster bot. |
| `foreign_bot` | Not in the roster, but carries a valid NIP-OA `auth` tag. If that tag's owner is in `owner.pubkeys`, log `roster drift` once. |
| `human` | Anything else. |

Additional checks:
- Verify event signatures (the `nostr` crate does this). Drop invalid events.
- An event carrying `["buzz-router", <version>, "status"]` is a router status note. It never wakes anyone.

## 6. Routing: the pure function

```rust
// router-core
pub fn route(ev: &InEvent, snap: &Snapshot, now: DateTime<Utc>) -> RouteResult;

pub struct RouteResult {
    pub control: Option<Control>,         // Stop{scope} | Resume{scope} | Cancel{scope}
    pub decisions: Vec<Decision>,         // one per local bot it considered
    pub thread_update: ThreadUpdate,      // participants added, discussion flag, new round
}
pub enum Decision {
    Wake { bot, reason: Reason, priority: Priority, debounce: bool },
    Suppress { bot, why: SuppressWhy },   // Halted | Cap | Quiet | Budget | RespondTo
}
pub enum Reason { Mention, Everyone, ReplyTarget, Participant, DefaultBot, Discussion, BotMention }
pub enum Priority { Owner, Human, Bot }
```

`Snapshot` holds the roster, the local bot set, halts, thread state for this event's thread, per-bot turns used in the current round, per-bot hourly and daily wake counts, and the quiet-hours flag. `route` does no I/O and reads no clock except `now`. Every rule below is a test case in section 15.

### 6.1 Parsing helpers (router-core)

Run these on the event content **after** stripping code regions (`strip_code_regions`) and quoted lines (lines starting with `>`):
- **Text mentions:** `extract_at_mentions_with_known(content, roster names + aliases)`. Case-insensitive, whole word, longest name first. `@dp-grok-bot` never matches `sf-GrokBot`.
- **Npub mentions:** `extract_nostr_uris` (`nostr:npub1…`, `nostr:nprofile1…`) mapped to roster bots.
- **Tag mentions:** `p` tags naming roster bots, excluding the author's own pubkey (the desktop client always tags the sender). **Counted only for `owner` and `human` authors, never for `bot` authors**, because bots tag the author they reply to and that must not wake anyone.
- **`@everyone`:** the regex `(?i)(^|\s)@everyone\b`. Counted only for `owner` authors.

### 6.2 Thread state

Each router keeps this per thread root:

```
ThreadState {
  root_id, channel_id,
  participants: Set<bot>,     // bots in this conversation
  discussion: bool,           // true once David used @everyone in this thread
  round_id: event id          // the owner message that started the current round
  round_mode: Direct | Discussion,
  turns_used: Map<bot, u32>,  // wakes dispatched in this round
}
```

- **Building it:** a thread the router hasn't seen (new machine, after restart, old thread) is built by fetching the thread from the relay (root plus replies, `#e` = root) and replaying its events through `route` in `created_at` order, without dispatching wakes. Turn counts for the current round are rebuilt from the `wakes` table when present. Otherwise they're approximated by counting each bot's posts since the round started.
- **Posting adds participants:** any roster bot that posts in a thread becomes a participant.

### 6.3 Owner messages (kind 9)

Evaluate in this order and stop at the first match:

1. **Control:** if section 8 matches, return `control` and no wakes.
2. **`@everyone`:**
   - Targets: every roster bot whose `channels` covers this channel.
   - `discussion = true`, `round_mode = Discussion`. Reason `Everyone`.
3. **Explicit mentions** (text, npub or `p` tag): targets are the mentioned bots, `round_mode = Direct`, reason `Mention`. This wins over a reply target: replying to A's message with "@C what about this?" wakes only C.
4. **Reply target:** the event is in a thread, the parent isn't the root, and the parent's author is a roster bot. Target is that bot, `Direct`, reason `ReplyTarget`.
5. **Untagged in a thread with participants:**
   - Targets: all participants. Reason `Participant`.
   - `round_mode = Discussion` if `thread.discussion`, else `Direct`.
   - This is the fix for the original bug.
6. **Channel default:** `default_bot` is set (top-level message, or a thread with no participants). Target is that bot, `Direct`, reason `DefaultBot`.
7. **Otherwise:** nobody. There's deliberately no guessing from "recent conversation". David uses a thread, a mention or the channel default.

When targets exist:
- Start a **new round**: `round_id` = this event, all `turns_used` reset to 0, and the targets join `participants`.
- Owner wakes ignore quiet hours and the hourly and daily budgets, but they still count as a turn. Halted bots get `Suppress(Halted)`.

### 6.4 Bot messages (kind 9)

Never wake the author.

1. The author joins `participants`. A top-level bot message creates a thread with a **bot round**: `round_mode = Direct`, and it never resets until David posts in the thread.
2. **Bot mentions:** text or npub mentions of other bots only, never `p` tags. Each mentioned bot joins `participants` with reason `BotMention`.
3. **Discussion:** if `round_mode = Discussion`, add every other participant with reason `Discussion`.
4. Gate each target in this order: Halted, then Quiet hours, then Cap (`turns_used ≥ turns_per_round`), then Budget (hourly or daily wakes exceeded).
   - A target that passes every gate gets `Wake { priority: Bot, debounce: true }`.
5. **`@everyone` in a bot's text is plain text.** Quoting it does nothing.

### 6.5 Human messages (non-owner, non-bot)

Only for bots with `respond_to = "anyone"`:
- An explicit mention (text, npub or `p` tag) or a reply target wakes that bot, `Priority::Human`, with no round reset.
- Gates: Halted, Quiet, Cap, Budget.
- No control commands, no `@everyone`.

For `owner-only` bots: `Suppress(RespondTo)`.

### 6.6 Foreign bots

Never wake anyone (`Suppress(RespondTo)`).

### 6.7 Edits (kind 40003)

- From the owner only: `p` tags on the edit are newly added mentions. Wake those bots (`Mention`, `Direct`, counted in the current round, no round reset).
- No control commands from edits, and edits by anyone else are ignored.

## 7. Limits and budgets

| Limit | Default | Applies to | On hit |
|---|---|---|---|
| `turns_per_round` | 4 | every wake (owner-caused wakes count too) | `Suppress(Cap)`. The bot reacts ⏸️ once per round on the event that would have woken it. |
| `wakes_per_hour` / `wakes_per_day` | 20 / 100 | bot- and human-caused wakes (owner wakes count but are never blocked) | `Suppress(Budget)`, logged, shown in `status` |
| quiet hours | 23:00–07:00 owner time | bot- and human-caused wakes | dropped, not deferred, so nothing floods in at 7am |
| `max_wake_minutes` | 20 | each wake | process tree killed, token revoked, ⌛ reaction |
| `max_posts_per_wake` | 3 | posts within one wake | further posts refused (HTTP 429) |
| `max_concurrent` | 1 per bot | running wakes | extra wakes wait in the queue |

**Debounce** (discussion and bot-mention wakes only): a bot-caused wake becomes dispatchable at the later of `last bot post in thread + 20s` and `first trigger + 20s`, but no later than `first trigger + 90s`. Owner and human wakes dispatch immediately.

**Coalescing:**
- At most one *queued* wake per (bot, thread). New triggers are appended to its trigger list, so a bot wakes once and sees every new message since its last turn.
- If a wake for (bot, thread) is running, new triggers go into one follow-up wake that starts after it finishes.
- A coalesced wake is one turn.

**Queue order:** Owner, then Human, then Bot priority. FIFO within a priority.

**Worst case for one `@everyone` round:** 4 × (number of bots) wakes, then the thread goes quiet until David posts again.

## 8. Stop, resume, cancel

These are parsed from **owner** kind-9 messages before any routing.

**Normalising the text:**
1. Strip code regions.
2. Remove `@everyone`, roster `@names` and `nostr:` URIs.
3. Lowercase, and turn every character that isn't alphanumeric or `!` into a space.
4. Split into words.

**Matching:**

| Command | Match | Scope |
|---|---|---|
| Stop | 1–5 words, and one word is `stop`, `halt` or `!shutdown` | the bots mentioned (any mention form). **No bots mentioned means every bot**, including when `@everyone` is used. |
| Cancel | the only word is `!cancel` | mentioned bots, else all |
| Resume | the only word is `resume` | mentioned bots, else all |

Stop deliberately errs towards firing:
- "stop", "stop it", "fucking stop[", "@everyone stop" and "please stop now" all halt.
- A false stop costs a "resume". A missed stop burns budget all night.
- "stop the dev server" (4 words) will halt. Document this, and tell David to phrase such requests as "shut down the dev server".

Resume is strict: one exact word.

**Effects of stop (every router, for each in-scope local bot):**
1. Persist the halt to SQLite (it survives restarts).
2. Kill running wakes:
   - Command adapter: kill the whole process tree (section 9.2).
   - Webhook adapter: revoke the token and call `cancel_url` if set.
3. Drop the bot's queue.
4. Refuse posts: the API returns 423, and nothing is published.
5. No new wakes until resume.
6. The bot reacts 🛑 on the stop message. That reaction is David's confirmation. A bot whose router is down won't react, which tells David which one didn't confirm.

**Other commands:**
- Cancel: steps 2 and 3 only. The bot reacts 🛑.
- Resume: clear the halt. The bot reacts ▶️.

**Local equivalents:**
- `buzz-router stop|resume|cancel [--bot NAME]...` acts through the admin API.
- `stop` must also work with the relay down: it writes the halt to SQLite and kills children even if the API is unreachable.

## 9. Wakes, adapters, posting

### 9.1 Lifecycle

```
queued ──► running ──► posted | passed | timeout | killed | failed | interrupted
```

On dispatch:
1. Create `wake_id` (UUID) and `token` (32 random bytes, hex). Store only the token's hash.
2. Set the deadline to now + `max_wake_minutes`.
3. Increment `turns_used`.
4. React 👀 if the triggers include an owner message.
5. Start the typing indicator in the thread, re-sent at buzz-acp's cadence until the wake ends.
6. Run the adapter.

Endings:
- Agent posts (API or stdout): `posted`.
- Agent passes (API, or stdout is empty or exactly `[no-reply]`): `passed`. If the wake was owner-caused and `Direct`, react ✅ ("seen, nothing to add").
- Deadline reached: kill, `timeout`, react ⌛.
- Stop or cancel: kill, `killed`, react 🛑.
- Adapter crashes, or exits non-zero without posting: `failed`, react ⚠️.
- Router crashes mid-wake: `interrupted` (section 11).

**Every owner-triggered wake ends with something David can see:** a reply, or a ✅, ⌛, 🛑 or ⚠️ reaction. Nothing addressed by David may end silently. This replaces the old `seen`/`pending` bookkeeping that dropped messages.

### 9.2 `command` adapter

- Spawn with the `command-group` crate: a process group on Unix and a Job Object on Windows. Killing the group kills every child the agent started.
- Environment:
  - `BUZZ_ROUTER_URL` (loopback API base)
  - `BUZZ_ROUTER_WAKE_TOKEN`
  - `BUZZ_ROUTER_PAYLOAD` (path to the payload JSON, section 9.3)
  - `BUZZ_ROUTER_PROMPT_FILE` (when `prompt_mode = "file"`)
  - plus the configured `env`
  - **Never** `BUZZ_PRIVATE_KEY`.
- `prompt_mode = "stdin"`: write the rendered prompt to stdin, then close it.
- `reply_mode = "stdout"`:
  - Capture stdout (cap 64 KiB) and trim it.
  - Empty or `[no-reply]` means pass. Anything else is posted as the reply when the process exits 0.
  - stderr goes to the log.
- `reply_mode = "api"`: the agent calls `buzz-router post` or `pass`. Exiting without either means pass.

### 9.3 `webhook` adapter

- `POST` the payload JSON to `url`.
- Headers: `X-Buzz-Router-Signature: sha256=<hex HMAC-SHA256 of body with secret>` and `X-Buzz-Router-Wake: <wake_id>`.
- `mode = "async"`:
  - Expect 2xx within 10 s.
  - The agent later calls `POST {public_url}/v1/post` (or `/v1/pass`, `/v1/eta`) with `Authorization: Bearer <token>`.
  - All bots run on one Tailscale network, so `public_url` is the router machine's MagicDNS name and `tailnet_bind` its Tailscale IP. WireGuard already encrypts the traffic, so there's no TLS, tunnel or reverse proxy. The router never binds to a public interface.
- `mode = "sync"`: the response body (within the deadline) is `{"text": "..."}` or `{"pass": true}`.

**Payload**, also written to the file for command adapters:

```json
{
  "wake_id": "uuid",
  "token": "hex",
  "bot": "dp-kyber-bot",
  "channel": {"id": "uuid", "name": "work-for-david"},
  "thread_root_id": "hex",
  "reply_parent_id": "hex",
  "reason": "participant",
  "round_mode": "discussion",
  "turns_left_after_this": 3,
  "turns_per_round": 4,
  "deadline": "2026-10-05T03:20:00Z",
  "triggers": ["hex", "..."],
  "context": [
    {"id": "hex", "author": "David", "class": "owner", "created_at": "...", "text": "...", "new": true}
  ],
  "api": {"url": "http://127.0.0.1:47821", "post": "/v1/post", "pass": "/v1/pass", "eta": "/v1/eta"}
}
```

`context` holds the last 20 messages of the thread, oldest first. `new: true` marks messages since this bot's last turn in the thread. `api.url` is the loopback address for command bots and `public_url` (the tailnet address) for webhook bots.

### 9.4 Built-in prompt template (stdout mode)

```
You are {bot} in the Buzz channel #{channel}.
You were woken because: {reason_text}.
This is turn {turn} of {turns_per_round} for you in this thread until David posts again.

Thread so far (oldest first, ★ = new since your last turn):
{context}

Write the single message you want to post in this thread.
If you have nothing useful to add, output exactly: [no-reply]
Do not post acknowledgements ("got it", "on it", "agreed", "standing by").
David already sees a 👀 reaction when you are woken.
```

`reason_text` by reason:
- `Mention`: "David mentioned you"
- `Everyone`: "David asked everyone; this is a discussion"
- `ReplyTarget`: "David replied to your message"
- `Participant`: "David replied in a thread you're part of"
- `Discussion`: "another bot posted in a discussion you're part of"
- `BotMention`: "{author} mentioned you"
- `DefaultBot`: "you're the default bot for this channel"

### 9.5 Local API (`api_bind`, default `127.0.0.1:47821`, plus `tailnet_bind` when set)

- Wake-token endpoints (`/v1/post`, `/v1/pass`, `/v1/eta`) are served on both binds.
- Admin endpoints are served on **loopback only** and return 404 on the tailnet bind.
- Tailscale ACLs should allow the router port only from the hosts that run agents.

| Method and path | Auth | Effect |
|---|---|---|
| `POST /v1/post {text}` | wake token | Publish a reply. 423 if halted, 410 if the wake ended, 429 over `max_posts_per_wake`. Returns `{event_id}`. |
| `POST /v1/pass` | wake token | End the wake as passed. |
| `POST /v1/eta {text}` | wake token | Text for the status note, e.g. "about 10 minutes". |
| `GET /v1/status` | admin token | Same data as `buzz-router status --json`. |
| `POST /v1/stop`, `/v1/resume`, `/v1/cancel` `{bots?}` | admin token | Section 8. |

The admin token is a random file `admin.token` in the data dir, readable only by the user (0600 on Unix; the default user-profile ACL on Windows).

### 9.6 Publishing a reply

- Kind 9, built with `buzz_sdk::builders::build_message`:
  - `h` = the wake's channel.
  - Thread tags: root = the wake's thread root, parent = the latest owner trigger, else the latest trigger.
  - `p` tags for any roster bot or owner the text @mentions, so mentions render. Resolve them with the same parser as section 6.1.
  - The bot's `auth_tag` if configured.
  - `["buzz-router", "<version>", "reply"]`.
- Record the event id in `posts` (section 12).
- Replies can only go into the wake's thread. There's no API for top-level or cross-channel posts in v1.

## 10. Confirmations

What David asked for: he must know a bot got his message, but without a confirmation on every small thing.

| Signal | When | Cost |
|---|---|---|
| 👀 reaction from the bot | the moment it's woken by a message from David | none, no chat noise |
| typing indicator | while the wake runs | none |
| **one status note** | only when the wake is owner-caused **and** `Direct`, and the bot has neither posted nor passed after `status_note_after_secs` (20 s) | one message |
| ⏸️ ⌛ 🛑 ⚠️ ✅ ▶️ reactions | the outcomes in sections 7–9 | none |

The status note:
- Text: `On it, this will take a bit.`, or `On it, about {eta}.` if the agent called `/v1/eta` first.
- Threaded under the trigger, tagged `["buzz-router", "<version>", "status"]` so it never wakes anyone and doesn't count as a turn.
- At most one per wake.
- **Never sent for `@everyone` or discussion wakes**, where ten bots posting "on it" is exactly the noise to avoid.

## 11. State and recovery

SQLite through `rusqlite` with the `bundled` feature, one file in the data dir.

```
events   (id PK, channel_id, root_id, author, class, kind, created_at, processed_at)
cursors  (bot, relay_url, last_created_at, PK(bot, relay_url))
threads  (root_id PK, channel_id, participants JSON, discussion INT,
          round_id, round_mode, round_started_at)
turns    (root_id, round_id, bot, used INT, cap_reacted INT, PK(root_id, round_id, bot))
wakes    (id PK, bot, root_id, round_id, reason, priority, triggers JSON, state,
          token_hash, attempt INT, created_at, dispatch_after, started_at, deadline,
          ended_at, outcome)
posts    (event_id PK, bot, wake_id, created_at)
halts    (scope PK, set_by_event, set_at)          -- scope = 'all' or a bot name
```

**Startup:**
1. Load halts.
2. Connect each bot.
3. Backfill from `cursor - 300s` by paging `POST /query` until caught up. Never use a fixed "last N" window.
4. Deduplicate by event id.
5. Route backfilled events normally, so messages sent while the router was down are answered. Owner messages older than 24 h are not woken; they're listed in `status` as `missed`.

**Interrupted wakes** (state `running` at startup):
- Mark them `interrupted`.
- If the triggers include an owner message and the bot hasn't posted in the thread since `started_at`, re-queue once (`attempt = 2`).
- Otherwise react ⚠️.

**Reconnect:** exponential backoff with jitter, as buzz-acp does, then backfill from the cursor.

## 12. Unmanaged-post detection

Any kind-9 event signed by a local bot's key whose id isn't in `posts` was published by something other than the router: an old listener, a routine, or a stray `buzz` CLI call. This is the "something else is still waking me" problem.

On detection:
- The bot reacts ⚠️ on that event.
- `status` counts it under `unmanaged_posts`.
- One log warning per bot per hour.

## 13. CLI

```
buzz-router run                              daemon (what the service runs)
buzz-router status [--json]                  bots, halts, running/queued wakes, budgets,
                                             missed, unmanaged_posts, roster hash, version
buzz-router stop|resume|cancel [--bot N]...
buzz-router post (--text T | --text-file P|-)   for agents; reads BUZZ_ROUTER_WAKE_TOKEN/URL
buzz-router pass
buzz-router eta --text "about 10 minutes"
buzz-router wakes [--bot N] [--state S]
buzz-router capture --channel ID --since 7d > events.jsonl     dump real events
buzz-router route --replay events.jsonl [--roster P]           offline: print every decision,
                                             simulated clock, no network, nothing published
buzz-router keys set --bot N                 read nsec from stdin into the OS keychain
buzz-router keys check
buzz-router roster check
buzz-router service install|uninstall|status
```

Errors are JSON on stderr, as the `buzz` CLI does. Exit codes: 0 ok, 1 bad input, 2 relay or network, 3 auth, 4 other.

## 14. Packaging

- **Targets:** `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`.
- Use `rustls` everywhere (no OpenSSL) and `rusqlite` with `bundled`.
- **Toolchain:** Rust ≥ 1.88 (Buzz's `rust-version`).
- **Keys:** `keyring` crate (macOS Keychain, Windows Credential Manager, Linux Secret Service). If Secret Service is unavailable on Linux, `key = "file:<path>"` with 0600 permissions is the fallback.
- **Services:**
  - macOS: user LaunchAgent `~/Library/LaunchAgents/com.buzz-router.plist`, `KeepAlive`.
  - Linux: `systemd --user` unit, `Restart=always`.
  - Windows: Task Scheduler task at logon (`schtasks`), restart on failure, running as the user so the Credential Manager works.
  - `service install` writes and loads these. The `service-manager` crate is fine for macOS and Linux.
- **CI:** build and run all tests on `macos-latest`, `ubuntu-latest`, `windows-latest`. A release workflow attaches the 5 binaries to a GitHub release.

## 15. Tests

### 15.1 Routing conformance (router-core, one fixture per row)

Setup for every row: owner O, roster bots A, B, C in the channel, no `default_bot`, not halted, not quiet hours, turns unused, unless the row says otherwise. "Wake" means a `Wake` decision. Bots not listed get no decision or a `Suppress`.

| # | Setup | Event | Expected |
|---|---|---|---|
| 1 | — | O top-level "@A status?" (`p` A) | Wake A (Mention, Owner); new round, Direct |
| 2 | — | O top-level "hey" | nobody |
| 3 | `default_bot = A` | O top-level "hey" | Wake A (DefaultBot) |
| 4 | — | O top-level "@everyone thoughts?" | Wake A, B, C (Everyone); `discussion = true`, Discussion round |
| 5 | thread T: root by O "@A …", A replied | O in T, parent = root, untagged | Wake A (Participant) |
| 6 | T from row 4; A, B, C replied | O in T, parent = root, untagged | Wake A, B, C (Participant); new round, Discussion, turns reset |
| 7 | T from row 6 | O replies to **B's** message, untagged | Wake B only (ReplyTarget); Direct |
| 8 | T from row 6 | O in T, parent = root, "@C …" | Wake C only (Mention); Direct |
| 9 | T from row 6 | O replies to B's message with "@C what about this?" | Wake C only (mention beats reply target) |
| 10 | T Discussion round, A used 1, C used 1 | B posts in T | Wake A and C (Discussion, Bot, debounce) |
| 11 | T Discussion round, A used 4, C used 2 | B posts in T | Suppress A (Cap), ⏸️ once; Wake C |
| 12 | T Direct round (O asked @A) | A replies, with `p` tag O | nobody |
| 13 | T Direct round | A posts "@B can you check X" | Wake B (BotMention); B joins participants |
| 14 | T Direct round | A posts with a `p` tag for B but no "@B" in the text | nobody |
| 15 | T Direct round | A posts "@everyone look at this" | nobody (bots can't broadcast) |
| 16 | bot-only thread: A top-level "@B …", then B "@A …" alternating | each post | each bot woken until it has used 4, then Suppress(Cap); no reset ever |
| 17 | — | O "@everyone stop" | Control Stop(all) |
| 18 | — | O "fucking stop[" | Control Stop(all) |
| 19 | — | O "@A stop" | Control Stop(A) |
| 20 | — | O "please stop the dev server and restart it" (8 words) | not a stop; normal routing → nobody |
| 21 | — | O "@A !cancel" | Control Cancel(A) |
| 22 | halts = all | O "resume" | Control Resume(all) |
| 23 | halts = all | O "@A hi" | Suppress A (Halted) |
| 24 | quiet hours; T Discussion round | B posts | Suppress A, C (Quiet) |
| 25 | quiet hours | O "@A hi" | Wake A |
| 26 | A over `wakes_per_hour` | O "@A hi" | Wake A (owner wakes are never budget-blocked) |
| 27 | A over `wakes_per_hour`; T Discussion | B posts | Suppress A (Budget) |
| 28 | — | event tagged `buzz-router … status` from A | nobody |
| 29 | A `respond_to = owner-only` | human H "@A hi" | Suppress A (RespondTo) |
| 30 | A `respond_to = anyone` | human H "@A hi" | Wake A (Mention, Human); no round reset |
| 31 | T: O top-level, no bot ever posted | O in T, untagged | nobody |
| 32 | — | O "```@A```" (mention only inside code) | nobody |
| 33 | — | O "> @everyone said…" then "what do you think @B" | Wake B only |
| 34 | roster has `dp-grok-bot` and `sf-GrokBot` | O "@dp-grok-bot hi" | Wake dp-grok-bot only |
| 35 | — | O kind-40003 edit adding `p` B | Wake B (Mention), no round reset |
| 36 | — | O top-level, `p` tags = [O's own key] only | nobody (the self tag is ignored) |
| 37 | foreign bot F (NIP-OA, not in roster) | F "@A hi" | Suppress A (RespondTo) |

### 15.2 Engine tests (the binary, fake clock, fake adapters)

- Debounce: three bot posts 5 s apart produce one wake per other participant, dispatched 20 s after the last post. A steady stream dispatches at 90 s.
- Coalescing: a trigger arriving while a wake runs produces exactly one follow-up wake.
- Stop: kills a command adapter whose child spawned a grandchild, on all 3 OSes in CI, and both processes are gone. Posts after stop return 423.
- Deadline: kill plus ⌛, and posts after it return 410.
- Status note: sent at 20 s exactly once for a Direct owner wake. Never sent for Discussion wakes. Not sent if the agent posted at 19 s.
- `max_posts_per_wake`: the 4th post returns 429.
- Restart: a `running` wake becomes `interrupted` and is re-queued once for owner triggers.
- Unmanaged post: an event signed by a bot key but not in `posts` gets a ⚠️ reaction.

### 15.3 End-to-end acceptance (against a local Buzz relay)

Run the relay from the Buzz repo (`just relay` / `docker-compose.yml`) with test identities: one owner, three bots on `command` adapters running a script that echoes after a configurable delay.

| # | Scenario | Pass when |
|---|---|---|
| E1 | O posts "@A", A replies, O replies untagged in the thread panel | A is woken and replies (the original bug) |
| E2 | O posts "@everyone" | 👀 from A, B and C within 5 s; discussion runs; no bot exceeds 4 wakes; the thread goes quiet |
| E3 | O sends "stop" during E2 | every agent process is gone within 5 s; 🛑 from each bot; nothing published afterwards; the halt survives a router restart; "resume" brings ▶️ and normal routing |
| E4 | O "@A" with a 60 s task | 👀 immediately, one status note at about 20 s, the final reply threaded under O's message |
| E5 | `kill -9` the router mid-wake, O posts while it's down, restart | the interrupted wake is re-run once; the message sent during downtime gets answered; no duplicate replies |
| E6 | Post with the `buzz` CLI using bot A's key | ⚠️ on that post; `status` shows `unmanaged_posts: 1` |
| E7 | Same binary, same tests | green on macOS, Linux and Windows CI |

## 16. Cutover (no shadow mode)

Per machine:
1. **Inventory every existing Buzz trigger for each local bot:** listeners, pollers (`bin/buzzlib.py`, `buzz_mentions_monitor.py`, the Zo router), routines, cron jobs and scheduled tasks. Write the list down.
2. Stop and **remove** all of them. Deleting a routine isn't enough if a poller still exists. That's what kept waking dp-grok-bot after it was told to stop.
3. Move each bot's key into the router (`buzz-router keys set --bot N`). **Remove `BUZZ_PRIVATE_KEY` and any nsec from the agent's environment, config and scripts.** Without this, stop and turn limits can be bypassed.
4. Install the shared `roster.toml` and check that `roster check` shows the same hash on every machine.
5. Write `router.toml`, then run `service install`.
6. Smoke test: David posts "@<bot> ping" for each bot and expects 👀 and a reply. Then "stop" (🛑 from every bot), then "resume".
7. Watch `status` for 24 h. Any `unmanaged_posts` means step 2 missed a trigger.

Tune with David afterwards by editing `[limits]` in the roster and redistributing it.

## 17. Build order

Each milestone ends with its tests green on all 3 OSes.

1. **router-core:** config types and validation, classification, parsing (mentions, `@everyone`, stop/resume/cancel), `route()`, limit gates. All of section 15.1. `buzz-router route --replay` and `roster check`.
2. **Relay I/O:**
   - One connection per bot: NIP-42 auth (standalone and NIP-OA), channel discovery, subscription, `POST /query` backfill with cursors, thread fetch, publishing messages and reactions, typing indicator.
   - `capture`.
   - Tested against the local relay.
3. **Wake engine plus stop:** queue, priority, debounce, coalescing, the command adapter with process-group kill, the loopback API, `post`/`pass`/`eta`, deadlines, reactions, the status note, stop/resume/cancel end to end. Stop is a safety feature, so it ships in this milestone, not later.
4. **State and recovery:** the SQLite schema, restart, interrupted wakes, missed messages, unmanaged-post detection, `status`.
5. **Webhook adapter** (async and sync).
6. **Packaging:** keychain, `service install` on 3 OSes, the CI matrix, the release workflow.

Then run the section 15.3 acceptance tests, then cut over.

## 18. Defaults David may tune later

These are all in `roster.toml` `[limits]` or per bot:
- `turns_per_round` (4)
- `wakes_per_hour` / `wakes_per_day` (20 / 100)
- quiet hours (23:00–07:00)
- debounce (20 s, capped at 90 s)
- `max_wake_minutes` (20)
- `status_note_after_secs` (20)
- `default_bot` per channel
- `respond_to` per bot

Behaviour choices worth revisiting after a week of use:
- **Untagged replies in an `@everyone` thread restart the whole discussion** (rule 6.3.5). The alternative is waking only the most recent bot.
- **The stop rule's 5-word limit.**
