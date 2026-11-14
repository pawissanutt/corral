# Corral app

Corral is a Rust app built on [NSH 0.5.0](https://nsh.dev), with desktop, web, phone and terminal forms. The crate is `nsh-app-corral` and its root component is `App`. Unpaired browsers open Connection for pairing; paired browsers and enrolled native builds can show `corrald`'s live health and board. Native builds without `CORRAL_SOCKET` open the explicit fixture preview, where actions change local state only. See [Live data](#live-data).

Fixture mode includes:

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

The app lives in the corral repository as `apps/corral-app/`. It is its own Cargo workspace with its own toolchain, not a member of corral's workspace. The [design handoff](../../docs/app-design/README.md) contains the requirements, architecture proposal and runnable mockup, with their authority and implementation-status caveats.

## Quick start

Install NSH 0.5.0 with `curl -fsSL https://nsh.dev/install.sh | sh`, then run these commands in the project directory:

```sh
nsh run --target web
# Or, in the current terminal:
nsh run --target tui
```

The web development server is at http://127.0.0.1:8790/ and rebuilds on save.
Use different dev-server ports when running both hosts: `nsh run --target web --port 8790`
and `nsh run --target tui --port 8791`. NSH replaces an existing development session
on the same port, even when its target differs.

## Live data

With a `corrald` running for the workspace (see `docs/guide/daemon.md` in this repository), start the terminal build with the daemon's socket:

```sh
CORRAL_SOCKET="$(corral daemon socket)" nsh run --target tui
```

The native app opens in its Daemon data state. In Settings › Connection, name the device
and press **Enrol** to load Now. The device id is stored in a private `0600` file at
`$XDG_STATE_HOME/corral-app/devices.json` (fallback `$HOME/.local/state`), keyed by the
daemon's store. This is a file fallback, not an OS keychain; Settings identifies it.
A restart attaches that enrolled identity without asking again.

### Browser

Opening the web app without a saved pairing goes straight to Settings › Connection,
even if an older build saved a mockup destination or Fixture preference. Until paired,
the shell reports **not connected**, without mockup dates, health or task counts.
Other destinations do not fall back to mockup content. Fixture remains an explicit
choice in the palette's Demo group.

Enable the optional listener in the workspace's `.corral/corral.toml`, then restart
`corrald`. It is off by default; the browser endpoint must bind `127.0.0.1`:

```toml
[daemon]
listen = "127.0.0.1:7841"
web_origins = ["http://127.0.0.1:8790"]
```

Run `corral device pair`, or choose **Pair a browser** in the enrolled native app's
Settings › Devices. Open the offered local pairing link. It opens Settings › Connection
with the code prefilled; press **Pair**. The short-lived code is in the URL fragment,
not the query/path, and the app clears it from the address bar before pairing.
It is never saved to NSH Storage. Raw-code entry remains available when no link is offered.

Compare the six-digit codes on both devices, then confirm in the terminal or native
app. A pairing code expires after two minutes; claiming it starts a separate two-minute
confirmation window. Opening a link does not claim, approve or replace a stored identity.
The browser cannot confirm itself. LAN access remains disabled.

Choose **Open live data** after pairing. Reload preserves the device and daemon pin:
the browser stores its credential encrypted in IndexedDB with a non-extractable
WebCrypto AES-GCM key. NSH Storage holds only the non-secret paired marker, never keys.
The WebSocket carrier uses Noise IK; initial pairing uses single-use Noise IKpsk2.

### Live behavior and revocation

Health and Now follow the daemon on both carriers. On ordinary link loss the last board
stays marked stale and reconnect restores subscriptions. Unserved destinations remain
explicitly unavailable; no fixture content is presented as daemon data. Neither app
sends task/question commands. Native device administration is separate from that boundary;
browser sessions can read only board and health.

`corral device list [--json]` lists identities; `corral device revoke <id>` closes their
live sessions and stops automatic reconnect. Native devices can explicitly enrol again.
Browsers recover with **Forget this browser**, which returns to Connection for pairing.
Forget removes local credentials, not the daemon's record; revoke obsolete records
from the trusted side.

The palette's Demo group still switches Data back to Fixture.

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

NSH Storage persists `corral.destination`, `corral.settings`, `corral.theme`, `corral.data`, `corral.conn`, `corral.mode`, `corral.now.layout` and the non-secret browser marker `corral.paired`.

## Screenshots

`docs/shots/` contains desktop and phone web renders in dark and light themes, and terminal renders at 120x36 and 80x24 with a `.txt` capture beside each PNG. Web renders are 1440x900 for desktop and 390x844 for phone, with two desktop renders at 900x600.

Start with `desktop-dark-now.png`, `desktop-dark-question.png`, `phone-dark-inbox.png`, `phone-dark-question.png` and `terminal-120x36-now.png`.

The build's scripts rendered the packaged web build in headless Chromium with a desktop pointer and phone emulation, and rendered the terminal target to PNG in a private tmux. These are not captures from a real phone or the owner's terminal.

## Toolchain on this host

NSH installs under `~/.nsh` and is linked at `~/.local/bin/nsh`. NSH 0.5.0 crates require rustc 1.97; `rust-toolchain.toml` pins 1.97.1 and the `wasm32-unknown-unknown` target. Corral itself stays on 1.94.

`.cargo/config.toml` disables the host's crashing global sccache wrapper and sets `CC_wasm32_unknown_unknown` and `AR_wasm32_unknown_unknown` to clang 18 unpacked under `~/.local/opt/llvm18`. This host has no system clang and no sudo. On a machine where `sudo apt install clang llvm` is available, the `[env]` table can be removed.

Native desktop backends are behind the off-by-default `desktop` Cargo feature because this host lacks GTK 4 development packages. On a machine with them (`sudo apt install libgtk-4-dev`), or on Windows or macOS, `cargo run --features desktop` starts the window. Whether `nsh run --target linux`, `windows` or `macos` works with the gated dependency is not verified. Do not build the `desktop` feature or the Linux or GPUI targets on this host.

## Not built

Not built: task/question commands to the daemon, live data for destinations other than Now and connection/device Settings, remote/LAN/relay transport, native window chrome, tray, system-level global hotkey, share and push integration, biometric confirmation and installer packaging. The hotkey window, share sheet and lock-screen push are local demos. The web package command produces a static site, not an installer. Native window-backend connection smoke remains unverified on this GTK-less host; terminal and Chromium journeys are recorded in [the connection increment](../../docs/app-design/NEXT-STEP.md#verification-receipt).
