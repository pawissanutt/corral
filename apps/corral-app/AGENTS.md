# Agent guidance

## Scope

This app lives in the corral repository as `apps/corral-app/`. It is its own Cargo workspace with its own toolchain and is not a member of corral's workspace; corral's gate does not build or test it. Design sources are in `docs/app-design/` at the repository root. Read its `README.md` for authority and implementation-status caveats before using `REQUIREMENTS.md`, `ARCHITECTURE.md` and `mockup/`.

The app has three data states (`DataState`):

- **Fixture**: explicitly selected mockup preview. Content comes from fixtures in `src/model/`; actions change local state only.
- **Daemon**: an enrolled native Unix session (`CORRAL_SOCKET`, peer-UID trust and a private `0600` device-id file), or a paired browser session over an explicitly enabled loopback WebSocket with pinned Noise IK. Typed reads and permitted actions cover the current destinations; capabilities, authority, stale data and missing facts remain visible. Newly enrolled native devices and paired browsers have all authority flags, including `admin` and `assistant_act`, editable per device.
- **Disconnected**: no daemon connection. Normal startup must not substitute fixture facts. Unpaired browsers open Settings › Connection.

The web build pairs using a trusted-side offer and matching-code confirmation. Its encrypted credential and non-extractable wrapping key live in IndexedDB; only `corral.paired` is in NSH Storage. Revocation closes sessions and stops automatic reconnect. Forget returns to disconnected Connection. No browser shell executor is enabled.

Ask is one composer for assistant and PM messages; the daemon hosts the assistant, and the PM receives inbox messages. Drafts and bounded assistant sends persist per device/store, with cancellation and same-ID reconnect recovery; other offline actions refuse. Images and bounded UTF-8 files upload from the client; links are opaque. Never represent missing provider, quota or raw-terminal facts as available. NSH Storage keys are `corral.destination`, `corral.settings`, `corral.theme`, `corral.data`, `corral.conn`, `corral.mode`, `corral.now.layout` and the non-secret `corral.paired` marker.

## Module map

- `src/lib.rs`: root `App`.
- `src/model/`: pure Rust, with no `nsh_*` imports or I/O; holds data, wording, state, reducers and view builders for every form.
- `src/model/daemon.rs`: owned daemon feed and its pure updates, including typed domain data and command states.
- `src/model/session_feed.rs`: a watched session's open turn and continued transcript items, with their pure updates; `src/model/agents/` holds the session action bar (`controls.rs`), the chat input (`live_reply.rs`), the keys bar under a foreign harness's pane (`keys.rs`) and the transcript replay (`replay*.rs`, `items.rs`).
- `src/client/`: shared frame/session core, native Unix adapter and device-id persistence, browser WebSocket/Noise adapter and encrypted IndexedDB credentials, wire-to-model mapping, and root-owned tasks feeding `AppState::feed`.
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

The explicit fixture preview offers Demo controls; live palette entries use real data.

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

