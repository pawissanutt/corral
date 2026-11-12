# Agent guidance

## Scope

This app contains the mockup's destination content: Inbox and its question handover sheet, Now, Changes, Plans, Agents and sessions, Review, Timeline, Usage, Settings, the Ask composer and assistant threads. There is no daemon, protocol or network connection: data comes from fixtures in `src/model/`, and actions change local state only. Never touch `/home/pawis/miti-workspace/corral`; this project is its sibling, not part of that repository or its Cargo workspace. Design sources are `/home/pawis/corral-app-design/REQUIREMENTS.md`, `ARCHITECTURE.md` and `mockup/`.

Replies to newly typed Ask messages are not simulated, as in the mockup. Only these NSH Storage keys persist: `corral.destination`, `corral.settings`, `corral.theme`, `corral.data`, `corral.conn`, `corral.mode` and `corral.now.layout`.

## Module map

- `src/lib.rs`: root `App`.
- `src/model/`: pure Rust, with no `nsh_*` imports or I/O; holds data, wording, state, reducers and view builders for every form.
- `src/model/hue.rs`, `text.rs`, `effect.rs`: semantic colours, rich text, environment and reducer effects.
- `src/model/nav.rs`, `health.rs`, `fixture.rs`, `commands.rs`: navigation, health and fixture snapshots, and palette commands.
- `src/model/board/`: shared board snapshot, tasks, changes and review hunk.
- `src/model/inbox/`, `work/`, `plans/`, `timeline/`, `agents/`, `usage/`, `review/`, `settings/`, `assistant/`: domain fixtures and view logic; `work` covers Now and Changes.
- `src/state.rs`: `AppState`, signals, effect dispatch and persistence.
- `src/ui/kit/`: shared pieces every screen is built from, including marks, rows, page scaffolds, notices, tables and overlays.
- `src/ui/kit/page_phone.rs`: phone page scaffolds and helpers.
- `src/ui/`: tokens, atoms, icons, Ask and composer surfaces, palette and terminal-specific pieces in `term.rs`.
- `src/ui/ask.rs`, `src/ui/ask/`: docked composer, model list, hotkey window and share sheet.
- `src/shell/`: desktop, phone and terminal shells, navigation and overlays.
- `src/shell/lock.rs`: phone lock-screen push demo.
- `src/screens/`: destination renderers and page routing; look in the destination's module and its subdirectory for its screen.
- `tests/shell.rs`: headless shell tests.
- `theme.json`, `nsh.json`: theme and app configuration.

## How a screen is built

Each domain's model supplies a `State`, reducers and pure view builders. Read the tracked state and environment in a view, render the resulting content with `src/ui/kit/`, and dispatch actions with `AppState::act(signal, reducer)`, which updates the signal and runs returned `Effect`s. Keep wording, numbers and branching in the model. Screens support desktop/web, phone and terminal forms; phone pushed pages use `PhonePage`.

Give pressable controls and testable regions IDs in these shapes: `task-<id>`, `plan-<id>`, `inbox-<key>`, `agent-<id>`, `thread-<id>`, `seg-<group>-<value>`, `btn-<action>`, `opt-<n>`, and `page-title`. Preserve the shell IDs `tab-<slug>`, `nav-<slug>`, `settings-<slug>`, `ask-open`, `ask-close`, `health`, `health-source` and `count-<slug>`.

The command palette's Demo group switches Connection, Data and Assistant states and opens the hotkey window, share sheet and lock-screen push demos.

## Conventions

- Rust 2024; at most 800 SLOC per file.
- No `unwrap()`, `expect()` or `panic!` outside tests; no `todo!()`.
- No dead or commented-out code. Comments say why, not what.
- Route UI, UX and visual work to the `ui` agent.

## Shared host and toolchain

Run only one cargo or NSH build at a time. Never generate load. Never build the `desktop` feature or the Linux or GPUI targets on this host; GTK 4 development packages are unavailable. Do not run `cargo run`, `nsh run` without `--target`, or `nsh run --target linux` or `gpui` here. The available targets are headless, `tui` and `web`.

`rust-toolchain.toml` pins rustc 1.97.1 and `wasm32-unknown-unknown`; NSH 0.5.0 needs rustc 1.97. Keep the pin and `.cargo/config.toml`, which disables the crashing global sccache wrapper and points the wasm compiler tools at the host's clang 18 installation.

If another process holds `~/.cargo/.package-cache`, builds can use `CARGO_HOME` pointing to a directory with `registry`, `git`, `bin` and `config.toml` symlinked from `~/.cargo`, together with `CARGO_NET_OFFLINE=true`.

## NSH API

Use the [NSH documentation index](https://nsh.dev/llms.txt); documentation pages are available as `.md`. The NSH 0.5.0 crate sources are under `~/.cargo/registry/src/*/nsh-*-0.5.0/`. Read the relevant documentation and source or a real call before using an API.

## Tests

Keep tests few and name each for the defect it catches. Test at the lowest layer that can fail. Do not pin wording, styles or incidental layout.

There are 33 unit tests in `src/model/` and `src/ui/kit/rows.rs`, and 13 headless shell tests in `tests/shell.rs`.
