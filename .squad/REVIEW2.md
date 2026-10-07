## Verdict: REQUEST_CHANGES (risk: MEDIUM)

The fixes close 19 of the 23 findings. #1 and #18 are only partly fixed, and #19 and #21 are deferred or dropped as you decided. But the fixes introduced one new critical bug and one new major one. I confirmed both in the code: the release publish job will fail on every tag push, and the move to core-only cursor writes stops the cursor advancing for bots that share a channel.

All four checks pass: `cargo fmt --check`, clippy with `-D warnings`, `cargo test --workspace --locked` (0 failures, all 15 e2e tests ignored as intended) and `kyber-weave docs validate .` (no findings). The e2e tests were not run here because they need a local relay.

```
REVIEW CYCLE 2 — mode: verifier, widened to full over the reworked files
Verdict:   REQUEST_CHANGES   Risk: MEDIUM
Findings:  6 surviving (19 fixed, 2 partial, 4 new since cycle 1)
Gates:     4/4
Next:      FIXING
```

## Prior findings

| # | Status | Evidence |
|---|---|---|
| 1 | Partial | The relay task no longer writes the cursor, opens the store read-only, and redials when a backfill fails. But a failed cursor read (`relay/conn.rs:577-580`) skips backfill and goes live, so the cursor moves past history that was never fetched. |
| 2 | Fixed | `finish_pass` picks `Posted` when the wake has posted; covered by a test in `api.rs`. |
| 3 | Fixed | ⚠️ goes on every unmanaged post and only the log line is limited (`core/apply.rs:575`). |
| 4 | Fixed | The ladder resets before choosing the delay (`conn.rs:69-76`), with unit tests. |
| 5 | Fixed | A failed discovery redials (`conn.rs:533-536`), with a test. |
| 6 | Fixed | The REST client has 5 s connect and 15 s total timeouts (`rest.rs:45-49`), with a test. |
| 7 | Fixed, as you decided | Shutdown cancels running wakes with a bounded 5 s grace, spawns use `kill_on_drop`, and design section 6.9 is added. |
| 8 | Fixed | `cli/control.rs:130` returns the error; covered by `cli_stop_fallback.rs`. |
| 9 | Fixed | `max_log_files(14)` (`logging.rs:84-89`). |
| 10 | Fixed | The wake token is redacted in agent stderr and webhook errors use `without_url()`. |
| 11 | Fixed | One connection loop; the tests drive the path that ships. |
| 12 | Partial: the structure is fixed, but see new finding A | Build jobs are read-only and upload artifacts; one publish job runs on tags only. |
| 13 | Fixed | E3 checks for no new wake row and no 👀, and allows 60 s after resume. |
| 14 | Fixed | E2 requires 2–4 wake rows per bot and a `discussion` wake. |
| 15 | Fixed | Fixtures 123–132 exist, are registered and match their rules. |
| 16 | Fixed | Both warnings go through `LogLimiter`. |
| 17 | Fixed | R17.4 now carves out foreign-bot `p` tags. |
| 18 | Partial | `tasks.md:26-29` still says last commit `22dd879`, 389 tests, 59 conformance cases and "CI skeleton". |
| 19 | Deferred | Your decision. |
| 20 | Fixed, as you decided | 1-minute repeating trigger with `IgnoreNew`, plus a runbook check. See new finding D. |
| 21 | Dropped | Your decision. |
| 22 | Fixed | The process group is killed when the pid file can't be written, with a test. |
| 23 | Fixed | All 15 e2e tests are `#[ignore]` and CI runs them with `--ignored`. |

The nine unticked task headers from the first review are now ticked.

## New findings

**A. Critical: the publish job can't find the repository.**
- **Where:** `.github/workflows/release.yml:70-94`. Only the build job has a checkout (`:33`).
- **Why it matters:** the `publish` job has no checkout and no `GH_REPO`, so `gh release create` exits with "not a git repository" under `set -e`. No binaries get attached on a tag push.
- **Fix:** add `GH_REPO: ${{ github.repository }}` to the step's `env`.

**B. Major: shared-channel bots stop advancing their cursor.**
- **Where:** `ingest.rs:165-167` drops an event that was already forwarded or processed, before the core sees it. The cursor only moves in `core/apply.rs:244-246`, `:378` and `:207`.
- **Why it matters:** when two bots share a channel, only the bot that delivers an event first advances its cursor. The other bot stays near its first-run cursor. On every reconnect it backfills its whole history, at up to about 63 s per page. Any page failure now discards the backfill and redials, so that bot may never get to live events.
- **Fix:** have ingest emit `IngestOutput::Seen { bot, created_at }` for duplicates, and have the core advance that bot's cursor. The core stays the only writer.

**C. Major: a failed cursor read still loses history (the rest of #1).**
- **Where:** `relay/conn.rs:577-580`.
- **Fix:** on a cursor read error, log it and return so the connection redials, instead of using `BackfillPlan::Skip`.

**D. Minor: the repeating Windows trigger can start a second router.**
- **Where:** `service/windows.rs:31-38`, together with `cli/run.rs:133` (where `recover()` runs) and `:169` (where the API port binds).
- **Why it matters:** `IgnoreNew` only covers duplicate task instances. If you run `buzz-router run` by hand, the task starts a second copy within a minute. That copy marks the first one's running wakes `interrupted` and re-queues them before it fails to bind the port. Ending the task also no longer keeps the router stopped.
- **Fix:** take a single-instance lock file in the data directory before `spawn_core`, and say in the runbook that ending the task is no longer a lasting stop.

**E. Minor, needs a decision from David: one permanently failing channel blocks the bot.**
- **Where:** `relay/conn.rs:654-669`.
- **Why it matters:** a persistent 4xx or decode error on one channel's backfill causes a redial every time, so that bot never gets live events. The only sign is a repeating warning.
- **Options:** skip a channel that fails with a 4xx, or after N failures log at error level and show it in `status`.

**F. Minor: `#18` text.** Refresh the progress table at `tasks.md:26-29`.

**G. Nit:** running with `--ignored` but without `BUZZ_E2E=1` still reports the e2e tests as passing. CI sets the variable, so this only affects local runs.

The two reviews came from the [findings re-check](c3c3b4d2-ec13-4d70-a46c-8bb9dbd83895) and the [fix-diff review](cbc723bb-63e5-41c4-8427-7380f878e6d7). I changed nothing and committed nothing. The gate output is in `/tmp/cu-rereview/`.
