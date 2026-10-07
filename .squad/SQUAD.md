# SQUAD BRIEFING (read fully before doing anything)

You are one of THREE coding agents (opencode, pi, cursor) building buzz-router v1 IN PARALLEL, orchestrated by Claude (who does not write code). Repo rules are in AGENTS.md; the build plan is docs/specs/buzz-router-v1/tasks.md (APPROVED). requirements.md and design.md are approved too.

## How to work
- Use the `conductor` workflow (or `bug-crusher` for small fixes) in SPEC-DRIVEN mode, test-first per tasks.md. The spec already exists: do NOT run the architect flow, do NOT write a new plan, do NOT create a new spec. Read the task, execute it. If the spec is ambiguous or wrong, STOP that task and report (final message) rather than guess.
- You work ONLY in your own git worktree/branch (path given in your prompt). Never touch another worktree or /Users/dave/git/personal/buzz-router directly except through merge.sh.
- Own only the files your task lists. Shared hot files (Cargo.toml, crates/*/src/lib.rs, tests/support/mod.rs): make minimal, additive edits (add a dependency line / a `pub mod` line), never reformat or reorder them, so rebases stay trivial.
- One commit per completed task, subject `T<id>: <summary> (R<ids>)`, trailer line `Co-Authored-By: <your model name> <noreply@anthropic.com>`. Tick the task's checkboxes in tasks.md in the SAME commit (only your task's lines).
- After each task commit run: bash .squad/merge.sh (in your worktree)
  It rebases on feat/buzz-router-v1, runs fmt+clippy+test, and fast-forwards the integration branch under a global lock. On rebase conflict: resolve keeping BOTH sides' intent, `git rebase --continue`, rerun. If the gate fails because of someone else's code, tell me in your final message; do not "fix" their files beyond trivial compile fixes.
- Then continue to your next assigned task. When your list is done or you are blocked, finish with a SHORT final report: tasks done (commit ids), blocked items, spec ambiguities. Nothing else.

## Hard rules
- router-core stays pure (no I/O, no tokio, no clock except `now`). The router never calls an LLM. Don't modify Buzz; depend on buzz-core/buzz-sdk pinned per brief §2.
- Never commit keys, nsecs, tokens, real pubkeys, relay URLs. NEVER connect to any real/live relay (e.g. anything *.ts.net) or use real bot keys. E2E runs only against a local Docker relay (task 7.1).
- No unwrap/expect/panic!/todo!/dbg! outside tests. thiserror in libs, anyhow only in main/CLI dispatch. Follow design §2.
- Docs under docs/ need kyber-weave frontmatter; run `kyber-weave docs validate .` before committing docs changes.

## Owner decisions already made (apply them; amend spec text accordingly)
- O1: github-devops may write .github/workflows/ci.yml exactly per task 1.1 criterion 6, actions pinned by SHA like docs-gate.yml.
- O2: config validation REJECTS duplicate channel ids, `max_concurrent = 0`, an empty command, and a bot named `all`, each with a clear ConfigIssue.
- O3: keep T1.4's interim reading (bare whole-word owner name in reply `p` tags -> every owner pubkey); record as spec clarification.
- O4: Reading 1 — a foreign bot's `p` tag naming a local bot yields Suppress(RespondTo) (R15.2 wins over R17.4 for foreign bots); verify existing fixtures, add a fixture.
- O5: replay does not invent thread state for roots it never saw (current behaviour; document).
- O6: accept keyring 4.2.0 + keyring-core pin; update design §11 to match.
- O7: add `ErrorKind::Key` (exit code 3, label `key_error`); update design §14. `keys check` uses it.
- O8: add `~` expansion of the adapter `cwd` to task 3.5 (DD-19).
- Docker: tests needing a relay use a LOCAL Docker relay (postgres:17-alpine + redis:7-alpine via Buzz's docker-compose, relay built from Buzz pinned rev f0eb5575ffc9d5f57af4ed3f574529d997c83a0d, or Buzz's published image if one exists). Not the Zo relay.

## Squad lanes (who owns what; do not start other lanes' tasks)
- opencode: 1.1 -> "AMEND" (apply O2-O8 spec + code changes above, own commit `AMEND: owner decisions O2-O8`) -> 2.1 -> 2.5 (after 2.3/2.4 land) -> 2.6
- pi: 2.2 -> 2.3 -> 2.4 -> 2.7 -> 2.8
- cursor: 3.1 -> 6.2 -> 7.1-prep script `scripts/e2e-relay.sh` (task 7.1 relay bring-up/tear-down only; do not run until told)
Later waves are assigned by Claude. Check `git log feat/buzz-router-v1` to see what has landed from others.

## LANE UPDATE 1 (supersedes earlier lists where they differ)
- cursor: DONE 3.1, 6.2, 7.1-prep. NOW: 2.1 (reassigned from opencode — critical path), then 6.1 (apply O7: add ErrorKind::Key -> exit 3, label key_error, if opencode's AMEND hasn't already), then 2.6 only if T2.3 has landed on feat/buzz-router-v1, else stop and report.
- opencode: 1.1, then AMEND, then STOP (do NOT do 2.1 — cursor owns it; check `git log feat/buzz-router-v1` and tasks.md for T2.1 before touching it). Then 2.5 only if T2.3 and T2.4 have landed.
- pi: unchanged (2.2 landed).

## CLARIFICATION on "AMEND"
AMEND is NOT progress-doc updates. It is the owner-decision change set O2-O8 above: code changes (O2 config rejections, O4 foreign-bot p-tag Suppress + fixture, O7 ErrorKind::Key) and spec text edits (O3, O5, O6 design §11, O7 design §14, O8 task 3.5). Own commit `AMEND: owner decisions O2-O8`.
Everything you need is inside your worktree under .squad/ (you cannot read outside it).

## LANE UPDATE 2
- T2.3 landed. cursor now owns: 2.6 -> 2.8 -> 3.2 -> 3.3 (then 3.4 once T2.7 lands). 2.8 is REMOVED from pi's list (pi: 2.4 -> 2.7 only).
- opencode: AMEND only, then stop.
- Always check `git log feat/buzz-router-v1` and tasks.md checkboxes before starting a task; skip any already landed.

## LANE UPDATE 3
- Landed: 2.1-2.4, 2.6, 2.8, 3.1, 3.2, 6.1, 6.2, AMEND.
- opencode: 2.5 -> then 3.6 -> 3.9 (each only once its deps have landed on feat/buzz-router-v1: 3.6 and 3.9 need T3.4 from cursor; if not landed, stop and report).
- pi: 2.7 (then stop).  cursor: 3.3 -> 3.4 (needs T2.7) -> 3.5 -> 3.7.

## LANE UPDATE 4
- Landed through: 2.1-2.4, 2.6-2.8, 3.1-3.5, 4.2, 5.1, 6.1, 6.2, AMEND.
- cursor now owns 3.6 -> 3.7 -> 3.9 -> 3.8 -> 3.10 (was opencode's 3.6/3.9). opencode: 2.5 only, then 2.9 (Docker relay via scripts/e2e-relay.sh). pi: 4.4.

## LANE UPDATE 5 (milestones 1-6 landed; checkboxes for some earlier tasks in tasks.md may be unticked: leave them, 8.2 reconciles)
- RELAY LOCK: only one agent may run the local Docker relay at a time. Wrap relay use: `until mkdir /private/tmp/buzz-relay.lock 2>/dev/null; do sleep 15; done; trap 'rmdir /private/tmp/buzz-relay.lock' EXIT` ... `scripts/e2e-relay.sh up` ... run ... `scripts/e2e-relay.sh down`. Never leave the relay running or the lock held.
- cursor: 7.1 -> 7.2 -> 7.5.  opencode: 8.1 (docs-dev, kyber-weave frontmatter, validate).  pi: 7.3 and 7.4 once T7.1 lands.
- Findings from 2.9: a `#e`-only REST query is rejected (403); filters need `kinds` too.

## LANE UPDATE 6: REVIEW FIXES (full council review is in .squad/REVIEW.md; numbers below refer to its findings)
All tasks are landed; this is the fix pass. Use the bug-crusher workflow per finding (or conductor test-first for behaviour changes): failing test first where the finding is behavioural, then fix. One commit per finding or tight group, subject `FIX #<n>: <summary>`, then `bash .squad/merge.sh`. Stay inside your lane's files.
- Lane A (cursor): #1 (CRITICAL), #4, #5, #6, #11, #22, #23 — relay/ , cli/run.rs, adapter/command.rs pid-file only.
- Lane B (pi): #2, #3, #8, #9, #10, #16 — core/dispatch.rs, core/apply.rs, cli/control.rs, logging.rs.
- Lane C (opencode): #12 (release.yml), #13, #14 (e2e tests), #15 (conformance fixtures, one per listed rule; if a fixture exposes a routing bug, fix it in router-core), #17, #18 and the unticked-header reconciliation (tick the 9 task headers listed in the review; leave the 3 CI-pending sub-boxes).
- HELD for owner (do NOT do): #7 shutdown behaviour, #19 (.squad removal is done last by Claude), #20 Windows restart, #21 runbook rollback section.

## LANE UPDATE 7 (owner decisions on held findings)
- #7 APPROVED: on SIGTERM/service stop, cancel each running wake and wait a short bounded time, kill if they don't stop; add kill_on_drop to command spawns. Update design text. (follow-up, assigned after Lane A)
- #20 APPROVED: add a 1-minute repeating trigger (indefinite) with MultipleInstancesPolicy IgnoreNew to the Windows task XML in service/windows.rs, keep the logon trigger and RestartOnFailure; update design section 13 and add a manual "kill the process, confirm it returns within ~1 min" check to the runbook (docs/runbooks/buzz-router-cutover.md). (follow-up, assigned after Lane B)
- #21 DROPPED: no rollback section needed.

## LANE UPDATE 8: cycle-2 review fixes (see .squad/REVIEW2.md, findings A-G)
- cursor: B (ingest Seen -> core advances that bot's cursor; core stays only writer), C (cursor read error -> redial, not Skip), G (without BUZZ_E2E=1 the ignored e2e tests must fail loudly or be skipped visibly, not pass).
- opencode: A (GH_REPO env in release.yml publish job), F (refresh tasks.md progress table to the real current state: last commit, test count, conformance count, CI/e2e state; do NOT tick 8.2).
- pi: D (single-instance lock file in the data dir taken before spawn_core; a second `run` exits with a clear error and exit code; test it; mention in design + runbook that ending the Windows task is no longer a lasting stop).
- HELD: E (needs owner decision).
