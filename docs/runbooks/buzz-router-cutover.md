---
id: runbooks/buzz-router-cutover
title: buzz-router cutover runbook
doc-type: runbook
status: draft
component: buzz-router
owner: dpalfery
last-reviewed: 2026-10-07
---

# buzz-router cutover runbook

Per-machine cutover procedure. Execute it on **every machine** that serves
bots, one machine at a time. Cut over directly: there is **no shadow mode**
(R63.1). The router becomes the only thing that wakes or posts as each bot.

Prerequisites: the release binary installed on the machine, the shared
`roster.toml` file available for distribution, and David available to post
the smoke-test messages.

## 1. Inventory every existing trigger (R63.2)

For each local bot, inventory every existing Buzz trigger and **write the
list down**:

- channel listeners and long-lived subscriber processes;
- pollers, including `bin/buzzlib.py`, `buzz_mentions_monitor.py`, and the
  Zo router;
- cloud routines and webhook receivers that act as the bot;
- cron jobs (`crontab -l`, `/etc/cron.*`) and systemd timers;
- macOS `launchd` agents and Windows Scheduled Tasks (`schtasks /query`).

Record each trigger with its host, its mechanism, and the bot it serves.
An unlisted trigger is a trigger that survives cutover.

## 2. Stop and remove every inventoried trigger (R63.3)

Stop and **remove** all of them — disable is not enough where a poller can
be re-enabled by a routine reinstall.

Warning: deleting a routine isn't enough while a poller still exists. That
is what kept waking bots after they were told to stop: the routine was
gone but the poller kept firing.

Verify: no listed trigger process is running, no cron/timer/launchd/task
entry remains, and no routine is still deployed for the bot.

## 3. Move each bot's key into the router (R63.4)

For each local bot listed in this machine's `router.toml`:

```
buzz-router keys set --bot <bot-name>
```

Paste the bot's nsec at the prompt; it is read from stdin into the OS
keychain (macOS Keychain, Windows Credential Manager, Linux Secret
Service; `key = "file:<path>"` with mode `0600` where Secret Service is
unavailable).

Then remove `BUZZ_PRIVATE_KEY` and any nsec from the agent's environment,
config files, and scripts. Otherwise stop and turn limits can be bypassed:
any process holding the key can post as the bot outside the router.

Verify with:

```
buzz-router keys check
```

It checks, for each `router.toml` bot, that the key loads and that its
public key matches the bot's roster `pubkey`, printing one line per bot.
It exits `0` when all pass and `3` (`key_error`) on any failure. Fix every
failure before continuing.

## 4. Install the shared roster and compare hashes (R63.5)

Install the shared `roster.toml` into the config dir, then run:

```
buzz-router roster check
```

It prints the lowercase hex SHA-256 of the file bytes. Confirm the hash
is **identical on every machine** before continuing. A mismatch means a
machine is routing against a different roster.

## 5. Write router.toml and install the service (R63.6)

Write this machine's `router.toml`: exactly the bots this machine serves
(see "One router per bot" below), the relay URL, adapter commands, and
`api_bind` on loopback. Then run:

```
buzz-router service install
```

This writes and loads the per-OS service definition (macOS LaunchAgent
with `KeepAlive`, Linux `systemd --user` unit with `Restart=always`,
Windows Task Scheduler logon task with restart on failure). Confirm the
service is running with `buzz-router service status`.

## 6. Smoke test (R63.7)

David posts, for **each bot** on this machine:

1. `@<bot> ping` — expect 👀 on the message promptly, then a threaded
   reply.
2. `stop` — expect 🛑 from **every** bot.
3. `resume` — expect ▶️ and normal routing afterwards.

If any bot misses its reaction, stop: do not proceed to the next machine
until every bot on this one passes.

### Stop-phrasing note (R29.8)

A 1–5-word owner message containing "stop" halts bots — for example
"stop the dev server" is a stop command, not a task. Phrase such requests
as "shut down the dev server" instead.

### CLI stop fallback (A5)

A router acts only on control messages its bots receive: a bot whose
router has no bot in that channel never sees the message and is not
halted. When a Buzz stop cannot reach a machine's bots, use the
per-machine fallback on that machine:

```
buzz-router stop [--bot <name>]...
```

If the admin API is reachable the CLI uses it; if it is unreachable
(connection refused or a 2 s timeout) the CLI writes the halt rows to
SQLite itself and kills the running wake process trees, then exits `0`.
The daemon honours those rows when it is alive. The CLI fallback never
calls a webhook `cancel_url`: without the daemon there is no wake state.

## 7. Watch status for 24 hours (R63.8)

```
buzz-router status
```

Watch for 24 hours after cutover on every machine. Any `unmanaged_posts`
means step 2 missed a trigger: a process outside the router posted with a
bot key. Find it via the inventory list, remove it, and keep watching.

Note: `unmanaged_posts`, `missed`, and budget-suppression counts are
in-memory, counted since the daemon started, and **reset on restart**
(DD-12). A restart during the watch restarts the counters; factor that
into the 24-hour reading.

## 8. One router per bot (R63.9)

Every bot is served by exactly one router:

- a `command` bot is served by the router on its agent's machine;
- a `webhook` bot is served by one chosen router — list it only in that
  machine's `router.toml`;
- never list the same bot on two routers: both would wake it and both
  would answer.

A machine with no command bots needs no router unless it hosts webhook
bots.

## 9. Tailscale ACLs (R63.10)

When the tailnet API is exposed, set Tailscale ACLs so the router port is
reachable only from the hosts that run agents. No other host needs to
reach it.

## 10. Tuning and restart (R63.11)

Tune by editing `[limits]` (or per-bot `limits`) in the roster:
`turns_per_round`, `wakes_per_hour` / `wakes_per_day`, quiet hours,
debounce, `max_wake_minutes`, `status_note_after_secs`, `default_bot`,
`respond_to`. Then redistribute the roster file to **every machine** and
restart each router — configuration is read at startup; v1 has no hot
reload. Re-run `roster check` on each machine to confirm the new hash
matches everywhere.
