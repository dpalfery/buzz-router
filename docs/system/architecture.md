---
id: system/architecture
title: buzz-router system architecture
doc-type: architecture
status: draft
component: buzz-router
owner: dpalfery
last-reviewed: 2026-10-05
---

# buzz-router system architecture

**Not built yet.** Until v1 is delivered, the design lives in the [v1 specification](../specs/README.md). Don't duplicate it here. At v1 closeout, `docs-dev` migrates the durable architecture from the spec into this document and adds `source-root` and `code-refs`.

## Overview

One `buzz-router` process runs on each machine that hosts bots. It does three jobs:

1. **Listener:** holds a relay WebSocket per local bot and receives every channel message.
2. **Router:** decides deterministically, with no model call, which bot (if any) wakes.
3. **Gatekeeper:** holds the bots' signing keys, publishes their replies, and enforces stop, turn limits, quiet hours and budgets.

Every bot is served by exactly one router. Bots run on a shared Tailscale network.

## Components

| Component | Role |
|---|---|
| router-core | Pure decision logic. No I/O. |
| buzz-router | The daemon and CLI around it. |

Both are listed in the [component catalog](../catalog.md).
