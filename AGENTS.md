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
- Run `scripts/gate.sh` before `corral done`. This is the full review gate: formatting, the exact 800-SLOC Rust ceiling, Clippy, workspace tests in `corral-test`, and a candidate build plus docs checks.
- The script uses null stdin and preserves gate identity exports; tests isolate their own subprocess environments. Run its temporary-fixture regression checks with `python3 scripts/test_gate.py`.

## herdr in tests

NEVER create panes or workspaces in the user's live herdr session. Use the isolated test server:
start it if needed with `herdr --session corral-test server` (background), and target it with
`herdr --session corral-test <cmd>` or `CORRAL_HERDR_SESSION=corral-test` for corral itself.
Tests that need herdr must skip (return early with a printed note) when `herdr --session corral-test workspace list` fails.
