# Corral app

Corral is a Rust app built on [NSH 0.5.1](https://nsh.dev), with desktop, web, phone-layout and terminal forms. The crate is `nsh-app-corral` (`0.1.0`); `nsh.json` currently declares package version `1.0.0`, build `1`. These are diagnostic/package versions, not protocol compatibility. The root component is `App`. Normal startup uses live daemon data or shows a disconnected, unknown or unpaired state; sample data is an explicitly selected preview only. See [Live data](#live-data).

Destinations:

- **Inbox**: items needing attention and a question handover with evidence and answers.
- **Now**: a single list with a Plan, Run, Gate, Review and Land loop band; Needs-you owner moves from the inbox with one action each; Corral and Workers task rows with named stage rails; a Waiting fold; and a Closed section. A side column shows the digest since the last visit or the selected ask or task.
- **Plans**: a list with member progress. An executing plan shows a headline, member graph and Path to done; an awaiting plan shows its document, reviewer verdicts and a pinned decide bar.
- **Builds**: records interactive build sessions publish (outcome, intent, slices, gate
  attempts, review rounds, agent messages, owner decisions, usage, timeline); never tasks.
  Needs `builds.v1`; a daemon without it shows Builds as unavailable.
- **Agents**: a roster with one row per agent session showing state, what it is doing, a 20-minute activity strip, last output, since and cost. Among assistant sessions, only the quick thread is a roster row, pinned first; named assistant sessions appear in the Assistant sessions section below. Activity older than two minutes is labelled with its recorded window rather than `now`. The side panel offers Summary (waits-on-you, steps, recent turns and reply) and Terminal; **Open session** opens the full session page. Activity, last-output and step facts require `agents.activity.v1` and are unknown without it.
- **Review**: Brief with a readiness chain, changes with inline diffs and the asked-for checklist; Diff; Gate log with Report and Artifacts when present; and a pinned decide bar.
- **Timeline**: the activity timeline and range selection.
- **Usage**: reported costs, completeness and sourced quota facts; absent facts stay unknown.
- **Settings**: device pairing, authority, notifications, profiles and local preferences.
- **Ask**: one composer for questions, ideas and instructions, addressed to the assistant or PM.

Counts are an icon and number, with the sentence in a hint on hover or keyboard focus on desktop and in the status line in a terminal; phone rows keep one explanatory line. The top-bar health display is one `healthy` chip when every component is healthy and names the component otherwise.

Projects (`projects.v1`): the sidebar (phone: More › Projects) lists the workspace's
configured projects with open, running and needs-me counts, plus Unassigned when a task
names no project. A **Project page** is Now scoped to that project: its loop band, Needs you,
Corral, Workers, Plans, Sessions, Waiting and Closed, followed by Builds and burn-up. The side
column shows project facts and the scoped digest; a row opens its record rather than a side panel.
Now, Changes, Plans, Builds, Inbox, Agents and Review each
keep their own project filter; rows show a project chip under All when there are two or
more projects, and Now can group by project. Ask sends the project in scope as context,
so the assistant files Ideas there unless told otherwise. A daemon without `projects.v1`
supplies project names only from task rows, without counts. When the daemon's complete
project list is empty the workspace has no project management: only Ask, Agents, Usage and
Settings are offered, and the landing explains how to add a project. The palette offers
**Preview: no projects** for that state.

Disconnected screens offer **Connect** and **Preview with sample data**. Preview exercises the same live screens and actions with sample data, never the daemon; it is not restored on restart. The UI uses real dates, not the mockup's date. Settings › General › **This device** exposes local preferences even without a connection. The palette (`Ctrl K` or `?`) also offers **Data: fixture**. Its Demo controls include hotkey, share-sheet and lock-screen-push illustrations where mounted; those illustrations do not establish OS integration.

The app lives in the corral repository as `apps/corral-app/`. It is its own Cargo workspace with its own toolchain, not a member of corral's workspace. The [design handoff](../../docs/app-design/README.md) contains the requirements, architecture proposal and runnable mockup, with their authority and implementation-status caveats.

## Quick start

Install NSH 0.5.1 with `curl -fsSL https://nsh.dev/install.sh | sh` (or `nsh update`), then run these commands in the project directory:

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
Other destinations do not fall back to sample content. **Connect** opens Settings › Connection;
**Preview with sample data** is an explicit, launch-only choice.

Enable the optional listener in the workspace's `.corral/corral.toml`, then restart
`corrald`. It is off by default. This example is local-only:

```toml
[daemon]
listen = "127.0.0.1:7841"
web_origins = ["http://127.0.0.1:8790"]
```

Run `corral device pair`, or choose **Pair a device** in Settings › Devices on any
administrator device, browser or native. Open the offered pairing link or QR. It opens Settings › Connection
with the code prefilled; press **Pair**. The short-lived code is in the URL fragment,
not the query/path, and the app clears it from the address bar before pairing.
It is never saved to NSH Storage. Raw-code entry remains available when no link is offered.

Compare the six-digit codes on both devices, then confirm in the CLI or existing
administrator app. A pairing code expires after two minutes; claiming it starts a separate
two-minute confirmation window. Opening a link does not claim, approve or replace a stored
identity. The new device cannot confirm its own pending pairing.

LAN/Tailscale access is opt-in: `daemon.listen` accepts one concrete numeric unicast
interface, not wildcard/multicast addresses. Remote browsers need an HTTPS-served app,
an exact `web_origins` entry and `daemon.web_endpoint = "wss://host[:port]"`.
The operator supplies TLS through a reverse proxy or Tailscale Serve; corrald has no
embedded TLS. Noise remains end-to-end inside WSS. See the
[daemon setup](../../docs/guide/daemon.md#enrolled-app-connections) for proxy paths and examples.

Pairing switches to live data automatically. Reload preserves the device and daemon pin:
the browser stores its credential encrypted in IndexedDB with a non-extractable
WebCrypto AES-GCM key. NSH Storage holds only the non-secret paired marker, never keys.
The WebSocket carrier uses Noise IK; initial pairing uses single-use Noise IKpsk2.

### Live behavior and revocation

Typed live subscriptions, details and permitted actions cover Inbox, Now, Changes, Plans,
Builds (read-only), Agents, Review, Timeline, Usage, Settings and Ask. Availability depends on
negotiated capabilities and device authority; missing facts, provider failures and unsupported
operations
are shown as such. Usage totals, daily costs, Timeline and burn-up retain bounded partial
results with completeness/scan-coverage notes; incomplete or unpriced totals are not complete
bills. Usage says its coverage once per page, in calendar days, and per provider or profile
only where that differs. Quota exhaustion/reset evidence is shown where sourced; unknown
limits, usage, windows and headroom stay unknown: a provider without a quota observation says
so once, and an unrecorded figure reads `—`.

Browser and native clients have the same authority model. Newly enrolled devices have
all five flags (`reply`, `act`, `assistant`, `assistant_act`, `admin`), editable per device.
Any administrator device can pair/revoke devices and edit authority. A device may lower
its own flags but cannot raise them. Changes setting `admin=true` require a fresh
device-bound preview/proof. Authority-change notifications refresh the session and
invalidate pending confirmations without reconnecting.

Commands use canonical revisions. Confirm-class actions include land/approve, plan
acceptance, revert, finishing a task, session termination and assistant-permission
revocation. Dispatch, queue, park/unpark, verdicts and unstick are Act-class. Config,
mode changes and raw terminal keys are Admin-class. Confirm and Admin commands require
a tap against corrald's current preview plus device-bound proof, not a biometric check.
Reply-class covers answers, follow-ups, task replies/comments and annotation replies.
The canonical table is `crates/corral-protocol/src/cmd/mod.rs::Command::app_class`.

On link loss, retained data is stale and subscriptions reconnect. A restart can open the
last complete Board/Inbox/Plans/Builds and Health snapshot for the saved credential, explicitly
stale/offline, not authenticated live data. Drafts and bounded assistant messages persist
per device/store; queued messages remain visible and cancellable with their original IDs.
Other offline actions refuse. IndexedDB transaction completion or native synced atomic
file replacement must acknowledge persistence before send acceptance, matching draft
retirement or command submission. Storage failures retain the composer's words.
Revision-checked per-item transactions and refreshes preserve other tabs' or instances'
messages and cancellation tombstones. Uncertain results recover with the same command ID,
never a new execution; conflicts and unknown results remain distinct.

Settings counts devices only from a listing completed on the current connection; before
it, after failure, or without an authenticated link, the count is loading, unavailable or
needs a connection, never an invented zero. Unsent drafts stay with their own quick thread
or named session, flush on unmount, and do not return after accepted send retirement.

The shared action menus, inspector buttons and palette expose dispatch, queue add/remove,
park/unpark, comment, new task, unstick, revert, plan cancel/add-member/move-member, review
verdict, question escalate/cancel/deadline, route/focus/move, mode off and permission revoke,
where the selected record, capability and authority allow them. Unavailable actions retain
their refusal reason. Review uses Unified below about 1100 pt of content width; wider
views offer side-by-side. Below about 900 pt, Plans and Review show either the list or the
opened record with a back control; opening a record by name from Now, Inbox, a link or the
palette shows that record.

Opening a task that is not on the board from Changes, plans, dependencies or the palette's `Open T-<n>` shows it read-only from its detail.

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

An agent session page carries the controls corrald reports for that session: Cancel, Pause
or Resume, Relaunch (Launch PM for an ended PM), Take over for a pane-hosted assistant, and
Terminate, which asks for confirmation first (with corrald's exact effects for task and PM
sessions). A control corrald does not offer is never pressable; one it refused for a reason
stays visible, dimmed, with the
reason. Words typed on the page go to the session as `session send`, after its current turn,
or interrupting it with Steer while it works; an assistant's page chats in its own thread
with the shared composer. The Agents roster's head offers Launch PM while no PM is live.

An agent session's transcript replays the recorded session as the corral-pi terminal shows
it, for every role: words as rendered Markdown, thinking (folding to one row), tool rows that
open into their arguments, output, diff or written content and recorded call and result, the
owner's `!` commands, images, compaction and branch dividers, model, effort, bookmark, stop
and error rows, run marks, host notices, presenter cards and each assistant turn's model,
stop and usage. A recorded item larger than its preview shows the preview until its
complete content arrives: words are fetched as soon as they are shown, everything else when
its row is opened. While the page is open it watches the session's live turn, drawing the
streamed words and running tools after the recorded messages until the transcript holds them,
and it keeps to the newest rows unless the owner scrolled up.

Pane-backed sessions negotiate `stream.terminal.v1`: acknowledged, scoped replacement
windows, published at most once per second, with at most 200 scrollback/screen rows,
52 KiB of line/style arrays and a 60 KiB CONTROL envelope. Slow consumers coalesce;
closing the view unsubscribes. Session Send/Cancel require Act; raw Keys require Admin
and fresh device proof. No terminal input queues offline. Daemon-hosted assistants have
no raw terminal; gate terminals remain recorded logs.

The first daemon start builds the derived filing index from canonical filing records;
an initialization failure stops startup with an error. Tasks, conversations and billing
records are unchanged by this indexing.

Delivered owner messages retain their original typed time; a different receipt time is
shown separately. Receipt order still orders the conversation. If an assistant cannot run,
File as idea saves the owner's words as written rather than inventing an AI summary.

`corral device list [--json]` lists identities; `corral device revoke <id>` closes their
live sessions and stops automatic reconnect. Native devices can explicitly enrol again.
Browsers recover with **Forget this browser**, which returns to Connection for pairing.
Forget removes local credentials and local state, not the daemon's record; revoke obsolete
records from any administrator device or the owner CLI. Failed Forget remains retryable.

### Ask and attachments

Ask uses one composer for questions and ideas, addressed to the assistant or PM. Assistant
threads use a supervised daemon-hosted assistant and stream actual replies; PM messages enter
the PM inbox rather than steering supervision. Settings › Assistant manages configured profiles
and the assistant profile. The model selector offers `corral-pi` profiles that name a model;
changing it applies to the next message. Provider setup and authentication must work on the
daemon host; discovery or a configured model does not prove access or execution. See
[profile setup](../../docs/profile-setup.md). Settings also supports optional
`assistant.summary_profile` for quick-thread compaction. It applies at the next
assistant-host start; unset uses the session model. Paid-API summary profiles are
refused because they have no separate spending authority. Summary usage retains
profile/model/connection attribution.

Named sessions use the same model picker and durable transcript. Stop interrupts the current
turn; End session stops the host while retaining its record and conversation.

The composer can attach a saved image or bounded UTF-8 file, or an opaque link. Native
clients can enter a local-client file path; it is not a daemon host path. Links are not fetched
by the app. Attachment uploads require a connection; ready uploads may accompany a queued
message, but expiry or delivery failure is shown. Queued messages can be cancelled; a
cancellation that loses a delivery race is reported rather than presented as withdrawn.

### Native entry points

On macOS, Windows and Linux, the app registers `mod+shift+space` system-wide
(Command on macOS, Control elsewhere). It raises the main window and opens the same
Ask composer as the in-app button, retaining the current draft and connection state.
Closing the main window hides it rather than terminating the root that owns the shortcut;
the native tray offers Show Corral, Ask and Quit Corral. Linux needs a desktop portal
supporting GlobalShortcuts, and its tray needs a StatusNotifierItem-capable panel
(stock GNOME needs an extension). Registration refusal is reported, not treated as success.
Set `CORRAL_ASK_SHORTCUT` before launching to use another NSH keystroke, for example
`ctrl+alt+space`; malformed overrides are refused without registering. Linux may ask
for confirmation or a different binding through its desktop portal.

The shortcut runs while the native app is running, including while its main window
is hidden, not after Quit. Packaged launches (and the direct desktop binary on
Windows/Linux) use single-instance behavior. Browser, terminal and headless targets
do not register system keys or a tray. Native execution has not been verified on
this host; `cargo run --features desktop` requires a desktop-capable build machine.

Inbound OS sharing is blocked by NSH 0.5.0's packaged-host API, not implemented by
the fixture share sheet. Android's
[`AppActivity.java:21–24,46–53`](https://github.com/longbridge/nsh/blob/v0.5.0/crates/nsh-platform-android/java/com/longbridge/nsh/AppActivity.java#L21-L53)
routes notifications and URL intents only;
[`NshView.java:610–613`](https://github.com/longbridge/nsh/blob/v0.5.0/crates/nsh-platform-android/java/com/longbridge/nsh/NshView.java#L610-L613)
rejects `ACTION_SEND`/`ACTION_SEND_MULTIPLE`. iOS's
[`app.rs:148–164`](https://github.com/longbridge/nsh/blob/v0.5.0/crates/nsh-platform-ios/src/app.rs#L148-L164)
routes URL contexts, not a share-extension payload; no packaged share-extension target
or inbound text/link/file share event is exposed. Desktop URL/file opening is not an
OS share-target registration API either. Web packaging
[`web.rs:1260–1287`](https://github.com/longbridge/nsh/blob/v0.5.0/crates/nsh-cli/src/web.rs#L1260-L1287)
generates a fixed `manifest.webmanifest` without `share_target`, and
[`WebConfig`](https://github.com/longbridge/nsh/blob/v0.5.0/crates/nsh-cli/src/package.rs#L532-L556)
has no manifest override/share-target setting. No unsupported manifest key or simulated
share receiver is installed. Paste, file picking and link attachments remain available
inside Ask.

### Web Push

Paired browsers can opt in under Settings › Notifications, with per-kind filters.
This requires service-worker support, notification permission, same-origin secure
hosting and operator-configured VAPID credentials/push-service origins. It is off by
default. Notifications are content-free; “push service accepted” does not prove display
or reading. See [Web Push setup](../../docs/guide/daemon.md#opt-in-web-push).
Push does not provide reach, and a stopped daemon cannot announce its own absence.

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

| Key | Action |
|---|---|
| `i n . p b a r t u ,` | Inbox, Now, Changes, Plans, Builds, Agents, Review, Timeline, Usage, Settings |
| `/` | Open Ask |
| `Ctrl K` or `?` | Open palette |
| `Ctrl B` | Toggle sidebar; narrow windows fold it to a rail, not remove it |
| `]` | Next Now inspector tab |
| `.` on a live Now/Review list | Selected row's action menu; otherwise Changes |
| `` ` `` | Toggle session transcript/terminal |
| `Ctrl Shift M` | Cycle assistant model |
| `Esc` | Close newest overlay; clear a Now or Agents selection before going back; in terminal session view, cancel an offered turn when no overlay is open |

On macOS, `Ctrl` shortcuts above use Command (`mod`). Letter shortcuts do not navigate
while editing a field. List keys (`j`/`k`, arrows, Enter and answer digits) bind only
while the destination list has focus or no control has focus, with no overlay/confirmation.
Inbox supports move/open/answer, Now move/open-session, and Plans and Builds move; question pages
leave arrows for scrolling. Tab moves focus; Enter/Space and arrows remain with focused
controls. Domain letters include Inbox `f`/`e`, Plans `J`/`K` (member) and `x` (preview
scale), and Review `s` (split when wide enough).

## Structure

- `src/lib.rs`: root `App`.
- `src/model/`: pure Rust data, wording, state, reducers and view builders, with no `nsh_*` imports or I/O. `preview.rs`/`preview/` supplies synthetic sample data to the live models; `work/` covers Now and Changes.
- `src/state.rs`: app signals, effect dispatch and persistence.
- `src/ui/kit/`: shared pieces used to build screens, including rows, page scaffolds, buttons, notices, tables and overlays.
- `src/ui/`: shared UI, Ask and composer surfaces, palette, tokens, icons and terminal-specific pieces in `term.rs`.
- `src/shell/`: desktop, phone and terminal shells.
- `src/screens/`: destination screen modules, their subdirectories and page routing.
- `tests/shell.rs`: headless shell tests.
- `theme.json` and `nsh.json`: theme and app configuration.
- `assets/fonts/`: one static file per face. Atkinson Hyperlegible Next and Mono set the interface; Cascadia Mono (regular, bold, italic, bold italic) sets an agent's live terminal, so it looks the same whatever the owner's own terminal font. Cascadia Mono is © Microsoft Corporation under the SIL Open Font License 1.1; its licence, `assets/fonts/CascadiaMono-LICENSE.txt`, ships with the app as a plain file of `assets!()`. `assets/icons/` holds the mock's glyphs as SVG.
- `docs/NSH-NOTES.md`: what NSH 0.5.0 does differently from what its docs suggest, with sources.
- `docs/shots/`: renders of the content build; see Screenshots below.

### Local storage

| Store | Keys / contents |
|---|---|
| NSH Storage | `corral.destination`, `corral.settings`, `corral.theme`, `corral.sidebar`, `corral.filters` (per daemon store: each screen's project filter and Now grouping), non-secret `corral.paired` |
| Browser IndexedDB `corral-app` v1, `credentials` | `device` (AES-GCM-wrapped credential and non-extractable key), retryable `forget` intent |
| Browser IndexedDB `corral-local-state` v1 | `records` plus `meta` (`revision`); transactional outbox/offline cache, separate from credentials |
| Native state directory | `$XDG_STATE_HOME/corral-app/` (fallback `$HOME/.local/state/corral-app/`): private `devices.json`, `durable.json`, retained locks |

Transactional records use `namespace/<hex(store + NUL + device)>/` with `meta`,
`draft/<id>`, `message/<id>`, `command/<id>` and `cancelled/<id>` entries. Offline hints
use `active` and `snapshot/<namespace>`; `forgotten/<namespace>` and
`migrated/<namespace>` prevent identity resurrection and repeated legacy imports.
The old NSH `corral.outbox.v1.<hex>` is migrated then removed; `corral.data`,
`corral.conn` and `corral.mode` are no longer read or written. Local records are bounded to 8 MiB across namespaces;
the outbox is bounded per namespace to 1 MiB, 128 messages, 32 drafts and 256 commands,
and the offline snapshot to 2 MiB. Quota pruning never discards undelivered messages.
Pairing secrets/proofs never go in the outbox or NSH Storage.

## Screenshots

`docs/shots/` contains desktop and phone web renders in dark and light themes, and terminal renders at 120x36 and 80x24 with a `.txt` capture beside each PNG. Web renders are 1440x900 for desktop and 390x844 for phone, with two desktop renders at 900x600.

Start with `desktop-dark-now.png`, `desktop-dark-question.png`, `phone-dark-inbox.png`, `phone-dark-question.png` and `terminal-120x36-now.png`.

The build's scripts rendered the packaged web build in headless Chromium with a desktop pointer and phone emulation, and rendered the terminal target to PNG in a private tmux. These are not captures from a real phone or the owner's terminal.

The Now, Agents, Plans and Review desktop and phone renders and their `terminal-120x36` and `terminal-80x24` captures were re-rendered after the main-loop rebuild from the packaged web build and a private tmux; the `terminal-120x36-now-16`, `-now-bandless`, `-now-light` and `-now-mono` variants and the other destinations' renders predate it.

## Toolchain on this host

NSH installs under `~/.nsh` and is linked at `~/.local/bin/nsh`. NSH 0.5.1 crates require rustc 1.97; `rust-toolchain.toml` pins 1.97.1 and the `wasm32-unknown-unknown` target. Corral itself stays on 1.94.

`.cargo/config.toml` disables the host's crashing global sccache wrapper and sets `CC_wasm32_unknown_unknown` and `AR_wasm32_unknown_unknown` to clang 18 unpacked under `~/.local/opt/llvm18`. This host has no system clang and no sudo. On a machine where `sudo apt install clang llvm` is available, the `[env]` table can be removed.

Native desktop backends are behind the off-by-default `desktop` Cargo feature because this host lacks GTK 4 development packages. On a machine with them (`sudo apt install libgtk-4-dev`), or on Windows or macOS, `cargo run --features desktop` starts the window. Whether `nsh run --target linux`, `windows` or `macos` works with the gated dependency is not verified. Do not build the `desktop` feature or the Linux or GPUI targets on this host.

## Not built

Relay rendezvous/dead-man delivery and native APNs/FCM push remain blocked on hosted TLS
infrastructure and publisher credentials/tokens. Inbound OS share targets are blocked by
NSH 0.5.0 as evidenced above. Native global Ask shortcut and tray are implemented but
unverified on this host; Web Push and opt-in direct LAN/Tailscale reach are implemented,
not evidence of exercised remote delivery.

Biometric confirmation, release installer packaging and native window chrome remain
unverified or unbuilt; `com.example.corral` is a placeholder before release. GTK desktop
and physical phone builds have not been exercised here. Existing headless/Chromium and
terminal captures do not establish real-device parity or a full release gate.
