#!/usr/bin/env bash
# Corral's review gate. Run from any directory; all input comes from /dev/null.
# Preserve CORRAL_GATE_ATTEMPT, CORRAL_CANDIDATE_HEAD and CORRAL_INTEGRATION_BASE:
# corral needs the runner's exports. Tests isolate their own subprocess environment
# in their harnesses (T-053); do not unset gate context here.
set -euo pipefail
exec </dev/null
printf '==> tier full\n'

# Each announced step closes with one '<== <step> <seconds>s' line, whether it succeeds or
# fails; a failure then reports FAILED. Failures outside a step stay untimed. Never print a
# line starting with 'corral gate:': that is the runner's result marker, and receipts
# require exactly one.
now_us() {
  if [[ -n ${EPOCHREALTIME:-} ]]; then
    now=${EPOCHREALTIME/[.,]/}
  else
    now=$(date +%s)000000
  fi
}
begin_step() {
  step=$1
  printf '\n==> %s\n' "$step"
  now_us
  step_started=$now
  step_active=1
}
end_step() {
  now_us
  elapsed=$((now - step_started))
  ((elapsed >= 0)) || elapsed=0
  printf '<== %s %d.%03ds\n' "$step" $((elapsed / 1000000)) $((elapsed / 1000 % 1000))
  step_active=0
}

on_error() {
  status=$?
  if ((step_active)); then end_step; fi
  printf 'FAILED: %s (exit %s)\n' "$step" "$status" >&2
  exit "$status"
}

step="repository setup"
step_active=0
trap on_error ERR
cd "$(dirname "$0")/.."

begin_step "cargo fmt --all -- --check"
cargo fmt --all -- --check
end_step

begin_step "800-SLOC Rust ceiling"
python3 scripts/check-candidate.py --worktree-sloc
end_step

# Reuse installed Darwin standard libraries; checking needs neither a Mac nor
# an Apple linker/SDK. Do not download targets during an ordinary review gate.
if command -v rustup >/dev/null 2>&1; then
  step="listing installed Rust targets"
  installed_targets=$(rustup target list --installed)
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    if [[ $'\n'"$installed_targets"$'\n' == *$'\n'"$target"$'\n'* ]]; then
      begin_step "cargo check --workspace --all-targets --target $target"
      cargo check --workspace --all-targets --target "$target"
      end_step
    else
      printf '\nSkipping %s check (install with: rustup target add %s).\n' "$target" "$target"
    fi
  done
else
  printf '\nSkipping Darwin checks (rustup unavailable).\n'
fi

begin_step "cargo clippy --workspace --all-targets -- -D warnings"
cargo clippy --workspace --all-targets -- -D warnings
end_step

begin_step "CORRAL_HERDR_SESSION=corral-test cargo nextest run --workspace"
CORRAL_HERDR_SESSION=corral-test cargo nextest run --workspace
end_step

begin_step "CORRAL_HERDR_SESSION=corral-test cargo test --workspace --doc"
CORRAL_HERDR_SESSION=corral-test cargo test --workspace --doc
end_step

begin_step "scripts/work-budget.sh"
scripts/work-budget.sh
end_step

begin_step "cargo build -p corral && scripts/check-docs.sh target/debug/corral"
cargo build -p corral
scripts/check-docs.sh target/debug/corral
end_step

if [[ -s "${CORRAL_TEST_HARNESS_SHIM_LOG:-}" ]]; then
  printf 'FAILED: tests invoked a real agent harness:\n' >&2
  cat "$CORRAL_TEST_HARNESS_SHIM_LOG" >&2
  exit 1
fi

printf '\nGate passed.\n'
