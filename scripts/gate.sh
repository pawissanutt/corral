#!/usr/bin/env bash
# Corral's review gate. Run from any directory; all input comes from /dev/null.
# Preserve CORRAL_GATE_ATTEMPT, CORRAL_CANDIDATE_HEAD and CORRAL_INTEGRATION_BASE:
# corral needs the runner's exports. Tests isolate their own subprocess environment
# in their harnesses (T-053); do not unset gate context here.
#
# Without --tier (or with --tier full) this is the full tier, which corral runs for every
# landing on main and for plan verify. --tier quick reads the gate context file corral
# exports as CORRAL_GATE_CONTEXT (schema v1) and checks only what the candidate touched.
set -euo pipefail
exec </dev/null

tier=full
if (($#)); then
  if (($# != 2)) || [[ $1 != --tier ]] || [[ $2 != full && $2 != quick ]]; then
    printf 'usage: scripts/gate.sh [--tier full|quick]\n' >&2
    exit 2
  fi
  tier=$2
fi
printf '==> tier %s\n' "$tier"

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

finish() {
  if [[ -s "${CORRAL_TEST_HARNESS_SHIM_LOG:-}" ]]; then
    printf 'FAILED: tests invoked a real agent harness:\n' >&2
    cat "$CORRAL_TEST_HARNESS_SHIM_LOG" >&2
    exit 1
  fi
  printf '\nGate passed.\n'
}

# The Bun lane for packages/corral-pi, run only while that package exists (always in the full
# tier; in the quick tier when the candidate touched it). Its absence is reported as a skip.
pi_lane() {
  if [[ -d packages/corral-pi ]]; then
    begin_step "scripts/pi-lane.sh"
    # Functions do not inherit the ERR trap; report the step's failure explicitly.
    scripts/pi-lane.sh || on_error
    end_step
  else
    printf '\nSkipping scripts/pi-lane.sh (packages/corral-pi/ is absent).\n'
  fi
}

# The quick tier's context logic (selection by touched module, expected failures, counts)
# lives in scripts/gate_select.py; its docstring states the selection rules.
quick_helper() {
  python3 scripts/gate_select.py "$@"
}

if [[ $tier == quick ]]; then
  step="reading the gate context"
  if [[ -z ${CORRAL_GATE_CONTEXT:-} || ! -f $CORRAL_GATE_CONTEXT ]]; then
    printf 'FAILED: --tier quick needs CORRAL_GATE_CONTEXT naming a gate context file\n' >&2
    exit 2
  fi
  quick_dir=$(mktemp -d)
  trap 'rm -rf "$quick_dir"' EXIT
  quick_helper plan "$CORRAL_GATE_CONTEXT" >"$quick_dir/plan.sh"
  # shellcheck disable=SC1091
  source "$quick_dir/plan.sh"
  scope=()
  if ((workspace)); then
    scope=(--workspace)
  else
    for package in "${packages[@]}"; do scope+=(-p "$package"); done
  fi
  printf 'Touched: %s\n' "$( ((workspace)) && echo workspace || echo "${packages[*]:-no crates}")"

  if ((${#scope[@]})); then
    fmt_scope=("${scope[@]}")
    ((workspace)) && fmt_scope=(--all)
    begin_step "cargo fmt ${fmt_scope[*]} -- --check"
    cargo fmt "${fmt_scope[@]}" -- --check
    end_step
  else
    printf '\nSkipping cargo fmt and clippy (no touched crates).\n'
  fi

  begin_step "800-SLOC Rust ceiling"
  python3 scripts/check-candidate.py --worktree-sloc
  end_step

  if ((${#scope[@]})); then
    begin_step "cargo clippy ${scope[*]} --all-targets -- -D warnings"
    cargo clippy "${scope[@]}" --all-targets -- -D warnings
    end_step
  fi

  # Declared tests replace the module selection and may name any package. The 'quick'
  # nextest profile leaves out the slow group (.config/nextest.toml) unless tests are
  # declared; the full tier runs every test.
  test_scope=("${scope[@]}")
  ((declared)) && test_scope=(--workspace)
  if ((${#test_scope[@]})); then
    profile=(--profile quick)
    list_args=("${test_scope[@]}" "${profile[@]}")
    if ((declared)); then
      profile+=(--ignore-default-filter)
      list_args=("${test_scope[@]}" "${profile[@]}" -E "$selection")
    fi
    begin_step "CORRAL_HERDR_SESSION=corral-test cargo nextest list ${list_args[*]}"
    CORRAL_HERDR_SESSION=corral-test cargo nextest list "${list_args[@]}" \
      --message-format json >"$quick_dir/list.json"
    quick_helper select "$CORRAL_GATE_CONTEXT" "$quick_dir/list.json" "${expected[@]}" \
      >"$quick_dir/select.sh"
    # shellcheck disable=SC1091
    source "$quick_dir/select.sh"
    end_step
    run_args=("${test_scope[@]}" "${profile[@]}")
    [[ $run_filter != "all()" ]] && run_args+=(-E "$run_filter")
    # Approved exclusions or the slow group may cover the whole selection; nextest would call
    # that an empty run. A declared selection that matched nothing still reaches nextest and
    # fails.
    if ((remaining == 0 && !(declared && selected == 0))); then
      printf '\nSkipping cargo nextest run (no selected test remains to run).\n'
    else
      begin_step "CORRAL_HERDR_SESSION=corral-test cargo nextest run ${run_args[*]}"
      CORRAL_HERDR_SESSION=corral-test cargo nextest run "${run_args[@]}"
      end_step
    fi
  else
    printf '\nSkipping cargo nextest (no touched crates and no declared tests).\n'
  fi

  if ((pi_touched)); then
    pi_lane
  else
    printf '\nSkipping scripts/pi-lane.sh (packages/corral-pi/ untouched).\n'
  fi

  printf '\nSkipping Darwin checks, doctests, scripts/work-budget.sh and the docs build (tier quick).\n'
  finish
  exit 0
fi

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

pi_lane

begin_step "cargo build -p corral && scripts/check-docs.sh target/debug/corral"
cargo build -p corral
scripts/check-docs.sh target/debug/corral
end_step

finish
