---
id: specs/buzz-router-v1/requirements
title: buzz-router v1 requirements
doc-type: requirements
status: current
component: buzz-router
owner: dpalfery
last-reviewed: 2026-10-05
---

# buzz-router v1 Requirements

**Phase status:** Approved

**Approval:** Approved by David on 2026-10-05 ("approved"), with assumptions A1–A18 accepted as written. Approve and execute: granted.

## Introduction

buzz-router is one Rust binary per machine. It holds a relay connection for every local Buzz bot, decides deterministically which bot (if any) each message wakes, and acts as the gatekeeper: it holds the bots' signing keys, publishes their replies, and enforces stop, turn limits and budgets. It replaces every per-bot Buzz listener (dp-, sf-, Hal, Zo, Claw). The router never calls a model.

The source is the [design brief](brief.md) that David approved on 2026-10-04. Its section 0 decisions are fixed requirements here. Every criterion cites its brief section as "(brief §x)". Where the brief is silent and a builder still has to choose, the criterion also cites a numbered assumption from the "Assumptions for owner confirmation" section at the end.

### Conventions

- **Owner** means David, identified by any pubkey in `owner.pubkeys`.
- **Local bot** means a roster bot listed in this machine's `router.toml`. Each router decides only for its local bots.
- **Owner-caused, human-caused and bot-caused** describe the author class of the event that produced a `Wake` decision. They map to priority `Owner`, `Human` and `Bot`.
- **Direct** and **Discussion** are round modes (brief §6.2). A wake carries the mode it was created under.
- A **thread** is identified by its root event. A top-level message is its own root.
- **The engine** is the wake engine in the `buzz-router` binary: queue, debounce, adapters and reactions. `route` is the pure function in `router-core`.
- **CONFORM criteria** each restate one row of brief §15.1. Unless the row says otherwise, the setup is: owner O; roster bots A, B and C in the channel, all local; no `default_bot`; nothing halted; not quiet hours; no turns used. "Wake" means a `Wake` decision. Bots the row doesn't list get no decision or a `Suppress`. The text after "SHALL return:" is the brief's expected result, verbatim. Requirement 64 turns each one into a fixture.

### Coverage map

| Brief section | Requirements |
|---|---|
| §0–§3 (decisions, scope, Buzz facts, architecture) | 5, 59, 61, 62, 63 |
| §4 Configuration | 1, 2, 3 |
| §5 Classifying authors | 4 |
| §6 Routing | 5–18 |
| §7 Limits and budgets | 19–28 |
| §8 Stop, resume, cancel | 29–33 |
| §9 Wakes, adapters, posting | 34–44 |
| §10 Confirmations | 45, 46 |
| §11 State and recovery | 47–50 |
| §12 Unmanaged-post detection | 51 |
| §13 CLI | 2, 33, 52–55, 57 |
| §14 Packaging | 56, 58, 59, 60 |
| §15 Tests | 64, 65, 66 |
| §16 Cutover | 63 |

## Requirements

### Requirement 1: Per-machine configuration (`router.toml`)
**User Story:** As the operator, I want one per-machine file that names the relay, the API binds, and each local bot's key and adapter, so that each router knows which bots it serves and how to wake them.

#### Acceptance Criteria
1.1. WHEN the router starts THEN it SHALL read `router.toml` from the config directory (Requirement 3) and accept the keys `relay_url`, `api_bind`, `tailnet_bind`, `public_url` and `roster_path`. (brief §4, §4.2)
1.2. The router SHALL resolve `roster_path` relative to the config directory, and SHALL use `roster.toml` when it's omitted. (brief §4.2; assumption A2)
1.3. WHERE `api_bind` is omitted, the router SHALL serve the loopback API on `127.0.0.1:47821`. The loopback API SHALL always be on. (brief §4.2, §9.5)
1.4. WHERE `tailnet_bind` is set to this machine's Tailscale IP and port, the router SHALL also serve the wake-token endpoints there (Requirement 42). WHERE it's empty, the router SHALL NOT open a second listener. (brief §4.2, §9.5)
1.5. WHERE `public_url` is set, the router SHALL give it to webhook agents as the callback base URL (Requirement 40). (brief §4.2, §9.3)
1.6. The router SHALL accept `name`, `key`, `auth_tag`, `max_concurrent` and one `[bots.adapter]` table in each `[[bots]]` entry. (brief §4.2)
1.7. IF a `[[bots]]` `name` isn't a bot in the roster THEN the router SHALL reject the configuration. (brief §4.2)
1.8. The router SHALL accept `key = "keychain"`, meaning the OS keychain entry `buzz-router/<name>`, or `key = "file:<path>"` (Requirement 59), and SHALL reject any other value. (brief §4.2, §14)
1.9. WHERE `auth_tag` is set, the router SHALL use it as the bot's NIP-OA tag JSON, the way `BUZZ_AUTH_TAG` is used, for relay authentication (Requirement 61) and on published replies (Requirement 44). (brief §2, §4.2, §9.6)
1.10. WHERE `max_concurrent` is omitted, the router SHALL use 1. (brief §7)
1.11. WHERE `[bots.adapter]` has `type = "command"`, the router SHALL accept `command` (an argument array), `cwd`, `env`, `prompt_mode` (`stdin` or `file`), `reply_mode` (`stdout` or `api`) and `prompt_template` (a file path, where empty means the built-in template). (brief §4.2)
1.12. WHERE `[bots.adapter]` has `type = "webhook"`, the router SHALL accept `url`, `secret_env` (the name of the environment variable that holds the HMAC secret), `mode` (`async` or `sync`) and `cancel_url` (optional, best-effort). (brief §4.2)
1.13. IF a required key is missing, or a key with listed options holds any other value, THEN the router SHALL reject the configuration. (brief §4.2; assumption A2)
1.14. IF `router.toml` breaks a validation rule in assumption A3 THEN the router SHALL reject it. (brief §4.2, §9.3; assumption A3)
1.15. IF `buzz-router run` finds `router.toml` or the roster invalid THEN it SHALL print a JSON error on stderr and exit with code 1 without serving any bot. (brief §13)
1.16. The repository SHALL include a `router.example.toml` that holds placeholders only: no real keys, nsecs, tokens, pubkeys or relay URLs. (brief §3; AGENTS.md)

### Requirement 2: Shared roster (`roster.toml`)
**User Story:** As the operator, I want one roster file, identical on every machine, that names the owner, the limits, the channels and every bot, so that every router classifies authors and applies limits the same way.

#### Acceptance Criteria
2.1. The router SHALL read a `roster.toml` containing `version = 1`, an `[owner]` table (`name`, `pubkeys`, `timezone`), a `[limits]` table, `[[channels]]` entries and `[[bots]]` entries. (brief §4.1)
2.2. The router SHALL accept more than one pubkey in `owner.pubkeys` and SHALL treat each one as the owner. (brief §4.1, §5)
2.3. IF `owner.timezone` isn't a valid IANA zone name THEN the router SHALL reject the roster. (brief §4.1)
2.4. The router SHALL accept `turns_per_round`, `wakes_per_hour`, `wakes_per_day`, `quiet_hours`, `discussion_debounce_secs`, `discussion_debounce_max_secs`, `max_wake_minutes`, `max_posts_per_wake` and `status_note_after_secs` in `[limits]` as the defaults for every bot. (brief §4.1)
2.5. WHERE a `[limits]` key is omitted, the router SHALL use its default: `turns_per_round` 4, `wakes_per_hour` 20, `wakes_per_day` 100, `quiet_hours` `"23:00-07:00"`, `discussion_debounce_secs` 20, `discussion_debounce_max_secs` 90, `max_wake_minutes` 20, `max_posts_per_wake` 3, `status_note_after_secs` 20. (brief §4.1, §18)
2.6. WHERE a `[[bots]]` entry has a `limits` table, the router SHALL apply those values to that bot only, and the `[limits]` defaults for every key the table omits. (brief §4.1, §18)
2.7. The router SHALL accept `id` (a channel UUID), `name` and an optional `default_bot` in each `[[channels]]` entry. (brief §4.1)
2.8. The router SHALL accept `name` (the exact Buzz display name), `pubkey` (hex), `aliases`, `channels`, `respond_to` (`owner-only` or `anyone`), `machine` and `limits` in each `[[bots]]` entry. (brief §4.1)
2.9. The router SHALL treat `machine` as informational and SHALL NOT use it in any decision. (brief §4.1)
2.10. WHERE a bot's `channels` is `["*"]`, the bot SHALL cover every channel it's a member of. WHERE it's a list of channel UUIDs, the bot SHALL cover only those channels. (brief §4.1)
2.11. The router SHALL treat each alias as an extra `@name` for its bot, matched as a whole word exactly as bot names are (Requirement 17). (brief §4.1, §6.1)
2.12. IF the roster breaks a validation rule in assumption A3 THEN the router SHALL reject it. (brief §4.1; assumption A3)
2.13. WHEN the operator runs `buzz-router roster check` on a valid roster THEN the CLI SHALL print the SHA-256 of the roster file and exit 0. (brief §4.1, §13)
2.14. IF the roster is invalid THEN `roster check` SHALL print a JSON error on stderr and exit with code 1. (brief §13)
2.15. `roster check` SHALL read the roster at `roster_path`, or at `roster.toml` in the config directory when `router.toml` is absent or omits `roster_path`, so that it works before `router.toml` exists. (brief §4.2, §16 steps 4–5; assumption A2)
2.16. `buzz-router status` SHALL show the SHA-256 of the roster the running router loaded, in the same form `roster check` prints, so that a stale copy on one machine is easy to spot. (brief §4.1, §13; assumption A15)
2.17. The repository SHALL include a `roster.example.toml` that holds placeholders only. (brief §3; AGENTS.md)

### Requirement 3: Configuration and data directories
**User Story:** As the operator, I want the router to find its files in each OS's standard user directories, so that setup is the same on every machine of a kind.

#### Acceptance Criteria
3.1. WHEN the router runs on macOS THEN it SHALL use `~/Library/Application Support/buzz-router` as its config directory. (brief §4)
3.2. WHEN the router runs on Linux THEN it SHALL use `~/.config/buzz-router` as its config directory. (brief §4)
3.3. WHEN the router runs on Windows THEN it SHALL use `%APPDATA%\buzz-router` as its config directory. (brief §4)
3.4. The router SHALL resolve the config directory and the data directory with the `directories` crate. (brief §4)
3.5. The router SHALL keep its SQLite database, `admin.token` and logs in the data directory. (brief §4, §9.5, §11)

### Requirement 4: Author classification and event validity
**User Story:** As David, I want every event's author placed in exactly one class, and invalid or router-status events neutralised, so that each routing rule applies the right permissions.

#### Acceptance Criteria
4.1. WHEN the router receives an event with an invalid signature THEN it SHALL drop the event without routing it. (brief §5)
4.2. The router SHALL classify each event's author into exactly one class, tested in this order: `owner` if the pubkey is in `owner.pubkeys`; else `bot` if the pubkey is a roster bot; else `foreign_bot` if the event carries an `auth` tag that `buzz_sdk::nip_oa` verifies as a valid NIP-OA tag; else `human`. (brief §2, §5)
4.3. IF an event's `auth` tag fails NIP-OA verification and the author is neither the owner nor a roster bot THEN the router SHALL classify the author as `human`. (brief §5)
4.4. IF a `foreign_bot` author's verified `auth` tag names an owner pubkey in `owner.pubkeys` THEN the router SHALL log `roster drift` once for that author, not on every event. (brief §5; assumption A12)
4.5. WHEN an event carries the tag `["buzz-router", <version>, "status"]` THEN `route` SHALL wake no bot for it, whatever its author or content. (brief §5, §10)
4.6. CONFORM: test case 28. WHEN A publishes an event tagged `buzz-router … status` THEN `route` SHALL return: nobody. (brief §15.1)

### Requirement 5: Pure routing function
**User Story:** As the builder, I want every routing decision made by one pure function, so that every rule can be tested offline without a relay, a database or a clock.

#### Acceptance Criteria
5.1. The `router-core` crate SHALL expose `route(ev: &InEvent, snap: &Snapshot, now: DateTime<Utc>) -> RouteResult`. (brief §6)
5.2. `RouteResult` SHALL carry `control` (an optional `Stop{scope}`, `Resume{scope}` or `Cancel{scope}`), `decisions`, and `thread_update` (participants added, discussion flag, new round). (brief §6)
5.3. Each decision SHALL be either `Wake { bot, reason, priority, debounce }` or `Suppress { bot, why }`, with `reason` one of `Mention`, `Everyone`, `ReplyTarget`, `Participant`, `DefaultBot`, `Discussion`, `BotMention`; `priority` one of `Owner`, `Human`, `Bot`; and `why` one of `Halted`, `Cap`, `Quiet`, `Budget`, `RespondTo`. (brief §6)
5.4. `Snapshot` SHALL hold the roster, the local bot set, halts, the state of the event's thread, per-bot turns used in the current round, per-bot hourly and daily wake counts, and the quiet-hours flag. (brief §6)
5.5. `route` SHALL perform no I/O and SHALL read no clock other than its `now` argument. (brief §6)
5.6. WHEN `route` is called twice with equal arguments THEN it SHALL return equal results. (brief §6)
5.7. `route` SHALL return decisions only for local bots, and at most one decision per bot. (brief §3, §6; assumption A12)
5.8. `route` SHALL consider a bot for an event only if the bot's `channels` covers the event's channel. (brief §4.1; assumption A12)
5.9. The `router-core` crate SHALL contain the config types and validation, author classification, parsing (mentions, `@everyone`, stop, resume and cancel), `route` and the limit gates, and SHALL NOT depend on tokio. (brief §3, §17)

### Requirement 6: Owner messages: evaluation order and explicit mentions
**User Story:** As David, I want my messages routed by a fixed order of rules, and a mention to wake exactly the bots I name, so that I always know who will answer.

#### Acceptance Criteria
6.1. WHEN the owner posts a kind-9 message THEN `route` SHALL evaluate these rules in order and stop at the first that matches: control (Requirement 29), `@everyone` (Requirement 7), explicit mentions (6.2), reply target (Requirement 8), untagged in a thread with participants (Requirement 9), channel default (Requirement 10), otherwise nobody (10.2). (brief §6.3)
6.2. WHEN an owner kind-9 message reaches the mention rule and mentions roster bots by text, npub or `p` tag (Requirement 17) THEN `route` SHALL target exactly the mentioned bots, with reason `Mention` and `round_mode = Direct`. (brief §6.3)
6.3. WHEN an owner message both mentions bots and replies to a roster bot's message THEN `route` SHALL target only the mentioned bots. (brief §6.3)
6.4. WHEN an owner kind-9 message produces at least one target (Requirements 6–10) THEN `thread_update` SHALL start a new round: `round_id` becomes this event, every bot's `turns_used` in the thread resets to 0, the targets join `participants`, and `round_mode` is set by the rule that matched. (brief §6.3)
6.5. WHEN an owner kind-9 message produces no target THEN `route` SHALL NOT start a new round. (brief §0, §6.3)
6.6. `route` SHALL give every owner-caused wake `priority = Owner` and `debounce = false`. (brief §6.3, §7)
6.7. `route` SHALL NOT suppress an owner-caused wake for quiet hours or for the hourly or daily budget. Owner-caused wakes still count as turns and toward the budgets. (brief §6.3, §7)
6.8. IF a target of an owner kind-9 message is halted THEN `route` SHALL return `Suppress(Halted)` for it. Halted is the only gate on owner kind-9 targets. (brief §6.3, §8)
6.9. CONFORM: test case 1. WHEN O posts top-level "@A status?" with a `p` tag for A THEN `route` SHALL return: Wake A (Mention, Owner); new round, Direct. (brief §15.1)
6.10. CONFORM: test case 8. WHILE thread T is as left by test case 6, WHEN O posts in T with parent = root and the text "@C …" THEN `route` SHALL return: Wake C only (Mention); Direct. (brief §15.1)
6.11. CONFORM: test case 9. WHILE thread T is as left by test case 6, WHEN O replies to B's message with "@C what about this?" THEN `route` SHALL return: Wake C only (mention beats reply target). (brief §15.1)

### Requirement 7: Owner `@everyone` discussions
**User Story:** As David, I want `@everyone` to wake every bot in the channel and start a discussion in which the bots are woken by each other's posts, so that I can get every bot's view at once.

#### Acceptance Criteria
7.1. `route` SHALL detect `@everyone` with the regex `(?i)(^|\s)@everyone\b`, applied to the content after removing code regions with `strip_code_regions` and removing lines that start with `>`. (brief §6.1)
7.2. `route` SHALL count `@everyone` only when the author is `owner`. In any other author's message it SHALL be plain text with no routing effect. (brief §6.1, §6.4, §6.5)
7.3. WHEN an owner kind-9 message that isn't a control command contains a counted `@everyone` THEN `route` SHALL target every roster bot whose `channels` covers the event's channel, with reason `Everyone`. (brief §6.3)
7.4. WHEN `@everyone` produces targets THEN `thread_update` SHALL set the thread's `discussion = true` and `round_mode = Discussion`, and start a new round (6.4). (brief §6.3)
7.5. WHEN an owner message contains both a counted `@everyone` and explicit mentions THEN `route` SHALL apply the `@everyone` rule. (brief §6.3)
7.6. Once a thread's `discussion` is true, the router SHALL keep it true for that thread. (brief §6.2)
7.7. CONFORM: test case 4. WHEN O posts top-level "@everyone thoughts?" THEN `route` SHALL return: Wake A, B, C (Everyone); `discussion = true`, Discussion round. (brief §15.1)

### Requirement 8: Owner reply to a bot's message
**User Story:** As David, I want replying to a specific bot message to wake that bot, so that I can continue a conversation with one bot inside a busy thread.

#### Acceptance Criteria
8.1. WHEN an owner kind-9 message reaches the reply-target rule, is a reply whose parent isn't the thread root, and the parent's author is a roster bot THEN `route` SHALL target that bot, with reason `ReplyTarget` and `round_mode = Direct`. (brief §6.3)
8.2. `route` SHALL take each event's root and parent from `ThreadMarkers::resolve()` (62.4), and SHALL treat a reply whose parent equals its root as a reply to the root, which the reply-target rule doesn't cover. (brief §2, §6.3)
8.3. WHEN the reply-target rule produces a target THEN `route` SHALL start a new round (6.4). (brief §6.3)
8.4. CONFORM: test case 7. WHILE thread T is as left by test case 6, WHEN O replies to B's message, untagged THEN `route` SHALL return: Wake B only (ReplyTarget); Direct. (brief §15.1)

### Requirement 9: Owner untagged message in a thread with participants
**User Story:** As David, I want an untagged reply in a thread to wake the bots already in that conversation, so that a thread keeps working without re-mentioning anyone (the original bug).

#### Acceptance Criteria
9.1. WHEN an owner kind-9 message reaches the participant rule, is in a thread rather than top-level, and the thread has participants THEN `route` SHALL target every participant, with reason `Participant`. (brief §6.3)
9.2. WHEN the participant rule produces targets THEN the new round SHALL be `Discussion` if the thread's `discussion` is true, and `Direct` otherwise. (brief §6.3)
9.3. `route` SHALL apply the participant rule whether the message's parent is the root or a message not authored by a roster bot. (brief §2, §6.3)
9.4. CONFORM: test case 5. WHILE thread T has a root by O "@A …" and A replied, WHEN O posts in T with parent = root, untagged THEN `route` SHALL return: Wake A (Participant). (brief §15.1)
9.5. CONFORM: test case 6. WHILE thread T is the thread from test case 4 and A, B and C replied, WHEN O posts in T with parent = root, untagged THEN `route` SHALL return: Wake A, B, C (Participant); new round, Discussion, turns reset. (brief §15.1)
9.6. CONFORM: test case 31. WHILE thread T has a top-level message by O and no bot has ever posted in it, WHEN O posts in T, untagged THEN `route` SHALL return: nobody. (brief §15.1)

### Requirement 10: Channel default and no target
**User Story:** As David, I want a channel's default bot to answer my untagged top-level messages there, and nobody to answer when no rule applies, so that routing never guesses.

#### Acceptance Criteria
10.1. WHEN an owner kind-9 message reaches the channel-default rule, the channel's `default_bot` is set, and the message is top-level or in a thread with no participants THEN `route` SHALL target `default_bot`, with reason `DefaultBot` and `round_mode = Direct`. (brief §4.1, §6.3)
10.2. WHEN an owner kind-9 message matches no rule THEN `route` SHALL wake nobody, and SHALL NOT infer a target from recent conversation. (brief §6.3)
10.3. `route` SHALL apply `default_bot` only to owner messages. (brief §4.1, §6.3)
10.4. CONFORM: test case 2. WHEN O posts top-level "hey" THEN `route` SHALL return: nobody. (brief §15.1)
10.5. CONFORM: test case 3. WHILE `default_bot = A`, WHEN O posts top-level "hey" THEN `route` SHALL return: Wake A (DefaultBot). (brief §15.1)

### Requirement 11: Bot messages: authorship and bot mentions
**User Story:** As a bot, I want to bring another bot into a thread by mentioning it in my text, so that bots can hand work to each other without tag noise waking anyone.

#### Acceptance Criteria
11.1. `route` SHALL never wake the author of a bot message. (brief §6.4)
11.2. WHEN a roster bot posts a kind-9 message THEN `thread_update` SHALL add the author to the thread's `participants`. (brief §6.2, §6.4)
11.3. WHEN a roster bot's message mentions other roster bots by text or npub (Requirement 17) THEN `route` SHALL target each mentioned bot with reason `BotMention`, and `thread_update` SHALL add each one to `participants`. (brief §6.4)
11.4. `route` SHALL NOT treat `p` tags on a bot's message as mentions. (brief §6.1, §6.4)
11.5. A bot message SHALL NOT start a new round or change `round_mode`, except that a top-level bot message creates a bot round (Requirement 13). (brief §0, §6.4)
11.6. CONFORM: test case 13. WHILE thread T is in a Direct round, WHEN A posts "@B can you check X" THEN `route` SHALL return: Wake B (BotMention); B joins participants. (brief §15.1)
11.7. CONFORM: test case 14. WHILE thread T is in a Direct round, WHEN A posts with a `p` tag for B but no "@B" in the text THEN `route` SHALL return: nobody. (brief §15.1)
11.8. CONFORM: test case 15. WHILE thread T is in a Direct round, WHEN A posts "@everyone look at this" THEN `route` SHALL return: nobody (bots can't broadcast). (brief §15.1)

### Requirement 12: Bot messages in discussions, and gates on bot-caused wakes
**User Story:** As David, I want bots in an `@everyone` discussion woken by each other's posts, within the limits, so that a discussion runs on its own and then goes quiet.

#### Acceptance Criteria
12.1. WHEN a roster bot posts in a thread whose `round_mode = Discussion` THEN `route` SHALL also target every other participant, with reason `Discussion`. (brief §6.4)
12.2. WHEN a bot is both mentioned in a bot message and a discussion participant THEN `route` SHALL return one decision for it, with reason `BotMention`. (brief §6, §6.4; assumption A12)
12.3. WHEN a roster bot posts in a thread whose `round_mode = Direct` THEN `route` SHALL target only the bots it mentions by text or npub. (brief §6.4)
12.4. `route` SHALL gate each bot-caused target in the order Halted, Quiet, Cap (`turns_used ≥ turns_per_round`), Budget (the hourly count has reached `wakes_per_hour` or the daily count has reached `wakes_per_day`), and SHALL return `Suppress` with the first gate that fails. (brief §6.4)
12.5. WHEN a bot-caused target passes every gate THEN `route` SHALL return `Wake` with `priority = Bot` and `debounce = true`. (brief §6.4)
12.6. CONFORM: test case 10. WHILE thread T is in a Discussion round, A has used 1 turn and C has used 1, WHEN B posts in T THEN `route` SHALL return: Wake A and C (Discussion, Bot, debounce). (brief §15.1)
12.7. CONFORM: test case 12. WHILE thread T is in a Direct round in which O asked @A, WHEN A replies with a `p` tag for O THEN `route` SHALL return: nobody. (brief §15.1)

### Requirement 13: Bot rounds in bot-started threads
**User Story:** As David, I want bot-to-bot threads I haven't joined to run out of turns and stop, so that two bots can never loop forever.

#### Acceptance Criteria
13.1. WHEN a roster bot posts a top-level message THEN `thread_update` SHALL create its thread with a bot round in which `round_mode = Direct`. (brief §6.4)
13.2. The router SHALL NOT reset a bot round's turns until an owner message in that thread starts a new round (6.4). (brief §0, §6.4)
13.3. CONFORM: test case 16. WHILE a bot-only thread was started by A top-level "@B …" and B and A then alternate "@A …" and "@B …" posts, WHEN each post arrives THEN `route` SHALL return: each bot woken until it has used 4, then Suppress(Cap); no reset ever. (brief §15.1)

### Requirement 14: Human messages
**User Story:** As a person other than David in a Buzz channel, I want to address a bot that's configured to answer anyone, so that I can get help from it while other bots stay owner-only.

#### Acceptance Criteria
14.1. WHEN a `human` author's kind-9 message mentions a roster bot (text, npub or `p` tag) or replies to a roster bot's message whose parent isn't the root THEN `route` SHALL consider that bot. If both apply, the mentioned bots win, as for the owner. (brief §6.5; assumption A12)
14.2. IF a considered bot has `respond_to = "owner-only"` THEN `route` SHALL return `Suppress(RespondTo)` for it, before any other gate. (brief §6.5)
14.3. WHERE a considered bot has `respond_to = "anyone"`, `route` SHALL gate it in the order Halted, Quiet, Cap, Budget and, if it passes, return `Wake` with reason `Mention` (for a mention) or `ReplyTarget` (for a reply), `priority = Human` and `debounce = false`. (brief §6.5, §7)
14.4. A human message SHALL NOT start a new round, issue a control command, or count `@everyone`. (brief §6.5)
14.5. The router SHALL apply `respond_to` only to human-authored events. It SHALL NOT affect owner-caused or bot-caused wakes. (brief §6.4, §6.5)
14.6. CONFORM: test case 29. WHILE A has `respond_to = owner-only`, WHEN human H posts "@A hi" THEN `route` SHALL return: Suppress A (RespondTo). (brief §15.1)
14.7. CONFORM: test case 30. WHILE A has `respond_to = anyone`, WHEN human H posts "@A hi" THEN `route` SHALL return: Wake A (Mention, Human); no round reset. (brief §15.1)

### Requirement 15: Foreign bots
**User Story:** As David, I want bots outside the roster unable to wake my bots, so that only managed bots take part in routing.

#### Acceptance Criteria
15.1. WHEN a `foreign_bot` author posts THEN `route` SHALL return no `Wake` decision. (brief §6.6)
15.2. WHEN a foreign bot's event mentions a local bot in any form, or replies to its message, THEN `route` SHALL return `Suppress(RespondTo)` for that bot. (brief §6.6)
15.3. CONFORM: test case 37. WHILE F is a foreign bot (valid NIP-OA, not in the roster), WHEN F posts "@A hi" THEN `route` SHALL return: Suppress A (RespondTo). (brief §15.1)

### Requirement 16: Owner edits (kind 40003)
**User Story:** As David, I want adding a mention by editing my message to wake the newly mentioned bot, so that I can fix a forgotten mention without reposting.

#### Acceptance Criteria
16.1. WHEN the owner publishes a kind-40003 edit THEN `route` SHALL treat each `p` tag on the edit that names a roster bot, other than the author's own pubkey, as a newly added mention, and SHALL target those bots with reason `Mention` in a Direct wake. The targets SHALL join `participants`. (brief §2, §6.7; assumption A11)
16.2. `route` SHALL count an edit's wakes as turns in the thread's current round, and SHALL NOT reset `turns_used` or change the thread's `round_id` or `round_mode`. (brief §6.7; assumption A11)
16.3. `route` SHALL gate edit targets for Halted and then Cap only, with `priority = Owner` and `debounce = false`. (brief §6.3, §6.7, §7; assumption A11)
16.4. `route` SHALL NOT parse control commands from an edit. (brief §6.7)
16.5. `route` SHALL ignore kind-40003 edits from any author other than the owner. (brief §6.7)
16.6. `route` SHALL NOT read mentions from the edit's text. Only its `p` tags count. (brief §2, §6.7)
16.7. The router SHALL place an edit in the thread of the message it edits, which is the event named by the edit's unmarked `e` tag (as `buzz_sdk::builders::build_edit` produces it at the pinned commit). It SHALL use that message, not the edit event, as the trigger for reactions and reply threading. (brief §2, §6.7; assumption A8)
16.8. CONFORM: test case 35. WHEN O publishes a kind-40003 edit adding a `p` tag for B THEN `route` SHALL return: Wake B (Mention), no round reset. (brief §15.1)

### Requirement 17: Mention parsing
**User Story:** As David, I want mentions detected exactly, ignoring code, quotes and my own tag, so that a bot is woken only when it's really addressed.

#### Acceptance Criteria
17.1. Before extracting text or npub mentions, `route` SHALL remove code regions with `buzz_sdk::mentions::strip_code_regions` and remove every line that starts with `>`. (brief §6.1)
17.2. `route` SHALL find text mentions with `extract_at_mentions_with_known(content, names)`, where `names` holds every roster bot name and alias. Matching SHALL be case-insensitive, whole-word and longest name first. Each match SHALL map to its roster bot, and tokens that aren't a roster name or alias SHALL be ignored. (brief §4.1, §6.1)
17.3. `route` SHALL find `nostr:npub1…` and `nostr:nprofile1…` URIs and map them to roster bots by pubkey, using `extract_nostr_uris` for npub URIs. (brief §6.1; assumption A18)
17.4. `route` SHALL count a `p` tag that names a roster bot as a mention only when the author is `owner` or `human`, and SHALL ignore any `p` tag that carries the author's own pubkey. (brief §2, §6.1)
17.5. `route` SHALL NOT count an `@name` or URI that appears only inside a code region or a `>`-quoted line. (brief §6.1)
17.6. CONFORM: test case 32. WHEN O posts ```` ```@A``` ````, so the mention is only inside code, THEN `route` SHALL return: nobody. (brief §15.1)
17.7. CONFORM: test case 33. WHEN O posts a message whose first line is "> @everyone said…" and whose next line is "what do you think @B" THEN `route` SHALL return: Wake B only. (brief §15.1)
17.8. CONFORM: test case 34. WHILE the roster has `dp-grok-bot` and `sf-GrokBot`, WHEN O posts "@dp-grok-bot hi" THEN `route` SHALL return: Wake dp-grok-bot only. (brief §15.1)
17.9. CONFORM: test case 36. WHEN O posts a top-level message whose only `p` tag is O's own key THEN `route` SHALL return: nobody (the self tag is ignored). (brief §15.1)

### Requirement 18: Thread state
**User Story:** As David, I want each router to know who is in a thread and where its round stands, even after a restart or on a new machine, so that routing stays consistent.

#### Acceptance Criteria
18.1. The router SHALL keep, per thread root: `root_id`, `channel_id`, `participants` (a set of roster bots), `discussion`, `round_id`, `round_mode` (`Direct` or `Discussion`) and `turns_used` per bot, counting wakes dispatched in the current round. (brief §6.2)
18.2. WHEN the router needs the state of a thread it hasn't seen (a new machine, after a restart, or an old thread) THEN it SHALL fetch the thread from the relay (the root plus its replies, filter `#e` = root) and replay its events through `route` in `created_at` order without dispatching any wake. (brief §6.2)
18.3. WHILE replaying a thread to rebuild its state, the router SHALL publish nothing (no reactions, replies, status notes or typing indicators) and SHALL NOT count unmanaged posts. (brief §6.2; assumption A17)
18.4. WHEN the router rebuilds a thread THEN it SHALL take the current round's turn counts from the `wakes` table where that table holds wakes for the round. Otherwise it SHALL approximate each bot's count as its posts in the thread since the round started. (brief §6.2)
18.5. The router SHALL persist thread state and turn counts in SQLite (Requirement 47). (brief §11)

### Requirement 19: Turns per round
**User Story:** As David, I want each bot limited to a fixed number of turns per round, so that no thread runs away while I'm not looking.

#### Acceptance Criteria
19.1. The router SHALL count every dispatched wake as one turn for that bot in the thread's current round, whatever caused it and whether the bot posts or passes. (brief §0, §7, §9.1)
19.2. WHEN a target subject to the Cap gate (Requirements 12, 14, 16) has `turns_used ≥ turns_per_round` (its effective limit) THEN `route` SHALL return `Suppress(Cap)`. (brief §6.4, §7)
19.3. WHEN `route` returns `Suppress(Cap)` for a bot that hasn't yet reacted ⏸️ in the current round THEN that bot SHALL react ⏸️ on the event that would have woken it. Later Cap suppressions in the same round SHALL NOT react again. (brief §7, §11)
19.4. A suppressed decision SHALL NOT consume a turn. (brief §7, §9.1)
19.5. The router SHALL reset turns only when an owner message starts a new round (6.4). (brief §0, §6.3)
19.6. WHILE the owner doesn't post in a thread, the router SHALL dispatch at most `turns_per_round` wakes per bot in the current round, so one `@everyone` round costs at most 4 × (number of bots) wakes before the thread goes quiet. (brief §7)
19.7. CONFORM: test case 11. WHILE thread T is in a Discussion round, A has used 4 turns and C has used 2, WHEN B posts in T THEN `route` SHALL return: Suppress A (Cap), ⏸️ once; Wake C. (brief §15.1)

### Requirement 20: Hourly wake budget
**User Story:** As David, I want a per-bot hourly wake budget on bot-caused and human-caused wakes, so that a busy hour can't burn unlimited agent runs.

#### Acceptance Criteria
20.1. The router SHALL count every dispatched wake for a bot, owner-caused ones included, toward that bot's hourly count. (brief §7; assumption A10)
20.2. WHEN a bot-caused or human-caused target's hourly count has reached `wakes_per_hour` THEN `route` SHALL return `Suppress(Budget)`, unless an earlier gate already failed. (brief §6.4, §6.5, §7)
20.3. `route` SHALL NOT block an owner-caused wake on the hourly budget. (brief §6.3, §7)
20.4. WHEN a wake is suppressed for Budget THEN the router SHALL log it and show it in `status`. (brief §7)
20.5. CONFORM: test case 26. WHILE A is over `wakes_per_hour`, WHEN O posts "@A hi" THEN `route` SHALL return: Wake A (owner wakes are never budget-blocked). (brief §15.1)
20.6. CONFORM: test case 27. WHILE A is over `wakes_per_hour` and thread T is in a Discussion round, WHEN B posts THEN `route` SHALL return: Suppress A (Budget). (brief §15.1)

### Requirement 21: Daily wake budget
**User Story:** As David, I want a per-bot daily wake budget as well, so that a busy day can't burn unlimited agent runs.

#### Acceptance Criteria
21.1. The router SHALL count every dispatched wake for a bot, owner-caused ones included, toward that bot's daily count. (brief §7; assumption A10)
21.2. WHEN a bot-caused or human-caused target's daily count has reached `wakes_per_day` THEN `route` SHALL return `Suppress(Budget)`, unless an earlier gate already failed. (brief §6.4, §6.5, §7)
21.3. `route` SHALL NOT block an owner-caused wake on the daily budget. (brief §6.3, §7)
21.4. WHEN a wake is suppressed for the daily budget THEN the router SHALL log it and show it in `status`. (brief §7)

### Requirement 22: Quiet hours
**User Story:** As David, I want bot-caused and human-caused wakes dropped overnight, so that nothing runs, or floods in at 7am, while I'm asleep.

#### Acceptance Criteria
22.1. `route` SHALL treat `now` as inside quiet hours when its wall-clock time in `owner.timezone` falls in the bot's `quiet_hours` range `"HH:MM-HH:MM"`. The range is a half-open interval [start, end) that wraps past midnight when start is later than end. (brief §4.1, §7; assumption A1)
22.2. WHERE a bot's `quiet_hours` is `""`, the router SHALL disable quiet hours for that bot. (brief §4.1)
22.3. IF `quiet_hours` is neither `""` nor of the form `"HH:MM-HH:MM"` THEN the router SHALL reject the roster. (brief §4.1)
22.4. WHEN a bot-caused or human-caused target is inside quiet hours and isn't halted THEN `route` SHALL return `Suppress(Quiet)`. (brief §6.4, §6.5, §7)
22.5. The router SHALL drop quiet-hours suppressions rather than defer them: no wake for a suppressed trigger SHALL run when quiet hours end. (brief §7)
22.6. `route` SHALL decide quiet hours from `now` only, never from the wall clock. (brief §6)
22.7. CONFORM: test case 24. WHILE it's quiet hours and thread T is in a Discussion round, WHEN B posts THEN `route` SHALL return: Suppress A, C (Quiet). (brief §15.1)
22.8. CONFORM: test case 25. WHILE it's quiet hours, WHEN O posts "@A hi" THEN `route` SHALL return: Wake A. (brief §15.1)

### Requirement 23: Debounce for bot-caused wakes
**User Story:** As David, I want a bot woken by other bots to wait for the burst of posts to settle, so that it answers once with everything in view rather than once per post.

#### Acceptance Criteria
23.1. WHEN the engine queues a wake from a `debounce: true` decision (reason `Discussion` or `BotMention`) THEN it SHALL make the wake dispatchable at the later of (the last roster-bot post in the thread + `discussion_debounce_secs`) and (the wake's first trigger + `discussion_debounce_secs`), but no later than the wake's first trigger + `discussion_debounce_max_secs`. (brief §7)
23.2. WHEN a later trigger coalesces into a debounced wake (Requirement 24) THEN the engine SHALL recompute its dispatch time from the newest bot post, still capped at the first trigger + `discussion_debounce_max_secs`. (brief §7)
23.3. The engine SHALL dispatch owner-caused and human-caused wakes without debounce. (brief §7)
23.4. WHEN three bot posts arrive 5 s apart under the default limits THEN the engine SHALL dispatch one wake per other participant, 20 s after the last post. (brief §15.2)
23.5. WHEN bot posts keep arriving less than `discussion_debounce_secs` apart THEN the engine SHALL dispatch at the first trigger + `discussion_debounce_max_secs` (90 s by default). (brief §7, §15.2)

### Requirement 24: Wake coalescing
**User Story:** As David, I want triggers for the same bot in the same thread merged into one wake, so that a bot runs once and sees every new message since its last turn.

#### Acceptance Criteria
24.1. The engine SHALL hold at most one queued wake per (bot, thread). (brief §7)
24.2. WHEN a trigger arrives for a (bot, thread) that has a queued wake THEN the engine SHALL append the trigger to that wake's trigger list instead of creating a wake. (brief §7)
24.3. WHEN a trigger arrives for a (bot, thread) whose wake is running THEN the engine SHALL put it into one follow-up wake that starts after the running wake ends. Further triggers SHALL join that follow-up, so exactly one follow-up wake results. (brief §7, §15.2)
24.4. The router SHALL count a coalesced wake as one turn. (brief §7)
24.5. The engine SHALL derive a queued wake's priority, reason, round mode and debounce from its triggers as set out in assumption A9. (brief §7; assumption A9)

### Requirement 25: Queue order
**User Story:** As David, I want my requests to jump ahead of bot chatter in a bot's queue, so that I'm answered first.

#### Acceptance Criteria
25.1. The engine SHALL keep a wake queue per bot. (brief §3)
25.2. WHEN a bot has a free concurrency slot and more than one dispatchable queued wake THEN the engine SHALL start them in priority order `Owner`, then `Human`, then `Bot`, first-in-first-out within a priority. (brief §7)

### Requirement 26: Concurrent wakes per bot
**User Story:** As the operator, I want each bot's running wakes capped, so that an agent never runs more copies than it can handle.

#### Acceptance Criteria
26.1. WHILE a bot has `max_concurrent` wakes running, the engine SHALL keep its further wakes queued. (brief §4.2, §7)
26.2. WHEN a running wake ends and the bot has a dispatchable queued wake THEN the engine SHALL start the next one in queue order (Requirement 25). (brief §7)

### Requirement 27: Wake deadline
**User Story:** As David, I want every wake killed at a fixed deadline, so that a hung agent can't hold a bot forever.

#### Acceptance Criteria
27.1. WHEN the engine dispatches a wake THEN it SHALL set the wake's deadline to the dispatch time + the bot's `max_wake_minutes`. (brief §7, §9.1)
27.2. WHEN a running wake reaches its deadline THEN the engine SHALL kill a command adapter's whole process tree, revoke the wake's token, end the wake as `timeout`, and have the bot react ⌛. (brief §7, §9.1)
27.3. WHEN an agent calls `POST /v1/post` after its wake's deadline THEN the API SHALL return 410 and publish nothing (42.4). (brief §9.5, §15.2)

### Requirement 28: Posts per wake
**User Story:** As David, I want a cap on how many messages one wake can post, so that one run can't flood a thread.

#### Acceptance Criteria
28.1. WHEN an agent calls `POST /v1/post` after its wake has already published `max_posts_per_wake` replies THEN the API SHALL return 429 and publish nothing. With the default limit, the 4th post SHALL return 429. (brief §7, §9.5, §15.2)
28.2. A 429 response SHALL NOT end the wake. (brief §7)

### Requirement 29: Control command parsing
**User Story:** As David, I want a short "stop" message to be recognised reliably and a resume to need one exact word, so that halting errs towards firing and resuming never happens by accident.

#### Acceptance Criteria
29.1. `route` SHALL test every owner kind-9 message for stop, cancel and resume before any routing rule. WHEN one matches THEN it SHALL return that command in `control`, with no `Wake` decision and no new round. (brief §6.3, §8)
29.2. To test for a command, `route` SHALL normalise the content: (1) remove code regions; (2) remove `@everyone`, every roster `@name` and alias, and every `nostr:` URI; (3) lowercase, and turn every character that is neither alphanumeric nor `!` into a space; (4) split into words. (brief §8)
29.3. WHEN the normalised text has 1 to 5 words and one of them is `stop`, `halt` or `!shutdown` THEN `route` SHALL return `Stop`. So "stop", "stop it", "fucking stop[", "@everyone stop" and "please stop now" all halt. (brief §2, §8)
29.4. WHEN the only normalised word is `!cancel` THEN `route` SHALL return `Cancel`. (brief §2, §8)
29.5. WHEN the only normalised word is `resume` THEN `route` SHALL return `Resume`. (brief §8)
29.6. `route` SHALL set a command's scope to the bots the message mentions in any form (Requirement 17). WHEN no bot is mentioned THEN the scope SHALL be every bot, including when `@everyone` is present. (brief §8)
29.7. `route` SHALL parse control commands only from owner kind-9 messages: never from edits, or from bot, human or foreign-bot messages. (brief §6.5, §6.7, §8)
29.8. The operator documentation SHALL state that a 1–5-word owner message containing "stop" halts bots, for example "stop the dev server", and SHALL advise phrasing such requests as "shut down the dev server". (brief §8)
29.9. CONFORM: test case 17. WHEN O posts "@everyone stop" THEN `route` SHALL return: Control Stop(all). (brief §15.1)
29.10. CONFORM: test case 18. WHEN O posts "fucking stop[" THEN `route` SHALL return: Control Stop(all). (brief §15.1)
29.11. CONFORM: test case 19. WHEN O posts "@A stop" THEN `route` SHALL return: Control Stop(A). (brief §15.1)
29.12. CONFORM: test case 20. WHEN O posts "please stop the dev server and restart it" (8 words) THEN `route` SHALL return: not a stop; normal routing → nobody. (brief §15.1)

### Requirement 30: Stop effects
**User Story:** As David, I want a stop to kill running agents, empty their queues, block their posts and survive restarts until I resume, so that a stop is a real safety switch.

#### Acceptance Criteria
30.1. WHEN `route` returns `Stop` THEN each router that processed the message SHALL, for each in-scope local bot: persist the halt in SQLite; kill its running wakes (command adapter: the whole process tree; webhook adapter: revoke the token and call `cancel_url` if set); drop its queued wakes; and have the bot react 🛑 on the stop message. (brief §8; assumption A5)
30.2. The router SHALL persist halts as one row per scope in the `halts` table: `'all'` for a stop without named bots, and the bot name for each named bot. Halts SHALL survive restarts. (brief §8, §11)
30.3. The router SHALL treat a bot as halted while a halt row exists for `'all'` or for its name, and SHALL clear halts as set out in assumption A4. (brief §8, §11; assumption A4)
30.4. WHILE a bot is halted, `route` SHALL return no `Wake` for it. Where it would otherwise be a target, `route` SHALL return `Suppress(Halted)`, except that `Suppress(RespondTo)` comes first for human and foreign-bot events (14.2, 15.2). (brief §6.3, §6.4, §6.5, §8)
30.5. WHILE a bot is halted, `POST /v1/post` with any of its wake tokens SHALL return 423, and the router SHALL publish no reply or status note for that bot. (brief §8, §9.5, §15.2)
30.6. CONFORM: test case 23. WHILE halts = all, WHEN O posts "@A hi" THEN `route` SHALL return: Suppress A (Halted). (brief §15.1)

### Requirement 31: Resume
**User Story:** As David, I want "resume" to lift a stop with a visible confirmation, so that I know bots are back.

#### Acceptance Criteria
31.1. WHEN `route` returns `Resume` THEN each router that processed the message SHALL clear the halt for each in-scope local bot (assumption A4), and each such bot SHALL react ▶️ on the resume message. (brief §8; assumption A4)
31.2. WHEN a bot's halt is cleared THEN `route` SHALL route events for it normally. (brief §8)
31.3. CONFORM: test case 22. WHILE halts = all, WHEN O posts "resume" THEN `route` SHALL return: Control Resume(all). (brief §15.1)

### Requirement 32: Cancel
**User Story:** As David, I want `!cancel` to end what a bot is doing without halting it, so that I can redirect it straight away.

#### Acceptance Criteria
32.1. WHEN `route` returns `Cancel` THEN each router that processed the message SHALL, for each in-scope local bot, kill its running wakes and drop its queued wakes (stop effects 2 and 3 only), and each such bot SHALL react 🛑 on the cancel message. (brief §8)
32.2. Cancel SHALL NOT halt a bot: later events SHALL wake it normally. (brief §8)
32.3. CONFORM: test case 21. WHEN O posts "@A !cancel" THEN `route` SHALL return: Control Cancel(A). (brief §15.1)

### Requirement 33: Local stop, resume and cancel
**User Story:** As the operator, I want to stop, resume or cancel bots from the machine itself, with stop working even when the relay or the daemon is unreachable, so that I can always halt a runaway agent.

#### Acceptance Criteria
33.1. WHEN the operator runs `buzz-router stop`, `resume` or `cancel` without `--bot` THEN the CLI SHALL apply the command to every local bot through the admin API (Requirement 43). WHEN one or more `--bot NAME` options are given THEN it SHALL apply the command to those bots only. (brief §8, §13)
33.2. A command received through the admin API SHALL have the same effects as the matching Buzz-message command (Requirements 30–32), except that there is no message for bots to react to. (brief §8, §9.5)
33.3. WHILE the relay is down, `buzz-router stop` SHALL still halt the in-scope bots and kill their running wakes. (brief §8)
33.4. WHEN the operator runs `buzz-router stop` and the admin API is unreachable THEN the CLI SHALL itself write the halt rows to SQLite and kill the in-scope bots' running command-adapter process trees. (brief §8)

### Requirement 34: Wake lifecycle states
**User Story:** As the operator, I want every wake to have a recorded state, so that I can see what each bot is doing and how each wake ended.

#### Acceptance Criteria
34.1. The router SHALL give every wake exactly one state from: `queued`, `running`, `posted`, `passed`, `timeout`, `killed`, `failed`, `interrupted`. (brief §9.1)
34.2. The engine SHALL create each wake as `queued`, move it to `running` at dispatch, and end it in exactly one of the six terminal states (Requirement 36). (brief §9.1)
34.3. The router SHALL persist each wake's state and outcome in the `wakes` table (Requirement 47). (brief §11)

### Requirement 35: Dispatch
**User Story:** As David, I want a dispatched wake to signal immediately that the bot has my message, so that I'm never left wondering whether it was seen.

#### Acceptance Criteria
35.1. WHEN the engine dispatches a wake THEN it SHALL perform these steps in order: create the wake id and token (35.2), set the deadline (27.1), increment `turns_used` (35.3), react 👀 if an owner message is among the triggers (35.4), start the typing indicator (35.5), and run the adapter (35.6). (brief §9.1)
35.2. The engine SHALL create a `wake_id` (a UUID) and a token (32 cryptographically random bytes, hex-encoded), and SHALL store only a hash of the token, never the token itself. (brief §9.1)
35.3. The engine SHALL increment the bot's `turns_used` for the thread's current round. (brief §9.1)
35.4. WHEN an owner message is among the wake's triggers THEN the bot SHALL react 👀 on the wake's reaction target. (brief §9.1, §10; assumption A8)
35.5. The engine SHALL publish a typing indicator (kind 20002, built as `buzz-acp` builds it in `crates/buzz-acp/src/relay.rs`) in the wake's thread, and SHALL re-send it every 3 seconds, `buzz-acp`'s cadence (`crates/buzz-acp/src/lib.rs:2879` at the pinned commit), until the wake ends. (brief §2, §9.1, §10)
35.6. The engine SHALL run the bot's configured adapter (Requirements 37–39). (brief §9.1)

### Requirement 36: Wake endings and visible outcomes
**User Story:** As David, I want every wake I cause to end with something I can see, so that nothing I address to a bot ends silently.

#### Acceptance Criteria
36.1. WHEN a wake publishes a reply (through the API, stdout or a sync webhook response) THEN its outcome SHALL be `posted`, including when the agent later exits non-zero. (brief §9.1; assumption A6)
36.2. WHEN the agent passes (an API pass; stdout empty or exactly `[no-reply]`; a sync `{"pass": true}`; or, in API reply mode, exiting 0 without posting or passing) THEN the wake SHALL end as `passed`. (brief §9.1, §9.2, §9.3; assumption A6)
36.3. WHEN a wake ends as `passed` and it was owner-caused and Direct THEN the bot SHALL react ✅ on the wake's reaction target. (brief §9.1; assumptions A7, A8)
36.4. WHEN stop or cancel kills a running wake THEN the wake SHALL end as `killed`, and the bot SHALL react 🛑 on the wake's reaction target. (brief §9.1; assumption A8)
36.5. WHEN the adapter crashes, or a command adapter exits non-zero without having posted, THEN the wake SHALL end as `failed`, and the bot SHALL react ⚠️ on the wake's reaction target. (brief §9.1; assumptions A6, A8)
36.6. IF an async webhook doesn't answer 2xx within 10 s, or a sync webhook answers non-2xx or with a body that is neither `{"text": ...}` nor `{"pass": true}`, THEN the wake SHALL end as `failed` with ⚠️. (brief §9.1, §9.3; assumption A13)
36.7. WHEN the router crashes during a wake THEN the next start SHALL mark the wake `interrupted` (Requirement 49). (brief §9.1, §11)
36.8. Every owner-caused Direct wake SHALL end with a reply or with one of ✅, ⌛, 🛑 or ⚠️, and every owner-caused wake SHALL show 👀 from dispatch. (brief §9.1, §10; assumption A7)

### Requirement 37: Command adapter
**User Story:** As the operator, I want local agents started as killable process groups with exactly the context they need and no signing key, so that stop and limits can't be bypassed.

#### Acceptance Criteria
37.1. The engine SHALL spawn a command adapter with the `command-group` crate, as a process group on Unix and a Job Object on Windows, so that killing the group kills every child the agent started. (brief §9.2)
37.2. The engine SHALL run `command` in `cwd` with these environment variables: `BUZZ_ROUTER_URL` (the loopback API base), `BUZZ_ROUTER_WAKE_TOKEN`, `BUZZ_ROUTER_PAYLOAD` (the path of the payload JSON file, Requirement 40), `BUZZ_ROUTER_PROMPT_FILE` (only when `prompt_mode = "file"`), plus the configured `env`. (brief §4.2, §9.2)
37.3. The child's environment SHALL NOT contain `BUZZ_PRIVATE_KEY`, even if the router's own environment or the configured `env` sets it. (brief §3, §9.2)
37.4. The engine SHALL render the prompt from `prompt_template` when it's set, and from the built-in template otherwise (Requirement 41). (brief §4.2, §9.4)
37.5. WHERE `prompt_mode = "stdin"`, the engine SHALL write the rendered prompt to the child's stdin and then close stdin. WHERE `prompt_mode = "file"`, it SHALL write the prompt to a file and pass its path in `BUZZ_ROUTER_PROMPT_FILE`. (brief §4.2, §9.2)
37.6. WHERE `reply_mode = "stdout"`, the engine SHALL capture at most 64 KiB of stdout and trim it. (brief §9.2)
37.7. WHERE `reply_mode = "stdout"`, WHEN the process exits 0 THEN the engine SHALL end the wake as `passed` if the trimmed output is empty or exactly `[no-reply]`, and otherwise SHALL post the output as the reply. (brief §9.2)
37.8. WHERE `reply_mode = "stdout"`, WHEN the process exits non-zero THEN the engine SHALL NOT post its output, and the wake SHALL end as `failed` unless it already published a reply through the API (36.1). (brief §9.1, §9.2; assumption A6)
37.9. WHERE `reply_mode = "api"`, the agent SHALL post or pass through `buzz-router post` or `pass`. WHEN it exits 0 without either THEN the wake SHALL end as `passed`. (brief §9.2; assumption A6)
37.10. The engine SHALL write the child's stderr to the log. (brief §9.2)

### Requirement 38: Webhook adapter, async mode
**User Story:** As the operator of a cloud routine, I want an authenticated HTTP wake and a tailnet callback, so that remote agents work exactly like local ones.

#### Acceptance Criteria
38.1. WHEN the engine runs a webhook adapter THEN it SHALL `POST` the payload JSON to `url` with the headers `X-Buzz-Router-Signature: sha256=<hex HMAC-SHA256 of the body>`, keyed with the secret read from the environment variable named by `secret_env`, and `X-Buzz-Router-Wake: <wake_id>`. (brief §4.2, §9.3)
38.2. WHERE `mode = "async"`, the engine SHALL expect a 2xx response within 10 s, after which the wake stays `running` until the agent calls back or the wake otherwise ends (assumption A6). (brief §9.3; assumption A6)
38.3. WHERE `mode = "async"`, the router SHALL accept the agent's `POST {public_url}/v1/post`, `/v1/pass` and `/v1/eta` calls with `Authorization: Bearer <token>`. (brief §9.3)
38.4. The router SHALL serve those callbacks as plain HTTP on `tailnet_bind`, with no TLS, tunnel or reverse proxy, and SHALL NOT bind to a public interface. (brief §9.3; assumption A3)

### Requirement 39: Webhook adapter, sync mode
**User Story:** As the operator of a simple routine, I want it to return its reply in the HTTP response, so that it needs no callback.

#### Acceptance Criteria
39.1. WHERE `mode = "sync"`, the engine SHALL send the same request as 38.1. (brief §9.3)
39.2. WHEN a sync response arrives before the deadline with body `{"text": "..."}` THEN the router SHALL publish that text as the reply. WHEN the body is `{"pass": true}` THEN the wake SHALL end as `passed`. (brief §9.3)
39.3. WHEN the deadline passes before a sync response arrives THEN the wake SHALL end as `timeout` (27.2). (brief §9.1, §9.3)

### Requirement 40: Wake payload
**User Story:** As an agent, I want one JSON payload describing why I was woken, the thread so far, and how to answer, so that I can respond without calling Buzz.

#### Acceptance Criteria
40.1. The engine SHALL build a payload JSON with the fields `wake_id`, `token`, `bot`, `channel` (`id`, `name`), `thread_root_id`, `reply_parent_id`, `reason`, `round_mode`, `turns_left_after_this`, `turns_per_round`, `deadline`, `triggers`, `context` and `api`. (brief §9.3)
40.2. The engine SHALL write `reason` and `round_mode` in lowercase, as in the brief's example (`"participant"`, `"discussion"`), and `deadline` as a UTC timestamp. (brief §9.3)
40.3. `turns_per_round` SHALL be the bot's effective limit, and `turns_left_after_this` SHALL be that limit minus the turns used including this wake. (brief §9.3)
40.4. `triggers` SHALL list the event ids of every trigger coalesced into the wake. (brief §7, §9.3)
40.5. `context` SHALL hold the last 20 messages of the thread in total, oldest first. Each entry SHALL have `id`, `author`, `class`, `created_at`, `text` and `new`, with `new: true` on every message since this bot's last turn in the thread. (brief §9.3)
40.6. `api` SHALL hold `url`, `post` (`/v1/post`), `pass` (`/v1/pass`) and `eta` (`/v1/eta`). `url` SHALL be the loopback API address for command bots and `public_url` for webhook bots. (brief §9.3)
40.7. `thread_root_id` SHALL be the wake's thread root, and `reply_parent_id` the parent the reply will be threaded under (44.3). (brief §9.3, §9.6)
40.8. For command adapters, the engine SHALL write the payload to a file whose path is in `BUZZ_ROUTER_PAYLOAD`. (brief §9.2, §9.3)

### Requirement 41: Prompt template
**User Story:** As an agent without a custom template, I want a built-in prompt that tells me why I was woken, my turn budget and the thread so far, so that I answer briefly or pass.

#### Acceptance Criteria
41.1. WHERE `prompt_template` is empty, the engine SHALL use this built-in template, exactly:

    You are {bot} in the Buzz channel #{channel}.
    You were woken because: {reason_text}.
    This is turn {turn} of {turns_per_round} for you in this thread until David posts again.

    Thread so far (oldest first, ★ = new since your last turn):
    {context}

    Write the single message you want to post in this thread.
    If you have nothing useful to add, output exactly: [no-reply]
    Do not post acknowledgements ("got it", "on it", "agreed", "standing by").
    David already sees a 👀 reaction when you are woken.

(brief §9.4)

41.2. The engine SHALL support exactly the template variables `{bot}`, `{channel}`, `{reason_text}`, `{turn}`, `{turns_per_round}` and `{context}`, in the built-in template and in custom templates alike. (brief §4.2, §9.4)
41.3. The engine SHALL fill `{bot}` with the bot's name, `{channel}` with the channel name, `{turn}` with this wake's 1-based turn number in the round, `{turns_per_round}` with the bot's effective limit, and `{context}` with the payload context, oldest first, marking new messages with ★. (brief §9.3, §9.4)
41.4. The engine SHALL fill `{reason_text}` by reason: `Mention` "David mentioned you"; `Everyone` "David asked everyone; this is a discussion"; `ReplyTarget` "David replied to your message"; `Participant` "David replied in a thread you're part of"; `Discussion` "another bot posted in a discussion you're part of"; `BotMention` "{author} mentioned you", with `{author}` replaced by the mentioning bot's name; `DefaultBot` "you're the default bot for this channel". (brief §9.4)

### Requirement 42: Wake-token API
**User Story:** As an agent, I want to post, pass and report an ETA through a token-scoped API, so that I never need a Buzz key and the router can enforce stop and limits on everything I publish.

#### Acceptance Criteria
42.1. The router SHALL serve `POST /v1/post`, `POST /v1/pass` and `POST /v1/eta` on `api_bind` and, when it's set, on `tailnet_bind`. (brief §9.5)
42.2. The router SHALL authorise these endpoints only by the wake token, presented as `Authorization: Bearer <token>`. (brief §9.3, §9.5)
42.3. WHEN `POST /v1/post {text}` is accepted THEN the router SHALL publish the reply (Requirement 44) and return `{event_id}`. (brief §9.5)
42.4. WHEN `POST /v1/post` presents the token of a known wake THEN the router SHALL respond in this order of precedence: 423 if the wake's bot is halted; 410 if the wake has ended, including by deadline, stop or cancel; 429 if the wake has already published `max_posts_per_wake` replies; otherwise it SHALL publish (42.3). A revoked token SHALL still identify its wake for these responses but SHALL authorise nothing. (brief §8, §9.5, §15.2)
42.5. WHEN `POST /v1/pass` is accepted THEN the router SHALL end the wake as `passed`. (brief §9.5)
42.6. WHEN `POST /v1/eta {text}` is accepted THEN the router SHALL keep the text for the wake's status note (Requirement 46). (brief §9.5, §10)
42.7. IF a request carries no token or an unknown token THEN the router SHALL respond as set out in assumption A13. (brief §9.5; assumption A13)

### Requirement 43: Admin API and admin token
**User Story:** As the operator, I want local-only admin endpoints protected by a user-private token, so that the CLI can control the daemon and nothing on the tailnet can.

#### Acceptance Criteria
43.1. The router SHALL serve `GET /v1/status`, returning the same data as `buzz-router status --json`, and `POST /v1/stop`, `/v1/resume` and `/v1/cancel` with an optional `{bots}` list, where an omitted list means every local bot (Requirement 33). (brief §8, §9.5, §13)
43.2. The router SHALL authorise admin endpoints only by the admin token. (brief §9.5)
43.3. The router SHALL serve admin endpoints on the loopback bind only, and SHALL return 404 for them on the tailnet bind. (brief §9.5)
43.4. WHEN the router starts and `admin.token` doesn't exist in the data directory THEN it SHALL create it with random content, readable only by the user (mode 0600 on Unix; the default user-profile ACL on Windows). (brief §9.5)
43.5. IF an admin request carries no token or a wrong token THEN the router SHALL respond as set out in assumption A13. (brief §9.5; assumption A13)

### Requirement 44: Publishing a reply
**User Story:** As David, I want bot replies threaded where I expect them, with working mentions and a router marker, so that the Buzz client shows the conversation correctly.

#### Acceptance Criteria
44.1. The router SHALL publish each reply as a kind-9 event built with `buzz_sdk::builders::build_message` and signed with the bot's key. (brief §3, §9.6)
44.2. The reply's `h` tag SHALL be the wake's channel. (brief §2, §9.6)
44.3. The reply's thread root SHALL be the wake's thread root, and its parent SHALL be the latest owner trigger, else the latest trigger (for an edit trigger, the edited message, 16.7). WHEN the parent is the root THEN the reply SHALL carry the single tag `["e", <root>, "", "reply"]`. Otherwise it SHALL carry `["e", <root>, "", "root"]` and `["e", <parent>, "", "reply"]`. (brief §2, §9.6)
44.4. The reply SHALL carry a `p` tag for each roster bot or owner that its text @mentions, resolved with the parser of Requirement 17, so that mentions render. In addition, WHEN the reply text names the owner as a bare whole word (without `@`), in any ASCII case, THEN the reply SHALL carry a `p` tag for every owner pubkey (owner decision O3: the T1.4 interim reading of finding F6, recorded as the spec). (brief §9.6)
44.5. WHERE the bot has an `auth_tag`, the reply SHALL carry it. (brief §9.6)
44.6. The reply SHALL carry the tag `["buzz-router", "<version>", "reply"]`. (brief §9.6)
44.7. The router SHALL record every kind-9 event it publishes, replies and status notes alike, in `posts`, so that its own events are never counted as unmanaged, even when the relay echoes one before publishing completes. (brief §9.6, §10, §12)
44.8. The router SHALL publish replies only into the wake's thread, and SHALL offer no way to post top-level or into another channel. (brief §1, §9.6)

### Requirement 45: Reaction confirmations
**User Story:** As David, I want confirmations as reactions rather than chat messages, so that I know what happened without noise in the thread.

#### Acceptance Criteria
45.1. The router SHALL confirm with reactions rather than chat messages. The only acknowledgement message it publishes SHALL be the status note (Requirement 46). (brief §0, §10)
45.2. Each reaction SHALL be a kind-7 event built with `buzz_sdk::builders::build_reaction`, with the emoji as content and an `e` tag naming its target, signed by the bot's key. (brief §2, §10)
45.3. The router SHALL place wake-related reactions (👀, ✅, ⌛, 🛑, ⚠️) on the wake's reaction target, as set out in assumption A8. ⏸️ goes on the event that would have woken the bot (19.3). 🛑 and ▶️ for control commands go on the command message (30.1, 31.1, 32.1). (brief §7, §8, §9.1, §10; assumption A8)

### Requirement 46: Status note
**User Story:** As David, I want one short "on it" note when a direct request to one bot is taking a while, so that I know it isn't stuck, without that note appearing on every small thing.

#### Acceptance Criteria
46.1. WHEN a running wake is owner-caused and Direct, and the agent has neither posted nor passed `status_note_after_secs` after dispatch, THEN the bot SHALL post one status note. (brief §10)
46.2. The note's text SHALL be `On it, this will take a bit.`, or `On it, about {eta}.` with the text from `/v1/eta` if the agent called it first. (brief §10)
46.3. The note SHALL be threaded under the wake's trigger (its reaction target, assumption A8) and tagged `["buzz-router", "<version>", "status"]`, so that it wakes nobody (4.5) and doesn't count as a turn. (brief §10; assumption A8)
46.4. The router SHALL send at most one status note per wake. (brief §10)
46.5. The router SHALL NOT send a status note for `@everyone` or Discussion wakes. (brief §10)
46.6. WHEN the defaults apply THEN the note SHALL be sent at 20 s exactly once for a Direct owner wake, never for a Discussion wake, and not at all if the agent posted at 19 s. (brief §15.2)

### Requirement 47: SQLite state
**User Story:** As the operator, I want all durable state in one local SQLite file, so that restarts and crashes lose nothing.

#### Acceptance Criteria
47.1. The router SHALL store its state in one SQLite file in the data directory, through `rusqlite` with the `bundled` feature. (brief §11, §14)
47.2. The database SHALL hold these tables:
`events (id PK, channel_id, root_id, author, class, kind, created_at, processed_at)`;
`cursors (bot, relay_url, last_created_at, PK(bot, relay_url))`;
`threads (root_id PK, channel_id, participants JSON, discussion INT, round_id, round_mode, round_started_at)`;
`turns (root_id, round_id, bot, used INT, cap_reacted INT, PK(root_id, round_id, bot))`;
`wakes (id PK, bot, root_id, round_id, reason, priority, triggers JSON, state, token_hash, attempt INT, created_at, dispatch_after, started_at, deadline, ended_at, outcome)`;
`posts (event_id PK, bot, wake_id, created_at)`;
`halts (scope PK, set_by_event, set_at)`, where scope is `'all'` or a bot name. (brief §11)
47.3. The router SHALL record each received event in `events` and route each event id at most once. (brief §3, §11)
47.4. The router SHALL advance each (bot, relay) cursor as it processes that connection's events. (brief §11)

### Requirement 48: Startup and backfill
**User Story:** As David, I want messages I send while a router is down answered when it comes back, so that a restart never loses a request.

#### Acceptance Criteria
48.1. WHEN the router starts THEN it SHALL, in order: load halts; connect each bot; backfill from `cursor − 300 s` by paging `POST /query` until caught up; deduplicate by event id; and route the backfilled events normally. (brief §11)
48.2. The router SHALL NOT backfill with a fixed "last N" window. (brief §11)
48.3. WHEN backfill routes an owner message older than 24 hours THEN the router SHALL NOT wake any bot for it, and SHALL list it in `status` as `missed`. (brief §11)
48.4. WHEN a (bot, relay) pair has no stored cursor THEN the router SHALL behave as set out in assumption A14. (brief §11; assumption A14)

### Requirement 49: Interrupted wakes
**User Story:** As David, I want a request that was in progress when a router crashed re-run once, so that it's answered exactly once.

#### Acceptance Criteria
49.1. WHEN the router starts THEN it SHALL mark every wake still in state `running` as `interrupted`. (brief §11)
49.2. WHEN an interrupted wake's triggers include an owner message, the bot hasn't posted in the thread since the wake's `started_at`, and the wake is on its first attempt THEN the router SHALL re-queue it once, with `attempt = 2`. (brief §11)
49.3. WHEN an interrupted wake isn't re-queued THEN its bot SHALL react ⚠️ on the wake's reaction target. (brief §11; assumption A8)

### Requirement 50: Relay reconnect
**User Story:** As the operator, I want dropped relay connections to recover on their own without losing messages, so that no one has to babysit the router.

#### Acceptance Criteria
50.1. WHEN a relay connection drops THEN the router SHALL reconnect with exponential backoff and jitter, as `buzz-acp` does, and then backfill from the cursor (48.1 steps 3–5). (brief §11)

### Requirement 51: Unmanaged-post detection
**User Story:** As David, I want to be told when something other than the router posts as one of my bots, so that I can find and remove an old listener, routine or stray CLI call.

#### Acceptance Criteria
51.1. WHEN a kind-9 event signed by a local bot's key arrives and its id isn't in `posts` THEN that bot SHALL react ⚠️ on the event, and `status` SHALL count it under `unmanaged_posts`. (brief §12, §15.2)
51.2. WHEN unmanaged posts are detected THEN the router SHALL log at most one warning per bot per hour. (brief §12)

### Requirement 52: `run` and `status`
**User Story:** As the operator, I want one daemon command and one status command, so that I can run the router and see its whole state at a glance.

#### Acceptance Criteria
52.1. WHEN the operator or the service runs `buzz-router run` THEN the router SHALL run as the daemon: connect the local bots, route events, run wakes and serve the APIs. (brief §13, §14)
52.2. WHEN the operator runs `buzz-router status` THEN the CLI SHALL show the bots, halts, running and queued wakes, budgets, `missed` messages, `unmanaged_posts`, the roster hash and the version. (brief §13)
52.3. WHEN the operator runs `buzz-router status --json` THEN the CLI SHALL print the same data as JSON, matching `GET /v1/status`. (brief §9.5, §13)

### Requirement 53: Agent CLI commands
**User Story:** As an agent in API reply mode, I want simple CLI commands to post, pass and report an ETA, so that I never handle tokens or HTTP myself.

#### Acceptance Criteria
53.1. WHEN an agent runs `buzz-router post --text T`, `post --text-file P`, or `post --text-file -` (stdin) THEN the CLI SHALL read `BUZZ_ROUTER_WAKE_TOKEN` and `BUZZ_ROUTER_URL` and call `POST /v1/post` with that text. (brief §13)
53.2. WHEN an agent runs `buzz-router pass` THEN the CLI SHALL call `POST /v1/pass` the same way. (brief §13)
53.3. WHEN an agent runs `buzz-router eta --text T` THEN the CLI SHALL call `POST /v1/eta` the same way. (brief §13)
53.4. WHEN the operator runs `buzz-router wakes [--bot N] [--state S]` THEN the CLI SHALL list wakes, filtered by bot and state when given. (brief §13)

### Requirement 54: Key CLI commands
**User Story:** As the operator, I want to load and check bot keys from the CLI, so that keys go straight into the OS keychain and never into agent environments.

#### Acceptance Criteria
54.1. WHEN the operator runs `buzz-router keys set --bot N` THEN the CLI SHALL read an nsec from stdin and store it in the OS keychain as `buzz-router/<N>`. (brief §4.2, §13, §14)
54.2. `keys set` SHALL work before `roster.toml` and `router.toml` are installed. (brief §16 steps 3–5)
54.3. IF stdin doesn't hold a valid nsec THEN `keys set` SHALL print a JSON error on stderr, store nothing, and exit with code 1. (brief §13)
54.4. WHEN the operator runs `buzz-router keys check` THEN the CLI SHALL check the keys as set out in assumption A16. (brief §13; assumption A16)

### Requirement 55: Capture and offline replay
**User Story:** As the builder, I want to capture real channel traffic and replay it offline through the router, so that I can tune and debug routing without touching live bots.

#### Acceptance Criteria
55.1. WHEN the operator runs `buzz-router capture --channel ID --since <duration>` (for example `7d`) THEN the CLI SHALL write the channel's real events since then to stdout as JSON Lines. (brief §13)
55.2. WHEN the operator runs `buzz-router route --replay FILE [--roster P]` THEN the CLI SHALL pass every event in FILE through `route` with a simulated clock, print every decision, use no network, publish nothing and dispatch no wake. (brief §13)
55.3. `route --replay` SHALL accept the file `capture` writes. (brief §13)
55.4. `route --replay` SHALL use the roster at P when given, otherwise the configured roster, and SHALL choose local bots as set out in assumption A17. (brief §13; assumption A17)

### Requirement 56: Service installation
**User Story:** As the operator, I want the router installed as a per-user service on each OS, so that it starts at login and restarts if it dies.

#### Acceptance Criteria
56.1. WHEN the operator runs `buzz-router service install` THEN the CLI SHALL write and load the OS's service definition, which runs `buzz-router run` as the user. (brief §13, §14)
56.2. WHEN `service install` runs on macOS THEN it SHALL install the user LaunchAgent `~/Library/LaunchAgents/com.buzz-router.plist` with `KeepAlive`. (brief §14)
56.3. WHEN `service install` runs on Linux THEN it SHALL install a `systemd --user` unit with `Restart=always`. (brief §14)
56.4. WHEN `service install` runs on Windows THEN it SHALL create a Task Scheduler task (`schtasks`) that runs at logon as the user, so that the Credential Manager works, and restarts on failure. (brief §14)
56.5. WHEN the operator runs `buzz-router service uninstall` THEN the CLI SHALL unload and remove the service definition. WHEN the operator runs `buzz-router service status` THEN the CLI SHALL report whether it's installed and running. (brief §13)

### Requirement 57: CLI errors and exit codes
**User Story:** As the operator or an agent script, I want machine-readable errors and stable exit codes, so that I can script against the CLI.

#### Acceptance Criteria
57.1. The CLI SHALL print errors as JSON on stderr, as the `buzz` CLI does. (brief §13)
57.2. The CLI SHALL exit with 0 on success, 1 on bad input, 2 on a relay or network error, 3 on an authentication error, and 4 on any other error. (brief §13)

### Requirement 58: Build targets and toolchain
**User Story:** As David, I want the same single binary for every machine I run, so that one build serves macOS, Linux and Windows.

#### Acceptance Criteria
58.1. The project SHALL build one `buzz-router` binary per target for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`. (brief §0, §14)
58.2. The binary SHALL use `rustls` for all TLS, SHALL NOT link OpenSSL, and SHALL use `rusqlite` with the `bundled` feature. (brief §14)
58.3. The project SHALL build with Rust 1.88 or later (Buzz's `rust-version`). (brief §14)

### Requirement 59: Signing keys
**User Story:** As David, I want only the router to hold the bots' signing keys, in the OS keychain, so that agents can only post through the router and stop and limits can't be bypassed.

#### Acceptance Criteria
59.1. The router SHALL store and read keys with the `keyring` crate: macOS Keychain, Windows Credential Manager and Linux Secret Service, under the entry `buzz-router/<name>`. (brief §4.2, §14)
59.2. WHERE `key = "file:<path>"`, the router SHALL read the key from that file, which SHALL have 0600 permissions (the fallback when Secret Service is unavailable on Linux). (brief §14; assumption A3)
59.3. The router SHALL sign every event it publishes for a bot (replies, status notes, reactions and typing indicators) with that bot's key. (brief §3, §9.6)
59.4. The router SHALL NOT give a bot's signing key to any agent, whether by environment, payload, prompt or API response. (brief §3, §9.2)

### Requirement 60: CI and release
**User Story:** As the builder, I want every change built and tested on all three OSes and releases published automatically, so that the binary is known to work everywhere it runs.

#### Acceptance Criteria
60.1. The workflow `.github/workflows/ci.yml` SHALL build the project and run all tests on `macos-latest`, `ubuntu-latest` and `windows-latest`. (brief §3, §14)
60.2. The workflow `.github/workflows/release.yml` SHALL attach the five target binaries (58.1) to a GitHub release. (brief §3, §14)

### Requirement 61: Relay connections
**User Story:** As David, I want each router to listen as each of its bots, so that it sees exactly the channels each bot can see.

#### Acceptance Criteria
61.1. The router SHALL hold one WebSocket connection to `relay_url` per local bot, authenticated as that bot with NIP-42 (standalone, or owner-attested when `auth_tag` is set), following `examples/countdown-bot/src/main.rs` in Buzz. (brief §2, §3)
61.2. The router SHALL discover each bot's channels as `buzz-acp` does: kind 39002 events with `#p` = the bot's pubkey, then kind 39000 events for channel names. (brief §2)
61.3. The router SHALL receive kind-9 messages and kind-40003 edits from each bot's channels, and SHALL ignore forum posts, DMs, canvases and workflows. (brief §1, §3)
61.4. WHEN the same event arrives on more than one connection THEN the router SHALL process it once, by event id. (brief §3, §11)
61.5. The router SHALL use `POST /query` and `POST /events` over HTTP with NIP-98 authentication, following `buzz-acp`'s `RestClient`, for backfill and thread fetches. (brief §2, §11)

### Requirement 62: Scope and Buzz dependencies
**User Story:** As David, I want the router built on Buzz's own crates and kept to the agreed scope, so that it behaves like Buzz and stays small.

#### Acceptance Criteria
62.1. The router SHALL NOT call any model or LLM. (brief §0, §1)
62.2. The router SHALL NOT offer a shadow or dry-run mode against live traffic. Offline replay of captured events (Requirement 55) is in scope. (brief §0, §1, §13)
62.3. The project SHALL depend on `buzz-core` and `buzz-sdk` as git dependencies on `https://github.com/block/buzz` pinned to rev `f0eb5575ffc9d5f57af4ed3f574529d997c83a0d`, and on `nostr = "0.44"`, and SHALL NOT modify Buzz. (brief §1, §2)
62.4. The router SHALL resolve thread structure with `buzz_core::nip10::parse_thread_markers` and `ThreadMarkers::resolve()`. A `root` and a `reply` marker give (root, parent). A `reply` marker alone gives a direct reply to the root (parent = root). No `reply` marker means a top-level message that is its own root. (brief §2)
62.5. The router SHALL use `buzz_core::kind::*` for kind constants, `buzz_sdk::builders` to build messages and reactions, `buzz_sdk::mentions` for mention parsing, and `buzz_sdk::nip_oa` to verify owner-attestation tags. (brief §2)
62.6. One `buzz-router` process per machine SHALL serve every local bot on that machine. (brief §0, §3)

### Requirement 63: Cutover runbook
**User Story:** As the operator, I want a written per-machine cutover procedure, so that the router becomes the only thing that wakes or posts as each bot, with no shadow period.

#### Acceptance Criteria
63.1. The operator documentation SHALL include a cutover runbook that is executed per machine and cuts over directly, with no shadow mode. (brief §0, §16)
63.2. The runbook SHALL direct the operator to inventory every existing Buzz trigger for each local bot, including listeners, pollers (`bin/buzzlib.py`, `buzz_mentions_monitor.py`, the Zo router), routines, cron jobs and scheduled tasks, and to write the list down. (brief §16 step 1)
63.3. The runbook SHALL direct the operator to stop and remove every inventoried trigger, and SHALL warn that deleting a routine isn't enough while a poller still exists. (brief §16 step 2)
63.4. The runbook SHALL direct the operator to move each bot's key into the router with `buzz-router keys set --bot N`, and then to remove `BUZZ_PRIVATE_KEY` and any nsec from the agent's environment, config and scripts, explaining that otherwise stop and turn limits can be bypassed. (brief §3, §16 step 3)
63.5. The runbook SHALL direct the operator to install the shared `roster.toml` and confirm that `roster check` shows the same hash on every machine. (brief §16 step 4)
63.6. The runbook SHALL direct the operator to write `router.toml` and then run `buzz-router service install`. (brief §16 step 5)
63.7. The runbook SHALL define the smoke test: David posts "@<bot> ping" for each bot and expects 👀 and a reply, then posts "stop" and expects 🛑 from every bot, then posts "resume". (brief §16 step 6)
63.8. The runbook SHALL direct the operator to watch `status` for 24 hours, and SHALL state that any `unmanaged_posts` means step 2 missed a trigger. (brief §16 step 7)
63.9. The runbook SHALL state that every bot is served by exactly one router: a `command` bot by the router on its agent's machine, a `webhook` bot by one chosen router, and never the same bot on two routers, because both would wake it and both would answer. (brief §3)
63.10. The runbook SHALL advise Tailscale ACLs that allow the router port only from the hosts that run agents. (brief §9.5)
63.11. The runbook SHALL describe tuning: edit `[limits]` (or per-bot `limits`) in the roster, redistribute it to every machine, and restart each router (assumption A15). (brief §16, §18; assumption A15)

### Requirement 64: Routing conformance suite
**User Story:** As the builder, I want every routing rule pinned by a fixture, so that `route` provably matches this specification on every OS.

#### Acceptance Criteria
64.1. `fixtures/conformance/` SHALL hold one JSON fixture per brief §15.1 case, 1 to 37. Each fixture SHALL encode the setup, event and expected result of the CONFORM criterion with the same case number in this document. (brief §3, §15.1)
64.2. A `router-core` test SHALL run every fixture through `route` and assert the expected result, and SHALL pass on macOS, Linux and Windows CI. (brief §15.1, §17)
64.3. A routing rule that no brief §15.1 case exercises SHALL get its own additional fixture. (brief §6; AGENTS.md)

### Requirement 65: Engine tests
**User Story:** As the builder, I want the wake engine tested with a fake clock and fake adapters, so that timing, coalescing, stop and recovery are proven without a relay.

#### Acceptance Criteria
65.1. The engine tests SHALL use a fake clock and fake adapters, and SHALL pass on macOS, Linux and Windows CI. (brief §15.2)
65.2. Debounce: three bot posts 5 s apart SHALL produce one wake per other participant, dispatched 20 s after the last post, and a steady stream SHALL dispatch at 90 s. (brief §15.2)
65.3. Coalescing: a trigger arriving while a wake runs SHALL produce exactly one follow-up wake. (brief §15.2)
65.4. Stop: stopping a command adapter whose child spawned a grandchild SHALL leave both processes gone on all three OSes in CI, and posts after the stop SHALL return 423. (brief §15.2)
65.5. Deadline: a wake that reaches its deadline SHALL be killed and get ⌛, and posts after it SHALL return 410. (brief §15.2)
65.6. Status note: it SHALL be sent at 20 s exactly once for a Direct owner wake, never for a Discussion wake, and not at all if the agent posted at 19 s. (brief §15.2)
65.7. `max_posts_per_wake`: the 4th post SHALL return 429. (brief §15.2)
65.8. Restart: a `running` wake SHALL become `interrupted` and SHALL be re-queued once when it has owner triggers. (brief §15.2)
65.9. Unmanaged post: an event signed by a bot key but not in `posts` SHALL get a ⚠️ reaction. (brief §15.2)

### Requirement 66: End-to-end acceptance
**User Story:** As David, I want the whole system proven against a real local Buzz relay before cutover, so that the original bug is fixed and stop really works.

#### Acceptance Criteria
66.1. The acceptance tests SHALL run against a local Buzz relay started from the Buzz repo (`just relay` or `docker-compose.yml`), with test identities for one owner and three bots on `command` adapters that run a script which echoes after a configurable delay. They SHALL NOT use the live relay or real bot keys. (brief §15.3; AGENTS.md)
66.2. E1: WHEN O posts "@A", A replies, and O replies untagged in the thread panel THEN A SHALL be woken and reply (the original bug). (brief §15.3)
66.3. E2: WHEN O posts "@everyone" THEN A, B and C SHALL each react 👀 within 5 s, the discussion SHALL run, no bot SHALL exceed 4 wakes, and the thread SHALL go quiet. (brief §15.3)
66.4. E3: WHEN O sends "stop" during E2 THEN every agent process SHALL be gone within 5 s, each bot SHALL react 🛑, nothing SHALL be published afterwards, the halt SHALL survive a router restart, and "resume" SHALL bring ▶️ and normal routing. (brief §15.3)
66.5. E4: WHEN O posts "@A" with a 60 s task THEN 👀 SHALL appear immediately, one status note SHALL appear at about 20 s, and the final reply SHALL be threaded under O's message. (brief §15.3)
66.6. E5: WHEN the router is killed with `kill -9` mid-wake, O posts while it's down, and the router restarts THEN the interrupted wake SHALL be re-run once, the message sent during the downtime SHALL be answered, and there SHALL be no duplicate replies. (brief §15.3)
66.7. E6: WHEN a message is posted with the `buzz` CLI using bot A's key THEN that post SHALL get ⚠️ and `status` SHALL show `unmanaged_posts: 1`. (brief §15.3)
66.8. E7: The same binary and the same tests SHALL pass on macOS, Linux and Windows CI. (brief §15.3)

## Measurable Success Criteria

1. All 37 conformance fixtures pass on macOS, Linux and Windows CI (Requirement 64).
2. Every brief §15.2 engine test passes on macOS, Linux and Windows CI (Requirement 65).
3. Acceptance scenarios E1 to E7 pass against a local Buzz relay (Requirement 66).
4. After cutover, 24 hours of `status` on every machine shows `unmanaged_posts` at 0 (Requirements 51 and 63).

## Open questions

None. Every gap a builder would otherwise hit is resolved by a proposed answer in the assumptions below.

## Assumptions for owner confirmation

Each assumption fills a gap the brief leaves open. Criteria that rely on one cite it by number. Confirm or correct each one before design starts.

- **A1. Quiet-hours boundaries.** Quiet hours are the half-open interval [23:00, 07:00) in owner time (`owner.timezone`): 23:00:00 is quiet and 07:00:00 isn't. Any `quiet_hours` range is read the same way, [start, end), wrapping past midnight when start is later than end.
- **A2. Required and optional configuration keys.** The keys that may be omitted are those the brief marks optional or shows with an empty value (`""`, `[]` or `{}`): `default_bot`, `aliases`, per-bot `limits`, `machine`, `tailnet_bind`, `public_url`, `auth_tag`, `cancel_url`, `env` and `prompt_template`. Omitted `[limits]` keys take the brief §18 defaults. `api_bind` defaults to `127.0.0.1:47821` (brief §9.5), `max_concurrent` to 1 (brief §7), and `roster_path` to `roster.toml`, because cutover runs `roster check` before `router.toml` exists (brief §16). Every other key shown in the brief's examples is required, including the option-valued keys `respond_to`, adapter `type`, `prompt_mode`, `reply_mode` and `mode`. No other default is invented.
- **A3. Validation rules the brief doesn't state.** Configuration is rejected when:
  - `version` isn't 1;
  - two bots share a name, alias or pubkey (names and aliases compared case-insensitively);
  - a bot is named `all`, in any ASCII case (`all` is the halt scope covering every bot; owner decision O2);
  - two channels share an id (owner decision O2);
  - a pubkey is both an owner key and a bot key;
  - `default_bot` doesn't name a roster bot;
  - an async webhook bot is configured but `public_url` or `tailnet_bind` is empty;
  - `api_bind` isn't a loopback address, or `tailnet_bind` is a wildcard address (`0.0.0.0` or `::`), which enforces "never binds to a public interface";
  - `max_concurrent` is 0 (owner decision O2);
  - a command adapter's `command` is empty (owner decision O2);
  - on Unix, a `file:` key is readable or writable by group or others (it must be 0600).
- **A4. Clearing halts that are stored one row per scope.** A bot is halted while a row exists for `'all'` or for its name. Resume(all) deletes every row. Resume naming bots while an `'all'` row exists replaces that row with one row per other roster bot and then deletes the named bots' rows, so only the named bots resume.
- **A5. How far a Buzz stop reaches.** A router acts on a stop, resume or cancel message only if one of its local bots receives it, which requires that bot to be a member of the channel. A bot whose router sees no copy of the message isn't halted. That is consistent with the brief's "a bot whose router is down won't react", but it also applies to routers that have no bot in that channel. The CLI and admin API (Requirement 33) remain the per-machine fallback.
- **A6. When a wake ends.** Posting doesn't end a wake by itself. A wake ends when:
  - a command adapter's process exits, or the agent calls pass;
  - a sync webhook responds;
  - an async webhook agent calls `/v1/post` or `/v1/pass`, so an async agent posts once per wake;
  - the deadline is reached, or a stop or cancel arrives.

  `max_posts_per_wake` therefore binds command adapters in API reply mode. A wake that published a reply ends `posted` even if the process then exits non-zero. "Exiting without either means pass" (brief §9.2) is read as exit 0. A non-zero exit without a post is `failed` (brief §9.1).
- **A7. Passes in owner-caused discussions.** ✅ is reacted only for owner-caused Direct passes, as brief §9.1 says. An owner-caused Discussion wake that passes shows only its 👀. Brief §9.1's "nothing addressed by David may end silently" is therefore read as applying to Direct wakes.
- **A8. Where wake reactions go.** A wake's reaction target is the event its reply would be threaded under: the latest owner trigger, else the latest trigger (brief §9.6). For an owner edit, it's the edited message rather than the kind-40003 event. 👀, ✅, ⌛, 🛑 (killed), ⚠️ and the status note go on or under that event. A wake killed by stop or cancel reacts 🛑 there as well as the 🛑 on the command message.
- **A9. Attributes of a coalesced wake.** A queued wake takes its priority, reason and round mode from its highest-priority trigger, the latest one among equals. If any trigger isn't debounced, the wake is dispatchable immediately.
- **A10. Budget windows.** The hourly and daily counts are trailing 60-minute and 24-hour windows of dispatched wakes, per bot, counting owner-caused wakes too.
- **A11. Wakes caused by edits.** An owner edit's targets are gated by Halted and then Cap, but not by Quiet or Budget, because brief §6.3 exempts owner wakes only from those two and edits don't reset the round. The wake is Direct and Owner priority. The thread's `round_id` and `round_mode` don't change. The targets join `participants`, as owner-mention targets do.
- **A12. Routing details.**
  - A bot is considered only in channels its `channels` setting covers, whatever the reason.
  - Each local bot gets at most one decision per event. A bot that is both mentioned and a discussion participant gets reason `BotMention`.
  - For human authors, as for the owner, mentions beat a reply target.
  - "Log `roster drift` once" means once per foreign pubkey per router process.
- **A13. HTTP responses the brief doesn't name.** A missing or unknown wake token, or a missing or wrong admin token, gets 401. Successful calls return 200 (`/v1/post` with `{event_id}`). `/v1/pass` and `/v1/eta` on an ended wake return 410. A webhook failure, meaning an async call without a 2xx within 10 s, or a sync call answered non-2xx or with an unrecognised body, ends the wake as `failed` with ⚠️.
- **A14. First start without a cursor.** For a (bot, relay) pair with no stored cursor, the router starts from the time it connects and backfills no history. This prevents re-answering old messages, or flagging every historical bot post as unmanaged, on the first run at cutover.
- **A15. Configuration changes.** `roster.toml` and `router.toml` are read at startup. Changes take effect when the router restarts, so v1 has no hot reload. `status` reports the hash of the roster that was loaded.
- **A16. What `keys check` checks.** For each bot in `router.toml`, it checks that the key loads and that its public key matches the bot's roster `pubkey`. It reports each failure and exits non-zero (Requirement 57) if any key fails.
- **A17. Replay and rebuild.** `route --replay` treats every roster bot as local, so it prints decisions for all of them. Rebuilding a thread's state by replaying it (18.2) publishes nothing and counts no unmanaged posts.
- **A18. `nprofile` mentions.** The brief says `extract_nostr_uris` handles `nostr:nprofile1…`. At the pinned commit, `buzz_sdk::mentions::extract_nostr_uris` decodes only `nostr:npub1…` URIs (`crates/buzz-sdk/src/mentions.rs`). To keep the brief's behaviour, `router-core` decodes `nprofile` URIs itself with the `nostr` crate's NIP-19 support. Confirm that `nprofile` mentions are still wanted.
