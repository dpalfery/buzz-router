---
id: specs/buzz-router-v1/tasks
title: buzz-router v1 tasks
doc-type: spec
status: current
component: buzz-router
owner: dpalfery
last-reviewed: 2026-10-06
---

# buzz-router v1 Implementation Tasks

**Phase status:** Approved

**Approval:** Approved by David on 2026-10-05 ("approved"). Approve and execute: granted (spec marked Ready on 2026-10-05; current status is under Progress). This is the build plan, written against the approved [requirements.md](requirements.md) and [design.md](design.md).

**Development mode:** test-first

## Progress

**Implementation status:** In progress. Paused by David on 2026-10-06 after milestone 1.

| | |
|---|---|
| Branch | `feat/buzz-router-v1` (pushed to origin) |
| Pull request | [#1 (draft)](https://github.com/dpalfery/buzz-router/pull/1) |
| Last task commit | `22dd879` (T1.11) |
| Tests | 389 passed, 0 failed (`cargo test --workspace --locked`, macOS arm64). `clippy -D warnings` and `fmt --check` are clean. `router-core` is tokio-free. 59 conformance cases. |
| CI | Rust CI skeleton (`.github/workflows/ci.yml`) exists plus Docs Gate. 3-OS green pending first CI run. |

| Milestone | Tasks | Status |
|---|---|---|
| 1. router-core and conformance | 1.1–1.11 | 11 of 11 done. 3-OS CI green pending first CI run. |
| 2. Relay I/O | 2.1–2.9 | Not started. **Resume point: 2.1** |
| 3. Wake engine plus stop | 3.1–3.10 | Not started |
| 4. State and recovery | 4.1–4.4 | Not started |
| 5. Webhook adapter | 5.1–5.2 | Not started |
| 6. Packaging, CI and release | 6.1–6.4 | Not started |
| 7. End-to-end acceptance | 7.1–7.5 | Not started |
| 8. Docs and closeout | 8.1–8.2 | Not started |

### Open items for the owner

Raised during milestone 1. Each needs David's decision before the task that depends on it.

- **O1, ci.yml (blocks 1.1 criterion 6 and all Linux/Windows CI).** github-devops declined to write `.github/workflows/ci.yml` without a declared `<github-actions-coding-standard>`. It asks whether it may write ci.yml exactly as specified in 1.1 criterion 6, with actions pinned by SHA like `docs-gate.yml`.
- **O2, F5, config edge cases.** The architect settled the config type shapes. Still open: duplicate channel ids; `max_concurrent = 0` or an empty command; a bot named `all`.
- **O3, F6.** A bare owner name in reply `p` tags: the contract literal versus R44.4. T1.4 implemented an interim reading (every owner pubkey for a bare whole-word `owner.name`).
- **O4, F8.** R17.4 versus R15.2 and design §5.5 on foreign-bot `p` tags.
- **O5, F9.** Replay doesn't invent thread state for roots it never saw.
- **O6, keyring 4.x.** keyring 4.x splits into `keyring-core` plus store crates. Design §11 and the task text assume `keyring::Entry` and keyring's own mock. T1.1 pinned keyring 4.2.0 (feature `v1`) with keyring-core 1.0.0.
- **O7, task 6.1.** The `ErrorKind::Key` question.
- **O8, DD-19.** The `~` expansion of the adapter `cwd` isn't in any task.

## How to work these tasks

### Notation

- **R6.2** is acceptance criterion 6.2 in requirements.md. **R6** is all of Requirement 6. **A1–A18** are the requirements assumptions.
- **§n** is a section of design.md. **DD-n** and **DA-n** are its design decisions and design assumptions.
- A **conformance case** is a row of brief §15.1 and its matching `CONFORM` criterion.

### Branch and commits

- Work on the branch `feat/buzz-router-v1`.
- Make **one commit per completed task**, after REFACTOR. Use the subject `T<id>: <summary> (R<ids>)`, for example `T1.6: route owner-message rules and conformance harness (R6, R7, R8, R9, R10, R17)`.
- RED evidence is recorded in the task's review notes, not committed separately.

### Test-first protocol (every implementation task)

1. **RED.** `test-dev` writes the tests named in the task's Test contract and runs the stated command. Record the failing output.
   - The failure must match the stated RED reason.
   - When the item under test doesn't exist yet, RED may be a compile error naming that item (`error[E0432]` unresolved import, or `error[E0425]` cannot find function).
   - Otherwise RED must be an assertion failure.
   - **No implementation starts before valid RED evidence exists.**
2. **GREEN.** `tauri-dev` writes the minimum implementation needed to pass the contract.
3. **REFACTOR.** Clean up without changing behaviour. Every task ends with this gate passing locally:

   ```
   cargo fmt --all --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   ```

4. **Group RED.** Some groups of conformance fixtures expect "nobody". Those negative fixtures may already pass against the placeholder `route`. They are kept as regression guards; the group's RED comes from its positive fixtures.

**No-test tasks** (CI workflows and documentation) are marked as such and name a replacement validation.

### Agents and standing notes

| Agent | Owns | Standing note |
|---|---|---|
| `tauri-dev` | All Rust implementation | **Non-Tauri Rust: ignore Tauri/IPC guidance; follow design §2 conventions.** No `unwrap`, `expect`, `panic!`, `todo!` or `dbg!` outside tests. `thiserror` in libraries, `anyhow` only in `main` and CLI dispatch. clippy `-D warnings`, rustfmt. |
| `test-dev` | Every test file, fixture and test-support module | **Rust test conventions:** `#[cfg(test)] mod tests` for unit tests inside modules; integration tests in `crates/<crate>/tests/*.rs`, sharing helpers through `tests/support/mod.rs` (or `tests/common/mod.rs` in router-core) declared with `mod support;`; conformance fixtures in `fixtures/conformance/`. Async tests use `#[tokio::test]`, or `#[tokio::test(start_paused = true)]` for fake time. |
| `github-devops` | `.github/workflows/ci.yml`, `release.yml` | Leave the existing `.github/workflows/docs-gate.yml` unchanged. It already runs `kyber-weave docs validate .`. |
| `docs-dev` | The runbook and closeout | Use the `kyber-weave-docs` skill and the frontmatter rules in `docs/documentation-ontology.md`. |

### Docker-dependent tasks

Tasks tagged **[Docker + local Buzz relay]** need Docker (Postgres and Redis from Buzz's `docker-compose.yml`) and a local `buzz-relay` built from Buzz rev `f0eb5575ffc9d5f57af4ed3f574529d997c83a0d`. If Docker isn't available, record those tasks as **Blocked** and continue with the others. They are listed in the Coverage section. **Never use the live relay or real bot keys** (AGENTS.md).

### Interfaces fixed for test-first

Tests are written before implementation, so these public names and signatures are fixed now, from §5–§13. Implementers must provide exactly these.

**`router_core` (crate `crates/router-core`):**
- `ids::{BotName, Pubkey, EventId, ChannelId}`
- `config::{parse_roster(&str) -> Result<Roster, ConfigErrors>, parse_router(&str, &Roster) -> Result<RouterConfig, ConfigErrors>, roster_hash(&[u8]) -> String, Roster, RouterConfig, Limits, ConfigErrors, ConfigIssue}`
- `thread::{thread_position(&[Vec<String>]) -> ThreadPos, ThreadPos, ThreadState, RoundMode}`
- `classify::{classify(&InEvent, &Roster) -> AuthorClass, AuthorClass}`
- `parse::{mention_text(&str) -> String, mentioned_bots(&InEvent, &AuthorClass, &Roster, Option<&BotName>) -> BTreeSet<BotName>, contains_everyone(&str) -> bool, parse_control(&str, &Roster, &BTreeSet<BotName>) -> Option<Control>, mentions_for_reply(&str, &Roster) -> Vec<Pubkey>}`
- `quiet::quiet_set(&Roster, DateTime<Utc>) -> BTreeSet<BotName>`
- `route::{route, InEvent, Snapshot, RouteResult, Decision, Reason, Priority, SuppressWhy, Control, Scope, ThreadUpdate, NewThread, NewRound, Halts, WakeCounts, EditTarget, Diagnostic, KIND_MESSAGE, KIND_EDIT}` (§5.2)
- `prompt::{BUILT_IN_TEMPLATE, PromptVars, render, reason_text, render_context}` and `payload::{WakePayload, ChannelRef, ContextMessage, ApiRef}`
- `replay::Replayer` with `new(Roster)` and `step(&mut self, &InEvent) -> RouteResult`

**`buzz_router`** (the library target of `crates/buzz-router`; `src/main.rs` only calls `buzz_router::cli::main()`):
- `paths::Dirs { config_dir, data_dir }` with `Dirs::resolve(config_override: Option<PathBuf>, data_override: Option<PathBuf>) -> Result<Dirs, PathsError>`
- `store::Store` with `open(&Path)` and `open_in_memory()`, and one repository module per table (§9)
- `keys::{KeySource, load_key(&KeySource, &BotName) -> Result<nostr::Keys, KeyError>}`
- `clock::{Clock, SystemClock, VirtualClock}`
- `relay::{RelayPort, RelayError, rest::RestClient}`, `relay::conn::spawn_connection`
- `ingest::{EnrichedEvent, Source}`
- `publish::{build_reply, build_status_note, build_reaction_event, build_typing}`
- `adapter::{Adapter, AdapterEvent, SyncReply, WakeContext, command::CommandAdapter, webhook::WebhookAdapter}`
- `core::{spawn_core, CoreDeps, CoreHandle}`. `CoreHandle` has:
  - `ingest(bot, nostr::Event, Source)`
  - `api(ApiRequest) -> ApiResponse`
  - `flush()`, which resolves when the core has drained its queue (tests only)
  - `debug_counters()` (tests only)
- `api::{loopback_router(ApiState) -> axum::Router, tailnet_router(ApiState) -> axum::Router, ApiState, admin_token::ensure(&Path) -> Result<String, ApiError>}`
- `service::{render_launchd_plist, render_systemd_unit, render_task_xml, CommandRunner}`
- `cli::main() -> std::process::ExitCode`

### Shared test support (owned by test-dev)

`crates/buzz-router/tests/support/mod.rs` starts in task 2.6 and is extended by later tasks. It provides:

- `keys(name)`: deterministic `nostr::Keys` with secret `sha256("buzz-router-fixture:" + name)`.
- Signed event builders for kind 9 and kind 40003, with `h`, `e` and `p` tags.
- `FakeRelay`, implementing `RelayPort`. It records publishes, serves `query` from seeded events, and can echo published events back synchronously.
- `FakeAdapter`, implementing `Adapter` from a script (§16.2).
- `VirtualClock` helpers, and `spawn_test_core()` returning `(CoreHandle, FakeRelay, Store)`.
- `test_agent_path()`. It builds `crates/test-agent` once (`std::sync::OnceLock`, running `$CARGO build -p test-agent`) and returns `<target>/debug/buzz-router-test-agent{EXE_SUFFIX}`, located from `std::env::current_exe()`.
  - This is the lookup design §16.3 specifies. `CARGO_BIN_EXE_*` is set only for binaries of the package under test, so it can't find the test agent.
- `pid_alive(pid) -> bool`. On Unix, `nix::sys::signal::kill(pid, None)`; on Windows, `tasklist /FI "PID eq <pid>"`.

## Milestone 1: router-core and conformance (brief §17.1)

- [x] **1.1 Scaffolding and verification**
  - **Objective:** Create the cargo workspace, crate skeletons, lints and CI skeleton. Prove the Buzz git dependencies resolve, and pin `keyring`.
  - **Files:** `Cargo.toml`, `Cargo.lock`, `clippy.toml`, `rustfmt.toml`, `crates/router-core/{Cargo.toml,src/lib.rs}`, `crates/buzz-router/{Cargo.toml,src/lib.rs,src/main.rs}`, `crates/test-agent/{Cargo.toml,src/main.rs}`, `.github/workflows/ci.yml`, `crates/router-core/tests/smoke.rs`, `crates/buzz-router/tests/smoke.rs`.
  - **Design:** §2, §3.1, §3.2, §3.3, §15.
  - **Requirements:** R5.9, R58.3, R60.1, R62.1, R62.3, R62.5.
  - **Depends on:** none.
  - **Agents:** `tauri-dev` (workspace, crates, dependencies); `github-devops` (ci.yml skeleton); `test-dev` (smoke tests).
  - **Acceptance criteria:**
    1. **Workspace settings:**
       - members are `crates/router-core`, `crates/buzz-router` and `crates/test-agent`;
       - `[workspace.package]` has `edition = "2021"` and `rust-version = "1.88"`; `resolver = "2"`;
       - `[workspace.dependencies]` exactly as in the §3.2 table, and `[workspace.lints]` exactly as in §3.1;
       - every crate has `[lints] workspace = true`;
       - `clippy.toml` sets `allow-unwrap-in-tests = true` and `allow-expect-in-tests = true`; `rustfmt.toml` sets `edition = "2021"`.
    2. `crates/buzz-router` has a library target (`src/lib.rs`, which holds every module) and a binary named `buzz-router` (`src/main.rs` calls `buzz_router::cli::main()`). Until task 1.11, `cli::main()` returns `ExitCode::SUCCESS`. `crates/test-agent` has `publish = false` and a binary named `buzz-router-test-agent`.
    3. **Buzz dependencies resolve:**
       - `buzz-core` and `buzz-sdk` are git dependencies on `https://github.com/block/buzz` at `rev = "f0eb5575ffc9d5f57af4ed3f574529d997c83a0d"`;
       - `cargo tree -p router-core -e normal` shows both from that git rev; save the output as evidence;
       - `cargo tree -p router-core -e normal -i tokio` reports no match.
       - **If resolution fails, stop and report the exact cargo error to the conductor (DA-3).**
    4. **`keyring` pinned:**
       - added to `crates/buzz-router` at the latest major version on crates.io;
       - the feature names for the macOS native keychain, the Windows Credential Manager and Linux Secret Service (sync) are confirmed from that version's docs;
       - the version and features are recorded in a comment in `crates/buzz-router/Cargo.toml` and in the evidence.
    5. **Lockfile and build:**
       - `Cargo.lock` is committed, and `cargo build --workspace --locked` passes.
       - `cargo tree --workspace` contains no LLM or model client (no `openai`, `async-openai`, `anthropic`, `ollama` or `llm*` crates; R62.1).
    6. **CI skeleton** (github-devops), `.github/workflows/ci.yml`:
       - triggers on push and pull request;
       - a `lint` job on ubuntu-latest running `cargo fmt --all --check`;
       - a `test` job over `[macos-latest, ubuntu-latest, windows-latest]` running `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo test --workspace --locked`;
       - an ubuntu step that fails if `cargo tree -p router-core -e normal -i tokio` prints anything.
       - The skeleton is green on all three OSes.
  - **Test contract:**
    - *Tests:* `crates/router-core/tests/smoke.rs` and `crates/buzz-router/tests/smoke.rs`.
    - *Run:* `cargo test --workspace --locked smoke`
    - *Behaviour:*
      - router-core smoke: `buzz_core::nip10::parse_thread_markers_from_parts` on an empty tag list returns `ThreadMarkers::default()`, and `` buzz_sdk::mentions::strip_code_regions("`x`") `` returns `" "`. This proves both git crates link.
      - buzz-router smoke: with `keyring`'s mock credential builder installed, an `Entry` for service `buzz-router-smoke` can `set_password` and then `get_password`.
    - *RED:* there is no workspace, so `cargo test` fails with `could not find Cargo.toml`.
    - *GREEN:* both smoke tests pass locally and in the CI skeleton on all three OSes.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR
  - **Done:** commits `9c932ee` (scaffold) and `a54ee98` (ci.yml skeleton, criterion 6). Local smoke tests are green; 3-OS CI green pending first CI run.

- [x] **1.2 Config types, validation and example files**
  - **Objective:** Parse and validate `roster.toml` and `router.toml` into resolved types, collecting every issue at once. Ship placeholder example files.
  - **Files:** `crates/router-core/src/ids.rs`, `crates/router-core/src/config/{mod.rs,roster.rs,router.rs,limits.rs,validate.rs}`, `roster.example.toml`, `router.example.toml`, `crates/router-core/tests/config.rs`, `crates/router-core/tests/config_examples.rs`.
  - **Design:** §4.2, §4.3, §5.1, DD-19.
  - **Requirements:** R1.2, R1.3 (default value), R1.6–R1.14, R1.16, R2.1–R2.12, R2.17, R22.3; A2, A3 (rules without I/O).
  - **Depends on:** 1.1.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. The raw structs use `deny_unknown_fields` and mirror §4.2. The resolved `Roster` exposes effective per-bot `Limits` (per-bot values over `[limits]` over defaults) and a lowercase `name_index` covering names and aliases.
    2. Every A2 and A3 rule, and R1.7, R1.8, R1.13, R2.3 and R22.3, rejects with a `ConfigIssue` carrying the TOML path. Several issues in one file are all reported.
    3. **Router defaults:** `api_bind` is `127.0.0.1:47821`, `max_concurrent` is 1, and `roster_path` is `roster.toml`.
    4. The example files follow brief §4 with placeholders only.
  - **Test contract:**
    - *Tests:* `crates/router-core/tests/config.rs` and `crates/router-core/tests/config_examples.rs`.
    - *Run:* `cargo test -p router-core --test config --test config_examples`
    - *Behaviour (roster):*
      - The brief §4.1 roster, with placeholders replaced by fixture keys, parses. Omitted limits take the R2.5 defaults, and a per-bot override merges (R2.6).
      - `["*"]` resolves to All, and a UUID list to exactly those channels (R2.10). An alias resolves through `name_index` (R2.11).
    - *Behaviour (rejections, one test each, asserting the issue path):* `version = 2`; timezone `"Mars/Olympus"`; `quiet_hours = "23-7"`; an alias duplicating another bot's name in different case; an owner key reused as a bot key; an unknown `default_bot`; the unknown key `turns_per_rnd`; a missing `respond_to`; a pubkey that isn't 64 hex characters.
    - *Behaviour (router):*
      - Defaults as in criterion 3.
      - Rejections: `api_bind = "0.0.0.0:47821"`; `tailnet_bind = "0.0.0.0:1"`; an async webhook with empty `public_url`; `key = "vault"`; a bot name missing from the roster; `prompt_mode = "pipe"`.
      - `key = "file:/x"` is accepted.
      - A file with three errors reports three issues.
      - `roster_hash(b"abc")` equals `ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad`.
    - *Behaviour (examples):* both example files parse and validate after every `<…>` placeholder is replaced with deterministic fake values. Neither file contains a 64-hex literal, `nsec1`, `npub1`, or a `wss://` host other than `<relay-host>`.
    - *RED:* compile error, unresolved import `router_core::config`; the example files don't exist.
    - *GREEN:* both test files pass.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR
  - **Done:** commit `49fd432`.

- [x] **1.3 Core types, thread position and author classification**
  - **Objective:** Define the router-core types and `route` signature, the NIP-10 thread resolution, and author classification.
  - **Files:** `crates/router-core/src/{thread.rs,classify.rs}`, `crates/router-core/src/route/mod.rs`, `crates/router-core/tests/{thread_position.rs,classify.rs}`.
  - **Design:** §5.2, §5.3.
  - **Requirements:** R4.2, R4.3, R5.1–R5.4, R8.2, R18.1, R62.4; A12 (the `owner_is_ours` flag).
  - **Depends on:** 1.2.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. All types in §5.2 exist with the listed fields. `Decision` has exactly the shape in R5.3. `Reason` and `Priority` serialise as snake_case.
    2. `route` has the exact §5.2 signature. **For now it is a placeholder** returning `RouteResult` with no decisions and `wake_mode: Direct`. Tasks 1.6–1.9 replace it. It does not panic.
    3. `thread_position` uses `buzz_core::nip10::parse_thread_markers_from_parts` and `ThreadMarkers::resolve()`.
    4. `classify` tests owner, then bot, then foreign_bot, then human. NIP-OA verification uses `buzz_sdk::nip_oa::verify_auth_tag`.
  - **Test contract:**
    - *Tests:* `crates/router-core/tests/thread_position.rs` and `crates/router-core/tests/classify.rs`.
    - *Run:* `cargo test -p router-core --test thread_position --test classify`
    - *Behaviour:*
      - `["e",R,"","reply"]` gives `Reply{root:R, parent:R}`; `root R` plus `reply P` gives `Reply{R,P}`; a lone `root` tag gives `TopLevel`; no `e` tag gives `TopLevel`; `["e","bad","","reply"]` gives `TopLevel`.
      - An owner pubkey gives `Owner`; a roster bot gives `Bot(name)`.
      - An unknown pubkey with an auth tag from `compute_auth_tag(keys("O"), author_pk, "")` gives `ForeignBot{owner_is_ours:true}`. With the owner key `keys("stranger")`, `owner_is_ours` is false.
      - An auth tag computed for a different agent pubkey gives `Human`. No auth tag gives `Human`.
    - *RED:* compile error, unresolved `router_core::thread::thread_position` and `router_core::classify::classify`.
    - *GREEN:* both files pass.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR
  - **Done:** commit `853e4d0`.

- [x] **1.4 Parsing: mentions, `@everyone`, control and reply mentions**
  - **Objective:** Implement the parsers in §5.4, including `nprofile` decoding (the A18 resolution) and the mention extractor for reply `p` tags.
  - **Files:** `crates/router-core/src/parse/{mod.rs,text.rs,mentions.rs,everyone.rs,control.rs,nip19.rs}`, `crates/router-core/tests/{parse_mentions.rs,parse_control.rs}`.
  - **Design:** §5.4, §6.8 (reply mentions), DD-18.
  - **Requirements:** R7.1, R7.2, R17.1–R17.5, R29.2–R29.6, R44.4; A18.
  - **Depends on:** 1.2, 1.3.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** All §5.4 algorithms are implemented as written. The regexes are compiled once with `std::sync::LazyLock`.
  - **Test contract:**
    - *Tests:* `crates/router-core/tests/parse_mentions.rs` and `crates/router-core/tests/parse_control.rs`.
    - *Run:* `cargo test -p router-core --test parse_mentions --test parse_control`
    - *Behaviour (mentions):*
      - Code spans and fences are removed, and lines starting with `>` are removed.
      - `@dp-grok-bot` matches only `dp-grok-bot` when `sf-GrokBot` also exists. Matching is case-insensitive. An alias matches its bot. An unknown `@bob` is ignored.
      - A `nostr:npub1…` URI maps to its bot. A `nostr:nprofile1…` URI built in the test with `Nip19Profile::to_bech32` maps to its bot.
      - `p` tags count for Owner and Human authors only. A `p` tag carrying the author's own pubkey is ignored.
      - `contains_everyone` is true for `"@everyone thoughts?"` and `"hi @everyone,"`, false for `"foo@everyone"`, and false for `` "`@everyone`" `` once `mention_text` has run.
      - `mentions_for_reply("thanks David and @dp-kyber-bot")` returns both owner pubkeys plus the bot's pubkey.
    - *Behaviour (control):*
      - These return `Stop(All)`: `"stop"`, `"stop it"`, `"fucking stop["`, `"@everyone stop"`, `"please stop now"`, `"stop the dev server"`, `"!shutdown"`.
      - `"@A stop"` returns `Stop({A})`; `"@A !cancel"` returns `Cancel({A})`; `"resume"` returns `Resume(All)`.
      - These return `None`: `"resume please"`, `"please stop the dev server and restart it"`, `` "`stop`" ``.
      - `"nostr:npub1… stop"` returns `Stop` with the URI stripped.
    - *RED:* compile error, unresolved module `router_core::parse`.
    - *GREEN:* both files pass.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR
  - **Done:** commit `36e9306`.

- [x] **1.5 Quiet hours and limit gates**
  - **Objective:** Implement `quiet_set` (half-open, owner timezone, per-bot) and the ordered gate helper.
  - **Files:** `crates/router-core/src/quiet.rs`, `crates/router-core/src/route/gates.rs` (with `#[cfg(test)] mod tests`), `crates/router-core/tests/quiet.rs`.
  - **Design:** §5.5 (gate helper), §5.6, DD-4.
  - **Requirements:** R12.4, R19.2, R20.2, R21.2, R22.1, R22.2, R22.6; A1, A10 (the `WakeCounts` meaning).
  - **Depends on:** 1.2, 1.3.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. The interval is half-open [start, end), wraps past midnight, and `start == end` means never quiet.
    2. Gates are evaluated in the order given, and the first failure is returned.
  - **Test contract:**
    - *Tests:* `crates/router-core/tests/quiet.rs`, plus unit tests in `src/route/gates.rs`.
    - *Run:* `cargo test -p router-core --test quiet` and `cargo test -p router-core --lib route::gates`
    - *Behaviour (quiet hours, `America/Chicago`, default range):*

      | Instant (UTC) | Local time | Quiet |
      |---|---|---|
      | `2026-07-01T03:59:59Z` | 22:59:59 | no |
      | `2026-07-01T04:00:00Z` | 23:00 | yes |
      | `2026-07-01T11:59:59Z` | 06:59:59 | yes |
      | `2026-07-01T12:00:00Z` | 07:00 | no |
      | `2026-01-15T05:00:00Z` | 23:00 CST | yes |

      Also: `quiet_hours = ""` means never quiet; `"12:00-12:00"` means never quiet; a per-bot override applies to that bot only.
    - *Behaviour (gates):*
      - Halted and quiet together give `Halted`; quiet and over cap give `Quiet`; over cap and over budget give `Cap`.
      - An hourly count at `wakes_per_hour` gives `Budget`, and so does a daily count at `wakes_per_day`.
      - Below every limit gives `None`.
    - *RED:* compile error, unresolved `router_core::quiet::quiet_set` and `route::gates::gate`.
    - *GREEN:* all pass.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR
  - **Done:** commit `4d725ac`.

- [x] **1.6 Conformance harness and owner-message routing**
  - **Objective:** Build the fixture harness, then implement `route` for owner kind-9 messages: rules (a)–(f), the new round, and halted gating.
  - **Files:**
    - `crates/router-core/tests/conformance.rs`, `crates/router-core/tests/common/mod.rs`;
    - `crates/router-core/src/route/{mod.rs,owner.rs}`;
    - fixtures `fixtures/conformance/01-owner-mention-top-level.json`, `02-owner-untagged-top-level.json`, `03-default-bot.json`, `04-everyone-top-level.json`, `05-owner-untagged-in-thread.json`, `06-everyone-thread-new-round.json`, `07-reply-target.json`, `08-mention-in-discussion-thread.json`, `09-mention-beats-reply-target.json`, `31-thread-without-participants.json`, `32-mention-inside-code.json`, `33-quoted-everyone.json`, `34-longest-name.json`, `36-self-p-tag-ignored.json`;
    - extra fixtures `106-nprofile-mention.json`, `107-npub-mention.json`, `108-alias-mention.json`, `111-everyone-with-mention.json`, `115-default-bot-empty-thread.json`.
  - **Design:** §5.5 (owner_message), §16.1.
  - **Requirements:** R5.6–R5.8, R6, R7, R8, R9, R10, R17 (through cases 32–34 and 36), R64.1–R64.3.
  - **Depends on:** 1.3, 1.4, 1.5.
  - **Agents:** `test-dev` (harness, fixtures, RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. **Harness:**
       - It loads fixtures with `deny_unknown_fields` in the §16.1 format, and derives identities, keys and event ids from symbolic names as §16.1 describes.
       - It registers one `#[test] fn case_NN()` per file through a `conformance_case!` macro.
       - `every_fixture_file_is_registered` fails on any unregistered `*.json`.
       - It compares `control` and `decisions` exactly, and `thread_update` and `wake_mode` when the fixture includes them.
       - It supports multi-step fixtures: a `"steps"` array of `{now?, event, expect}`. After each step it applies `thread_update` and adds one turn per `Wake`. Tasks 1.7 and 1.8 use this.
    2. Each fixture's `expect` equals the brief §15.1 expected result for its case. The `requirement` field names the CONFORM criterion: case 1 is 6.9, case 2 is 10.4, case 3 is 10.5, case 4 is 7.7, case 5 is 9.4, case 6 is 9.5, case 7 is 8.4, case 8 is 6.10, case 9 is 6.11, case 31 is 9.6, case 32 is 17.6, case 33 is 17.7, case 34 is 17.8, case 36 is 17.9.
    3. owner_message follows §5.5 exactly.
  - **Test contract:**
    - *Tests:* `crates/router-core/tests/conformance.rs` with the fixtures above.
    - *Run:* `cargo test -p router-core --test conformance -- case_01 case_02 case_03 case_04 case_05 case_06 case_07 case_08 case_09 case_31 case_32 case_33 case_34 case_36 case_106 case_107 case_108 case_111 case_115 every_fixture_file_is_registered`
    - *Behaviour:* each conformance case returns its brief result. **Cases 1, 2, 3, 4, 5, 6, 7, 8, 9, 31, 32, 33, 34 and 36**, plus the extras: nprofile and npub mentions wake their bot; an alias wakes its bot; `@everyone` plus `@A` wakes every covered bot with reason `Everyone`; `default_bot` applies in a thread with no participants.
    - *RED:* the positive cases fail because the placeholder `route` returns no decisions. The negative cases 2, 31, 32 and 36 may pass, and are kept as guards (group RED).
    - *GREEN:* every listed test passes.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR
  - **Done:** commit `e7d71f3`.

- [x] **1.7 Bot-message routing, discussions and bot rounds**
  - **Objective:** Implement bot_message: participants, bot mentions, discussion targets, the bot round, and gates.
  - **Files:**
    - `crates/router-core/src/route/bot.rs`;
    - fixtures `10-discussion-bot-post.json`, `11-discussion-cap.json`, `12-direct-bot-reply-p-owner.json`, `13-bot-mention.json`, `14-bot-p-tag-ignored.json`, `15-bot-everyone-plain.json`, `16-bot-only-thread-cap.json` (multi-step, 10 alternating posts), `24-quiet-hours-bot-caused.json`, `27-budget-bot-caused.json`;
    - extras `103-mention-and-discussion-single-decision.json`, `109-outside-channels-list.json`, `119-daily-budget-bot-caused.json`.
  - **Design:** §5.5 (bot_message).
  - **Requirements:** R11, R12, R13, R19.2, R19.6, R20.2, R21.2, R22.4.
  - **Depends on:** 1.6.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. Case 16's steps reproduce the brief exactly. A and B alternate; each is woken until it has used 4 turns; the 9th and 10th posts give `Suppress(Cap)`; nothing resets.
    2. Case 11 asserts `Suppress A (Cap)` and `Wake C`. Its "⏸️ once" part is an engine behaviour, asserted in task 3.2.
  - **Test contract:**
    - *Run:* `cargo test -p router-core --test conformance -- case_10 case_11 case_12 case_13 case_14 case_15 case_16 case_24 case_27 case_103 case_109 case_119`
    - *Behaviour:* **cases 10, 11, 12, 13, 14, 15, 16, 24 and 27** match the brief. Extra 103 gives one decision, with reason `BotMention`. Extra 109 gives no decision for a local bot whose `channels` list excludes the channel. Extra 119 gives `Suppress(Budget)` when the daily count is reached.
    - *RED:* the placeholder returns no decisions for bot authors, so cases 10, 11, 13, 16, 24, 27, 103 and 119 fail.
    - *GREEN:* all listed pass, and so do the 1.6 cases (no regression).
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR
  - **Done:** commit `395e000`.

- [x] **1.8 Human, foreign-bot and edit routing; status tag; halts; owner exemptions**
  - **Objective:** Implement human_message, foreign_message, owner_edit, the status-tag short-circuit, the RespondTo precedence, and the owner exemptions from quiet hours and budgets.
  - **Files:**
    - `crates/router-core/src/route/{human.rs,edit.rs}` and dispatch in `route/mod.rs`;
    - fixtures `23-halted-owner-mention.json`, `25-quiet-owner-mention.json`, `26-budget-owner-mention.json`, `28-status-tag.json`, `29-human-owner-only.json`, `30-human-anyone.json`, `35-owner-edit-adds-p.json`, `37-foreign-bot.json`;
    - extras `101-human-reply-target.json`, `102-human-mention-beats-reply.json`, `104-edit-over-cap.json`, `105-edit-ignores-quiet-budget.json`, `110-halted-owner-only-respond-to.json`, `112-non-owner-edit-ignored.json`, `116-foreign-roster-drift.json`, `117-human-everyone-plain.json`, `118-quiet-boundary.json` (two steps, at 23:00:00 and 07:00:00), `120-daily-budget-owner-not-blocked.json`.
  - **Design:** §5.5 (human, foreign, edit), §5.3.
  - **Requirements:** R4.4 (diagnostic), R4.5, R4.6, R6.7, R6.8, R14, R15, R16.1–R16.6, R16.8, R20.3, R20.5, R20.6, R21.3, R22.4, R22.7, R22.8, R30.4, R30.6; A11, A12.
  - **Depends on:** 1.6.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** Behaviour follows §5.5. Edit fixtures supply `thread` and `edit_target` in the snapshot.
  - **Test contract:**
    - *Run:* `cargo test -p router-core --test conformance -- case_23 case_25 case_26 case_28 case_29 case_30 case_35 case_37 case_101 case_102 case_104 case_105 case_110 case_112 case_116 case_117 case_118 case_120`
    - *Behaviour:* **cases 23, 25, 26, 28, 29, 30, 35 and 37** match the brief. The extras:
      - 101: a human reply to a bot's message wakes it with `ReplyTarget`.
      - 102: a human's mention beats the reply target.
      - 104: an edit target at its cap gives `Suppress(Cap)`.
      - 105: an edit wake ignores quiet hours and the budget.
      - 110: a halted owner-only bot addressed by a human gives `RespondTo`.
      - 112: a non-owner edit gives nothing.
      - 116: a foreign bot whose owner is ours gives a `RosterDrift` diagnostic.
      - 117: a human's `@everyone` is plain text.
      - 118: at 23:00:00 the wake is suppressed `Quiet`; at 07:00:00 it wakes.
      - 120: the owner isn't blocked by the daily budget.
    - *RED:* the human, foreign and edit paths are unimplemented, so cases 23, 25, 26, 29, 30, 35, 37, 101, 102, 104, 105, 110, 116 and 118 fail. Case 28 may already pass (group RED).
    - *GREEN:* all listed pass, with no regression in 1.6 or 1.7.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR
  - **Done:** commit `7938bd8`.

- [x] **1.9 Control routing (stop, resume, cancel)**
  - **Objective:** Wire `parse_control` into owner_message as rule 1. Control comes only from owner kind 9.
  - **Files:**
    - `crates/router-core/src/route/owner.rs`;
    - fixtures `17-everyone-stop.json`, `18-fucking-stop.json`, `19-scoped-stop.json`, `20-long-sentence-not-stop.json`, `21-scoped-cancel.json`, `22-resume-all.json`;
    - extras `113-shutdown.json`, `114-scoped-resume-two-bots.json`, `121-bot-stop-not-control.json`.
  - **Design:** §5.4 (control), §5.5 step 1.
  - **Requirements:** R29.1, R29.3–R29.7, R29.9–R29.12, R31.3, R32.3.
  - **Depends on:** 1.6.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** When a control command matches, the result has `control` set, no decisions, and an empty `thread_update`.
  - **Test contract:**
    - *Run:* `cargo test -p router-core --test conformance -- case_17 case_18 case_19 case_20 case_21 case_22 case_113 case_114 case_121`
    - *Behaviour:* **cases 17, 18, 19, 20, 21 and 22** match the brief. Extra 113: `!shutdown` gives `Stop(All)`. Extra 114: `"@A @B resume"` gives `Resume({A,B})`. Extra 121: a bot posting "stop" is not control.
    - *RED:* no control is returned yet, so cases 17, 18, 19, 21, 22, 113 and 114 fail.
    - *GREEN:* all listed pass, and the whole conformance suite passes (`cargo test -p router-core --test conformance`).
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR
  - **Done:** commit `7a6806f`.

- [x] **1.10 Offline replay simulator**
  - **Objective:** Implement `Replayer` as described in §5.8.
  - **Files:** `crates/router-core/src/replay.rs`, `crates/router-core/tests/replay.rs`.
  - **Design:** §5.8.
  - **Requirements:** R55.2–R55.4 (simulator); A17.
  - **Depends on:** 1.6–1.9.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. The simulated clock is `now = created_at`. Every roster bot is local.
    2. Thread updates and controls are applied, and each `Wake` counts as dispatched immediately.
  - **Test contract:**
    - *Run:* `cargo test -p router-core --test replay`
    - *Behaviour:*
      - An owner `@everyone`, then bot posts, gives Cap after 4 wakes for each bot.
      - `"stop"`, then `"@A hi"`, gives `Suppress(Halted)`; `"resume"`, then `"@A hi"`, gives a wake.
      - A bot post whose `created_at` falls inside quiet hours gives `Quiet`.
      - Replies resolve their parent author from earlier events.
    - *RED:* compile error, unresolved `router_core::replay::Replayer`.
    - *GREEN:* passes.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR
  - **Done:** commit `546f097`.

- [x] **1.11 CLI skeleton: paths, errors, `roster check`, `route --replay`**
  - **Objective:** Build the clap command tree with the exact brief §13 command list, path resolution, the JSON error and exit-code mapping, and the first two working commands.
  - **Files:** `crates/buzz-router/src/main.rs`, `src/cli/{mod.rs,roster.rs,replay.rs}`, `src/paths.rs`, `src/logging.rs` (stderr init only), and `crates/buzz-router/tests/{cli_roster_check.rs,cli_replay.rs,cli_surface.rs,cli_errors.rs}`.
  - **Design:** §4.1, §12.1, §14, DD-11.
  - **Requirements:** R2.13–R2.15, R3.1–R3.4, R55.2–R55.4, R57, R62.2.
  - **Depends on:** 1.2, 1.10.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. **Subcommands:** exactly `run`, `status`, `stop`, `resume`, `cancel`, `post`, `pass`, `eta`, `wakes`, `capture`, `route`, `keys`, `roster` and `service`, with the flags in §12.1.
       - Hidden global flags `--config-dir` and `--data-dir`, with env `BUZZ_ROUTER_CONFIG_DIR` and `BUZZ_ROUTER_DATA_DIR`.
       - Commands not yet built return `CliError{Other, "not implemented"}`, exit 4, until their tasks.
    2. **Errors:** `{"error":"<category>","message":"…","retryable":bool}` on stderr. The mapping is `BadInput→1/user_error`, `Network→2/network_error`, `Auth→3/auth_error`, `Other→4/error`.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test cli_roster_check --test cli_replay --test cli_surface --test cli_errors` and `cargo test -p buzz-router --lib paths`
    - *Behaviour:* the tests run `env!("CARGO_BIN_EXE_buzz-router")` with temporary dirs.
      - **`roster check`** on a valid roster prints exactly the lowercase hex SHA-256 of the file bytes and exits 0. An invalid roster gives one JSON error line, category `user_error`, exit 1.
      - Without `router.toml` it reads `<config-dir>/roster.toml`. With `roster_path = "other.toml"` it reads that.
      - **`route --replay file.jsonl --roster r.toml`** prints one JSON line per valid signed event, with `decisions`. A line with a bad signature is skipped. The command works with no `router.toml` and no network.
      - **`--help`** lists exactly the 14 subcommands: no shadow or dry-run mode (R62.2).
      - **Paths:** the default config dir ends with `Library/Application Support/buzz-router` (macOS), `.config/buzz-router` (Linux) or `AppData\Roaming\buzz-router` (Windows), via `cfg(target_os)` tests. Overrides win.
      - **Exit codes:** each `ErrorKind` maps to its code (unit test).
    - *RED:* `cli::main` returns success and prints nothing, so the assertions fail.
    - *GREEN:* all pass on all three OSes.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR
  - **Done:** commit `22dd879`. The task-reviewer audit was in progress when the run stopped on a usage limit, so it isn't confirmed.

## Milestone 2: relay I/O (brief §17.2)

The SQLite store lands here, earlier than brief §17.4 places it, because ingest and the engine both use it. Recovery behaviours stay in Milestone 4.

- [x] **2.1 SQLite store: schema and repositories**
  - **Objective:** Open and migrate `state.sqlite3` with the §9.2 DDL, and provide one typed repository per table.
  - **Files:** `crates/buzz-router/src/store/{mod.rs,schema.rs,events.rs,threads.rs,wakes.rs,posts.rs,halts.rs,cursors.rs}`, `crates/buzz-router/tests/store.rs`.
  - **Design:** §9.
  - **Requirements:** R18.5, R30.2 (storage), R34.3, R47.1–R47.4, R58.2 (bundled SQLite).
  - **Depends on:** 1.1.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. Migration 1 runs once and sets `user_version` to 1. A file database sets WAL, `synchronous = NORMAL` and `busy_timeout = 5000`.
    2. The columns and indexes match §9.2 exactly.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test store`
    - *Behaviour:*
      - Reopening doesn't re-run the migration.
      - `PRAGMA table_info` for each of the seven tables equals the §9.2 columns.
      - **events:** insert-or-ignore, then mark processed, then `is_processed`.
      - **threads:** upsert and load, with participants as JSON.
      - **turns:** increment, and set `cap_reacted`.
      - **wakes:** insert; update state; find by `token_hash`; find the queued wake by (bot, root); count by `started_at` since a time.
      - **posts:** insert, exists, delete.
      - **halts:** set and clear `'all'` and a bot; list.
      - **cursors:** get; advance never moves backwards.
    - *RED:* compile error, unresolved `buzz_router::store::Store`.
    - *GREEN:* passes.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR

- [ ] **2.2 Signing keys from files**
  - **Objective:** Load `file:<path>` keys with the Unix permission check. Keychain loading comes in task 6.1.
  - **Files:** `crates/buzz-router/src/keys.rs`, `crates/buzz-router/tests/keys_file.rs`.
  - **Design:** §11.
  - **Requirements:** R59.2; A3 (the 0600 rule).
  - **Depends on:** 1.2.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** `KeySource::Keychain` returns `KeyError::Unsupported` until task 6.1.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test keys_file`
    - *Behaviour:*
      - An nsec file and a hex file both load, and surrounding whitespace is trimmed.
      - On Unix, mode 0644 gives `KeyError::Permissions` naming the path, and 0600 loads.
      - A missing file gives `KeyError::Io`.
    - *RED:* compile error, unresolved `buzz_router::keys::load_key`.
    - *GREEN:* passes on all three OSes (the permission cases are `cfg(unix)`).
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR

- [ ] **2.3 Relay REST client and `RelayPort`**
  - **Objective:** Implement `RestClient` (NIP-98, `x-auth-tag`, retries, paging, `submit_event`) and define the `RelayPort` trait.
  - **Files:** `crates/buzz-router/src/relay/{mod.rs,rest.rs}`, `crates/buzz-router/tests/relay_rest.rs`.
  - **Design:** §10.4, §6.1 (ports).
  - **Requirements:** R48.2, R61.5.
  - **Depends on:** 1.1, 2.2.
  - **Agents:** `test-dev` (RED, including the axum mock server); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** The behaviour mirrors `buzz-acp`'s `RestClient`, as §10.4 describes.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test relay_rest`
    - *Behaviour:* against an axum mock on `127.0.0.1:0`:
      - **NIP-98:** `POST /query` sends `Authorization: Nostr <b64>`, which decodes to a kind-27235 event signed by the bot, with tags `u` (the exact URL), `method=POST`, `payload=sha256(body)` and `nonce`.
      - `x-auth-tag` is present if and only if an auth tag is configured.
      - **Retries:** 503, 503, then 200 succeeds, with delays near 500 ms and 1 s (±20%, paused time). A 400 fails at once.
      - **Paging:** pages of 500, 500 and 7 events return 1 007 events. Requests 2 and 3 carry `until` and `before_id` from the oldest event of the previous page.
      - `submit_event` posts to `/events`.
      - `relay_ws_to_http` maps `wss` to `https` and `ws` to `http`, and trims a trailing `/`.
    - *RED:* compile error, unresolved `buzz_router::relay::rest::RestClient`.
    - *GREEN:* passes.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR

- [ ] **2.4 WebSocket connection, NIP-42 auth, publish acks and reconnect**
  - **Objective:** Implement one connection task per bot: authentication, publish with OK tracking, ping and pong, and the reconnect ladder.
  - **Files:** `crates/buzz-router/src/relay/{conn.rs,auth.rs}`, `crates/buzz-router/tests/relay_conn.rs`.
  - **Design:** §10.1, §10.5, §6.8 (publish transport).
  - **Requirements:** R50.1, R61.1.
  - **Depends on:** 2.2, 2.3.
  - **Agents:** `test-dev` (RED, with an in-process `tokio-tungstenite` server); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** Authentication follows `examples/countdown-bot`. The ladder is 1, 2, 4, 8, 16 and 32 s, then 60 s, each with ±20% jitter, resetting after 60 s of stable connection. DNS failures retry on a flat 2 s.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test relay_conn`
    - *Behaviour:*
      - **Auth:** the server sends `["AUTH", c]`. The client answers with a kind-22242 event signed by the bot, tagged `relay` and `challenge`, plus the `auth` tag when one is configured. After the server's OK, the connection reports Up.
      - **Publish:** `publish` resolves on `["OK", id, true]`, errors on `false` with the server's reason, and times out after 10 s (paused time).
      - **Reconnect:** after the server closes, attempts come at the ladder times within the jitter bounds (paused time). The ladder resets after 60 s stable.
      - A Ping gets a Pong.
    - *RED:* compile error, unresolved `buzz_router::relay::conn::spawn_connection`.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **2.5 Discovery, subscription, backfill and cursors**
  - **Objective:** Discover channels, subscribe per channel, backfill from the cursor before flushing buffered live events, and handle first-run cursors.
  - **Files:** `crates/buzz-router/src/relay/{discovery.rs,backfill.rs}`, connection integration in `relay/conn.rs`, `crates/buzz-router/tests/relay_backfill.rs`.
  - **Design:** §10.2, §10.3, §10.4, §6.2 (first run).
  - **Requirements:** R47.4, R48.1 (steps 2–4), R48.2, R48.4, R50.1, R61.2, R61.3; A14.
  - **Depends on:** 2.1, 2.3, 2.4.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** Exactly as §10.2–§10.4.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test relay_backfill`
    - *Behaviour:* with a mock REST server and a mock WebSocket server:
      - **Discovery:** kind-39002 events name two channels; one channel's kind-39000 metadata marks it archived, so it's skipped.
      - **Subscription:** one REQ `ch-<uuid>` per remaining channel, with `kinds [9,40003]`, `#h` and `since` = connect time, spaced at least 125 ms apart (paused time).
      - **Backfill:** from `cursor − 300`, paged to completion. The sink receives backfill events in ascending `(created_at, id)` order before any buffered live event.
      - **No cursor:** no backfill, and the cursor is set to connect time (A14).
      - **Reconnect:** discovery, subscription and backfill from `cursor − 300` run again.
    - *RED:* compile errors for the missing discovery and backfill modules; the subscription assertions fail.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [x] **2.6 Ingest pipeline and shared test support**
  - **Objective:** Implement ingest: signature check, kind and `h` filter, dedupe, thread, edit-target and parent resolution, and thread-rebuild requests. Create `tests/support/mod.rs`.
  - **Files:** `crates/buzz-router/src/ingest.rs`, `crates/buzz-router/tests/support/mod.rs`, `crates/buzz-router/tests/ingest.rs`.
  - **Design:** §6.3 (ingest), DD-13.
  - **Requirements:** R4.1, R16.7, R18.2, R47.3, R61.3, R61.4.
  - **Depends on:** 2.1, 2.3, 1.3.
  - **Agents:** `test-dev` (support module and RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. The steps match §6.3 ingest 1–7.
    2. `RebuildThread` contains only events strictly before the current one by `(created_at, id)`.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test ingest`
    - *Behaviour:*
      - A bad signature is dropped. Kind 7 is ignored. An event without an `h` tag is ignored.
      - The same id arriving from two bots is forwarded once. An id already processed is skipped.
      - A kind-9 reply's thread is resolved.
      - An edit's target is resolved from `events`; when it's unknown there, it comes from `FakeRelay` via `{ids:[target]}`.
      - **Unknown root:** a `RebuildThread` is sent with only the earlier events, then the event itself as an `EnrichedEvent`.
      - The parent author comes from the store, else from `FakeRelay`.
    - *RED:* compile error, unresolved `buzz_router::ingest`.
    - *GREEN:* passes.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR

- [ ] **2.7 Publisher: replies, status notes, reactions, typing**
  - **Objective:** Build and publish every outbound event type, with the `posts` row written before sending, the REST fallback, and the halt refusal.
  - **Files:** `crates/buzz-router/src/publish.rs`, `crates/buzz-router/tests/publish.rs`.
  - **Design:** §6.8, DD-6, DD-7, DD-18.
  - **Requirements:** R30.5 (publish side), R35.5, R44, R45.2, R46.2, R46.3, R59.3.
  - **Depends on:** 2.1, 2.3, 2.4, 1.4.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** Event shapes exactly as §6.8. `VERSION` is `env!("CARGO_PKG_VERSION")`.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test publish`
    - *Behaviour (reply):*
      - kind 9 with an `h` tag;
      - a single reply `e` tag when parent equals root, and root plus reply tags when nested;
      - `p` tags for mentioned bots, and both owner pubkeys when the owner is named;
      - the `auth` tag when configured;
      - the tag `["buzz-router", VERSION, "reply"]`;
      - signed by the bot's key.
    - *Behaviour (publish path):*
      - The `posts` row exists at the moment `FakeRelay` receives the event (a hook checks the store), and is deleted if the publish fails.
      - A WebSocket failure falls back once to REST `submit_event`.
    - *Behaviour (other events):*
      - **Status note:** text `On it, this will take a bit.`, or `On it, about 10 minutes.` when an ETA is set; the status tag; a `posts` row.
      - **Reaction:** kind 7 with the emoji as content and an `e` tag.
      - **Typing:** kind 20002 with the `buzz-acp` `e`-tag shape.
      - Publishing a reply for a halted bot returns an error, and nothing is sent.
    - *RED:* compile error, unresolved `buzz_router::publish::build_reply`.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [x] **2.8 `capture` command**
  - **Objective:** Dump a channel's events as JSON Lines.
  - **Files:** `crates/buzz-router/src/cli/capture.rs`, `crates/buzz-router/tests/cli_capture.rs`.
  - **Design:** §12.1.
  - **Requirements:** R55.1, R57.
  - **Depends on:** 2.2, 2.3, 1.11.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** Uses the first local bot that is a member of the channel. `--since` accepts `<n>m`, `<n>h` or `<n>d`.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test cli_capture`
    - *Behaviour:* the binary runs against an axum mock relay (`router.toml` `relay_url` is `ws://127.0.0.1:<port>`, with a `file:` key).
      - `capture --channel <uuid> --since 2h` prints ascending JSONL. Every line is a valid signed event. The request's `since` is now − 7200 (±5 s).
      - `--since 7x` exits 1 with a JSON error.
      - A channel with no member bot exits 1.
    - *RED:* the command returns "not implemented", exit 4.
    - *GREEN:* passes.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR

- [ ] **2.9 Relay I/O against a local Buzz relay [Docker + local Buzz relay]**
  - **Objective:** Prove the relay layer against a real local Buzz relay, as brief §17.2 asks.
  - **Files:** `crates/buzz-router/tests/e2e_support/mod.rs` (relay URL from `BUZZ_E2E_RELAY_URL`, identities, channel provisioning with `build_create_channel` and `build_add_member` over REST), `crates/buzz-router/tests/e2e_relay_io.rs`.
  - **Design:** §10, §16.4.
  - **Requirements:** R35.5, R44 (real publish), R50.1, R61.1–R61.5.
  - **Depends on:** 2.5, 2.6, 2.7.
  - **Agents:** `test-dev` (tests and harness); `tauri-dev` (any fix, each with its own RED).
  - **Acceptance criteria:**
    1. **Relay setup** (done before the test runs):
       1. Check out Buzz at the pinned rev.
       2. Run `docker compose up -d postgres redis`.
       3. Run `buzz-relay` the way `just relay` does, with `.env.example` and migrations.
       4. Configure relay auth to admit the four throwaway test identities, and record the settings used.
    2. Tests are skipped unless `BUZZ_E2E=1`.
  - **Test contract:**
    - *Run:* `BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=ws://127.0.0.1:3000 cargo test -p buzz-router --test e2e_relay_io -- --test-threads=1`
    - *Behaviour:*
      - Standalone NIP-42 auth succeeds, and so does owner-attested auth with a NIP-OA tag.
      - Discovery finds the provisioned channel.
      - A live owner message arrives.
      - Messages sent during a forced disconnect arrive through backfill.
      - Thread fetch returns the root and its replies.
      - A reply and a reaction are published and visible through a REST query. A typing indicator gets a relay OK.
    - *RED:* the e2e test file and harness don't exist yet, so the run fails to compile. Any product defect found later is its own RED.
    - *GREEN:* passes against the local relay.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

## Milestone 3: wake engine plus stop (brief §17.3)

- [x] **3.1 Prompt rendering and payload model**
  - **Objective:** Implement the built-in template, `render`, `reason_text`, `render_context` and `WakePayload` serialisation.
  - **Files:** `crates/router-core/src/{prompt.rs,payload.rs}`, `crates/router-core/tests/{prompt.rs,payload.rs}`.
  - **Design:** §5.7, DD-17.
  - **Requirements:** R40.1–R40.4, R40.6, R40.7, R41.1–R41.4.
  - **Depends on:** 1.3.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** `BUILT_IN_TEMPLATE` is byte-identical to brief §9.4.
  - **Test contract:**
    - *Run:* `cargo test -p router-core --test prompt --test payload`
    - *Behaviour:*
      - The template equals the expected string embedded in the test.
      - `render` replaces exactly the six variables and leaves `{other}` as is.
      - `reason_text` returns all seven strings, including `"{author} mentioned you"` with the author filled in.
      - `render_context` marks new messages with `★` and indents continuation lines by 4 spaces.
      - The payload JSON has the brief §9.3 keys. `reason` is `"reply_target"`, `round_mode` is `"discussion"`, `deadline` looks like `2026-10-05T03:20:00Z`, `turns_left_after_this` is `limit − used_after`, and `api` holds the paths.
    - *RED:* compile error, unresolved `router_core::prompt` and `router_core::payload`.
    - *GREEN:* passes.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR

- [ ] **3.2 Core actor, clock, snapshot building, applying results, thread rebuild**
  - **Objective:** Implement the core actor loop, `Clock`, building the snapshot, the transactional apply of `RouteResult`, the ⏸️-once rule, budget counters, cursor advance and `RebuildThread`.
  - **Files:**
    - `crates/buzz-router/src/clock.rs`, `crates/buzz-router/src/core/{mod.rs,apply.rs}`;
    - `crates/buzz-router/tests/support/mod.rs`, adding `spawn_test_core`, `FakeAdapter` stubs and `VirtualClock` helpers;
    - `crates/buzz-router/tests/engine_apply.rs`.
  - **Design:** §6.1, §6.3 (core and rebuild), §6.4, §9.3, DD-1, DD-13.
  - **Requirements:** R5.4 (snapshot), R18.2–R18.5, R19.3, R19.4, R20.1, R20.4, R21.1, R21.4, R30.4, R47.3, R47.4, R65.1; A10, A17.
  - **Depends on:** 2.1, 2.6, 1.9.
  - **Agents:** `test-dev` (RED, support); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** The core follows §6.1 and §6.3. All state changes for one event commit in one transaction.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test engine_apply`
    - *Behaviour:*
      1. **Owner mention:** a queued `wakes` row with the §6.5 trigger JSON, a new round in `threads`, and turns reset.
      2. **First Cap suppression** in a round: the bot reacts ⏸️ on the triggering event, and `cap_reacted` becomes 1. A second Cap suppression in the same round produces no reaction (**case 11, "⏸️ once"**).
      3. A Budget suppression increments `debug_counters()`.
      4. **Rebuild:** replaying earlier events restores participants, discussion and round, publishes nothing, and creates no halt rows for a historical "stop". Turn counts come from `wakes` when rows exist, else from bot posts since the round started.
      5. A halt row written directly to the store makes the next owner mention produce `Suppress(Halted)`.
      6. The cursor advances after the apply.
      7. Wakes started 59 minutes and 61 minutes ago give an hourly count of 1 and a daily count of 2.
    - *RED:* compile error, unresolved `buzz_router::core::spawn_core`.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **3.3 Wake queue, coalescing, debounce and scheduling**
  - **Objective:** Implement the per-bot queue, coalescing, A9 attributes, `dispatch_after`, and `schedule()` with priority, FIFO and `max_concurrent`.
  - **Files:** `crates/buzz-router/src/core/queue.rs`, `crates/buzz-router/tests/{engine_debounce.rs,engine_coalesce.rs,engine_queue_order.rs}`.
  - **Design:** §6.5, DD-2.
  - **Requirements:** R23, R24, R25, R26, R65.2, R65.3; A9.
  - **Depends on:** 3.2.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** Exactly §6.5.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test engine_debounce --test engine_coalesce --test engine_queue_order`
    - *Behaviour:* paused time, with `FakeAdapter` recording dispatch instants.
      - **R65.2:** three bot posts 5 s apart give one wake per other participant at the last post + 20 s. A post every 10 s gives a dispatch at the first trigger + 90 s.
      - Owner and human wakes dispatch immediately.
      - **R65.3:** a trigger during a running wake creates exactly one follow-up, which starts only after the running wake ends.
      - Triggers appended to a queued wake leave one row with two triggers, and cost one turn.
      - An owner trigger appended to a debounced bot wake makes it priority `Owner` and dispatchable now.
      - With `max_concurrent = 1` and queued Bot, Human and Owner wakes, the dispatch order is Owner, Human, Bot. Within one priority it's FIFO. With `max_concurrent = 2`, two wakes run at once.
    - *RED:* compile error, unresolved `buzz_router::core::queue`; dispatch never happens.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **3.4 Dispatch, lifecycle, timers and endings**
  - **Objective:** Implement the dispatch steps, the `Adapter` trait, payload context building, typing every 3 s, the deadline and status-note timers, and the endings table with its reactions.
  - **Files:** `crates/buzz-router/src/core/{dispatch.rs,timers.rs}`, `crates/buzz-router/src/adapter/mod.rs`, and `crates/buzz-router/tests/{engine_dispatch.rs,engine_deadline.rs,engine_status_note.rs,engine_endings.rs}`.
  - **Design:** §6.6, §7 (payload building), DD-10, DD-17.
  - **Requirements:** R19.1, R27.1, R27.2, R34, R35, R36.1–R36.5, R36.8, R40.5, R45.1, R45.3, R46.1, R46.4–R46.6, R65.5 (kill and ⌛), R65.6; A6–A8.
  - **Depends on:** 3.3, 2.7, 3.1.
  - **Agents:** `test-dev` (RED, `FakeAdapter` scripts); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** §6.6 exactly, including the endings precedence: killed, then timeout, then posted, then passed, then failed.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test engine_dispatch --test engine_deadline --test engine_status_note --test engine_endings`
    - *Behaviour (dispatch):*
      - `wakes.token_hash` equals the SHA-256 hex of the token the adapter received, and doesn't equal the token.
      - `deadline = started_at + max_wake_minutes`; `turns.used` goes up by 1.
      - 👀 appears on the reaction target only when an owner message is among the triggers.
      - A typing indicator is published at dispatch and every 3 s, and stops when the wake ends.
      - **Payload context:** with 25 messages seeded, it holds the last 20, oldest first. The `new` flags follow DD-17. The channel name is the roster name, else the discovered name.
    - *Behaviour (timers):*
      - **R65.5:** at the deadline, the adapter is cancelled, the state is `timeout`, and ⌛ appears.
      - **R65.6:** a status note is sent at 20 s exactly once for an owner Direct wake; never for a Discussion wake; not at all when a post arrives at 19 s. An ETA is used in the note.
    - *Behaviour (endings):*
      - A post followed by a non-zero exit is `posted`.
      - A pass on an owner Direct wake reacts ✅. A pass on an owner Discussion wake has no ✅.
      - A failure reacts ⚠️.
      - Across all scenarios, the only kind-9 events published are agent replies and status notes (R45.1).
    - *RED:* compile error, unresolved `buzz_router::adapter::Adapter`; dispatch assertions fail.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **3.5 Command adapter and test agent**
  - **Objective:** Implement `CommandAdapter` (process group, environment, prompt modes, capped stdout, stderr logging, scratch files, pid file) and the `buzz-router-test-agent` modes.
  - **Files:** `crates/buzz-router/src/adapter/command.rs`, `crates/test-agent/src/main.rs`, `crates/buzz-router/tests/adapter_command.rs`.
  - **Design:** §7.1, DD-20, DD-21, §16.3.
  - **Requirements:** R36.5, R37 (all), R40.8, R59.4.
  - **Depends on:** 3.4.
  - **Agents:** `test-dev` (RED, test-agent modes); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. **Test-agent modes:** `echo --delay <secs>`, `print-env`, `write-stdout --bytes N`, `exit --code N [--stdout T]`, `stderr --text T`, `spawn-grandchild <pidfile>`, `api-post --text T`, `api-pass-then-sleep`.
    2. `env_remove("BUZZ_PRIVATE_KEY")` is applied last.
    3. Stdout is drained past 64 KiB.
    4. `wakes/<id>/{payload.json,prompt.txt,pid}` are mode 0600 on Unix and deleted when the wake ends.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test adapter_command`
    - *Behaviour:*
      - **Environment:** `print-env` shows `BUZZ_ROUTER_URL`, `BUZZ_ROUTER_WAKE_TOKEN`, `BUZZ_ROUTER_PAYLOAD` and the configured `env`. `BUZZ_PRIVATE_KEY` is absent even when it's set in the test's own env and in the configured `env`.
      - **Prompt:** stdin mode delivers the rendered prompt, and file mode passes a readable `BUZZ_ROUTER_PROMPT_FILE`. A `prompt_template` file is used when set.
      - **Stdout:** 200 KiB of output exits without blocking, and the captured text is at most 65 536 bytes.
      - **Outcomes:** `[no-reply]` and empty output give `passed`. Exit 3 with stdout gives `failed`, and nothing is posted. API mode with exit 0 and no post gives `passed`.
      - **Logging:** stderr lines appear in the captured `tracing` output.
      - **Files:** the payload file is 0600 on Unix and gone after the wake.
      - **Pass then linger:** `api-pass-then-sleep` is killed within 5 s of the pass.
    - *RED:* compile error, unresolved `buzz_router::adapter::command::CommandAdapter`.
    - *GREEN:* passes on all three OSes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **3.6 HTTP API and admin token**
  - **Objective:** Implement the loopback and tailnet routers, token and admin auth, the `/v1/post` precedence, the error bodies, body limits and the admin-token file.
  - **Files:** `crates/buzz-router/src/api/{mod.rs,token.rs,admin.rs,error.rs,admin_token.rs}`, and `crates/buzz-router/tests/{api.rs,engine_max_posts.rs}`. Dev-dependencies: `tower = { version = "0.5", features = ["util"] }` and `http-body-util = "0.1"`.
  - **Design:** §8, DD-22.
  - **Requirements:** R1.3, R1.4, R27.3, R28, R30.5, R38.3, R38.4, R42, R43, R65.5 (410), R65.7; A13.
  - **Depends on:** 3.4.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** The §8 table exactly. The admin control routes call into the core; their effects are tested in task 3.7.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test api --test engine_max_posts`
    - *Behaviour (token routes):*
      - An unknown or missing token gets 401 `{"error":"unauthorized"}`.
      - A halted bot gets 423, even when the wake has ended. An ended wake gets 410, including after the deadline (R65.5). With the default limit, the 4th post gets 429 (R65.7).
      - A pass after a 429 still gets 200.
      - A successful post gets 200 `{"event_id"}`, and the event reaches `FakeRelay`.
      - A pass ends the wake as passed. An ETA is used by the status note.
    - *Behaviour (admin and binds):*
      - `/v1/status` without the admin token gets 401.
      - On the tailnet router, `/v1/status` and `/v1/stop` get 404 and token routes work.
      - With `api_bind` omitted, the resolved bind is `127.0.0.1:47821`.
      - A body over 70 KiB, or text over 64 KiB, gets 400.
      - `admin_token::ensure` creates 64 hex characters with mode 0600 on Unix, and doesn't overwrite an existing file.
    - *RED:* compile error, unresolved `buzz_router::api::loopback_router`.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **3.7 Control execution**
  - **Objective:** Execute stop, resume and cancel from Buzz messages and from the admin API, including halt rows, kills, queue drops, reactions and partial resume.
  - **Files:** `crates/buzz-router/src/core/control.rs`, `crates/buzz-router/tests/engine_control.rs`.
  - **Design:** §6.7, DD-15.
  - **Requirements:** R30.1–R30.3, R31.1, R31.2, R32.1, R32.2, R33.2, R36.4, R43.1; A4.
  - **Depends on:** 3.4, 3.6.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** Exactly §6.7.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test engine_control`
    - *Behaviour (stop):*
      - A message `Stop(All)` creates the halt row `'all'` with `set_by_event` = the message id.
      - The running `FakeAdapter` wake is cancelled, its state is `killed`, and 🛑 appears on its reaction target.
      - Queued wakes become `killed` with `{"dropped":true}` and no reaction.
      - Each local bot reacts 🛑 on the stop message.
      - Afterwards, an owner mention gives `Suppress(Halted)`, and an API post gets 423.
      - `Stop(A)` creates the row `'A'` only.
    - *Behaviour (resume and cancel):*
      - `Resume(All)` deletes every row, and each bot reacts ▶️.
      - After `Stop(All)`, `Resume(A)` leaves one row per other roster bot, and A is no longer halted (A4).
      - `Cancel(A)` kills and drops, reacts 🛑 on the cancel message, writes no halt row, and the next mention wakes A.
    - *Behaviour (admin API):* admin stop, resume and cancel have the same effects with no reactions, and `set_by_event = "admin-api"`.
    - *Behaviour (persistence):* halts survive reopening the store.
    - *RED:* compile error, unresolved `buzz_router::core::control`; halt assertions fail.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **3.8 Stop end to end with real processes; CLI control and fallback**
  - **Objective:** Prove the real process-group kill on all three OSes. Implement `stop`, `resume` and `cancel` on the CLI, with the stop fallback when the API is unreachable.
  - **Files:** `crates/buzz-router/src/cli/control.rs`, and `crates/buzz-router/tests/{engine_stop_kill.rs,cli_control.rs,cli_stop_fallback.rs}`.
  - **Design:** §6.7 (CLI fallback), §7.1, §16.2.
  - **Requirements:** R33.1, R33.3, R33.4, R37.1, R65.4.
  - **Depends on:** 3.5, 3.7.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** The fallback is triggered by connection refused or a 2 s timeout. Unix uses `killpg(pid, SIGKILL)`, and Windows uses `taskkill /PID <pid> /T /F`.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test engine_stop_kill --test cli_control --test cli_stop_fallback`
    - *Behaviour:*
      - **R65.4:** a real `CommandAdapter` runs `buzz-router-test-agent spawn-grandchild <pidfile>`. After a stop, both pids are gone within 5 s (`pid_alive`), on macOS, Linux and Windows, and posts after the stop get 423.
      - **CLI with an API:** with an in-process API server, `stop`, `resume` and `cancel` with and without `--bot` send the right admin bodies and exit 0.
      - **CLI fallback:** with no daemon, a seeded `running` wake and a real test-agent process tree whose pid is in `wakes/<id>/pid`, `stop --bot A` writes the halt row `'A'` with `set_by_event = "cli"`, kills the tree, and exits 0.
    - *RED:* the CLI control commands return "not implemented"; the kill test fails because the pid file isn't written yet, or because the processes survive.
    - *GREEN:* passes on all three OSes in CI.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **3.9 Agent CLI: `post`, `pass`, `eta`**
  - **Objective:** Implement the agent-side commands that call the wake-token API.
  - **Files:** `crates/buzz-router/src/cli/agent.rs`, `crates/buzz-router/tests/cli_agent.rs`.
  - **Design:** §12.1, §14.
  - **Requirements:** R53.1–R53.3, R57.
  - **Depends on:** 3.6.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. The CLI reads `BUZZ_ROUTER_URL` and `BUZZ_ROUTER_WAKE_TOKEN`.
    2. **Exit-code mapping:**
       - connection failure gives `Network`, exit 2;
       - 401 gives `Auth`, exit 3;
       - 400 gives `BadInput`, exit 1;
       - 410, 423, 429 and 502 give `Other`, exit 4, with the API error code in `message`.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test cli_agent`
    - *Behaviour:* against an in-process API with a running `FakeAdapter` wake:
      - `post --text hi` prints `{"event_id":…}` and exits 0.
      - `post --text-file -` reads stdin, and `post --text-file <path>` reads the file.
      - `pass` and `eta --text "about 10 minutes"` work.
      - Missing env vars give exit 1, `user_error`.
      - A halted bot gives exit 4 with `"halted"` in the message.
    - *RED:* the commands return "not implemented".
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **3.10 `run` wiring**
  - **Objective:** Wire the daemon startup (§6.2 steps 1–5 and 7): rustls provider, config, store, admin token, keys, halts, core, ingest, API listeners and relay connections.
  - **Files:** `crates/buzz-router/src/cli/run.rs`, wiring in `crates/buzz-router/src/core/mod.rs`, `crates/buzz-router/tests/cli_run.rs`.
  - **Design:** §6.2, DD-23, DD-24.
  - **Requirements:** R1.1, R1.4, R1.15, R3.5, R43.4, R52.1, R62.6.
  - **Depends on:** 3.2–3.9, 2.5.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. Startup order as §6.2. Recovery (step 6) and missed messages (step 8) come in task 4.1.
    2. A single process serves every configured bot.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test cli_run`
    - *Behaviour:*
      - An invalid roster gives exit 1 with one JSON error line, and nothing listens on the configured `api_bind`.
      - **Valid config** against the mock WebSocket relay from the 2.4 helpers:
        - `POST /v1/pass` without a token gets 401;
        - `admin.token` exists with mode 0600 on Unix;
        - with `tailnet_bind` set to a second loopback port, token routes answer there and `/v1/status` gets 404;
        - a bot whose key file is missing is reported unavailable while the other bot connects;
        - the process exits 0 on ctrl-c or SIGTERM.
    - *RED:* `run` returns "not implemented".
    - *GREEN:* passes on all three OSes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

## Milestone 4: state and recovery (brief §17.4)

- [ ] **4.1 Startup recovery and missed messages**
  - **Objective:** Implement recovery of interrupted wakes (§6.2 step 6), the rule for missed owner messages older than 24 hours (step 8), and halts loaded before connecting.
  - **Files:** recovery in `crates/buzz-router/src/core/mod.rs` and ordering in `src/cli/run.rs`; `crates/buzz-router/tests/{engine_restart.rs,engine_missed.rs}`.
  - **Design:** §6.2, DD-14, DA-2.
  - **Requirements:** R36.7, R48.1 (order), R48.3, R49, R65.8.
  - **Depends on:** 3.10.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** Exactly §6.2 steps 5, 6 and 8.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test engine_restart --test engine_missed`
    - *Behaviour (restart):*
      - **R65.8:** a `running` wake with an owner trigger, attempt 1 and no posts becomes `interrupted` after a restart on the same store. A new wake with attempt 2 is dispatched exactly once.
      - A running wake with a post since `started_at` becomes `interrupted` with ⚠️ and no re-queue. So does an attempt-2 wake, and a wake whose bot is halted.
      - The first snapshot after startup already sees the stored halts.
    - *Behaviour (missed):*
      - A backfilled owner mention 25 hours old wakes nobody, appears in `missed`, and still applies its new round.
      - A backfilled "stop" 25 hours old writes its halt (DA-2).
      - A backfilled mention 23 hours old wakes.
    - *RED:* recovery isn't implemented, so `running` rows stay `running` and old owner messages wake.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **4.2 Unmanaged-post detection**
  - **Objective:** Flag kind-9 events signed by a local bot key that the router didn't publish, without ever flagging its own echoes.
  - **Files:** the unmanaged step in `crates/buzz-router/src/core/apply.rs`; `crates/buzz-router/tests/engine_unmanaged.rs`.
  - **Design:** §6.3 (core step 2), DD-6, DD-12.
  - **Requirements:** R44.7, R51, R65.9.
  - **Depends on:** 3.2, 2.7.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** The warning is rate-limited to once per bot per hour.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test engine_unmanaged`
    - *Behaviour:*
      - **R65.9:** a kind-9 event signed by bot A's key and not in `posts` gets ⚠️ from A, and the counter becomes 1.
      - A router-published reply echoed back by `FakeRelay` *during* publish is not flagged.
      - Two unmanaged posts 10 minutes apart produce one warning; a third after 61 minutes produces a second warning.
      - A kind-40003 event from a bot key is not flagged.
    - *RED:* no ⚠️ is published.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **4.3 `status` and `wakes`**
  - **Objective:** Implement the status document (§12.2) behind `GET /v1/status` and `status [--json]`, and `wakes`, which reads SQLite directly.
  - **Files:** `crates/buzz-router/src/core/status.rs`, status in `src/api/admin.rs`, `src/cli/{status.rs,wakes.rs}`, and `crates/buzz-router/tests/{api_status.rs,cli_status.rs,cli_wakes.rs}`.
  - **Design:** §12.1, §12.2, DD-12.
  - **Requirements:** R2.16, R20.4, R21.4, R48.3 (`missed` shown), R51.1 (count shown), R52.2, R52.3, R53.4.
  - **Depends on:** 3.6, 4.1, 4.2.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** The JSON exactly as §12.2.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test api_status --test cli_status --test cli_wakes`
    - *Behaviour:*
      - The status JSON has `bots` (`available`, `connected`, `halted`, `running`, `queued`, hourly and daily budget used and limit, `suppressed_since_start`), `halts`, `missed` and `unmanaged_posts`. `roster_hash` equals the `roster check` output, and `version` equals `CARGO_PKG_VERSION`.
      - `status --json` prints exactly the body of `GET /v1/status`.
      - With the daemon down, `status` gives exit 2, `network_error`, and `wakes --bot A --state queued` still lists rows from SQLite.
    - *RED:* the commands and route return "not implemented" or 501.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **4.4 Logging: file output, rate limiting and redaction**
  - **Objective:** Complete `logging.rs`: the JSON file layer with daily rotation keeping 14 files, `BUZZ_ROUTER_LOG`, `LogLimiter`, and redaction of secrets.
  - **Files:** `crates/buzz-router/src/logging.rs`, `crates/buzz-router/tests/logging.rs`.
  - **Design:** §14, §2.
  - **Requirements:** R4.4 (log once), R20.4, R21.4, R51.2, R59.4.
  - **Depends on:** 1.11, 3.5.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** No wake token, nsec, admin token or webhook secret ever appears in any log line.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test logging`
    - *Behaviour:*
      - `LogLimiter` allows once per key, and once per key per hour (virtual time).
      - A JSON log file appears under `<data-dir>/logs/`.
      - `BUZZ_ROUTER_LOG=debug` enables debug output.
      - **Redaction:** after a `FakeAdapter` wake and a run with a file key, the captured logs contain neither the wake token, the test nsec, nor the admin token.
    - *RED:* no file layer exists, so `LogLimiter` is unresolved.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

## Milestone 5: webhook adapter (brief §17.5)

- [ ] **5.1 Webhook adapter (async and sync)**
  - **Objective:** Implement `WebhookAdapter`: HMAC signing, async with a 10 s timeout, sync until the deadline, failure mapping, and `cancel_url`.
  - **Files:** `crates/buzz-router/src/adapter/webhook.rs`, `crates/buzz-router/tests/adapter_webhook.rs`.
  - **Design:** §7.2, DD-16.
  - **Requirements:** R30.1 (webhook branch), R36.6, R38.1, R38.2, R39.1–R39.3; A13.
  - **Depends on:** 3.4.
  - **Agents:** `test-dev` (RED, with an axum test server); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** Exactly §7.2.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test adapter_webhook`
    - *Behaviour (signing):*
      - The server verifies `X-Buzz-Router-Signature: sha256=<hex HMAC-SHA256(secret, raw body)>` and `X-Buzz-Router-Wake`.
      - The secret is read from the env var named by `secret_env` at wake time. If that var is unset, the result is `Failed`.
    - *Behaviour (modes):*
      - **async:** a 2xx gives `AsyncAccepted`; a 500 gives `Failed`; no response within 10 s (paused time) gives `Failed`.
      - **sync:** `{"text":"hi"}` gives `Text`; `{"pass":true}` gives `Pass`; `{"foo":1}` gives `Failed`; a non-2xx gives `Failed`; cancellation drops the request.
    - *Behaviour (cancel):* `cancel_url` receives a POST with `{"wake_id"}` and the signature headers, and its errors are logged.
    - *RED:* compile error, unresolved `buzz_router::adapter::webhook::WebhookAdapter`.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **5.2 Webhook wakes through the engine and the tailnet API**
  - **Objective:** Select adapters per bot, use `public_url` as the payload's API URL, end async wakes at the first post or pass, and stop webhook wakes.
  - **Files:** adapter selection in `crates/buzz-router/src/core/dispatch.rs`; `crates/buzz-router/tests/engine_webhook.rs`.
  - **Design:** §6.6 (endings), §7.2, §8, A6.
  - **Requirements:** R1.5, R30.1, R38.2–R38.4, R39.2, R39.3, R40.6; A6.
  - **Depends on:** 5.1, 3.6, 3.7.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:** Exactly as cited.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test engine_webhook`
    - *Behaviour (async):*
      - The payload's `api.url` equals `public_url`.
      - `POST <tailnet>/v1/post` with the Bearer token publishes the reply and ends the wake as `posted`; a second post gets 410.
      - A pass gives `passed`, with ✅ for an owner Direct wake.
      - A stop during an async wake makes posts get 423 and calls `cancel_url`.
    - *Behaviour (sync):* a sync text reply is published under the reaction target. A sync wake that hits its deadline gives `timeout` and ⌛.
    - *RED:* webhook bots aren't dispatched to `WebhookAdapter`.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

## Milestone 6: packaging, CI and release (brief §17.6)

- [x] **6.1 OS keychain keys and the `keys` commands**
  - **Objective:** Implement `KeySource::Keychain`, `keys set` and `keys check`.
  - **Files:** `crates/buzz-router/src/keys.rs`, `crates/buzz-router/src/cli/keys.rs`, `crates/buzz-router/tests/{keys_keychain.rs,cli_keys.rs}`.
  - **Design:** §11, DD-20.
  - **Requirements:** R54, R59.1; A16.
  - **Depends on:** 2.2, 1.11.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. The keychain entry is service `buzz-router` with the bot name as account.
    2. `keys set` loads no configuration.
    3. `keys check` exits 3 (`key_error`) on any failure.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test keys_keychain --test cli_keys`
    - *Behaviour (in process, with keyring's mock store):*
      - A keychain source reads the entry for (`buzz-router`, `A`).
      - `cli::keys::set` stores a valid nsec read from a reader.
      - `cli::keys::check` passes when the keys match the roster pubkeys; a mismatch or a missing entry gives exit code 3.
    - *Behaviour (process):* `keys set --bot A` with an invalid nsec on stdin and an empty `--config-dir` exits 1 with a JSON error, without needing any configuration.
    - *RED:* `Keychain` returns `Unsupported`, and the commands are not implemented.
    - *GREEN:* passes.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR

- [x] **6.2 Service installation**
  - **Objective:** Render and install the per-OS service definitions through a `CommandRunner`.
  - **Files:** `crates/buzz-router/src/service/{mod.rs,macos.rs,linux.rs,windows.rs}`, `crates/buzz-router/src/cli/service.rs`, `crates/buzz-router/tests/service_defs.rs`.
  - **Design:** §13, DD-8.
  - **Requirements:** R56.
  - **Depends on:** 1.11.
  - **Agents:** `test-dev` (RED); `tauri-dev` (GREEN, REFACTOR).
  - **Acceptance criteria:**
    1. The renderers are pure functions, tested on every OS. The OS commands go through `CommandRunner`.
    2. CI does not perform a real install, because that would modify the runner. Real installs are verified manually on each OS during the cutover smoke test, with evidence recorded. On Windows, that check confirms a non-zero exit triggers `RestartOnFailure`.
  - **Test contract:**
    - *Run:* `cargo test -p buzz-router --test service_defs`
    - *Behaviour (renderers):*
      - The plist has `Label com.buzz-router`, `ProgramArguments [exe, run]`, `RunAtLoad`, `KeepAlive true` and log paths.
      - The systemd unit has `ExecStart=<exe> run`, `Restart=always` and `WantedBy=default.target`.
      - The task XML has `LogonTrigger`, `InteractiveToken`, `LeastPrivilege`, `RestartOnFailure PT1M/999`, `ExecutionTimeLimit PT0S` and `Exec <exe> run`.
    - *Behaviour (commands, recorded by a fake `CommandRunner`):*
      - `install` runs `launchctl bootstrap gui/<uid> <plist>`, or `systemctl --user daemon-reload` then `enable --now buzz-router.service`, or `schtasks /Create /TN buzz-router /XML <f> /F` then `/Run`.
      - `uninstall` reverses each of these.
      - `status` runs the §13 query command.
    - *RED:* compile error, unresolved `buzz_router::service`.
    - *GREEN:* passes on all three OSes.
  - [x] RED evidence recorded
  - [x] GREEN
  - [x] REFACTOR

- [ ] **6.3 CI: full matrix and build checks** *(no-test task)*
  - **Objective:** Extend `ci.yml` to the full matrix and add the build-property checks.
  - **Files:** `.github/workflows/ci.yml`.
  - **Design:** §15.
  - **Requirements:** R5.9, R58.2, R58.3, R60.1, R64.2, R65.1.
  - **Depends on:** 1.1, and the M1–M6 test suites.
  - **Agent:** `github-devops`.
  - **Acceptance criteria:**
    1. The test matrix (macOS, Linux, Windows) runs clippy `-D warnings`, `cargo test --workspace --locked` and `cargo build --release --locked -p buzz-router --bin buzz-router`.
    2. `cargo tree -p buzz-router -e normal -i openssl-sys` must print nothing (R58.2).
    3. `cargo tree -p buzz-router -e features -i libsqlite3-sys` must show `bundled` (R58.2).
    4. An MSRV job runs `cargo +1.88 check --workspace --locked` (R58.3).
    5. The router-core tokio guard is kept (R5.9).
    6. `docs-gate.yml` is unchanged.
  - **Replacement validation:** a green workflow run on all three OSes, with the run URL recorded in the evidence.
  - [ ] Workflow updated
  - [ ] Green run recorded

- [ ] **6.4 Release workflow** *(no-test task)*
  - **Objective:** Build and attach the five release binaries.
  - **Files:** `.github/workflows/release.yml`.
  - **Design:** §15.
  - **Requirements:** R58.1, R60.2.
  - **Depends on:** 6.3.
  - **Agent:** `github-devops`.
  - **Acceptance criteria:**
    1. Triggers on `v*` tags and `workflow_dispatch`.
    2. Builds the five target and runner pairs in §15 with `cargo build --release --locked --target <t> -p buzz-router --bin buzz-router`, so the test agent is never built.
    3. Packages `buzz-router-<version>-<target>.tar.gz`, or `.zip` for Windows.
    4. Uploads with `gh release upload`.
  - **Replacement validation:** a `workflow_dispatch` dry run producing the five artifacts. Record the run URL and the artifact names.
  - [ ] Workflow written
  - [ ] Dry run recorded

## Milestone 7: end-to-end acceptance (brief §15.3)

All tasks here are **[Docker + local Buzz relay]**. They are acceptance verification of behaviour already delivered. RED is the scenario test not yet existing or failing. A product defect found here is fixed under its own RED → GREEN, with the failing scenario as the RED evidence.

- [ ] **7.1 E2E harness [Docker + local Buzz relay]**
  - **Objective:** Extend `tests/e2e_support/mod.rs` into the full §16.4 harness.
    - Per-run throwaway identities O, A, B and C. Channel provisioning.
    - `roster.toml` and `router.toml` written to temporary dirs, with `file:` keys at mode 0600 and command adapters running `buzz-router-test-agent echo --delay N`.
    - Helpers to start, stop and kill the router child process (`Child::kill`). Polling helpers for reactions and replies. An owner publish helper.
  - **Files:** `crates/buzz-router/tests/e2e_support/mod.rs`, `crates/buzz-router/tests/e2e_harness_smoke.rs`.
  - **Design:** §16.4.
  - **Requirements:** R66.1.
  - **Depends on:** Milestones 1–6, 2.9.
  - **Agents:** `test-dev`; `tauri-dev` for product fixes.
  - **Acceptance criteria:** Never touches the live relay or real keys. The relay setup is as in task 2.9.
  - **Test contract:**
    - *Run:* `BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=ws://127.0.0.1:3000 cargo test -p buzz-router --test e2e_harness_smoke -- --test-threads=1`
    - *Behaviour:* the router starts; `status` shows all three bots connected; O posting "@A ping" gets 👀 and a reply from A.
    - *RED:* the harness module doesn't exist yet.
    - *GREEN:* passes.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **7.2 E1 and E4 [Docker + local Buzz relay]**
  - **Objective:** Automate acceptance scenarios E1 and E4.
  - **Files:** `crates/buzz-router/tests/{e2e_thread_reply.rs,e2e_status_note.rs}`.
  - **Design:** §16.4.
  - **Requirements:** R66.2, R66.5.
  - **Depends on:** 7.1.
  - **Agents:** `test-dev`; `tauri-dev` for product fixes.
  - **Acceptance criteria:** Each scenario test asserts exactly its brief pass condition.
  - **Test contract:**
    - *Run:* `BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=… cargo test -p buzz-router --test e2e_thread_reply --test e2e_status_note -- --test-threads=1`
    - *Behaviour:*
      - **E1:** O posts "@A", A replies, then O replies untagged in the thread. A is woken and replies.
      - **E4:** O posts "@A" with a 60 s task. 👀 appears immediately, one status note at about 20 s, and the final reply is threaded under O's message.
    - *RED:* the scenario files don't exist yet.
    - *GREEN:* both pass.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **7.3 E2 and E3 [Docker + local Buzz relay]**
  - **Objective:** Automate acceptance scenarios E2 and E3.
  - **Files:** `crates/buzz-router/tests/{e2e_everyone.rs,e2e_stop.rs}`.
  - **Design:** §16.4.
  - **Requirements:** R66.3, R66.4.
  - **Depends on:** 7.1.
  - **Agents:** `test-dev`; `tauri-dev` for product fixes.
  - **Acceptance criteria:** Each scenario test asserts exactly its brief pass condition.
  - **Test contract:**
    - *Run:* `BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=… cargo test -p buzz-router --test e2e_everyone --test e2e_stop -- --test-threads=1`
    - *Behaviour:*
      - **E2:** "@everyone" gets 👀 from A, B and C within 5 s; no bot exceeds 4 wakes; the thread goes quiet.
      - **E3:** "stop" during E2 leaves no agent process within 5 s; 🛑 from each bot; nothing is published afterwards; the halt survives a router restart; "resume" brings ▶️ and normal routing.
    - *RED:* the scenario files don't exist yet.
    - *GREEN:* both pass.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **7.4 E5 and E6 [Docker + local Buzz relay]**
  - **Objective:** Automate acceptance scenarios E5 and E6.
  - **Files:** `crates/buzz-router/tests/{e2e_crash_recovery.rs,e2e_unmanaged.rs}`.
  - **Design:** §16.4.
  - **Requirements:** R66.6, R66.7.
  - **Depends on:** 7.1.
  - **Agents:** `test-dev`; `tauri-dev` for product fixes.
  - **Acceptance criteria:** Each scenario test asserts exactly its brief pass condition.
  - **Test contract:**
    - *Run:* `BUZZ_E2E=1 BUZZ_E2E_RELAY_URL=… cargo test -p buzz-router --test e2e_crash_recovery --test e2e_unmanaged -- --test-threads=1`
    - *Behaviour:*
      - **E5:** kill the router child mid-wake, O posts while it's down, then restart. The interrupted wake is re-run exactly once, the message sent during the downtime is answered, and there are no duplicate replies.
      - **E6:** publishing directly with bot A's key gets ⚠️ on that post, and `status` shows `unmanaged_posts: 1`.
    - *RED:* the scenario files don't exist yet.
    - *GREEN:* both pass.
  - [ ] RED evidence recorded
  - [ ] GREEN
  - [ ] REFACTOR

- [ ] **7.5 E7: the E2E CI job [Docker + local Buzz relay]** *(no-test task)*
  - **Objective:** Add the `e2e` job to `ci.yml` and make the whole matrix green (E7).
  - **Files:** `.github/workflows/ci.yml`.
  - **Design:** §15, DA-1.
  - **Requirements:** R60.1, R66.8.
  - **Depends on:** 7.2, 7.3, 7.4, 6.3.
  - **Agent:** `github-devops`.
  - **Acceptance criteria:**
    1. The `e2e` job on ubuntu-latest:
       1. checks out Buzz at the pinned rev;
       2. runs `docker compose up -d postgres redis`;
       3. builds and runs `buzz-relay` with `.env.example` and migrations, plus the test-identity auth settings recorded in task 2.9;
       4. runs `BUZZ_E2E=1 cargo test -p buzz-router --test 'e2e_*' -- --test-threads=1`.
    2. Native macOS and Windows E2E legs are added with `continue-on-error: true` until David decides DA-1.
  - **Replacement validation:** a green `e2e` job plus a green three-OS test matrix on the same commit, with the run URL recorded.
  - [ ] Workflow updated
  - [ ] Green run recorded

## Milestone 8: documentation and closeout

- [ ] **8.1 Cutover runbook** *(no-test task)*
  - **Objective:** Write the operator cutover runbook.
  - **Files:** `docs/runbooks/buzz-router-cutover.md`, with frontmatter `doc-type: runbook`, `status: draft`, `component: buzz-router` and `owner: dpalfery`. Link it from the documentation index if the `kyber-weave-docs` skill requires that.
  - **Design:** §17.
  - **Requirements:** R29.8, R63.1–R63.11.
  - **Depends on:** 4.3, 6.1, 6.2.
  - **Agent:** `docs-dev`.
  - **Acceptance criteria:** One section per R63.2–R63.11 item, plus:
    - the stop-phrasing note (R29.8);
    - the CLI stop fallback for stops a router can't see (A5);
    - the note that status counters reset on restart (DD-12);
    - `keys check` usage.
  - **Replacement validation:**
    - `kyber-weave docs validate .` reports 0 critical, 0 error, 0 warning and 0 info;
    - a checklist in the review notes mapping each R63 criterion and R29.8 to its runbook section.
  - [ ] Runbook written
  - [ ] Validation recorded

- [ ] **8.2 Specification closeout**
  - **Objective:** Retire the specification after delivery, following the product-owner closeout procedure.
  - **Steps:**
    1. Confirm the preconditions: every task above is complete with task-review passes; the test contracts have current passing evidence; every requirement R1–R66 traces to code and tests; the end-of-run `code-reviewer` verdict is `APPROVE`.
    2. Verify each requirement against the implementation, tests and review evidence.
    3. Migrate the durable architecture, behaviour, configuration, interfaces and operations into canonical docs:
       - `docs/system/architecture.md` (adding `source-root` and `code-refs`);
       - component and reference docs under the Config Reg paths;
       - ADRs only where the repository's ADR criteria are met.
    4. Move buzz-router-v1 from Active to the Archive register in `docs/specs/README.md`, naming the replacing documents.
    5. Archive the specification folder to `docs/archive/specs/buzz-router-v1/`.
    6. Run `kyber-weave docs validate .` and the drift checks.
  - **Requirements:** all, R1–R66 (verification).
  - **Depends on:** every task above, and the final council approval.
  - **Agent:** `docs-dev`, assigned through the conductor.
  - **Replacement validation:** the closeout digest `STATUS: ARCHIVED`, with the requirements verified (66 of 66) and a clean validation run.
  - [ ] Closeout complete

## Coverage

### Tasks per milestone

| Milestone | Tasks | Count |
|---|---|---|
| 1. router-core and conformance | 1.1–1.11 | 11 |
| 2. Relay I/O | 2.1–2.9 | 9 |
| 3. Wake engine plus stop | 3.1–3.10 | 10 |
| 4. State and recovery | 4.1–4.4 | 4 |
| 5. Webhook adapter | 5.1–5.2 | 2 |
| 6. Packaging, CI and release | 6.1–6.4 | 4 |
| 7. End-to-end acceptance | 7.1–7.5 | 5 |
| 8. Documentation and closeout | 8.1–8.2 | 2 |
| **Total** | | **47** |

### Requirements to tasks

| Req | Tasks | Req | Tasks | Req | Tasks |
|---|---|---|---|---|---|
| 1 | 1.2, 3.6, 3.10, 5.2 | 23 | 3.3 | 45 | 2.7, 3.4 |
| 2 | 1.2, 1.11, 4.3 | 24 | 3.3 | 46 | 2.7, 3.4 |
| 3 | 1.11, 3.10 | 25 | 3.3 | 47 | 2.1, 2.5, 3.2 |
| 4 | 1.3, 1.8, 2.6, 4.4 | 26 | 3.3 | 48 | 2.3, 2.5, 4.1, 4.3 |
| 5 | 1.1, 1.3, 1.6, 3.2, 6.3 | 27 | 3.4, 3.6 | 49 | 4.1 |
| 6 | 1.6, 1.8 | 28 | 3.6 | 50 | 2.4, 2.5, 2.9 |
| 7 | 1.4, 1.6 | 29 | 1.4, 1.9, 8.1 | 51 | 4.2, 4.3, 4.4 |
| 8 | 1.3, 1.6 | 30 | 1.8, 2.1, 2.7, 3.6, 3.7, 5.1, 5.2 | 52 | 3.10, 4.3 |
| 9 | 1.6 | 31 | 1.9, 3.7 | 53 | 3.9, 4.3 |
| 10 | 1.6 | 32 | 1.9, 3.7 | 54 | 6.1 |
| 11 | 1.7 | 33 | 3.7, 3.8 | 55 | 1.10, 1.11, 2.8 |
| 12 | 1.5, 1.7 | 34 | 2.1, 3.4 | 56 | 6.2 |
| 13 | 1.7 | 35 | 2.7, 3.4, 2.9 | 57 | 1.11, 2.8, 3.9 |
| 14 | 1.8 | 36 | 3.4, 3.5, 4.1, 5.1 | 58 | 1.1, 2.1, 6.3, 6.4 |
| 15 | 1.8 | 37 | 3.5, 3.8 | 59 | 2.2, 2.7, 3.5, 4.4, 6.1 |
| 16 | 1.8, 2.6 | 38 | 3.6, 5.1, 5.2 | 60 | 1.1, 6.3, 6.4, 7.5 |
| 17 | 1.4, 1.6 | 39 | 5.1, 5.2 | 61 | 2.3, 2.4, 2.5, 2.6, 2.9 |
| 18 | 1.3, 2.6, 3.2 | 40 | 3.1, 3.4, 3.5, 5.2 | 62 | 1.1, 1.3, 1.11, 3.10 |
| 19 | 1.5, 1.7, 3.2, 3.4 | 41 | 3.1 | 63 | 8.1 |
| 20 | 1.5, 1.7, 1.8, 3.2, 4.3, 4.4 | 42 | 3.6 | 64 | 1.6, 1.7, 1.8, 1.9, 6.3 |
| 21 | 1.5, 1.7, 1.8, 3.2, 4.3, 4.4 | 43 | 3.6, 3.7, 3.10, 4.3 | 65 | 3.2, 3.3, 3.4, 3.6, 3.8, 4.1, 4.2, 6.3 |
| 22 | 1.2, 1.5, 1.7, 1.8 | 44 | 1.4, 2.7, 2.9, 4.2 | 66 | 7.1, 7.2, 7.3, 7.4, 7.5 |

Every requirement from 1 to 66 is covered. Task 8.2 re-verifies all of them at closeout.

### Conformance cases to tasks

| Case | Task | Case | Task | Case | Task | Case | Task |
|---|---|---|---|---|---|---|---|
| 1 | 1.6 | 11 | 1.7 (⏸️ once: 3.2) | 21 | 1.9 | 31 | 1.6 |
| 2 | 1.6 | 12 | 1.7 | 22 | 1.9 | 32 | 1.6 |
| 3 | 1.6 | 13 | 1.7 | 23 | 1.8 | 33 | 1.6 |
| 4 | 1.6 | 14 | 1.7 | 24 | 1.7 | 34 | 1.6 |
| 5 | 1.6 | 15 | 1.7 | 25 | 1.8 | 35 | 1.8 |
| 6 | 1.6 | 16 | 1.7 | 26 | 1.8 | 36 | 1.6 |
| 7 | 1.6 | 17 | 1.9 | 27 | 1.7 | 37 | 1.8 |
| 8 | 1.6 | 18 | 1.9 | 28 | 1.8 | | |
| 9 | 1.6 | 19 | 1.9 | 29 | 1.8 | | |
| 10 | 1.7 | 20 | 1.9 | 30 | 1.8 | | |

Extra fixtures (R64.3):

| Task | Fixtures |
|---|---|
| 1.6 | 106, 107, 108, 111, 115 |
| 1.7 | 103, 109, 119 |
| 1.8 | 101, 102, 104, 105, 110, 112, 116, 117, 118, 120 |
| 1.9 | 113, 114, 121 |

All of these extras, 101–121, are listed in design §16.1.

### Engine tests (brief §15.2, R65) and acceptance scenarios (brief §15.3, R66)

| Test | Task | Test file |
|---|---|---|
| Fake clock and fake adapters, on three OSes (R65.1) | 3.2, 6.3 | `tests/support/mod.rs`, `ci.yml` |
| Debounce (R65.2) | 3.3 | `engine_debounce.rs` |
| Coalescing (R65.3) | 3.3 | `engine_coalesce.rs` |
| Stop kills the grandchild; then 423 (R65.4) | 3.8 | `engine_stop_kill.rs` |
| Deadline ⌛ and 410 (R65.5) | 3.4, 3.6 | `engine_deadline.rs`, `api.rs` |
| Status note (R65.6) | 3.4 | `engine_status_note.rs` |
| 4th post gets 429 (R65.7) | 3.6 | `engine_max_posts.rs` |
| Restart: interrupted and re-queued (R65.8) | 4.1 | `engine_restart.rs` |
| Unmanaged post gets ⚠️ (R65.9) | 4.2 | `engine_unmanaged.rs` |
| E1 | 7.2 | `e2e_thread_reply.rs` |
| E2 | 7.3 | `e2e_everyone.rs` |
| E3 | 7.3 | `e2e_stop.rs` |
| E4 | 7.2 | `e2e_status_note.rs` |
| E5 | 7.4 | `e2e_crash_recovery.rs` |
| E6 | 7.4 | `e2e_unmanaged.rs` |
| E7 | 7.5 | `ci.yml` `e2e` job and the matrix |

### Docker-dependent tasks

These need Docker and a local Buzz relay. Record them as Blocked if Docker is unavailable:

- **2.9** relay I/O integration;
- **7.1** E2E harness;
- **7.2** E1 and E4;
- **7.3** E2 and E3;
- **7.4** E5 and E6;
- **7.5** the E2E CI job. This one needs Docker on the GitHub runner, not locally.
