---
id: guides/getting-started
title: Getting started with buzz-router
doc-type: onboarding
status: draft
component: buzz-router
source-root: crates/buzz-router
owner: dpalfery
last-reviewed: 2026-10-08
---

# Getting started with buzz-router

This takes you from nothing to one machine whose bots answer on Buzz. It covers a single
machine. Moving a fleet over from older triggers is the
[cutover runbook](../runbooks/buzz-router-cutover.md).

## What it is

Each machine that hosts bots runs one `buzz-router`. It listens to the Buzz relay, decides
which bot should wake (without calling a model), holds the bots' signing keys, posts their
replies, and enforces turn limits, quiet hours and "stop". Agents never hold their own
Buzz keys. The rules are in the [design brief](../specs/buzz-router-v1/brief.md).

## Before you start

You need:

- a Buzz relay URL and the channel ids your bots serve;
- for each bot, its Buzz public key (64 hex characters) and its secret key (`nsec`);
- your own public key(s), which the router treats as the owner;
- the command that runs each bot, for example `claude -p`, or a webhook endpoint.

Keep every secret key out of files and out of git. The router reads them from your
operating system's keychain.

## 1. Install

macOS and Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/dpalfery/buzz-router/main/scripts/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/dpalfery/buzz-router/main/scripts/install.ps1 | iex
```

```bash
buzz-router --version
```

Versions, release candidates, upgrades and troubleshooting are in
[Installing buzz-router](install.md).

## 2. Create the config files

The router reads two files from its config directory:

| OS | Config directory |
|---|---|
| macOS | `~/Library/Application Support/buzz-router` |
| Linux | `~/.config/buzz-router` |
| Windows | `%APPDATA%\buzz-router` |

- `roster.toml` is **shared**: the same bytes on every machine. It names the owner, the
  channels, every bot and the default limits.
- `router.toml` is **per machine**: the relay URL, which bots this machine serves, and how
  to run each one.

Start from the examples in the repository, which show every key with placeholders:

```bash
cd "<your config directory>"
curl -fsSLO https://raw.githubusercontent.com/dpalfery/buzz-router/main/roster.example.toml
curl -fsSLO https://raw.githubusercontent.com/dpalfery/buzz-router/main/router.example.toml
mv roster.example.toml roster.toml
mv router.example.toml router.toml
```

Edit both files and replace every `<placeholder>`. The router rejects a file that still has
one. The main things to set:

- In `roster.toml`: `[owner]` pubkeys and timezone, each `[[channels]]` id, and each
  `[[bots]]` name and pubkey.
- In `router.toml`: `relay_url`, and one `[[bots]]` entry per bot **this machine** runs,
  with its adapter. Leave `api_bind` on a loopback address.

A bot is served by exactly one router. Never list the same bot on two machines.

Check the roster:

```bash
buzz-router roster check
```

It validates the file and prints its SHA-256. With more than one machine, the hash must
match on all of them.

## 3. Store the bot keys

For each bot in `router.toml`:

```bash
buzz-router keys set --bot <bot-name>
```

Paste the bot's `nsec` at the prompt. It goes into the OS keychain and never into a file.
Then check them all:

```bash
buzz-router keys check
```

Each bot's key must load and match the public key in the roster. Remove any `nsec` or
`BUZZ_PRIVATE_KEY` from the agents' own environment and scripts, or they could post as the
bot without the router's limits.

## 4. Run it

To watch it start in a terminal first:

```bash
buzz-router run
```

When you are happy, stop it with Ctrl-C and install it as the per-user service, which
starts at login and restarts if it dies:

```bash
buzz-router service install
buzz-router service status
```

If your agents reply by running `buzz-router post`, read
[Bots that call `buzz-router`](install.md#bots-that-call-buzz-router) first: the service
may not have the install directory on its `PATH`.

## 5. Check it works

```bash
buzz-router status
```

Then, as the owner, post in a channel:

1. `@<bot> ping`: you should see a 👀 reaction quickly and then a threaded reply.
2. `stop`: every bot reacts with 🛑 and no agent posts until you say `resume`.
3. `resume`: bots answer again.

If a bot does not react, run `buzz-router status` and `buzz-router wakes` to see what the
router decided and why.

To stop bots from the machine itself, even without Buzz:

```bash
buzz-router stop
buzz-router resume
```

## Where to go next

- [Cutover runbook](../runbooks/buzz-router-cutover.md): moving a machine from older
  triggers, tuning limits, and the 24-hour watch.
- [Installing buzz-router](install.md): versions, upgrades, uninstalling.
- [Design brief](../specs/buzz-router-v1/brief.md): the routing rules, turn caps and
  wake lifecycle in full.
- [System architecture](../system/architecture.md).
