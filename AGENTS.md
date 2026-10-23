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

## Build and test

- Always build with the repo's `.cargo/config.toml` (it disables the crashing global sccache wrapper). If a build fails with an sccache error, prefix the command with `RUSTC_WRAPPER=`.
- One shared target dir (`corral/target`); cargo serializes concurrent builds on its lock, which is expected.
- Check one crate: `cargo check -p <crate>`; test one crate: `cargo test -p <crate>`.
- Before `corral done`, run the gate your run contract names. In a tiered run that is `corral gate <id> --wait`, exactly once after the final rebase: corral selects the tier (quick during review rounds and for Plan-target members), records the receipt, and autopilot reuses it. Audited tasks then capture evidence, record it against the attempt that gate printed, run `corral done <id> --check`, then `corral done`. Evidence recorded for a quick receipt follows the candidate only through corral's authorized carries (tier promotion, clean replay); any other change needs fresh evidence. Legacy and untiered runs keep the gate commands their contract lists.
- `scripts/gate.sh` without `--tier` is the full tier, which corral runs for every landing on main and for plan verify: formatting, the exact 800-SLOC Rust ceiling, Clippy, `cargo nextest run --workspace` in `corral-test`, separate workspace doctests, and a candidate build plus docs checks. `scripts/gate.sh --tier quick` is the tier command corral runs with its gate context; it checks only the touched crates and the declared tests.
- Install cargo-nextest before running the gate. `.config/nextest.toml` caps tests at 12 slots, serializes live herdr fixtures, reports tests over 30 seconds, and disables retries. Keep new shared-server tests in its `herdr` group; fake-engine tests remain parallel.
- The script uses null stdin and preserves gate identity exports; tests isolate their own subprocess environments. Run its temporary-fixture regression checks with `python3 scripts/test_gate.py`.
- The Linux concurrent-dequeue regression requires `python3` and `strace`. It traces only disposable fixture processes and synchronizes on lock syscalls, never timing sleeps.

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
