# Corral app

Corral is a Rust app built on [NSH 0.5.0](https://nsh.dev), with desktop, web, phone and terminal forms. The crate is `nsh-app-corral` and its root component is `App`. By default it presents the interactive mockup's content using fixtures in `src/model/`, and actions change local state only. Started with a daemon socket, a native build shows `corrald`'s health and board instead; see [Live data](#live-data).

The app includes:

- **Inbox**: items needing attention and a question handover with evidence and answers.
- **Now**: the workspace, task details and plan graph. **Changes** shows changes since the last visit.
- **Plans**: plan lists, details, review and graphs.
- **Agents**: the agent wall, sessions, quick thread and filed chat records.
- **Review**: a candidate replay, verdicts, evidence and diff.
- **Timeline**: the activity timeline and range selection.
- **Usage**: illustrative provider costs, quotas and profiles.
- **Settings**: device pairing, notifications, profiles and other preferences.
- **Ask**: a composer for questions, ideas and instructions, plus assistant threads. Replies to newly typed messages are not simulated, as in the mockup.

The command palette's **Demo** group changes Connection, Data and Assistant states and opens the hotkey window, share sheet and lock-screen push demos. Open the palette with `Ctrl K`.

The app lives in the corral repository as `apps/corral-app/`. It is its own Cargo workspace with its own toolchain, not a member of corral's workspace. Design sources are `/home/pawis/corral-app-design/REQUIREMENTS.md`, `ARCHITECTURE.md` and `mockup/`.

## Quick start

Install NSH 0.5.0 with `curl -fsSL https://nsh.dev/install.sh | sh`, then run these commands in the project directory:

```sh
nsh run --target web
# Or, in the current terminal:
nsh run --target tui
```

The web development server is at http://127.0.0.1:8790/ and rebuilds on save.

## Live data

With a `corrald` running for the workspace (see `docs/guide/daemon.md` in this repository), start the terminal build with the daemon's socket:

```sh
CORRAL_SOCKET="$(corral daemon socket)" nsh run --target tui
```

The app then opens in its Daemon data state. The health strip and Now come from the daemon and follow it as it changes; if the daemon goes away the last board stays, marked stale, and the app reconnects by itself. The daemon serves only health and the board today, so the other destinations say so, and the selected task shows the facts the board row carries (no steps, evidence or session yet). The app is read-only towards the daemon: it sends no commands.

The palette's Demo group switches Data back to Fixture. The web build cannot open a Unix socket and has no Daemon state.

## Commands

Run one cargo or NSH build at a time on this shared host.

| Command | Purpose |
|---|---|
| `nsh check` | Check the app |
| `cargo test` | Run unit tests and headless shell tests |
| `nsh test --target tui` | Run TypeScript test files on the terminal target; none at present |
| `nsh test --target web` | Run TypeScript test files on the web target; none at present |
| `nsh package --target web` | Write a static site to `.nsh/out/Corral/`; a cold build takes about 2m 40s |
| `python3 -m http.server --bind 127.0.0.1 --directory .nsh/out/Corral 4173` | Serve the packaged site on loopback |
| `nsh run --target web` | Run the web development server |
| `nsh run --target tui` | Run in the current terminal |

## Keys

Destination keys are `i` Inbox, `n` Now, `.` Changes, `p` Plans, `a` Agents, `r` Review, `t` Timeline, `u` Usage and `,` Settings. `/` opens Ask, `Ctrl K` opens the palette and `Esc` closes the newest overlay.

## Structure

- `src/lib.rs`: root `App`.
- `src/model/`: pure Rust data, wording, state, reducers and view builders, with no `nsh_*` imports or I/O. Shared types and fixtures sit alongside `board/` and the destination domain directories; `work/` covers Now and Changes.
- `src/state.rs`: app signals, effect dispatch and persistence.
- `src/ui/kit/`: shared pieces used to build screens, including rows, page scaffolds, buttons, notices, tables and overlays.
- `src/ui/`: shared UI, Ask and composer surfaces, palette, tokens, icons and terminal-specific pieces in `term.rs`.
- `src/shell/`: desktop, phone and terminal shells.
- `src/screens/`: destination screen modules, their subdirectories and page routing.
- `tests/shell.rs`: headless shell tests.
- `theme.json` and `nsh.json`: theme and app configuration.
- `assets/fonts/` (one static face per weight) and `assets/icons/` (the mock's glyphs as SVG).
- `docs/NSH-NOTES.md`: what NSH 0.5.0 does differently from what its docs suggest, with sources.
- `docs/shots/`: renders of the content build; see Screenshots below.

NSH Storage persists only `corral.destination`, `corral.settings`, `corral.theme`, `corral.data`, `corral.conn`, `corral.mode` and `corral.now.layout`.

## Screenshots

`docs/shots/` contains desktop and phone web renders in dark and light themes, and terminal renders at 120x36 and 80x24 with a `.txt` capture beside each PNG. Web renders are 1440x900 for desktop and 390x844 for phone, with two desktop renders at 900x600.

Start with `desktop-dark-now.png`, `desktop-dark-question.png`, `phone-dark-inbox.png`, `phone-dark-question.png` and `terminal-120x36-now.png`.

The build's scripts rendered the packaged web build in headless Chromium with a desktop pointer and phone emulation, and rendered the terminal target to PNG in a private tmux. These are not captures from a real phone or the owner's terminal.

## Toolchain on this host

NSH installs under `~/.nsh` and is linked at `~/.local/bin/nsh`. NSH 0.5.0 crates require rustc 1.97; `rust-toolchain.toml` pins 1.97.1 and the `wasm32-unknown-unknown` target. Corral itself stays on 1.94.

`.cargo/config.toml` disables the host's crashing global sccache wrapper and sets `CC_wasm32_unknown_unknown` and `AR_wasm32_unknown_unknown` to clang 18 unpacked under `~/.local/opt/llvm18`. This host has no system clang and no sudo. On a machine where `sudo apt install clang llvm` is available, the `[env]` table can be removed.

Native desktop backends are behind the off-by-default `desktop` Cargo feature because this host lacks GTK 4 development packages. On a machine with them (`sudo apt install libgtk-4-dev`), or on Windows or macOS, `cargo run --features desktop` starts the window. Whether `nsh run --target linux`, `windows` or `macos` works with the gated dependency is not verified. Do not build the `desktop` feature or the Linux or GPUI targets on this host.

## Not built

Not built: a daemon transport for the web and phone builds (the daemon speaks only its Unix socket), any command to the daemon, live data for destinations other than Now, native window chrome, tray, system-level global hotkey, share and push integration, biometric confirmation and installer packaging. The hotkey window, share sheet and lock-screen push are local demos. The web package command produces a static site, not an installer.
