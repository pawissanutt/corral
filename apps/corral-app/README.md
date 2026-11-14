# Corral app

Corral is a Rust app built on [NSH 0.5.0](https://nsh.dev), with desktop, web, phone-layout and terminal forms. The crate is `nsh-app-corral` and its root component is `App`. Normal startup uses live daemon data or shows an honest disconnected, unknown or unpaired state; fixture data is an explicitly selected preview only. See [Live data](#live-data).

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

The explicit fixture preview includes the palette's Demo controls and local hotkey window, share sheet and lock-screen push demos. Open the palette with `Ctrl K` and choose `Data: fixture` to enter it.

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

Use a daemon built from this checkout; an older installed daemon may not support the
app contracts. The frontend does not emulate missing capabilities.

With a `corrald` running for the workspace (see `docs/guide/daemon.md` in this repository), start the terminal build with the daemon's socket:

```sh
CORRAL_SOCKET="$(corral daemon socket)" nsh run --target tui
```

The native app uses the Unix socket and peer-UID trust. In Settings › Connection, name the device
and enrol it. The device id is stored in a private `0600` file at
`$XDG_STATE_HOME/corral-app/devices.json` (fallback `$HOME/.local/state`), keyed by the
daemon's store. This is a file fallback, not an OS keychain; Settings identifies it.
A restart attaches that enrolled identity.

### Browser

Opening the web app without a saved pairing goes straight to Settings › Connection,
even if an older build saved a mockup destination or Fixture preference. Until paired,
the shell reports **not connected**, without mockup dates, health or task counts.
Other destinations do not fall back to mockup content. Fixture is an explicit
data choice, not a connection fallback.

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

Pairing switches to live data automatically. Reload preserves the device and daemon pin:
the browser stores its credential encrypted in IndexedDB with a non-extractable
WebCrypto AES-GCM key. NSH Storage holds only the non-secret paired marker, never keys.
The WebSocket carrier uses Noise IK; initial pairing uses single-use Noise IKpsk2.

### Live behavior and revocation

Typed live subscriptions, details and permitted actions cover Inbox, Now, Changes, Plans,
Agents, Review, Timeline, Usage, Settings and Ask. Availability depends on negotiated
capabilities and device authority; missing facts, provider failures and unsupported operations
are shown as such. Quotas without a source are unknown, not invented headroom. Raw terminal
access is not supplied for daemon-hosted assistants.

Native enrolled devices have Admin authority. A newly paired browser has `reply`, `act` and
`assistant` authority, but not `admin` or `assistant_act`; trusted-side administration
controls device authority. Commands use canonical revisions and device-bound confirmations
where required. They do not run locally when disconnected.

On link loss, last known data is marked stale and subscriptions reconnect. Drafts and the
bounded assistant message outbox persist per device and store; queued sends retain their
identity. Other offline actions are refused. An uncertain command result is recovered with
the same command ID, not a new execution; conflicts and unknown results remain distinct.
Settings counts devices only from a listing completed on the current connection, native or
browser; before it, after a failed listing, or without an authenticated link, the count is
shown as loading, unavailable or needing a connection, never as zero.

Accepting a send into the durable outbox also retires its matching saved composer draft.
Reloading or remounting the composer does not restore that sent text. Unsent drafts stay
with their own quick thread or named session.

The Upcoming bucket includes ideas, backlog and ready tasks, and Worker runs in progress
includes any run that has not ended, which may be idle or waiting; each row retains its
canonical state, and Timeline waits name no owner unless one is known. Task bodies are
selectable Markdown source, without automatic image fetches or HTML.

Reopening a scoped subscription waits for the previous close response. Retired pages are
neither displayed nor acknowledged; rapid reopen requests coalesce within the original
deadline. Sent assistant messages retire when their canonical thread entry arrives.

Agents › Filed from chat reads a server-wide filing registry, including ended threads,
without opening their histories. Newest, Newer and Older page through at most 50 summaries
at a time. The count is corrald's total, not the device's loaded thread count. This history
remains available without a configured assistant runtime.

The first daemon start builds the derived filing index from canonical filing records;
an initialization failure stops startup with an error. Tasks, conversations and billing
records are unchanged by this indexing.

Delivered owner messages retain their original typed time; a different receipt time is
shown separately. Receipt order still orders the conversation. If an assistant cannot run,
File as idea saves the owner's words as written rather than inventing an AI summary.

`corral device list [--json]` lists identities; `corral device revoke <id>` closes their
live sessions and stops automatic reconnect. Native devices can explicitly enrol again.
Browsers recover with **Forget this browser**, which returns to Connection for pairing.
Forget removes local credentials, not the daemon's record; revoke obsolete records
from the trusted side.

### Ask and attachments

Ask uses one composer for questions and ideas, addressed to the assistant or PM. Assistant
threads use a supervised daemon-hosted assistant and stream actual replies; PM messages enter
the PM inbox rather than steering supervision. Settings › Assistant manages configured profiles
and the assistant profile. The model selector offers `corral-pi` profiles that name a model;
changing it applies to the next message. Provider setup and authentication must work on the
daemon host; discovery or a configured model does not prove access or execution. See
[profile setup](../../docs/profile-setup.md). An alternate rollover summary profile is not a
canonical supported setting.

Named sessions use the same model picker and durable transcript. Stop interrupts the current
turn; End session stops the host while retaining its record and conversation.

The composer can attach a saved image or bounded UTF-8 file, or an opaque link. Native
clients can enter a local-client file path; it is not a daemon host path. Links are not fetched
by the app. Attachment uploads require a connection; ready uploads may accompany a queued
message, but expiry or delivery failure is shown. Queued messages can be cancelled; a
cancellation that loses a delivery race is reported rather than presented as withdrawn.

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

Not built: LAN or relay access, native window chrome, tray, system-level global hotkey,
share and push integration, biometric confirmation and installer packaging. The hotkey
window, share sheet and lock-screen push are local demos. The web package is a static site,
not an installer; `com.example.corral` is a placeholder identifier before release.
GTK desktop and physical phone builds have not been exercised on this host; phone web
layout is part of the app. Local terminal and paired Chromium verification is not evidence
of native OS integration or a full release gate.
