# Corral app

Corral is a Rust app built on [NSH 0.6.1](https://nsh.dev), with desktop, web, phone-layout and terminal forms. The crate is `nsh-app-corral` (`0.1.0`); `nsh.json` currently declares package version `1.0.0`, build `1`. These are diagnostic/package versions, not protocol compatibility. The root component is `App`. Normal startup uses live daemon data or shows a disconnected, unknown or unpaired state; sample data is an explicitly selected preview only. See [Live data](#live-data).

The app has four places and Settings (keys in the [key table](#keys)). Every task and every session opens one page, on every form.

- **Home**: what waits on you, then what Corral is doing. On the desktop and in a terminal at least 100 columns wide it is a dashboard: six tiles, **Needs you** as cards (Blocked tasks, reviews ready to land, and chats or builds that wait on you) with one card open, and panels for what is working (lanes over today), tasks by status, the projects, today's cost, the latest events, what just finished and the burn-up. The open card holds the decision: a Blocked task's reason, the log excerpt the engine served, a reply box when the recommended step is a reply and one button per other next step; a ready review's pipeline phrase and Approve; a session's own dock and composer. Ask's composer is docked at the foot. A narrower terminal and the phone show Needs you, Working and Just finished as one list; the phone answers a reply or a chat's question in the list.
- **Tasks**: every task in scope as one list grouped by status (Blocked, Review, Running, Ready, Backlog, Idea), with Done and Dropped in one fold at the bottom, read a page at a time. A row shows the task's status, id, title, what is happening, its pipeline (run, gate, review, land), project, profile, age and its one next step; columns give way as the list narrows. corrald serves no cost per task, and the Cost column says so. A plan is a parent task: its caret unfolds its children in place. The filter field takes id, title or project words. The inspector beside the list shows the selected task with the same decision band as its page.
- **Task page**: the one page of a task; a plan and a review candidate open it too. Under its head and facts, the decision band holds what you decide: a Blocked task's reason, detail, reply box and next steps; a review's **Onto** line for the landing branch and its decide bar (Approve and land, Request changes, Run gate, Start review, and Land anyway while the gate is red or after Approve was refused); otherwise the task's one next step. The tabs are **Overview** (description, a plan's children, dependencies, pipeline, each round's checks, runs that open their sessions, artifacts), **Changes** (the worker's `done` summary and the stored diff) and **History** (the task's events and a Comment box). There is no separate plan review.
- **Sessions**: every session in scope (runs of every role, chats, builds) as one list grouped Blocked, Working, Idle, then Gone folded. A row shows the session's state, id, name, role, what it does now, project, model, context share, cost, age and its one next step. The head says how many are live and what they spent, summing only reported costs, and holds **New**. The inspector leads with what the session asks, answerable there. A session opens its [session page](#agent-sessions).
- **Activity**: four pages, with a strip between them on the desktop and in a terminal; the phone's Activity tab lists them.
  - **Feed**: the engine's events in scope, newest first, under a heading per local day. A row opens its task's page; a forced operation's kind is drawn in the failure colour. Load older pages back, and Mark seen moves this device's seen cursor, which Home's "since your last visit" counts from.
  - **Timeline**: lanes of runs, gates, reviews, landings, waits and idle per task or run (Today, 7 days; By task, By run), the goal burn-up and the gaps worth a look.
  - **Usage**: tokens and cost that agent hosts reported with `corral usage add`, for 7 days, 30 days, this month or a custom range, grouped by profile, model, task or day. The header says how many runs the totals cover and why the rest are missing (they ran on a harness that reports no usage); requests with tokens but no price are counted apart. Quota stays unknown unless a harness reports it; an unrecorded figure reads `—`.
  - **Artifacts**: the artifact finder; see [Artifact finder](#artifact-finder).
- **Settings**: Devices, This daemon, Connection, Notifications, Configuration (edits `corral.toml`) and General (local preferences).
- **Ask**: one composer for the one chat agent; see [Ask](#ask-and-attachments). The desktop and terminal sidebar has no Ask entry: `/` opens it, and on Home it focuses the docked composer. The phone keeps its floating Ask button.

The former destinations (Now, Inbox, Plans, Review, Agents, Builds, Changes and the Project page) are gone; their slugs, addresses and the keys `n`, `p` and `r` open their new homes.

Counts are an icon and number, with the sentence in a hint on hover or keyboard focus on desktop and in the status line in a terminal; phone rows keep one explanatory line. The top-bar health display is one `healthy` chip when every component is healthy and names the component otherwise. The UI shows real dates.

Projects (`projects.v1`): one scope switcher (the top of the sidebar, the desktop's title
bar, the phone's head) sets the project scope of the whole app: All projects, or one of the
workspace's configured projects, listed with its open-task count. Home, Tasks, Sessions and
Activity all follow it; there are no per-screen filters, no sidebar project list and no
Project page. With a project in scope Home shows that project's facts in place of the
project list, and an old `/projects/<id>` address sets the scope and opens Home. Ask sends
the project in scope as context, so the assistant files Ideas there unless told otherwise.
A daemon without `projects.v1` supplies project names only from task rows, without counts.
When the daemon's complete project list is empty the workspace has no project management:
only Home (a landing that says how to add a project and opens Ask), Sessions, Activity ›
Usage and Settings are offered. The palette offers **Preview: no projects** for that state.

One trusted local owner: a paired browser, an enrolled native app and the owner CLI have the same authority. There are no authority classes, previews or proofs; an enrolled or paired device is the owner. Every engine operation is offered for every task, in its More menu, and the engine decides what is allowed. See [Actions, refusals and confirmation](#actions-refusals-and-confirmation).

Disconnected screens offer **Connect** and **Preview with sample data**; see [Sample preview](#sample-preview). Settings › General › **This device** exposes local preferences even without a connection.

The app lives in the corral repository as `apps/corral-app/`. It is its own Cargo workspace with its own toolchain, not a member of corral's workspace. The [design handoff](../../docs/app-design/README.md) contains the requirements, architecture proposal and runnable mockup, with their authority and implementation-status caveats.

## Platform and release support

The app and the new engine are separate from the classic `corral` release. The matrix
below describes the intended release artifacts, not a claim that every source target or
native desktop host has been tested. A release must contain the assets before an install
or upgrade can succeed; older releases do not necessarily contain them.

| Component | OS and architecture | Distribution / limitations |
|---|---|---|
| `corral-next`, `corrald-next`, `corral-pi` | Linux glibc x86_64 and aarch64; macOS x86_64 and arm64 | `corral-next-<Rust target>.tar.gz`, with a `.sha256` sidecar. The daemon requires Unix sockets; native Windows is not supported. |
| Web app | Browser with WebAssembly, WebSocket, IndexedDB and WebCrypto; independent of host CPU | `corral-app-web.tar.gz` and `.sha256`; static files, no Rust or NSH at runtime. Pairing and remote use have the secure-origin requirements below. |
| Native desktop / terminal app | Source targets include Linux and macOS; desktop backend also names Windows | No native app release artifact is promised. Native live connection needs Unix peer credentials; Windows native connection is unavailable. Desktop builds and platform integrations require verification on the target host. |
| Phone layout | Browser on a compatible phone | Responsive web UI, not a promised Android or iOS packaged release. |

Linux release targets are `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`;
macOS targets are `x86_64-apple-darwin` and `aarch64-apple-darwin`. Linux packages are built
and tested on Ubuntu 24.04 (glibc 2.39); older glibc distributions are not verified. macOS
packages target macOS 14 or later. There is no new-engine Windows or Linux musl artifact in this matrix. The classic CLI's separate Windows installer
and musl archives do not install this app's daemon.

## Install and upgrade

On a supported Unix host, the repository installer downloads the new engine and static web
app, verifies their SHA-256 sidecars, and installs the executables under `~/.local/bin` by
default. It needs network access, `curl` or `wget`, `tar`, and `sha256sum` or `shasum`.
For agent work, install Git and an authenticated agent provider; herdr 0.9.3 or newer is
needed for the terminal engine. The installer may offer to install or upgrade herdr;
`--yes` grants that consent. See the [installation guide](../../docs/guide/install.md)
for the classic CLI and its prerequisites.

```sh
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh -o install-corral.sh
sh install-corral.sh                 # install, or rerun to upgrade to latest
# Optional pin instead of latest:
CORRAL_VERSION=vX.Y.Z sh install-corral.sh
export PATH="$HOME/.local/bin:$PATH"
corral-next --version
corrald-next --version
```

Use an actual published tag for the pin. `CORRAL_INSTALL_DIR` overrides the binary
directory; the static site goes to `$XDG_DATA_HOME/corral-next/web`, or
`~/.local/share/corral-next/web` when `XDG_DATA_HOME` is unset, with
`CORRAL_APP_INSTALL_DIR` as an override. Use a dedicated directory: upgrades replace the
marked site wholesale, and nonempty unrelated or manually unpacked directories are not adopted.
`sh install-corral.sh --app-only` installs only
the static site; `--dry-run` shows the plan. On native Windows, the PowerShell installer
can install just the web site, not the new daemon:

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/pawissanutt/corral/main/install.ps1 -OutFile install-corral.ps1
.\install-corral.ps1 -AppOnly -Version vX.Y.Z
```

On Windows the default site directory is `$env:LOCALAPPDATA\Programs\corral\web`;
`-AppInstallDir` or `CORRAL_APP_INSTALL_DIR` overrides it. The Windows installer's
legacy CLI mode is separate and needs an older published tag.

Keep the binaries and app from the same release: this is a prototype and the frontend
does not emulate missing daemon capabilities. The normal Unix installer resolves the app
against the installed runtime's version. Rerun the installer to upgrade; it does not
register or restart a daemon service, or serve the web app. Restart your daemon and replace
the served web files after upgrading. Back up your workspace and configuration before
upgrading; no automatic rollback or downgrade compatibility is promised.
`sh install-corral.sh --legacy` selects the older classic CLI instead.

In the intended workspace directory, initialize with `corral-next init`, then run
`corrald-next --root <workspace> run` on the Unix host (use an absolute workspace path).
The local socket is
`<workspace>/.corral-next/daemon.sock`. For the web app, serve the installed static site
at the host root with SPA fallback to `index.html`; configure the optional daemon listener
and exact app origin, then pair as described in [Browser](#browser). A plain file server
without fallback does not support deep-link reloads. Do not expose the daemon listener
without the TLS and origin configuration described there.

## Development quick start

Install NSH with `curl -fsSL https://nsh.dev/install.sh | sh` (or `nsh update`). Check `nsh --version`: the app is tested with 0.6.1. Then run these commands in the project directory:

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

Use the matching release's `corrald-next`, or build it from this checkout
(`crates/corral-daemon`); an older installed daemon does not necessarily support the app
contracts. The frontend does not emulate missing capabilities.

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

For development with the separate NSH server above, enable the optional listener, then
restart `corrald-next`. It is off by default. In `.corral-next/corral.toml` (or with
`run --listen ADDR --web-origin ORIGIN`) this example is local-only:

```toml
[daemon]
listen = "127.0.0.1:7841"
web_origin = ["http://127.0.0.1:8790"]
```

Alternatively, serve the packaged app on the daemon's listener, without a separate Python
service. From the repository root run `just app-web-install`; it installs the bundle at
`$XDG_DATA_HOME/corral-next/web`, or `~/.local/share/corral-next/web` when unset.
In `.corral-next/corral.toml`, use the actual absolute path to that dedicated bundle:

```toml
[daemon]
listen = "127.0.0.1:7841"
web_dir = "/absolute/path/to/dedicated/web/bundle"
web_origin = ["http://127.0.0.1:7841"]
```

Restart `corrald-next` and open http://127.0.0.1:7841/. `web_dir` requires an explicit
`listen`; `run --web-dir PATH` overrides the configured directory. Relative paths are
workspace-relative; shell `~` is not expanded in configuration. Use only a dedicated
bundle directory, never a repository, workspace, home directory, or any directory with
secrets. A missing bundle makes the UI unavailable but does not stop the daemon. Updating
only the web files does not require a daemon restart. The native Unix app is unchanged.
The separate loopback Python service remains supported if preferred.

Run `corrald-next pair` on the daemon host, or choose **Pair a device** in Settings › Devices on any
paired device, browser or native. The command prints a pairing link, then waits for the device. Open the link or QR. It opens Settings › Connection
with the code prefilled; press **Pair**. The short-lived code is in the URL fragment,
not the query/path, and the app clears it from the address bar before pairing.
It is never saved to NSH Storage. Raw-code entry remains available when no link is offered.

Compare the six-digit codes on both devices. `corrald-next pair` shows the browser's name and code and asks; answer `y` only if they match,
or confirm on any attached device. For scripts, `corrald-next pair --json` prints the offer and returns at once, and `corrald-next pair status|confirm|cancel <pair id>` remain. A pairing code expires after two minutes; claiming it starts a separate
two-minute confirmation window. Opening a link does not claim, approve or replace a stored
identity. The new device cannot confirm its own pending pairing.

LAN/Tailscale access is opt-in: `daemon.listen` accepts one concrete numeric unicast
interface, not wildcard/multicast addresses. Remote browsers need an HTTPS-served app, an exact `web_origin` entry and
`web_endpoint = "wss://public-host"` in `[daemon]` (`run --web-endpoint` overrides it).
The operator supplies TLS through an HTTPS reverse proxy or Tailscale Serve; corrald has no
embedded TLS. If the daemon serves the bundle, forward all app traffic to it; otherwise,
forward only `/v1/session` and `/v1/pair/<id>`. Preserve the browser's `Origin` header for
WebSocket upgrades. Noise remains end-to-end inside WSS.

Browser credentials and saved data are scoped to the origin. Keeping the old URL while a
proxy points it to the new upstream retains that origin; otherwise pair again. Old drafts
do not automatically move to a new origin.

Pairing switches to live data automatically. Reload preserves the device and daemon pin:
the browser stores its credential encrypted in IndexedDB with a non-extractable
WebCrypto AES-GCM key. NSH Storage holds only the non-secret paired marker, never keys.
The WebSocket carrier uses Noise IK; initial pairing uses single-use Noise IKpsk2.

### Addresses

In a browser the address names the place shown, so a reload or a link opens there:

| Path | Place |
|---|---|
| `/home`, `/tasks`, `/sessions` | A place |
| `/activity/feed`, `/activity/timeline`, `/activity/usage`, `/activity/artifacts` | An Activity page |
| `/task/<id>`, `/task/<id>/changes`, `/task/<id>/history` | A task's page at its tab |
| `/session/<id>` | A session's page; a child's conversation is `/session/<session>/child/<id>` |
| `/settings/<section>` | A Settings section: `devices`, `daemon`, `connection`, `notifications`, `config`, `general` |

The former addresses still open: `/now`, `/plans`, `/review`, `/agents`, `/changes` and their
record paths (`/now/<task>`, `/agents/<session>`) go to their new homes, and `/projects/<id>`
sets the scope and opens Home.

On the desktop every destination or page visited is an entry of the browser's history, so Back
and Forward step through them; switching a task page's tab only rewrites the current entry. The
phone's pushed pages are history entries already, so there the address only follows; its
Settings list is `/settings`, and its Activity list is not named (the address keeps the last
destination).
A list's selection, the project scope, a session page's views, filters and scroll position are
not in the address. The site's root opens
the last destination, and an unpaired browser opens Settings › Connection whatever the path.
The sample preview is not restored, so a reload at one of its records shows what corrald has
under that id. The native and terminal forms have no address and restore the last destination.

Serve the site at the host's root (`nsh.json` `web.base` is `/`). Daemon hosting,
`just app-serve`, `nsh run --target web` and `nsh package --target web --run` serve `index.html`
for browser navigation to app paths. With daemon hosting or `just app-serve`, existing files
are served normally and missing assets such as `.js` files still return 404.

If your existing `corral-next-app-web.service` uses `python3 -m http.server`, it will return
404 on reload below `/`. Run `just app-web-install` to install the site and its new loopback
server, then run `systemctl --user edit corral-next-app-web.service` and add this override
(adjust both paths if you install under a custom `XDG_DATA_HOME`):

```ini
[Service]
ExecStart=
ExecStart=/usr/bin/python3 %h/.local/share/corral-next/web.serve.py --directory %h/.local/share/corral-next/web 8890
```

Run `systemctl --user restart corral-next-app-web.service` to apply the change. Future
`just app-web-install` runs update the same site and server paths.

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

A task page's Changes tab shows the diff unified below about 1100 pt of content width, on a
phone and in a terminal; a wider page can split it side by side.

Opening a task the open-task list does not hold (a Done or Dropped one, from Activity › Feed, the Done and Dropped fold, a dependency or the palette's `Open T-<n>`) shows its page from its detail.

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

Every task shows its one **next step**, the single press picked from its status, and **More** beside it: on a Tasks row, in the inspector, on Home and in the task page's head. More opens with `.`, its button, right-click or long-press, and the palette lists the same entries for the selected task. It lists all fifteen engine operations in a fixed order, each with a few words on what it does, marks the next step and never dims one: Edit…, Comment…, Move to…, Dispatch…, Reply…, Mark done…, Run gate, Start review…, Record verdict…, Request changes…, Approve and land…, Retry…, Stop, Drop, Reopen. An item ending in `…` opens a form prefilled from the task: Dispatch offers the profile list (the engine's default is marked) and the base; Move to offers all eight statuses. **New task…** (palette, under Workspace actions) takes title, body, status (idea, backlog or ready), project, priority, kind, size, parent, depends-on and base.

Each press sends one `op.submit { op, force }` and the engine decides; the app predicts no refusal. A receipt shows the answer: sent, running (a gate or a landing can take minutes; the task shows the outcome), done, refused or failed. A refused operation opens the **refusal sheet**: the engine's reason and one button per typed next step, the first one primary. A step that forces is the "anyway" form (Dispatch anyway, Move anyway, Land anyway); it says what it skips and is its own confirmation. A Blocked task shows the same next-step buttons in its decision band (on its page and in the Tasks inspector) and on Home's open card, with a reply box in place when the recommended step is a reply. A review's decide bar offers Land anyway while the gate is red or after Approve was refused.

Destructive or forced operations ask once, on this device, before they are sent: Drop (type `drop`), Stop, Approve and land, Retry with a fresh start, Move to Done or Dropped, anything forced, device revoke, Forget, ending a named chat session, and leaving Configuration with unsaved edits. Nothing else asks.

### Sessions and their controls

Each run is a session, keyed `<task>_<run>`; a chat (`quick`, `A-n`) and a build (`B-…`) are sessions too. A session shows its one next step and **More** (`.` on a Sessions row or the session page, its button, right-click or long-press), which lists the controls that apply (Cancel turn, Pause, Resume, Rewind…, Terminate or Stop, Relaunch or Resume), then for a run Reply… and **Task T-n** with the full More menu of its task. A reviewer is never paused or rewound; a lost host is relaunched, not terminated. Terminate asks once, and a refused control opens the refusal sheet with corrald's reason. Reply… is the task's engine operation `reply`.

An agent session's transcript replays the recorded session as the corral-pi terminal shows
it, for every role: words as rendered Markdown, thinking (folding to one row), tool rows that
open into their arguments, output, diff or written content and recorded call and result, the
owner's `!` commands, images, compaction and branch dividers, model, effort, bookmark, stop
and error rows, run marks, host notices, presenter cards and each assistant turn's model,
stop and usage. A recorded item larger than its preview shows the preview until its
complete content arrives: words are fetched as soon as they are shown, everything else when
its row is opened. While the page is open it watches the session's live turn, drawing the
streamed words and running tools after the recorded messages until the transcript holds them,
and it keeps to the newest rows unless the owner scrolled up. A session without a Pi host shows a read-only log instead: the pane's screen for a run on another harness, or the kept tail of `session.log` for a headless run that has ended.

**New** in Sessions' head starts a named assistant session in corrald, which opens on its page once corrald made it, or opens the New build sheet (`N`).

### Agent sessions

Every agent session opens on one session page, whatever its role: a task's worker or reviewer, a chat, a build session, a child agent's conversation. The page shows the transcript as the Pi terminal shows it; above the composer, the dock holds what waits on the owner (answer cards, a docked choice such as a build's exit, a child's question or a proposed board plan, and the queue with Edit, ↑, ↓ and Remove), and one status line under the composer gives the role, cost, context, model and effort. Beside the conversation is one **Details** panel with a section per part the host serves: **Changes** (the files of each comparison and one file's hunks), **Agents** (the child agents with their details, conversations and models, the messages between them, and the background jobs with their output, older bytes on request), **Plan**, **Usage**, **Context**, **Board** with the build record for a build, and **Task** and **Proof** for a task's run. A section folds on its own, and its Open shows the part's full view in the conversation's place; Escape returns to the conversation. Details is open beside the conversation on a wide page (from about 900 pt, or 100 columns in a terminal) and closed until opened on a narrower one (`Alt B`, or Details in the head); on the phone it is a sheet, and the phone's page has no queue and no status line. The ⋯ menu lists every action, an action the host cannot run with its reason, and opens Usage. The host is the source of truth: the page reads its parts through corrald and shows a sent message until the transcript holds it under the same input id. **Sessions › New** starts a build session in a project; Stop and Resume control its host, and Remove… shows prune's dry run first.

**Ask** is the same transcript, dock and composer over the quick chat, in a drawer on desktop and in the terminal and in a sheet on the phone. End thread finishes the quick chat; the next message starts a new one.

The sample preview carries one build session, `B-k3x9` (*parity-guide*), that serves every part and answers every session action as a host would. It tells the same story as `packages/corral-pi/test/fixtures/sessions/parts.json`.

### Settings

- **Devices**: this device and the paired ones, **Pair a device**, Revoke and Forget.
- **This daemon**, **Connection**: daemon state, last sync and its error, endpoint and store, and this device's enrolment. The device id is kept in a private `0600` file, named as such, not an OS keychain.
- **Notifications**: Web Push (below).
- **Configuration**: edits `corral.toml` through `config.get` and `config.put`, one part at a time: Workspace (limits, defaults, Auto dispatch, Auto land), Projects (gate lines, reviewer), Profiles, Harnesses, Routing, Artifacts (globs) and File (the whole file as text). A form field changes one key in place, so comments, order and unknown tables survive. **Save** (`Ctrl S` from any field) sends the whole draft once; corrald writes the file only after the engine's config check passes, and a refusal's reasons land on the field they name, or at the top. **Discard** drops the edits. Leaving with unsaved edits asks once. When the file changed outside the editor, a notice offers **Read again** and says unsaved edits are dropped only when there are some; a file on disk that does not parse is reported with its position. The `[daemon]` listener is read when corrald starts: change it in File and restart.
  - **Profiles** lists the profiles defined in the file. Search by name or configuration, then open a profile to change its harness, model, effort or arguments. Model and effort suggestions come from configured profiles, not a live catalog; exact custom values are still accepted for validation. Profile actions change the draft only.
  - **Harnesses** lists the five built-ins even when the file has no harness tables, alongside custom launch commands. Select **corral-pi** (profile key `pi`) for terminal sign-in instructions and to distinguish task profiles in `corral.toml` from Pi child profiles in `pi.toml`. Browsing the catalog does not create overrides.
  - **Routing** lists worker rules in priority order. A dispatch override or task profile takes precedence. Otherwise, the first matching rule wins; if none matches, Corral tries the project profile, then the workspace default.
- **General**: theme, sidebar and local preferences, which work without a connection.

### Artifact finder

Activity › Artifacts (`f`, the sidebar, the palette, or **Find in Artifacts** on a task page's Overview) is one filter over every task's artifacts, newest first, with the kind, name, task, run, project and time. The field is always focused. Words match; `kind:`, `task:` and `project:` narrow, and every token must match. The side shows the selected artifact (Markdown, diff, log, text, image, or its facts); Enter opens it full size, **Open T-n** goes to its task and **Load older artifacts** pages. Esc leaves full size, then clears the filter.

### Sample preview

The sample preview is explicit and launch-only: **Preview with sample data** on a disconnected screen, or `Ctrl K` › **Data: fixture**. It is never restored on restart and never shown as daemon facts. The same live screens run against a small local engine that applies operations, refuses with typed next steps and records what it did as the daemon would; nothing is sent to corrald. The top bar reads `sample`, and a confirmation says it is a preview simulation. The palette's Demo entries (hotkey, share-sheet and lock-screen-push illustrations, where mounted) do not establish OS integration.

### Ask and attachments

Ask is one composer for the one chat agent: questions, ideas and instructions go to the same agent, so there is no target to pick. `/` opens it; on Home, on the desktop and in a terminal, the same composer is docked at the foot and `/` focuses it. The transcript is shown as the terminal UI shows it. An agent message offers **File as idea**. The quick thread can be ended; using it afterward starts a new session. The model picker (`Ctrl Shift M` cycles it) lists the profiles that name a model; changing it applies to the next message. Choices distinguish **Anthropic · API (metered)** from **Anthropic · subscription**, including when they name the same model. Conversation, launch and relaunch depend on no automation setting.

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
| `nsh package --target web --run` | Package, then serve the site on loopback at the address it prints, answering the app's [paths](#addresses) with the page |
| `nsh run --target web` | Run the web development server |
| `nsh run --target tui` | Run in the current terminal |

### Walking the terminal UI

`scripts/tui-walk.py` starts `nsh run --target tui` in a pseudo-terminal, feeds it steps and prints the screen as text, so a change can be looked at without a terminal of one's own. From this directory:

```sh
python3 scripts/tui-walk.py --size 140x44 fixture key:ctrl+k type:Home key:enter sleep:1 screen
```

`--size COLSxROWS` sets the terminal (default 140x44) and `--steps FILE` reads one step per line. Steps: `fixture` (Ctrl+K, `Data: fixture`, Enter: the sample preview), `key:NAME[*N]` (`enter tab stab esc up down left right home end pgup pgdn bs del space`, `ctrl+x`, `alt+x` or one character), `type:TEXT`, `wait:TEXT`, `gone:TEXT`, `sleep:SECS`, `screen`, `rows:A-B`, `attrs[:all]` and `diff` (the styled runs, to read focus and selection). The app restores its last destination, and on Artifacts the filter takes letters, so start a walk with `key:ctrl+k type:Home key:enter` and go on from there with a place's key (`h`, `t`, `s`, `a`, `,`). `--colors` (`truecolor`, `256`, `16`, `mono`) and `--background` set what the terminal reports. The driver keeps the app's saved state under `/tmp`, uses `pyte` from a virtual environment it creates under `/tmp`, and counts as the one NSH build at a time; see `AGENTS.md` for walking against a scratch daemon. It is a text capture, not a real owner's terminal.

### Walking against a scratch daemon

From the repository root, `just stage-parity` stages a scratch workspace under `/tmp` with one Git project and a running `corrald-next`, on the real Pi host with a scripted model provider, so a walk makes no model requests. `just stage-parity-fake <script>` stages the same with a scripted Pi host; `test/fixtures/sessions/parts.json` serves every session part (script paths are relative to `packages/corral-pi`). Each prints the workspace, the daemon socket, the browser listener and origin, a seeded build session and a `Pair:` command.

- **Terminal**: from this directory, `XDG_STATE_HOME=<workspace>/app-state XDG_DATA_HOME=<workspace>/app-data CORRAL_SOCKET=<socket> nsh run --target tui --port 8891`, then enrol in Settings › Connection.
- **Browser**: `nsh run --target web --port 8893` (the staged origin), open it in a private window, run the printed `Pair:` command and open the link it prints.

Ctrl-C in the staging terminal stops the daemon and deletes the workspace.

## Keys

| Key | Action |
|---|---|
| `h t s a ,` | Home, Tasks, Sessions, Activity (the page last shown there), Settings |
| `u f` | Activity › Usage, Activity › Artifacts |
| `n p r` | The former destinations' keys: Home (`n`), Tasks (`p`, `r`) |
| `j k` or `↓ ↑`, `Enter` | On Home, Tasks and Sessions: move the selection; open the selected one's page |
| `i` on Tasks and Sessions | Show or hide the inspector |
| `.` on Home, Tasks, Sessions, a task page or a session page | The More menu of the selected or open task or session |
| `1 2 3` on a task page | Overview, Changes, History |
| `N` (Shift+N) on Sessions | Open the New build sheet |
| `/` | Open Ask; on Home (desktop and terminal) focus the docked composer |
| `Ctrl K` or `?` | Open palette (`Ctrl K` also on the phone layout with a hardware keyboard) |
| `Ctrl B` | Toggle sidebar; narrow windows fold it to a rail, not remove it |
| `Ctrl S` in Settings › Configuration | Save the `corral.toml` draft (one `config.put`), from any field |
| `Alt B` on a session page | Show or hide Details |
| `Ctrl Shift M` | On a session page, its model picker; elsewhere cycle Ask's model |
| `Esc` | Close the newest sheet, menu or overlay; leave Home's docked composer; on a session page return a view to the conversation, then stop the working turn; then go back (a task page to Tasks, a session page to Sessions) |

On macOS, `Ctrl` shortcuts above use Command (`mod`). Letter shortcuts do not navigate
while editing a field. List keys (`j`/`k`, arrows, Enter) bind only
while the place's list has focus or no control has focus, with no overlay/confirmation.
Enter opens the selected row's or card's page only while nothing, or that row, has the focus.
On a session page a place's letter does not leave the page (its views use letters); on a task
page it does.
Tab moves focus; Enter/Space and arrows remain with focused
controls. In a menu (the More menu `.` opens, one opened from its button or a row,
the palette's results) `↑ ↓` move, `Home`/`End` reach the ends and Enter runs the item reached;
Tab still moves, and the list under an open menu leaves it the arrows. On the web a popup menu
opened with the pointer does not yet take the arrows until an item has the focus. In an operation's
form a Markdown or prose field (a body, a comment, a reply, notes) takes several lines: Enter
breaks the line there and Tab reaches Send. A form taller than the window scrolls inside its
sheet, with Cancel and Send kept below it. While a
menu, form, sheet or the palette is open, the key line shows that surface's keys.

## Structure

- `src/lib.rs`: root `App`.
- `src/model/`: pure Rust data, wording, state, reducers and view builders, with no `nsh_*` imports or I/O. `preview.rs`/`preview/` supplies synthetic sample data to the live models; `nav.rs` holds the four places, and `home/`, `tasks/` and `sessions/` are their models (`session/` is the session page).
- `src/state.rs`: app signals, effect dispatch and persistence; `src/state/address.rs` keeps the browser's address on the place shown (`src/model/address.rs`).
- `src/ui/kit/`: shared pieces used to build screens, including rows, page scaffolds, buttons, notices, tables and overlays.
- `src/ui/`: shared UI, Ask and composer surfaces, palette, tokens, icons and terminal-specific pieces in `term.rs`.
- `src/shell/`: desktop, phone and terminal shells.
- `src/screens/`: the places' screens (`home/`, `tasks/`, `sessions/`, `activity/` with `timeline*`, `usage*` and `artifacts*`), the task page (`task/`), the session page (`agents/session/`), Settings and page routing.
- `tests/shell.rs`, `tests/home.rs`, `tests/tasks.rs`, `tests/sessions.rs`: headless tests. `scripts/tui-walk.py`: the terminal walk driver.
- `theme.json` and `nsh.json`: theme and app configuration.
- `assets/fonts/`: one static file per face. Atkinson Hyperlegible Next and Mono set the interface; Cascadia Mono (regular, bold, italic, bold italic) sets an agent's live terminal, so it looks the same whatever the owner's own terminal font. Cascadia Mono is © Microsoft Corporation under the SIL Open Font License 1.1; its licence, `assets/fonts/CascadiaMono-LICENSE.txt`, ships with the app as a plain file of `assets!()`. `assets/icons/` holds the mock's glyphs as SVG.
- `docs/NSH-NOTES.md`: what NSH 0.5.0 does differently from what its docs suggest, with sources.
- `docs/shots/`: renders of the content build; see Screenshots below.

### Local storage

| Store | Keys / contents |
|---|---|
| NSH Storage | `corral.destination`, `corral.settings`, `corral.theme`, `corral.sidebar`, `corral.scope` (per daemon store: the one project scope), non-secret `corral.paired` |
| Browser IndexedDB `corral-app` v1, `credentials` | `device` (AES-GCM-wrapped credential and non-extractable key), retryable `forget` intent |
| Browser IndexedDB `corral-local-state` v1 | `records` plus `meta` (`revision`); transactional outbox/offline cache, separate from credentials |
| Native state directory | `$XDG_STATE_HOME/corral-app/` (fallback `$HOME/.local/state/corral-app/`): private `devices.json`, `durable.json`, retained locks |

Transactional records use `namespace/<hex(store + NUL + device)>/` with `meta`,
`draft/<id>`, `message/<id>` and `cancelled/<id>` entries; records of the retired command
journal are removed. Offline hints
use `active` and `snapshot/<namespace>`; `forgotten/<namespace>` and
`migrated/<namespace>` prevent identity resurrection and repeated legacy imports.
The old NSH `corral.outbox.v1.<hex>` is migrated then removed; `corral.data`,
`corral.conn` and `corral.mode` are no longer read or written, and `corral.filters` (the
former per-screen project filters) is removed on load. Local records are bounded to 8 MiB across namespaces;
the outbox is bounded per namespace to 1 MiB, 128 messages and 32 drafts,
and the offline snapshot to 2 MiB. Quota pruning never discards undelivered messages.
Pairing secrets/proofs never go in the outbox or NSH Storage.

## Screenshots

`docs/shots/` contains desktop and phone web renders in dark and light themes, and terminal renders at 120x36 and 80x24 with a `.txt` capture beside each PNG. Web renders are 1440x900 for desktop and 390x844 for phone, with two desktop renders at 900x600.

Start with `desktop-dark-now.png`, `desktop-dark-question.png`, `phone-dark-inbox.png`, `phone-dark-question.png` and `terminal-120x36-now.png`.

The build's scripts rendered the packaged web build in headless Chromium with a desktop pointer and phone emulation, and rendered the terminal target to PNG in a private tmux. These are not captures from a real phone or the owner's terminal.

These renders predate the move to the simple engine: they still show the Inbox, Builds and question pages, authority and previews, which no longer exist.

The Now, Agents, Plans and Review desktop and phone renders and their `terminal-120x36` and `terminal-80x24` captures were re-rendered after the main-loop rebuild from the packaged web build and a private tmux; the `terminal-120x36-now-16`, `-now-bandless`, `-now-light` and `-now-mono` variants and the other destinations' renders predate it.

Every render here predates the redesign into Home, Tasks, Sessions and Activity: they show Now, Plans, Review, Agents, Changes and the Project page, which no longer exist. `docs/shots/` holds no render of the redesigned places.

## Web packaging gate

Add `"bash scripts/app-web-gate.sh"` to the existing `gate` array under `[projects.corral]`
in the workspace's `.corral-next/corral.toml`. The root Cargo workspace excludes this app, so
its Clippy and test commands cannot catch app packaging failures. The new command runs
`just app-web` on every configured gate run, under the shared app build lock, and fails if
packaging fails. It builds the same site as `just app-web-install` without installing it or
starting a server. Put `just`, `flock`, and NSH 0.6.1 on the gate runner's `PATH`.

## Toolchain on this host

NSH installs under `~/.nsh` and is linked at `~/.local/bin/nsh`. NSH 0.6.1 crates require rustc 1.97; `rust-toolchain.toml` pins 1.97.1 and the `wasm32-unknown-unknown` target. Corral itself stays on 1.94.

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
