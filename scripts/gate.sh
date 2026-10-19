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

# Reuse installed Darwin standard libraries; checking needs neither a Mac nor
# an Apple linker/SDK. Do not download targets during an ordinary review gate.
if command -v rustup >/dev/null 2>&1; then
  step="listing installed Rust targets"
  installed_targets=$(rustup target list --installed)
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    if [[ $'\n'"$installed_targets"$'\n' == *$'\n'"$target"$'\n'* ]]; then
      step="cargo check --workspace --all-targets --target $target"
      printf '\n==> %s\n' "$step"
      cargo check --workspace --all-targets --target "$target"
    else
      printf '\nSkipping %s check (install with: rustup target add %s).\n' "$target" "$target"
    fi
  done
else
  printf '\nSkipping Darwin checks (rustup unavailable).\n'
fi

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

if [[ -s "${CORRAL_TEST_HARNESS_SHIM_LOG:-}" ]]; then
  printf 'FAILED: tests invoked a real agent harness:\n' >&2
  cat "$CORRAL_TEST_HARNESS_SHIM_LOG" >&2
  exit 1
fi

printf '\nGate passed.\n'
