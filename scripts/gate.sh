#!/usr/bin/env bash
# Corral's review gate. Run from any directory; all input comes from /dev/null.
# Preserve CORRAL_GATE_ATTEMPT, CORRAL_CANDIDATE_HEAD and CORRAL_INTEGRATION_BASE:
# corral needs the runner's exports. Tests isolate their own subprocess environment
# in their harnesses (T-053); do not unset gate context here.
#
# Without --tier (or with --tier full) this is the full tier, which corral runs for every
# landing on main and for plan verify. Full landings scope optional lanes and torture
# suites by changed paths; plan verify keeps all lanes and suites. --tier quick reads
# CORRAL_GATE_CONTEXT (schema v1) and checks only what the candidate touched.
set -euo pipefail
exec </dev/null
cd "$(dirname "$0")/.."

# Keep the native mbx lease across compilation, prebuilt execution and gaps.
if [[ ${1:-} == --corral-mbx-protected ]]; then
  shift
  python3 scripts/cargo_cache_targets.py --verify-protection
elif [[ ${CORRAL_BUILD_CACHE:-off} == mbx && ${CORRAL_MBX_TARGETS:-private} == managed ]]; then
  exec python3 scripts/cargo_cache.py --protect-script "$PWD/scripts/gate.sh" "$@"
fi

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
  if [[ -f ${CORRAL_TEST_BUDGET_STATE:-/absent} ]]; then
    python3 scripts/gate_timer.py report "$CORRAL_TEST_BUDGET_STATE" || true
  fi
  # A failed nextest run names narrow rerun commands for the worker; diagnostic only.
  if [[ -n $repair_profile ]]; then
    python3 scripts/gate_repair.py report "$repair_profile" || true
  fi
  printf 'FAILED: %s (exit %s)\n' "$step" "$status" >&2
  exit "$status"
}

step="repository setup"
step_active=0
repair_profile=""
trap on_error ERR
if [[ ${CORRAL_BUILD_CACHE:-off} == mbx ]]; then
  printf 'Build cache: mbx 1.29.2 local profile, targets=%s (verified on invocation)\n' "${CORRAL_MBX_TARGETS:-private}"
fi
# Keep Cargo's worktree-local default: shared top-level CLI executable paths can be
# overwritten by a concurrent candidate after Cargo releases its build lock.
# An explicit caller target remains supported; an empty override means the default.
if [[ -z ${CARGO_TARGET_DIR:-} ]]; then unset CARGO_TARGET_DIR; fi
docs_binary="${CARGO_TARGET_DIR:-target}/debug/corral"
gate_dir=$(mktemp -d)
trap 'rm -rf "$gate_dir"' EXIT
export CORRAL_TEST_BUDGET_STATE="$gate_dir/budget.json"
python3 scripts/gate_timer.py init "$CORRAL_TEST_BUDGET_STATE" "$tier"
now_us
gate_started=$now
pi_tests=()
python3 scripts/test_cost.py count

finish() {
  python3 scripts/gate_timer.py report "$CORRAL_TEST_BUDGET_STATE"
  now_us
  printf 'Gate total wall (including compilation): %d.%03ds\n' \
    $(((now - gate_started) / 1000000)) $(((now - gate_started) / 1000 % 1000))
  python3 scripts/test_cost.py "$([[ $tier == quick ]] && echo quick || echo default)" || on_error
  if [[ -s "${CORRAL_TEST_HARNESS_SHIM_LOG:-}" ]]; then
    printf 'FAILED: tests invoked a real agent harness:\n' >&2
    cat "$CORRAL_TEST_HARNESS_SHIM_LOG" >&2
    exit 1
  fi
  printf '\nGate passed.\n'
}

# The Bun lane runs for Pi or shared-contract changes, and always in plan verify.
# Its absence is reported as a skip.
pi_lane() {
  if [[ -d packages/corral-pi ]]; then
    begin_step "scripts/pi-lane.sh"
    # Functions do not inherit the ERR trap; report the step's failure explicitly.
    # The Bun slow catalog (gate_lanes.PI_SLOW) runs only when the lane plan restores it.
    CORRAL_PI_SLOW="${pi_slow:-0}" scripts/pi-lane.sh "${pi_tests[@]}" || on_error
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

# Compile once outside the test budget. Runtime commands use only these candidate-bound
# metadata files: they cannot silently rebuild when a cache is cold.
prepare_test_binaries() {
  export CORRAL_GATE_CARGO_METADATA="$gate_dir/cargo.json"
  export CORRAL_GATE_BINARIES_METADATA="$gate_dir/binaries.json"
  python3 scripts/gate_timer.py compile "$CORRAL_TEST_BUDGET_STATE" cargo-metadata \
    --output "$CORRAL_GATE_CARGO_METADATA" -- cargo metadata --format-version 1 || return
  CORRAL_HERDR_SESSION=corral-test python3 scripts/gate_timer.py compile \
    "$CORRAL_TEST_BUDGET_STATE" nextest-binaries --output "$CORRAL_GATE_BINARIES_METADATA" -- \
    bash scripts/cargo.sh nextest list "$@" --list-type binaries-only --message-format json || return
  CORRAL_HERDR_SESSION=corral-test python3 scripts/gate_case.py prepare "$gate_dir/case.json" \
    "$CORRAL_GATE_CARGO_METADATA" "$CORRAL_GATE_BINARIES_METADATA" >"$gate_dir/runner.sh" || return
  # shellcheck disable=SC1091
  source "$gate_dir/runner.sh"
}

if [[ $tier == quick ]]; then
  step="reading the gate context"
  if [[ -z ${CORRAL_GATE_CONTEXT:-} || ! -f $CORRAL_GATE_CONTEXT ]]; then
    printf 'FAILED: --tier quick needs CORRAL_GATE_CONTEXT naming a gate context file\n' >&2
    exit 2
  fi
  quick_dir=$gate_dir
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
    begin_step "cargo clippy ${scope[*]} -- -D warnings"
    python3 scripts/gate_timer.py compile "$CORRAL_TEST_BUDGET_STATE" clippy -- \
      bash scripts/cargo.sh clippy "${scope[@]}" -- -D warnings
    end_step
  fi

  # Declared tests replace the module selection and may name any package. The 'quick'
  # nextest profile leaves out the slow group (.config/nextest.toml) unless tests are
  # declared; full landings restore torture suites on owning-module changes.
  # Quarantine runs in every tier, even when the changed files select no Rust tests.
  test_scope=(--workspace)
  if ((${#test_scope[@]})); then
    profile=(--profile quick)
    list_args=("${test_scope[@]}" "${profile[@]}")
    if ((declared)); then
      profile+=(--ignore-default-filter)
      list_args=("${test_scope[@]}" "${profile[@]}" -E "$selection")
    fi
    begin_step "CORRAL_HERDR_SESSION=corral-test cargo nextest list ${list_args[*]}"
    prepare_test_binaries --workspace || on_error
    reuse=(--cargo-metadata "$CORRAL_GATE_CARGO_METADATA" --binaries-metadata "$CORRAL_GATE_BINARIES_METADATA")
    actual_list=("${profile[@]}" "${reuse[@]}")
    ((declared)) && actual_list+=(-E "$selection")
    CORRAL_HERDR_SESSION=corral-test cargo nextest list "${actual_list[@]}" \
      --message-format json >"$quick_dir/list.json"
    if ((!declared && !workspace)); then
      python3 scripts/gate_quarantine.py scope-list "$quick_dir/list.json" "${packages[@]}"
    fi
    quick_helper select "$CORRAL_GATE_CONTEXT" "$quick_dir/list.json" "${expected[@]}" \
      >"$quick_dir/select.sh"
    # shellcheck disable=SC1091
    source "$quick_dir/select.sh"
    end_step
    run_args=("${profile[@]}" "${reuse[@]}")
    # Approved exclusions or the slow group may cover the whole selection; nextest would call
    # that an empty run. A declared selection that matched nothing still reaches nextest and
    # fails.
    if ((!declared && ${#scope[@]} == 0)) || ((remaining == 0 && !(declared && selected == 0))); then
      run_filter='none()'
    elif ((!declared && !workspace)); then
      package_filter=''
      for package in "${packages[@]}"; do
        [[ -z $package_filter ]] || package_filter+=' or '
        package_filter+="package(=$package)"
      done
      run_filter="($run_filter) and ($package_filter)"
    fi
    begin_step "CORRAL_HERDR_SESSION=corral-test cargo nextest run (prebuilt, tier quick)"
    # A valid selection can consist entirely of quarantined tests.
    CORRAL_HERDR_SESSION=corral-test CORRAL_GATE_REPAIR_PROFILE=quick CORRAL_ALLOW_EMPTY_ORDINARY=$((selected > 0)) \
      python3 scripts/gate_timer.py run \
      "$CORRAL_TEST_BUDGET_STATE" nextest -- python3 scripts/gate_quarantine.py "$run_filter" \
      cargo nextest run "${run_args[@]}"
    end_step
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

lane_args=()
[[ -z ${CORRAL_GATE_CONTEXT:-} ]] || lane_args+=("$CORRAL_GATE_CONTEXT")
lane_plan=$(python3 scripts/gate_lanes.py "${lane_args[@]}")
eval "$lane_plan"

begin_step "cargo fmt --all -- --check"
cargo fmt --all -- --check
end_step

begin_step "800-SLOC Rust ceiling"
python3 scripts/check-candidate.py --worktree-sloc
end_step

# Reuse installed Darwin standard libraries; this checks all Rust (including macOS
# cfg paths), not native linking. Do not download targets during a review gate.
# Skip only the bundled C work: SQLite's linked path still copies its bundled Rust
# bindings, and zstd has checked-in bindings so Cargo can override its build script.
# These command-local overrides must never reach a build/test command: real builds
# still compile and link the bundled C libraries with the platform's toolchain.
if ((!darwin_touched)); then
  printf '\nSkipping Darwin checks (no platform or dependency changes).\n'
elif command -v rustup >/dev/null 2>&1; then
  step="listing installed Rust targets"
  installed_targets=$(rustup target list --installed)
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    if [[ $'\n'"$installed_targets"$'\n' == *$'\n'"$target"$'\n'* ]]; then
      begin_step "cargo check --workspace --all-targets --target $target"
      printf 'Rust-only check: skipping SQLite/zstd C compilation (no Apple SDK required).\n'
      LIBSQLITE3_SYS_USE_PKG_CONFIG=1 python3 scripts/gate_timer.py compile \
        "$CORRAL_TEST_BUDGET_STATE" "$target" -- bash scripts/cargo.sh check --workspace --all-targets --target "$target" \
        --config "target.$target.zstd.rustc-link-lib=[]"
      end_step
    else
      printf '\nSkipping %s check (install with: rustup target add %s).\n' "$target" "$target"
    fi
  done
else
  printf '\nSkipping Darwin checks (rustup unavailable).\n'
fi

begin_step "cargo clippy --workspace --all-targets -- -D warnings"
python3 scripts/gate_timer.py compile "$CORRAL_TEST_BUDGET_STATE" clippy -- \
  bash scripts/cargo.sh clippy --workspace --all-targets -- -D warnings
end_step

nextest_args=()
nextest_label="CORRAL_HERDR_SESSION=corral-test cargo nextest run --workspace"
if [[ $landing_filter != "all()" ]]; then
  printf '\nTorture suites: owning-module selection (see profile.slow).\n'
  nextest_args+=(-E "$landing_filter")
  nextest_label+=" -E <owning-module selection>"
fi
begin_step "$nextest_label"
prepare_test_binaries --workspace || on_error
reuse=(--cargo-metadata "$CORRAL_GATE_CARGO_METADATA" --binaries-metadata "$CORRAL_GATE_BINARIES_METADATA")
CORRAL_HERDR_SESSION=corral-test CORRAL_GATE_REPAIR_PROFILE=default python3 scripts/gate_timer.py run \
  "$CORRAL_TEST_BUDGET_STATE" nextest -- python3 scripts/gate_quarantine.py "$landing_filter" \
  cargo nextest run --profile default "${reuse[@]}"
end_step

begin_step "CORRAL_HERDR_SESSION=corral-test cargo test --workspace --doc"
CORRAL_HERDR_SESSION=corral-test bash scripts/cargo.sh test --workspace --doc
end_step

begin_step "scripts/work-budget.sh"
scripts/work-budget.sh
end_step

if ((pi_touched)); then
  pi_lane
else
  printf '\nSkipping scripts/pi-lane.sh (Pi package and contracts untouched).\n'
fi

begin_step "cargo build -p corral && scripts/check-docs.sh $docs_binary"
python3 scripts/gate_timer.py compile "$CORRAL_TEST_BUDGET_STATE" candidate-build -- bash scripts/cargo.sh build -p corral
scripts/check-docs.sh "$docs_binary"
end_step

finish
