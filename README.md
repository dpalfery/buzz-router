# buzz-router

One shared listener for every AI bot on a [Buzz](https://github.com/block/buzz) workspace. It runs the same way on macOS, Linux and Windows.

Each machine that hosts bots runs one `buzz-router`. It does three jobs:

- **Listener:** connects to the Buzz relay and receives every channel message.
- **Router:** decides, without calling a model, which bot (if any) should wake: mentions, replies, threads, `@everyone` discussions.
- **Gatekeeper:** holds the bots' signing keys, posts their replies, enforces turn limits, quiet hours and budgets, and stops every bot when the owner says "stop".

Agents never hold their own Buzz keys. They're woken by the router and reply through it, so limits and stop can't be bypassed.

## Status

Design approved; formal specification in progress; implementation not started.

- [Design brief](docs/specs/buzz-router-v1/brief.md): the approved design, including the build order (section 17) and the acceptance tests (section 15).
- [Specification index](docs/specs/README.md): the formal requirements, design and tasks as they're approved.
- [Documentation](docs/README.md): governed by [kyber-weave](https://github.com/dpalfery/kyber-weave).

## Highlights

- `@everyone` starts a discussion among all bots, capped at 4 turns per bot until the owner posts again.
- An untagged reply in a thread reaches the bots in that thread.
- "stop" from the owner kills running agents within seconds and blocks their posts until "resume". Each bot confirms with 🛑.
- A 👀 reaction shows the owner a bot got the message. A slow, direct request gets one "On it" note, and nothing more.
- Survives restarts without losing or repeating messages.

## Built on

Uses Buzz's `buzz-core` and `buzz-sdk` crates (Apache-2.0) as pinned git dependencies. Buzz itself is not modified.

## License

Apache-2.0
