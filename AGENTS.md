# AGENTS.md - corral

corral is a project-management control plane for agentic coding that runs inside herdr.
For the current app design, see `docs/app-design/README.md`.
herdr CLI shapes (captured from herdr 0.9.3): docs/herdr-cli.md.

## Layout

| Crate | Owns |
|---|---|
| crates/corral-types | data shared unchanged by the engine, the CLI and the wire protocol |
| crates/corral-git | git subprocess helpers (argv only): worktrees, diffs, merges |
| crates/corral-herdr | typed wrapper over the `herdr` CLI (JSON output) |
| crates/corral-facts | built-in system map facts read from committed Git objects |
| crates/corral-engine | the task engine: SQLite store whose operations share transactions with their events, config, runners |
| crates/corral-protocol | wire contract shared by the daemon and its clients |
| crates/corral-daemon | local owner RPC server over the engine (the `corrald-next` binary) |
| crates/corral-next | the `corral-next` binary: clap CLI over the engine |

Dependencies flow corral-next -> {engine, protocol}; daemon -> {engine, facts, herdr, protocol, types}; engine -> {facts, git, herdr, types}; protocol and facts -> types. `corral-types`, `corral-git` and `corral-herdr` depend on no other crate here.

Classic is retired. Use the v0.4.0 tag for its source and guides. `.corral/lenses/` is live input to the current engine and app.

`apps/corral-app/` is the Corral app (NSH, Rust): its own Cargo workspace and toolchain (rustc 1.97),
not a workspace member, and not built or tested by `scripts/gate.sh`. It depends on
`crates/corral-protocol` by path and reaches the daemon only through its socket. Read
`apps/corral-app/AGENTS.md` before working there; run its commands from that directory.

## Conventions

- Rust 2024, MSRV 1.94, errors via `anyhow` with context on every external call.
- Module ceiling: 800 SLOC per Rust file (tests in sibling `*_tests.rs` files or `#[cfg(test)] mod tests`).
- Public signatures in each crate's contract are fixed; change one only when the task requires it and update every caller.
- No `unwrap()`/`expect()` outside tests. No `todo!()` left in shipped code.
- Subprocesses (herdr, git) are invoked with argv, never through a shell, except agent launch commands which are sent to a herdr pane's shell by design.

### Commit messages

- Use Conventional Commits for new commit subjects, including squash commits: `type(scope): short imperative summary`.
- Pick the type that describes the change: `feat` for new behavior, `fix` for bug fixes, `docs` for documentation, `refactor` for behavior-preserving changes, and `test` for test-only changes.
- Use a short scope when one area owns the change, e.g. `feat(app): add model favorites` or `fix(engine): skip inherited artifacts`. For cross-cutting changes, omit the scope but keep the prefix, e.g. `fix: preserve favorites across sessions`.

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
- When fixing a test listed in `.config/gate-quarantine.toml`, remove its entry when the fix lands.

## Build and test

- Always build with the repo's `.cargo/config.toml` (it disables the crashing global sccache wrapper). If a build fails with an sccache error, prefix the command with `RUSTC_WRAPPER=`.
- Gates retain Cargo's worktree-local target by default, keeping concurrent candidates
  from overwriting each other's top-level CLI executable. Explicit `CARGO_TARGET_DIR` is
  respected; share executable targets only with external serialization through execution,
  not just Cargo's build lock. Compile once per gate and reuse its candidate-bound metadata.
- Check one crate: `cargo check -p <crate>`; test one crate: `cargo test -p <crate>`.
- The current binaries are `corral-next` and `corrald-next`. Builds never enable workspace services.
- Test binaries carry no debuginfo (`[profile.test] debug = false`); panic messages keep
  their file and line. When a backtrace needs line numbers, rerun that one test with
  `CARGO_PROFILE_TEST_DEBUG=line-tables-only cargo test -p <crate> <name>`.
- Run the gate named by your run contract. The full `scripts/gate.sh` checks formatting, the 800-SLOC Rust ceiling, Clippy, workspace tests and doctests, and builds the current binaries. The app has a separate gate.
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
  See [gate cost policy](docs/guide/gate-costs.md) for added-test cost reporting.
  Keep new shared-server tests in `herdr`; fake-engine tests remain parallel.
- The script uses null stdin and preserves gate identity exports; tests isolate their own subprocess environments. Run its targeted temporary-fixture regressions with `python3 scripts/test_gate.py`,
  `python3 scripts/test_gate_select.py`, `python3 scripts/test_gate_policy.py`.
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
scripts/load-sandbox.sh --cpus 2 --memory 2g --pids-limit 256 --burners 2 -- cargo test -p corral-engine
```

It runs the command and optional burners in one Podman container (default 2 CPUs,
hard maximum 4; default 2g memory and 256 PIDs), then forcibly removes it on success,
failure, or interruption. Never fall back to host load if Podman/cgroups are unavailable;
report the blocked check instead. This does not authorize an extra gate or change its tier.
See [load sandbox](docs/guide/load-sandbox.md) for image prerequisites and smoke checks.

## herdr in tests

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
different server.

Tests that need herdr must skip (return early with a printed note) when
`herdr --session corral-test workspace list` fails. Never start an owner-configured server to
make a skipped test run. No plugin-disable CLI flag is assumed.
