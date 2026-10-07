#!/usr/bin/env bash
# usage: merge.sh   (run from inside your worktree, on your squad/* branch, all work committed)
# Rebases onto feat/buzz-router-v1, runs the full gate, fast-forwards the integration branch. Serialized by a lock.
set -euo pipefail
R=/Users/dave/git/personal/buzz-router
LOCK=/private/tmp/buzz-router-merge.lock
BR=$(git rev-parse --abbrev-ref HEAD)
[ -z "$(git status --porcelain)" ] || { echo "commit your work first"; exit 2; }
until mkdir "$LOCK" 2>/dev/null; do echo "waiting for merge lock..."; sleep 20; done
trap 'rmdir "$LOCK"' EXIT
git rebase feat/buzz-router-v1 || { echo "REBASE CONFLICT: resolve (keep both sides' intent), 'git rebase --continue', then rerun merge.sh"; exit 3; }
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
git -C "$R" merge --ff-only "$BR"
echo "MERGED $BR into feat/buzz-router-v1 at $(git -C $R rev-parse --short HEAD)"
