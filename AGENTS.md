# AGENTS.md - corral

corral is a project-management control plane for agentic coding that runs inside herdr.
Design: docs/DESIGN.md. UX reference (interactive HTML mock): docs/mockup/corral-tui.html.
herdr CLI shapes (captured from herdr 0.9.3): docs/herdr-cli.md.

## Layout

| Crate | Owns |
|---|---|
| crates/corral-core | task model, `.corral/` file store (markdown + YAML frontmatter), config, events log |
| crates/corral-herdr | typed wrapper over the `herdr` CLI (JSON output) |
| crates/corral-git | git subprocess helpers: worktrees, diffs, merges |
| crates/corral-ops | application services shared by CLI and TUI (dispatch, sync, review, artifacts, PM) |
| crates/corral-tui | ratatui board and screens |
| crates/corral | the `corral` binary: clap CLI wiring, herdr plugin manifest in `plugin/` |

Dependencies flow corral -> {tui, ops} -> {core, herdr, git}. core, herdr and git never depend on each other.

## Conventions

- Rust 2024, MSRV 1.94, errors via `anyhow` with context on every external call.
- Module ceiling: 800 SLOC per Rust file (tests in sibling `*_tests.rs` files or `#[cfg(test)] mod tests`).
- Public signatures in each crate's contract are fixed; change one only when the task requires it and update every caller.
- No `unwrap()`/`expect()` outside tests. No `todo!()` left in shipped code.
- Subprocesses (herdr, git) are invoked with argv, never through a shell, except agent launch commands which are sent to a herdr pane's shell by design.

## Test cost and placement

- Judge tests by the named defect they catch, not their count. Use the lowest layer that
  can fail and at most one end-to-end test per user journey; variants belong in cheaper tests.
- Bug fix: one failing-first regression. Refactor: no new tests. Prose/docs: no tests,
  especially no wording pins. Attached logs/command output count as evidence, not test debt.
- Use the smallest fixture that proves it. History independence uses exact work counters
  at two small sizes, never large histories. Never relax a failing test to go green.
- Reviewers flag duplicates, prose pins, oversized fixtures and unclassified slow tests.
  Request tests only for a named defect, never for evidence volume. Review the gate's added
  count/seconds; >30s needs a `Test-cost justification:` in the change report.

## Build and test

- Always build with the repo's `.cargo/config.toml` (it disables the crashing global sccache wrapper). If a build fails with an sccache error, prefix the command with `RUSTC_WRAPPER=`.
- Gates retain Cargo's worktree-local target by default, keeping concurrent candidates
  from overwriting each other's top-level CLI executable. Explicit `CARGO_TARGET_DIR` is
  respected; share executable targets only with external serialization through execution,
  not just Cargo's build lock. Compile once per gate and reuse its candidate-bound metadata.
  Keep the gate/work-budget test-clock feature union aligned to avoid a redundant rebuild.
- Check one crate: `cargo check -p <crate>`; test one crate: `cargo test -p <crate>`.
- Test binaries carry no debuginfo (`[profile.test] debug = false`); panic messages keep
  their file and line. When a backtrace needs line numbers, rerun that one test with
  `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p <crate> <name>`.
- Before `corral done`, run the gate your run contract names. In a tiered run that is `corral gate <id> --wait`, exactly once after the final rebase: corral selects the tier (quick during review rounds and for Plan-target members), records the receipt, and autopilot reuses it. Audited tasks then capture evidence, record it against the attempt that gate printed, run `corral done <id> --check`, then `corral done`. Evidence recorded for a quick receipt follows the candidate only through corral's authorized carries (tier promotion, clean replay); any other change needs fresh evidence. Legacy and untiered runs keep the gate commands their contract lists.
- `scripts/gate.sh` without `--tier` is the full landing tier: formatting, the exact
  800-SLOC Rust ceiling, Clippy, workspace nextest in `corral-test`, doctests, small work
  budgets, and a candidate build/docs check. Quick checks touched crates/modules (or declared
  tests), with crate-wide fallback when selection cannot decide. Slow has fixed membership
  in `.config/nextest.toml`: race/crash/torture/restart matrices, with reasons. Full excludes
  slow except on owning-module changes (`scripts/gate_lanes.py`); plan verify keeps all suites.
  Quick leaves slow out unless explicitly declared. Pi runs on package/shared-contract
  changes; Darwin runs on platform/dependency changes; plan verify keeps both lanes.
  Missing/legacy context is conservative: all suites and optional lanes.
- Install cargo-nextest before running the gate. `.config/nextest.toml` uses 12 slots,
  serializes shared live fixtures and disables retries. After each nextest run,
  `scripts/gate_case.py` reads its JUnit report, observes the 10s per-test reference and
  classifies candidate test changes against the integration base; no per-test wrapper
  process runs. ALL time thresholds and load observations are
  report-only in T-761; enforcement is deferred to T-776. Timing never changes a command's
  exit status. Overruns appear in the log and `.evidence/<attempt>-existing-test-costs.md`
  for T-766. Assertions still fail. Native nextest reports without termination. Explicit
  measured integration/torture exceptions retain their original 30s report-only guards,
  with one-line reasons; they are not extra full-tier exclusions. Never bump a failing timeout.
  `.config/gate.toml` records reference thresholds of quick180s/full600s execution,
  excluding compilation/preparation. They are observations, not gate failures, including
  at quiet load. Logs retain build/total wall time and load/core separately.
  Quick lints touched production targets; full retains all-target Clippy. Selected tests
  still compile and execute in quick, using fresh prebuilt metadata (no runtime rebuild).
  See [gate cost policy](docs/guide/gate-costs.md) for added-test cost reporting and the host
  nightly slow lane (nice 19, idle I/O, load/core <=1.0, private herdr, deduplicated failure
  tasks). PM installs its documented cron line after landing; workers never install it.
  Keep new shared-server tests in `herdr`; fake-engine tests remain parallel.
- The script uses null stdin and preserves gate identity exports; tests isolate their own subprocess environments. Run its targeted temporary-fixture regressions with `python3 scripts/test_gate.py`,
  `python3 scripts/test_gate_select.py`, `python3 scripts/test_gate_policy.py` and
  `python3 scripts/test_slow_lane.py`.
- The Linux concurrent-dequeue regression requires `python3` and `strace`. It traces only disposable fixture processes and synchronizes on lock syscalls, never timing sleeps.
- The Pi Claude wire-cache regression requires Linux `unshare`, `ip`, and unprivileged user/network/PID namespaces. It runs the real SDK/CLI against a synthetic loopback endpoint with no external network or owner credentials; unavailable isolation fails the Linux gate. Non-Linux runs skip it. `CORRAL_TEST_CLAUDE_EXECUTABLE=/path/to/claude` tests an installed CLI instead of the pinned SDK binary; see the Pi guide.
- Compilation in the gate goes through `scripts/cargo.sh`, which is plain Cargo unless
  `CORRAL_BUILD_CACHE=mbx` opts into the shared mbx 1.29.2 cache pilot. Bare `cargo` is not
  intercepted; use `bash scripts/cargo.sh <args>` to share artifacts. See
  [build cache](docs/guide/build-cache.md) for settings, policy capture and rollback.

### Load testing: container only

Never generate intentional CPU/memory load on the shared host: no host burners,
spin loops, `stress-ng`, or parallel stress harnesses. The **only allowed way to
load-test** (workers, reviewers, and gates alike) is `scripts/load-sandbox.sh`:

```bash
scripts/load-sandbox.sh --cpus 2 --memory 2g --pids-limit 256 --burners 2 -- cargo test -p corral-core
```

It runs the command and optional burners in one Podman container (default 2 CPUs,
hard maximum 4; default 2g memory and 256 PIDs), then forcibly removes it on success,
failure, or interruption. Never fall back to host load if Podman/cgroups are unavailable;
report the blocked check instead. This does not authorize an extra gate or change its tier.
See [load sandbox](docs/guide/load-sandbox.md) for image prerequisites and smoke checks.

## herdr in tests

Pane ownership and legacy migration are specified in `docs/session-ownership.md`.
Do not infer legacy ownership from mutable config or the test client's selectors.
Hand-built nonlegacy run fixtures must save their fake client's socket explicitly.

NEVER create panes or workspaces in the user's live herdr session. A session name alone does
not isolate plugins: even `corral-test` can load the owner's plugin hooks. Do not reuse or stop
an existing server. Start a fresh plugin-free server and run its clients in the same subshell:

```bash
(
  source scripts/test-herdr.sh
  corral_test_herdr_start || exit 1
  scripts/gate.sh                 # full tier; or cargo test -p <crate>, QA scripts, etc.
)
```

The helper gives HOME and all XDG config/data/state/cache directories private temporary paths,
clears inherited herdr config/socket/pane overrides, and preserves Cargo/Rustup paths and gate
identity exports. It targets `corral-test` inside that private configuration, waits for readiness,
supplies a deterministic test-only Git identity instead of reading the owner's global config,
creates one baseline fixture pane so T-092's empty-inventory guard does not block initial
queue admission, and terminates only its own child server and removes its temporary directories on exit.
Keep clients and panes inside this environment; a bare command from another shell targets a
different server. Scripts that own startup (e2e, screenshots, and the shell TUI drivers) use
this helper automatically. The mouse driver, plan QA, gate, and live Rust tests only consume
a server: launch them inside the subshell above. Gate fixtures use fake commands, not herdr;
the startup CLI tests likewise use a fake engine with isolated HOME/XDG.

Tests that need herdr must skip (return early with a printed note) when
`herdr --session corral-test workspace list` fails. Never start an owner-configured server to
make a skipped test run. No plugin-disable CLI flag is assumed.
