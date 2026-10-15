#!/usr/bin/env bash
# Corral's review gate. Run from any directory; all input comes from /dev/null.
# Preserve CORRAL_GATE_ATTEMPT, CORRAL_CANDIDATE_HEAD and CORRAL_INTEGRATION_BASE:
# corral needs the runner's exports. Tests isolate their own subprocess environment
# in their harnesses (T-053); do not unset gate context here.
set -euo pipefail
exec </dev/null

step="repository setup"
trap 'status=$?; printf "FAILED: %s (exit %s)\n" "$step" "$status" >&2; exit "$status"' ERR
cd "$(dirname "$0")/.."

step="cargo fmt --all -- --check"
printf '\n==> %s\n' "$step"
cargo fmt --all -- --check

step="800-SLOC Rust ceiling"
printf '\n==> %s\n' "$step"
python3 scripts/check-candidate.py --worktree-sloc

step="cargo clippy --workspace --all-targets -- -D warnings"
printf '\n==> %s\n' "$step"
cargo clippy --workspace --all-targets -- -D warnings

step="CORRAL_HERDR_SESSION=corral-test cargo test --workspace"
printf '\n==> %s\n' "$step"
CORRAL_HERDR_SESSION=corral-test cargo test --workspace

step="cargo build -p corral && scripts/check-docs.sh target/debug/corral"
printf '\n==> %s\n' "$step"
cargo build -p corral
scripts/check-docs.sh target/debug/corral

printf '\nGate passed.\n'
