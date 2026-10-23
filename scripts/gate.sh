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

# The quick tier's context logic. 'plan <context>' prints Bash assignments of the touched
# packages, whether a workspace-wide input changed, the nextest selection and the expected
# failures. 'exclude <nextest list JSON> <id>...' logs each expected failure that matches no
# selected test, then prints how many selected tests remain and, on a second line, one exact
# '- (binary_id(=..) and test(=..))' term per match.
quick_helper() {
  python3 - "$@" <<'PY'
import json, shlex, sys, tomllib
from pathlib import Path

def package(directory):
    manifest = directory / "Cargo.toml"
    if not manifest.is_file():
        return None
    return tomllib.loads(manifest.read_text()).get("package", {}).get("name")

def plan(path):
    context = json.loads(Path(path).read_text())
    if context.get("schema_version") != 1:
        sys.exit(f"unsupported gate context schema {context.get('schema_version')!r}")
    workspace_inputs = {"Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml",
                        "rustfmt.toml", ".rustfmt.toml", "clippy.toml", ".clippy.toml"}
    packages, workspace = [], False
    for changed in context["changed_paths"]:
        if changed in workspace_inputs or changed.startswith(".cargo/"):
            workspace = True
            continue
        name = next(filter(None, map(package, Path(changed).parents)), None)
        if name is not None and name not in packages:
            packages.append(name)
    tests = context["tests"]
    selection = " or ".join(f"({test})" for test in tests) if tests else "all()"
    print(f"packages=({' '.join(map(shlex.quote, sorted(packages)))})")
    print(f"workspace={int(workspace)}")
    print(f"declared={len(tests)}")
    print(f"selection={shlex.quote(selection)}")
    ids = [failure["test"] for failure in context["expected_failures"]]
    print(f"expected=({' '.join(map(shlex.quote, ids))})")

def escape(value):
    return value.replace("\\", "\\\\").replace(")", "\\)").replace(",", "\\,")

def exclude(listing, ids):
    selected = {(binary, name)
                for binary, suite in json.loads(Path(listing).read_text())["rust-suites"].items()
                for name, case in suite["testcases"].items()
                if case["filter-match"]["status"] == "matches"}
    excluded, terms = set(), []
    for test in ids:
        binary, name = test.split(" ", 1)
        if (binary, name) in selected:
            excluded.add((binary, name))
            terms.append(f" - (binary_id(={escape(binary)}) and test(={escape(name)}))")
        else:
            print(f"Expected failure matches no selected test and excludes nothing: {test}",
                  file=sys.stderr)
    print(len(selected - excluded))
    print("".join(terms))

if sys.argv[1] == "plan":
    plan(sys.argv[2])
else:
    exclude(sys.argv[2], sys.argv[3:])
PY
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

  # Declared tests replace the touched-crate selection and may name any package.
  test_scope=("${scope[@]}")
  ((declared)) && test_scope=(--workspace)
  if ((${#test_scope[@]})); then
    filter=$selection
    if ((${#expected[@]})); then
      begin_step "CORRAL_HERDR_SESSION=corral-test cargo nextest list ${test_scope[*]} -E $selection"
      CORRAL_HERDR_SESSION=corral-test cargo nextest list "${test_scope[@]}" -E "$selection" \
        --message-format json >"$quick_dir/list.json"
      quick_helper exclude "$quick_dir/list.json" "${expected[@]}" >"$quick_dir/exclude.txt"
      { read -r remaining && IFS= read -r exclusions; } <"$quick_dir/exclude.txt"
      end_step
      [[ -n $exclusions ]] && filter="($selection)$exclusions"
    fi
    run_args=("${test_scope[@]}")
    [[ $filter != "all()" ]] && run_args+=(-E "$filter")
    # Approved exclusions may cover the whole selection; nextest would call that an empty
    # run. A selection that matched nothing to begin with still reaches nextest and fails.
    if [[ -n ${exclusions:-} ]] && ((remaining == 0)); then
      printf '\nSkipping cargo nextest run (every selected test is an expected failure).\n'
    else
      begin_step "CORRAL_HERDR_SESSION=corral-test cargo nextest run ${run_args[*]}"
      CORRAL_HERDR_SESSION=corral-test cargo nextest run "${run_args[@]}"
      end_step
    fi
  else
    printf '\nSkipping cargo nextest (no touched crates and no declared tests).\n'
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

begin_step "cargo build -p corral && scripts/check-docs.sh target/debug/corral"
cargo build -p corral
scripts/check-docs.sh target/debug/corral
end_step

finish
