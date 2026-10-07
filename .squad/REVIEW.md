## Verdict: REQUEST_CHANGES (risk: HIGH)

No review gates or policy are declared in `.kyber-weave/kyber-weave.yml`, so I applied the code-review skill's rules by hand. One confirmed critical finding blocks the change, and the major findings are well past any blocking count. The diff is about 25,000 lines; with the skill's example `max-reviewable-lines` of 10,000 it would have escalated to NEEDS_HUMAN before any finding was weighed. I checked every critical and major finding below against the actual code.

## Gates (all pass)

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `cargo test --workspace --locked` | pass, 0 failed |
| `kyber-weave docs validate .` | pass, no findings |

The `e2e_*` tests return early and report `ok` unless `BUZZ_E2E=1` is set. So the e2e scenarios were not actually run here; only CI's ubuntu e2e job runs them.

## Repo rules (all clean)

- **router-core purity:** no I/O, tokio or clock reads. The only `std::net` use is the `SocketAddr` type in config.
- **No `unwrap`/`expect`/`panic!` outside tests:** every hit is inside a `#[cfg(test)]` module.
- **No secrets, real pubkeys or relay URLs:** all are placeholders. The only `ts.net` string is the prohibition text in `.squad/SQUAD.md`.
- **No live relay:** `e2e_support/mod.rs:46-49` refuses any relay URL that isn't `ws://127.0.0.1:` or `ws://localhost:`.
- **Buzz pin:** `buzz-core` and `buzz-sdk` are pinned to `f0eb557…`.
- **Action pinning:** every workflow action is pinned by full SHA.

## Findings

### Critical

1. **The relay task advances the backfill cursor before the core has processed the event.**
   - **Where:** `crates/buzz-router/src/relay/conn.rs:907-934` and `:949-960`, using the extra read-write store opened at `cli/run.rs:183`. Also `conn.rs:753-756`.
   - **Why it matters:** design §6.3 step 7 advances the cursor only after the apply transaction commits, and DD-1 makes the core the only writer. Two ways events are lost for good, breaking R47.4 and R49:
     - A crash while backfilled events are still queued restarts from `newest − 300 s`, so older events in that batch are never fetched again.
     - A channel whose backfill fails is skipped, and the next live event on any channel moves the shared cursor past the gap.
   - **Fix:** remove the cursor writes from `conn.rs`, give the relay task a read-only store (or have it ask the core for the cursor), and redial when a channel's backfill fails.

### Major

2. **A pass after a reply ends the wake as `passed`.**
   - **Where:** `core/dispatch.rs:399-408`.
   - **Why it matters:** design §6.6 ranks posted above passed. A bot that posts and then passes gets the wrong state and ✅.
   - **Fix:** pick `Posted` when `running.posts > 0` in both branches.
3. **The ⚠️ reaction for unmanaged posts is rate-limited along with the log line.**
   - **Where:** `core/apply.rs:563-570`, with `tests/engine_unmanaged.rs:93-105` asserting the wrong behaviour.
   - **Why it matters:** R51.1 requires ⚠️ on every unmanaged post; R51.2 only limits the log.
   - **Fix:** react every time, limit only the `warn!`, and expect three reactions in the test.
4. **The reconnect ladder resets one attempt late.**
   - **Where:** `relay/conn.rs:232-234` and `:563-567`.
   - **Why it matters:** after hours of stable connection, a drop still waits about 60 s instead of 1 s, which breaks task 2.4's "resets after 60 s stable".
   - **Fix:** reset the step before computing the delay.
5. **A failed channel discovery leaves the bot connected with no subscriptions.**
   - **Where:** `relay/conn.rs:638-641`.
   - **Why it matters:** status shows connected but no events arrive until the socket happens to drop.
   - **Fix:** treat it as a connection failure and redial.
6. **The REST and webhook HTTP clients have no timeout.**
   - **Where:** `relay/rest.rs:70` and `cli/run.rs:123`.
   - **Why it matters:** a relay that accepts the connection but never answers hangs discovery and backfill. The retry-on-timeout check at `rest.rs:187` can never fire.
   - **Fix:** build the client with `connect_timeout(5s)` and `timeout(10–15s)`. Leave the sync webhook path as design §7 specifies, where the core's deadline cancels it.
7. **Command agents keep running after the router shuts down.**
   - **Where:** `core/mod.rs:358`, `cli/run.rs:190-193`, and `adapter/command.rs:174`.
   - **Why it matters:** shutdown never cancels running wakes, and the spawn has no kill-on-drop, so agents outlive SIGTERM or a service stop.
   - **Fix:** on shutdown, cancel each running wake and wait a bounded time; add `kill_on_drop`. The spec says nothing about shutdown, so confirm the intended behaviour with David.
8. **The `stop` fallback hides a database error.**
   - **Where:** `cli/control.rs:126-128` (`.unwrap_or_default()`).
   - **Why it matters:** if the running wakes can't be read, it reports nothing killed and exits 0 while the agents keep running.
   - **Fix:** surface the error in the output and exit non-zero.
9. **Log retention only runs at startup.**
   - **Where:** `logging.rs:82-83`.
   - **Why it matters:** a long-running daemon breaks design §14's "keeping 14 files".
   - **Fix:** use `tracing_appender::rolling::Builder` with `max_log_files(14)`.
10. **Redaction is never applied in production.**
    - **Where:** `redact` is never called outside `logging.rs`. Agent stderr is logged as-is at `adapter/command.rs:277`.
    - **Why it matters:** an agent that echoes its environment writes `BUZZ_ROUTER_WAKE_TOKEN` to the log, though task 4.4 claims redaction.
    - **Fix:** redact the wake token in `log_lines`, and use `error.without_url()` for webhook errors.
11. **Two copies of the connection loop, and the tests cover the one that doesn't ship.**
    - **Where:** `relay/conn.rs:221-285` duplicates `:543-616`.
    - **Why it matters:** production uses `spawn_synced_connection`, but the auth, publish-ack and reconnect tests in `tests/relay_conn.rs` and `tests/e2e_relay_io.rs` drive `spawn_connection`. The copies have already drifted: the test-only one ignores a Close frame (`:393`), the shipped one redials (`:836`).
    - **Fix:** keep one path, with discovery and backfill switchable.
12. **Five parallel release jobs race to create the GitHub release.**
    - **Where:** `.github/workflows/release.yml:69-72`.
    - **Why it matters:** every matrix job runs "view, else create" with `contents: write`. One job can fail and its binary never gets attached (R60.2). A `workflow_dispatch` dry run discards its archives, so task 6.4 can't be closed.
    - **Fix:** builds upload with `actions/upload-artifact` under `contents: read`, and one tag-only `publish` job creates the release.
13. **The E3 stop scenario will time out, and two of its checks can't fail.**
    - **Where:** `tests/e2e_stop.rs:110-111`, `:79-95`, and `:41`.
    - **Why it matters:** the agent sleeps 30 s before replying, so the resume step's 30 s `WAIT` is too short. The two "nothing published" checks after a 10 s sleep would pass even if the halt were broken.
    - **Fix:** use `wait_replies_within(…, 60 s)` for the resume step, and assert no 👀 or new wake row instead of no reply.
14. **The E2 everyone scenario never proves the discussion runs.**
    - **Where:** `tests/e2e_everyone.rs:113-119`.
    - **Why it matters:** `(1..=4).contains(&rows)` still passes if discussion wakes are completely broken (one row per bot).
    - **Fix:** assert at least 2 rows per bot, or that wakes with reason `discussion` exist.

### Minor

15. **Eight routing rules have no conformance fixture, which breaks the AGENTS.md rule.** Missing:
    - the Halted gate on bot-caused targets (R12.4, R30.4);
    - the Halted gate for a human mentioning a `respond_to = anyone` bot (R14.3);
    - the Cap and Budget gates on human-caused wakes (R14.3, R19.2, R20.2, R21.2);
    - the Halted gate on owner-edit targets (R16.3);
    - an edit containing "stop" not being parsed as a command (R16.4, R29.7);
    - a foreign bot replying to a local bot gives Suppress(RespondTo) (R15.2);
    - `default_bot` applying only to owner messages (R10.3);
    - the participant rule when the parent isn't the root (R9.3).

    **Fix:** add one fixture per rule under `fixtures/conformance/`.
16. **`LogLimiter` is unused.** `logging.rs:119-151` is tested, but `core/apply.rs:134-136` and `:563-568` reimplement both limits by hand. **Fix:** route both warnings through `LogLimiter`, or delete it and update design §13.
17. **Requirement 17.4 still contradicts owner decision O4.** **Where:** `requirements.md:263`. **Fix:** amend the text to say a foreign bot's `p` tags count, with suppression only.
18. **The status text in `tasks.md` is stale.** **Where:** `tasks.md:21` and `:44-52` still say paused after milestone 1, with O2–O8 open. **Fix:** update the progress table and mark the decisions as decided.
19. **Squad coordination files are in the change.** **Where:** `.squad/` holds orchestration notes with local absolute paths. **Fix:** remove it before merge, or move O1–O8 into the spec or an ADR.
20. **The Windows task may not restart a crashed router.** **Where:** `service/windows.rs:42-45`. The XML matches design §13, but Task Scheduler's `RestartOnFailure` may not fire when a running process exits non-zero (R56.4). **Fix:** David to decide between a supervisor loop, a repeating trigger, or a documented limitation.
21. **The cutover runbook has no rollback section.** **Where:** `docs/runbooks/buzz-router-cutover.md`. R63 doesn't require one, so raise it with David.
22. **A pid-file write error leaves the agent running.** **Where:** `adapter/command.rs:176-179`. **Fix:** kill the child before returning the error.
23. **E2E skips look like passes.** Without `BUZZ_E2E` the tests report `ok`, and the macOS and Windows e2e legs are advisory (`continue-on-error`). **Fix:** mark them `#[ignore]` and run them with `--ignored` in the e2e job.

## Tasks unticked although their commit exists

Each of these has its sub-boxes ticked but its header line still `- [ ]`:

| `tasks.md` line | Task | Commit |
|---|---|---|
| 511 | 2.2 | `81fd83b` |
| 531 | 2.3 | `723d836` |
| 554 | 2.4 | `8de180a` |
| 622 | 2.7 | `eb464d7` |
| 673 | 2.9 | `537df2d` |
| 941 | 3.10 | `2ad5a78` |
| 970 | 4.1 | `3344f2c` |
| 1084 | 5.2 | `62ec11c` |
| 1277 | 7.5 | `51ce62a` |

Three sub-boxes are rightly still open because they wait on a CI run: 6.3 at line 1176, 6.4 at line 1192 and 7.5 at line 1293. Task 8.2 has no commit yet.

## Requirements with no covering test

- **No test at all:**
  - R4.4: drift is logged only once per author.
  - R19.4: a suppressed decision uses no turn.
  - R22.5: quiet-hours suppressions are dropped, not deferred.
  - R51.1: ⚠️ on every unmanaged post (the existing test asserts the opposite).
- **Tested only through fakes:**
  - R54.1 and R59.1: the keychain, through a mock.
  - R56.1–R56.4: service install, through a fake command runner.
  - R43.4: the Windows permissions half.
- **Manual, documentation or CI only:** R3.4, R29.8, R58, R60, R62, R63 and R66.8. R66.8 is only enforced on Linux.

I changed no files and committed nothing. The gate output is in `/private/tmp/cu-review/`.
