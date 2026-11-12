# Agent guidance

## Scope

This project is the shell-only Corral app: navigation frames for desktop, web, phone and terminal. It has fixture and disconnected data states, not a daemon, protocol, client core or destination content. Send is disabled. Never touch `/home/pawis/miti-workspace/corral`; this project is its sibling, not part of that repository or its Cargo workspace. Design sources are `/home/pawis/corral-app-design/REQUIREMENTS.md`, `ARCHITECTURE.md` and `mockup/`.

## Module map

- `src/lib.rs`: root `App`.
- `src/model/mod.rs`, `nav.rs`, `health.rs`, `fixture.rs`: pure Rust model and seed of the future sans-IO `corral-client` crate. Do not import `nsh_*` here.
- `src/state.rs`: app state and persistence.
- `src/ui/mod.rs`, `tokens.rs`, `atoms.rs`, `composer.rs`, `palette.rs`: shared UI. `term.rs`: the terminal's own pieces (joined lines, framed regions, key caps, the top bar), used by the terminal shell and the `Form::Terminal` branches of the shared UI.
- `src/shell/mod.rs`, `desktop.rs`, `phone.rs`, `terminal.rs`: shells. Idiom-specific layout lives in `src/shell/`; shared UI branches on `Form` and takes terminal pieces from `ui/term.rs`.
- `src/screens/mod.rs`, `now.rs`: destination frames and Now workspace.
- `tests/shell.rs`: headless shell tests.
- `theme.json`, `nsh.json`: theme and app configuration.

## Conventions

- Rust 2024; at most 800 SLOC per file.
- No `unwrap()`, `expect()` or `panic!` outside tests; no `todo!()`.
- No dead or commented-out code. Comments say why, not what.
- Route UI, UX and visual work to the `ui` agent.

## Shared host and toolchain

Run only one cargo or NSH build at a time. Never generate load. Never build the `desktop` feature or the Linux or GPUI targets on this host; GTK 4 development packages are unavailable. Do not run `cargo run`, `nsh run` without `--target`, or `nsh run --target linux` or `gpui` here. The available targets are headless, `tui` and `web`.

`rust-toolchain.toml` pins rustc 1.97.1 and `wasm32-unknown-unknown`; NSH 0.5.0 needs rustc 1.97. Keep the pin and `.cargo/config.toml`, which disables the crashing global sccache wrapper and points the wasm compiler tools at the host's clang 18 installation.

## NSH API

Use the [NSH documentation index](https://nsh.dev/llms.txt); documentation pages are available as `.md`. The NSH 0.5.0 crate sources are under `~/.cargo/registry/src/*/nsh-*-0.5.0/`. Read the relevant documentation and source or a real call before using an API.

## Tests

Keep tests few and name each for the defect it catches. Test at the lowest layer that can fail. Do not pin wording, styles or incidental layout.
