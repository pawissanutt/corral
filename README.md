# Corral app

This is the shell-only Corral app, written in Rust on [NSH 0.5.0](https://nsh.dev). The crate is `nsh-app-corral` and its root component is `App`. It provides the navigation frame for desktop, web, phone and terminal.

The two data states are `fixture`, clearly labelled as fixture data, and `disconnected`. There is no daemon, protocol, client core or destination content in this build. The composer's Send button is disabled.

This is a sibling of `/home/pawis/miti-workspace/corral`, not part of that repository or its Cargo workspace. Design sources are `/home/pawis/corral-app-design/REQUIREMENTS.md`, `ARCHITECTURE.md` and `mockup/`.

## Quick start

Install NSH 0.5.0 with `curl -fsSL https://nsh.dev/install.sh | sh`, then run these commands in the project directory:

```sh
nsh run --target web
# Or, in the current terminal:
nsh run --target tui
```

The web development server is at http://127.0.0.1:8790/ and rebuilds on save.

## Commands

Run one cargo or NSH build at a time on this shared host.

| Command | Purpose |
|---|---|
| `nsh check` | Check the app |
| `cargo test` | Run model unit tests and headless shell tests |
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
- `src/model/`: pure Rust model, with no `nsh_*` imports; seed of the future sans-IO `corral-client` crate. Includes `mod.rs`, `nav.rs`, `health.rs` and `fixture.rs`.
- `src/state.rs`: app state and persistence.
- `src/ui/`: shared UI, including `tokens.rs`, `atoms.rs`, `composer.rs` and `palette.rs`, and `term.rs`, the terminal's own pieces (joined lines, framed regions, key caps, the top bar) used by the terminal shell and the shared UI's terminal branches.
- `src/shell/`: `desktop.rs`, `phone.rs` and `terminal.rs`, the shells; the shared UI branches on the form.
- `src/screens/`: destination frames and `now.rs`.
- `tests/shell.rs`: headless shell tests.
- `theme.json` and `nsh.json`: theme and app configuration.
- `assets/fonts/` (one static face per weight) and `assets/icons/` (the mock's glyphs as SVG).
- `docs/NSH-NOTES.md`: what NSH 0.5.0 does differently from what its docs suggest, with sources.
- `docs/shots/`: the shell as it ran on 2026-11-12: web desktop and phone browser in both themes, and the terminal at 120x36 and 80x24 as PNG with its text grid beside each (`terminal-<size>-<state>[-light|-bandless|-16|-mono]`).

NSH Storage holds preferences under `corral.destination`, `corral.settings`, `corral.theme`, `corral.data` and `corral.now.layout`.

## Toolchain on this host

NSH installs under `~/.nsh` and is linked at `~/.local/bin/nsh`. NSH 0.5.0 crates require rustc 1.97; `rust-toolchain.toml` pins 1.97.1 and the `wasm32-unknown-unknown` target. Corral itself stays on 1.94.

`.cargo/config.toml` disables the host's crashing global sccache wrapper and sets `CC_wasm32_unknown_unknown` and `AR_wasm32_unknown_unknown` to clang 18 unpacked under `~/.local/opt/llvm18`. This host has no system clang and no sudo. On a machine where `sudo apt install clang llvm` is available, the `[env]` table can be removed.

Native desktop backends are behind the off-by-default `desktop` Cargo feature because this host lacks GTK 4 development packages. On a machine with them (`sudo apt install libgtk-4-dev`), or on Windows or macOS, `cargo run --features desktop` starts the window. Whether `nsh run --target linux`, `windows` or `macos` works with the gated dependency is not verified. Do not build the `desktop` feature or the Linux or GPUI targets on this host.

## Not built

Destination content, transport, sending, native window chrome, tray, global hotkey window, share sheet, push, biometric confirm and packaging are not built. The web package command produces a static site, not an installer.
