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
