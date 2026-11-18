# Corral app

Corral is a Rust app built on [NSH 0.5.1](https://nsh.dev), with desktop, web, phone-layout and terminal forms. The crate is `nsh-app-corral` (`0.1.0`); `nsh.json` currently declares package version `1.0.0`, build `1`. These are diagnostic/package versions, not protocol compatibility. The root component is `App`. Normal startup uses live daemon data or shows a disconnected, unknown or unpaired state; sample data is an explicitly selected preview only. See [Live data](#live-data).

Destinations (keys in the [key table](#keys)):

- **Now**: the home. One list with a Plan, Run, Gate, Review and Land loop band and a running-slots readout. Rows are grouped by who moves next: **Needs you** (Blocked tasks, and reviews ready to land), **Corral** (gate and review), **Workers** (running), then a Waiting fold and a **Closed** section (Done and Dropped, paged). Each row carries a named stage rail. A Blocked task shows the engine's reason and detail, a reply box and one button per typed next step. A side column shows the digest since the last visit, or the selected task (Story, Evidence and Session tabs). The old Inbox is the Needs you group; the `i` key and the `inbox` slug open Now.
- **Changes**: the engine's events since this device's last visit (filters All, Needs you, Verdicts, Runs, Tasks, Forced; Mark seen). A forced step names what it skipped.
- **Plans**: a plan is a parent task. A plan shows its members laid out by `depends_on`, its Markdown document and Path to done (the members not yet Done). There is no separate plan review.
- **Agents**: one row per live run (state, role, project, session, what it is doing, agent, quiet time), filtered All, Workers, Reviewers or Assistant. Chat sessions are listed below under Assistant sessions, with **+ New session**. A selected session's side panel shows its summary and terminal; **Open session** opens the full page. The old Builds destination is part of Agents.
- **Review**: candidates split into Your verdict and In verification. A candidate shows Brief (readiness chain: gate for HEAD, review for the round, your verdict), Diff, Summary and Artifacts, its rounds, an **Onto** line for the landing branch, and the decide bar: Approve and land, Request changes, Run gate, Start review.
- **Timeline**: lanes of runs, gates, reviews, landings, waits and idle per task or run (Today, 7 days; By task, By run), the gaps worth a look and the goal burn-up.
- **Usage**: tokens and cost that agent hosts reported with `corral usage add`, for 7 days, 30 days, this month or a custom range, grouped by profile, model, task or day. The header says how many runs the totals cover and why the rest are missing (they ran on a harness that reports no usage); requests with tokens but no price are counted apart. Quota stays unknown unless a harness reports it; an unrecorded figure reads `—`.
- **Artifacts**: the artifact finder; see [Artifact finder](#artifact-finder).
- **Settings**: Devices, This daemon, Connection, Notifications, Configuration (edits `corral.toml`) and General (local preferences).
- **Ask**: one composer for the one chat agent; see [Ask](#ask-and-attachments).

Counts are an icon and number, with the sentence in a hint on hover or keyboard focus on desktop and in the status line in a terminal; phone rows keep one explanatory line. The top-bar health display is one `healthy` chip when every component is healthy and names the component otherwise. The UI shows real dates.

Projects (`projects.v1`): the sidebar (phone: More › Projects) lists the workspace's
configured projects with open, running and needs-me counts, plus Unassigned when a task
names no project. A **Project page** is Now scoped to that project: its loop band, Needs you,
Corral, Workers, Plans, Sessions, Waiting and Closed, followed by burn-up. The side
column shows project facts and the scoped digest; a row opens its record rather than a side panel.
Now, Changes, Plans, Agents and Review each
keep their own project filter; rows show a project chip under All when there are two or
more projects, and Now can group by project. Ask sends the project in scope as context,
so the assistant files Ideas there unless told otherwise. A daemon without `projects.v1`
supplies project names only from task rows, without counts. When the daemon's complete
project list is empty the workspace has no project management: only Ask, Agents, Usage and
Settings are offered, and the landing explains how to add a project. The palette offers
**Preview: no projects** for that state.

One trusted local owner: a paired browser, an enrolled native app and the owner CLI have the same authority. There are no authority classes, previews or proofs; an enrolled or paired device is the owner. Every engine operation is on every task row and the engine decides what is allowed. See [Actions, refusals and confirmation](#actions-refusals-and-confirmation).

Disconnected screens offer **Connect** and **Preview with sample data**; see [Sample preview](#sample-preview). Settings › General › **This device** exposes local preferences even without a connection.

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

Use the daemon built from this checkout, `corrald-next` (`crates/corral-daemon`); an older
installed daemon does not support the app contracts. The frontend does not emulate missing capabilities.

With `corrald-next --root <workspace> run` serving the workspace, start the terminal build with its socket:

```sh
CORRAL_SOCKET=<workspace>/.corral-next/daemon.sock nsh run --target tui
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

Enable the optional listener, then restart `corrald-next`. It is off by default. In
`.corral-next/corral.toml` (or with `run --listen ADDR --web-origin ORIGIN`) this example is local-only:

```toml
[daemon]
listen = "127.0.0.1:7841"
web_origin = ["http://127.0.0.1:8790"]
```

Run `corrald-next pair`, or choose **Pair a device** in Settings › Devices on any
paired device, browser or native. Open the offered pairing link or QR. It opens Settings › Connection
with the code prefilled; press **Pair**. The short-lived code is in the URL fragment,
not the query/path, and the app clears it from the address bar before pairing.
It is never saved to NSH Storage. Raw-code entry remains available when no link is offered.

Compare the six-digit codes on both devices, then confirm with `corrald-next pair confirm <id>` or on any
attached device. A pairing code expires after two minutes; claiming it starts a separate
two-minute confirmation window. Opening a link does not claim, approve or replace a stored
identity. The new device cannot confirm its own pending pairing.

LAN/Tailscale access is opt-in: `daemon.listen` accepts one concrete numeric unicast
interface, not wildcard/multicast addresses. Remote browsers need an HTTPS-served app,
an exact `web_origin` entry and `run --web-endpoint wss://host[:port]`.
The operator supplies TLS through a reverse proxy or Tailscale Serve; corrald has no
embedded TLS. Noise remains end-to-end inside WSS.

Pairing switches to live data automatically. Reload preserves the device and daemon pin:
the browser stores its credential encrypted in IndexedDB with a non-extractable
WebCrypto AES-GCM key. NSH Storage holds only the non-secret paired marker, never keys.
The WebSocket carrier uses Noise IK; initial pairing uses single-use Noise IKpsk2.

### Live behavior and revocation

Typed live subscriptions and details cover every destination. Availability depends on
negotiated capabilities; missing facts, provider failures and unsupported operations are
shown as such. Timeline and burn-up retain bounded partial results with completeness notes;
Usage states what its totals cover (see Usage above). A paired device is the owner: nothing
asks a device for authority, and no operation needs a preview or a proof.

On link loss, retained data is stale and subscriptions reconnect. A restart can open the
last complete Tasks, Plans, Projects and Health snapshot for the saved credential, explicitly
stale/offline, not authenticated live data. Operations and replies need a live connection and
are refused offline; nothing is queued. Drafts and bounded assistant messages persist
per device/store; queued messages remain visible and cancellable with their original IDs.
IndexedDB transaction completion or native synced atomic
file replacement must acknowledge persistence before send acceptance, matching draft
retirement. Storage failures retain the composer's words.
Revision-checked per-item transactions and refreshes preserve other tabs' or instances'
messages and cancellation tombstones.

Settings counts devices only from a listing completed on the current connection; before
it, after failure, or without an authenticated link, the count is loading, unavailable or
needs a connection, never an invented zero. Unsent drafts stay with their own quick thread
or named session, flush on unmount, and do not return after accepted send retirement.

Review uses Unified below about 1100 pt of content width; wider
views offer side-by-side (`s` in a terminal). Below about 900 pt, Plans and Review show either the list or the
opened record with a back control; opening a record by name from Now, a link or the
palette shows that record.

Opening a task that is not on the board from Changes, plans, dependencies or the palette's `Open T-<n>` shows it read-only from its detail.

Task bodies are selectable Markdown source, without automatic image fetches or HTML.

Reopening a scoped subscription waits for the previous close response. Retired pages are
neither displayed nor acknowledged; rapid reopen requests coalesce within the original
deadline. Sent assistant messages retire when their canonical thread entry arrives.
Delivered owner messages retain their original typed time; a different receipt time is
shown separately. Receipt order still orders the conversation.

Revoking a device in Settings › Devices (asks once) closes its live sessions and stops
its automatic reconnect. Native devices can explicitly enrol again.
Browsers recover with **Forget this browser** (also asks once), which returns to Connection for pairing.
Forget removes local credentials and local state, not the daemon's record; revoke obsolete
records from any paired device. Failed Forget remains retryable.

### Actions, refusals and confirmation

One **Actions** menu serves every task: Now rows, Plans members, Review candidates, Project page rows, Closed rows and the task page header. It opens with `.` in a terminal, the `…` button, right-click or long-press, and the palette lists the same entries for the selected task. It lists all fifteen engine operations in a fixed order and never dims one: Edit…, Comment…, Move to…, Dispatch…, Reply…, Mark done…, Run gate, Start review…, Record verdict…, Request changes…, Approve and land…, Retry…, Stop, Drop, Reopen. An item ending in `…` opens a form prefilled from the task: Dispatch offers the profile list (the engine's default is marked) and the base; Move to offers all eight statuses. **New task…** (palette, under Workspace actions) takes title, body, status (idea, backlog or ready), project, priority, kind, size, parent, depends-on and base.

Each press sends one `op.submit { op, force }` and the engine decides; the app predicts no refusal. A receipt shows the answer: sent, running (a gate or a landing can take minutes; the task shows the outcome), done, refused or failed. A refused operation opens the **refusal sheet**: the engine's reason and one button per typed next step, the first one primary. A step that forces is the "anyway" form (Dispatch anyway, Move anyway, Land anyway); it says what it skips and is its own confirmation. A Blocked task on Now shows the same next-step buttons on its card, and a reply box in place when the first step is a reply. Review's decide bar offers Land anyway when Approve was refused.

Destructive or forced operations ask once, on this device, before they are sent: Drop (type `drop`), Stop, Approve and land, Retry with a fresh start, Move to Done or Dropped, anything forced, device revoke, Forget, ending a named chat session, and leaving Configuration with unsaved edits. Nothing else asks.

### Agents and sessions

Each run is a session, keyed `<task>_<run>`. A session's menu (`.` on the roster or the session page, or its button) lists the controls that apply (Cancel turn, Pause, Resume, Rewind…, Terminate, Relaunch), then Reply… and Focus pane, then **Task ›** with the full Actions menu of its task. A worker offers Pause or Resume as its run reports; a reviewer is never paused or rewound; a lost host is relaunched, not terminated. Terminate asks once, and a refused control opens the refusal sheet with corrald's reason. Reply, and words typed on the session page, go to the task as the engine's `reply`. Focus pane works for a local herdr pane and says why when it cannot.

An agent session's transcript replays the recorded session as the corral-pi terminal shows
it, for every role: words as rendered Markdown, thinking (folding to one row), tool rows that
open into their arguments, output, diff or written content and recorded call and result, the
owner's `!` commands, images, compaction and branch dividers, model, effort, bookmark, stop
and error rows, run marks, host notices, presenter cards and each assistant turn's model,
stop and usage. A recorded item larger than its preview shows the preview until its
complete content arrives: words are fetched as soon as they are shown, everything else when
its row is opened. While the page is open it watches the session's live turn, drawing the
streamed words and running tools after the recorded messages until the transcript holds them,
and it keeps to the newest rows unless the owner scrolled up. `` ` `` switches to the terminal: pane text for a herdr session, the tail of `session.log` for a process session. Nothing typed into a terminal is queued offline.

**+ New session** under Assistant sessions starts a named chat session in corrald. It is disabled, with the reason beside it, while corrald serves no assistant sessions. Named sessions use the same model picker and durable transcript. Stop interrupts the current turn; End session (asks once) stops the host while retaining its record and conversation.

### Settings

- **Devices**: this device and the paired ones, **Pair a device**, Revoke and Forget.
- **This daemon**, **Connection**: daemon state, last sync and its error, endpoint and store, and this device's enrolment. The device id is kept in a private `0600` file, named as such, not an OS keychain.
- **Notifications**: Web Push (below).
- **Configuration**: edits `corral.toml` through `config.get` and `config.put`, one part at a time: Workspace (limits, defaults, Auto dispatch, Auto land), Projects (gate lines, reviewer), Profiles, Harnesses, Routing, Artifacts (globs) and File (the whole file as text). A form field changes one key in place, so comments, order and unknown tables survive. **Save** (`Ctrl S` from any field) sends the whole draft once; corrald writes the file only after the engine's config check passes, and a refusal's reasons land on the field they name, or at the top. **Discard** drops the edits. Leaving with unsaved edits asks once. When the file changed outside the editor, a notice offers **Read again** and says unsaved edits are dropped only when there are some; a file on disk that does not parse is reported with its position. The `[daemon]` listener is read when corrald starts: change it in File and restart.
- **General**: theme, sidebar and local preferences, which work without a connection.

### Artifact finder

Artifacts (`f`, the sidebar, the palette, or Find in all artifacts on a task's Evidence tab) is one filter over every task's artifacts, newest first, with the kind, name, task, run, project and time. The field is always focused. Words match; `kind:`, `task:` and `project:` narrow, and every token must match. The side shows the selected artifact (Markdown, diff, log, text, image, or its facts); Enter opens it full size, **Open T-n** goes to its task and **Load older artifacts** pages. Esc leaves full size, then clears the filter.

### Sample preview

The sample preview is explicit and launch-only: **Preview with sample data** on a disconnected screen, or `Ctrl K` › **Data: fixture**. It is never restored on restart and never shown as daemon facts. The same live screens run against a small local engine that applies operations, refuses with typed next steps and records what it did as the daemon would; nothing is sent to corrald. The top bar reads `sample`, and a confirmation says it is a preview simulation. The palette's Demo entries (hotkey, share-sheet and lock-screen-push illustrations, where mounted) do not establish OS integration.

### Ask and attachments

Ask is one composer for the one chat agent: questions, ideas and instructions go to the same agent, so there is no target to pick (`/` opens it). The transcript is shown as the terminal UI shows it. An agent message offers **File as idea**. The quick thread can be ended; using it afterward starts a new session. The model picker (`Ctrl Shift M` cycles it) lists the profiles that name a model; changing it applies to the next message. Choices distinguish **Anthropic · API (metered)** from **Anthropic · subscription**, including when they name the same model. Conversation, launch and relaunch depend on no automation setting.

The composer can attach a saved image or bounded UTF-8 file, or an opaque link. Native
clients can enter a local-client file path; it is not a daemon host path. Links are not fetched
by the app. Attachment uploads require a connection and a daemon that accepts attachments; ready uploads may accompany a queued
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

### Walking the terminal UI

`scripts/tui-walk.py` starts `nsh run --target tui` in a pseudo-terminal, feeds it steps and prints the screen as text, so a change can be looked at without a terminal of one's own. From this directory:

```sh
python3 scripts/tui-walk.py --size 140x44 fixture key:n sleep:1 screen
```

`--size COLSxROWS` sets the terminal (default 140x44) and `--steps FILE` reads one step per line. Steps: `fixture` (Ctrl+K, `Data: fixture`, Enter: the sample preview), `key:NAME[*N]` (`enter tab stab esc up down left right home end pgup pgdn bs del space`, `ctrl+x`, `alt+x` or one character), `type:TEXT`, `wait:TEXT`, `gone:TEXT`, `sleep:SECS`, `screen`, `rows:A-B`, `attrs[:all]` and `diff` (the styled runs, to read focus and selection). The app restores its last destination, so start a walk with the destination's key (`n`, `r`, `p`, `a`, `t`, `u`, `f`, `,`, `/`); on Artifacts the filter takes letters, so go there with `key:ctrl+k type:Now key:enter` first. `--colors` (`truecolor`, `256`, `16`, `mono`) and `--background` set what the terminal reports. The driver keeps the app's saved state under `/tmp`, uses `pyte` from a virtual environment it creates under `/tmp`, and counts as the one NSH build at a time; see `AGENTS.md` for walking against a scratch daemon. It is a text capture, not a real owner's terminal.

## Keys

| Key | Action |
|---|---|
| `n . p a r t u f ,` | Now, Changes, Plans, Agents, Review, Timeline, Usage, Artifacts, Settings (`i` opens Now) |
| `R` (Shift+R) on Now | Reply to the selected Blocked task in place |
| `A` (Shift+A) on Review | Focus Approve and land |
| `j k`, `J K` | Move the list selection; Plans members |
| `/` | Open Ask |
| `Ctrl K` or `?` | Open palette (`Ctrl K` also on the phone layout with a hardware keyboard) |
| `Ctrl B` | Toggle sidebar; narrow windows fold it to a rail, not remove it |
| `Ctrl S` in Settings › Configuration | Save the `corral.toml` draft (one `config.put`), from any field |
| `]` | Next Now inspector tab |
| `.` on a Now, Review, Plans or Agents list or open page | The selected task's Actions menu, or the session's menu in Agents; elsewhere it opens Changes |
| `` ` `` | Toggle session transcript/terminal |
| `Ctrl Shift M` | Cycle assistant model |
| `Esc` | Close newest overlay; clear a Now or Agents selection before going back; in terminal session view, cancel an offered turn when no overlay is open |

On macOS, `Ctrl` shortcuts above use Command (`mod`). Letter shortcuts do not navigate
while editing a field. List keys (`j`/`k`, arrows, Enter) bind only
while the destination list has focus or no control has focus, with no overlay/confirmation.
Now moves and opens the selected task's session with Enter; Review and Agents move; Plans move with `j k` and members with `J K`.
Tab moves focus; Enter/Space and arrows remain with focused
controls. In a menu (the Actions or Session menu `.` opens, one opened from its button or a row,
the palette's results) `↑ ↓` move, `Home`/`End` reach the ends and Enter runs the item reached;
Tab still moves, and the list under an open menu leaves it the arrows. On the web a popup menu
opened with the pointer does not yet take the arrows until an item has the focus. In an operation's
form a Markdown or prose field (a body, a comment, a reply, notes) takes several lines: Enter
breaks the line there and Tab reaches Send. A form taller than the window scrolls inside its
sheet, with Cancel and Send kept below it. Review has `s` (split when wide enough). While a
menu, form, sheet or the palette is open, the key line shows that surface's keys.

## Structure

- `src/lib.rs`: root `App`.
- `src/model/`: pure Rust data, wording, state, reducers and view builders, with no `nsh_*` imports or I/O. `preview.rs`/`preview/` supplies synthetic sample data to the live models; `work/` covers Now and Changes.
- `src/state.rs`: app signals, effect dispatch and persistence.
- `src/ui/kit/`: shared pieces used to build screens, including rows, page scaffolds, buttons, notices, tables and overlays.
- `src/ui/`: shared UI, Ask and composer surfaces, palette, tokens, icons and terminal-specific pieces in `term.rs`.
- `src/shell/`: desktop, phone and terminal shells.
- `src/screens/`: destination screen modules, their subdirectories and page routing.
- `tests/shell.rs`: headless shell tests. `scripts/tui-walk.py`: the terminal walk driver.
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
`draft/<id>`, `message/<id>` and `cancelled/<id>` entries; records of the retired command
journal are removed. Offline hints
use `active` and `snapshot/<namespace>`; `forgotten/<namespace>` and
`migrated/<namespace>` prevent identity resurrection and repeated legacy imports.
The old NSH `corral.outbox.v1.<hex>` is migrated then removed; `corral.data`,
`corral.conn` and `corral.mode` are no longer read or written. Local records are bounded to 8 MiB across namespaces;
the outbox is bounded per namespace to 1 MiB, 128 messages and 32 drafts,
and the offline snapshot to 2 MiB. Quota pruning never discards undelivered messages.
Pairing secrets/proofs never go in the outbox or NSH Storage.

## Screenshots

`docs/shots/` contains desktop and phone web renders in dark and light themes, and terminal renders at 120x36 and 80x24 with a `.txt` capture beside each PNG. Web renders are 1440x900 for desktop and 390x844 for phone, with two desktop renders at 900x600.

Start with `desktop-dark-now.png`, `desktop-dark-question.png`, `phone-dark-inbox.png`, `phone-dark-question.png` and `terminal-120x36-now.png`.

The build's scripts rendered the packaged web build in headless Chromium with a desktop pointer and phone emulation, and rendered the terminal target to PNG in a private tmux. These are not captures from a real phone or the owner's terminal.

These renders predate the move to the simple engine: they still show the Inbox, Builds and question pages, authority and previews, which no longer exist.

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
