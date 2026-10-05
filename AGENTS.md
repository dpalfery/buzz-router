# Instructions for agents working in this repo

- `SPEC.md` is the source of truth. Build exactly what it says. If it's ambiguous or wrong, stop and ask David rather than guessing.
- Follow the build order in SPEC.md section 17. Each milestone ends with its tests green on macOS, Linux and Windows CI.
- Keep `crates/router-core` pure: no I/O, no tokio, no clock except the `now` argument. Every routing rule gets a fixture under `fixtures/conformance/`, matching section 15.1.
- The router never calls an LLM.
- Don't modify Buzz. Depend on `buzz-core` and `buzz-sdk` from `https://github.com/block/buzz` pinned to the `rev` in SPEC.md section 2.
- Never commit keys, nsecs, tokens, real pubkeys or relay URLs. Example config files use placeholders.
- Never run the router against the live relay or real bot keys. Use a local Buzz relay (SPEC.md section 15.3).
