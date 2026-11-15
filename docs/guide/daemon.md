# The workspace daemon

`corrald` is the always-on sync loop for one workspace, run through `corral daemon`.
It reconciles runs, advances autopilot, refreshes usage, maintains the database and
runs PM recovery. It is not the only store writer: owner and agent commands run
through the daemon when it can take them, and in the calling process otherwise
(see [Commands through the daemon](#commands-through-the-daemon)); the remaining
CLI commands still read and write the store directly.

A kernel-held lease allows only one loop owner. A fence at the sync entry point
prevents other processes from running a competing pass. A crash releases the lease;
no lock-file deletion is needed or safe.

## Commands

- `corral daemon run [--once]`: run in the foreground, or complete one pass and exit.
  A second daemon prints that it is already running and exits successfully.
  One-shot pass failures return an error; a runtime upgrade latch exits 75.
- `corral daemon status [--json]`: read health; exit 0 healthy, 1 degraded, 2 down.
- `corral daemon socket`: print the absolute socket path, even when no daemon is running.
- `corral daemon nudge`: request a pass; exit 2 if no daemon answers.
- `corral daemon stop [--drain]`: stop after the running pass; exit 2 if absent.
- `corral daemon install [--print] [--force]`: render or write the workspace service.
  It never starts or enables services. A different existing file needs `--force`.

`run`, `stop` and `install` require the owner, not a PM or agent shell. Anyone may
read `status` and `socket`; the PM may `nudge` and should report a down daemon to the owner.

Clients that cannot discover the store themselves, such as the Corral app, take the
socket path from this command: `CORRAL_SOCKET="$(corral daemon socket)"`.

With a daemon owning the loop, `corral sync` requests a pass, waits at most 20 seconds
and reports whether it completed. It exits successfully even if that wait expires.
The hook nudges without opening the store. The classic board follows the frame the
daemon pushes (see [Served reads](#served-reads)) without nudging, so the daemon keeps
its own pace; an explicit board refresh asks for a full pass and waits up to five
seconds for it.
The daemon performs usage refresh and database maintenance instead of the board.
Without a daemon, sync runs one pass, hooks retain coalesced passes, and the board
retains its periodic sync and maintenance. Read-only PM commands never drive sync.

Passes are incremental and paced as described in
[Incremental passes](#incremental-passes). A nudge starts the next pass at once.
PM, runtime and store gauges are sampled about every 15 seconds.
Connections are capped at 64, require hello within five seconds, and have absolute
per-frame I/O deadlines. Client connect is nonblocking and deadline-bounded.
Orderly shutdown allows at most two seconds for outstanding connection writers
after closing the loop, so a draining stop's reply is written before process exit.
Interrupted or aborted accepts retry; descriptor exhaustion retries after 500 ms.
Other listener failures drain and exit nonzero so the service manager can restart.

## Enrolled app connections

Native apps use the existing peer-UID-checked Unix socket and enrol a device with
`device.v1`. Browsers need the optional app listener, off by default:

```toml
[daemon]
listen = "127.0.0.1:7841"
web_origins = ["http://127.0.0.1:8790"]
```

These values are read at daemon startup; edits require a restart. Origins match exactly,
never as wildcards. HTTPS origins and HTTP loopback origins are supported; paths,
credentials and insecure non-loopback origins are invalid. Every WebSocket still requires
pinned Noise IK (IKpsk2 during pairing); there is no plaintext application protocol or
WebSocket-to-Unix proxy.

- `corral device pair`: offer a short-lived pairing link suitable for QR encoding (raw-code fallback),
  wait for the browser's explicit Pair action, then confirm only if its six-digit
  code matches. Settings › Devices on any administrator app, browser or native, offers the same flow.
- `corral device list [--json]`: list native/browser records, including revoked ones.
- `corral device revoke <id>`: revoke once and close every live session for that device.
  Pair and revoke require the owner; list is read-only. An absent daemon exits 2.

Pairing uses Noise IKpsk2 with a single-use token, a two-minute offer window and a
separate two-minute confirmation window. Later sessions use pinned Noise IK.
At most four offers/claims and eight app connections are live; the total connection
cap remains 64. Sessions renew after one hour or 2^20 encrypted messages.
The device registry is bounded to 256 records including revoked devices.

`listen` accepts one explicit numeric unicast interface, including IPv6, LAN or a Tailscale
address. Hostnames, unspecified/wildcard and multicast binds are refused. Omitting it opens
no TCP port; the existing Unix socket is unchanged. Binding a remote interface is an
explicit opt-in, not discovery or firewall configuration. Keep the listener on an interface
whose exposure you intend; use the host firewall to restrict access.

Remote **browsers need HTTPS for the app and WSS for its carrier**, even though Noise
already encrypts application frames. WebCrypto needs a secure context; HTTP-delivered app
code can be rewritten, and an HTTPS page cannot open an insecure LAN WebSocket. Put a
trusted TLS reverse proxy (or Tailscale Serve with HTTPS enabled) in front of the listener
and set the externally reachable WebSocket base URL:

```toml
[daemon]
listen = "100.64.0.2:7841"
web_endpoint = "wss://workstation.tailnet.ts.net"
web_origins = ["https://corral.example"]
```

The proxy forwards `/v1/session` and `/v1/pair/<id>` as WebSocket upgrades to the exact
configured address, preserving the browser's `Origin` header. Serve the packaged app at
the allow-listed HTTPS origin. TLS certificates, DNS and proxy deployment are operator
prerequisites, not supplied by `corrald`. Do not rewrite Origin to bypass admission.
For a same-host TLS terminator, keep `listen = "127.0.0.1:7841"` instead. Noise remains
end-to-end through that terminator; it never gets daemon/device private keys.

Pairing on a remote listener refuses until `web_endpoint` is configured. Offers include
`reach` hints and a version-2 remote code containing the WSS endpoint; ordinary IPv4
loopback offers retain version-1 codes. The app URL carries the code in its fragment (`#corral-pair=…`),
never a query/path. The browser clears the fragment before pairing and still requires an
explicit Pair and trusted-side matching-code confirmation. A QR is just that same URL,
not additional authority. Device listings include coarse `last_seen` and the last
authenticated `reach`; `app.session` reports the current connection's reach. This is the
daemon-observed final hop (a local TLS terminator appears as loopback), not a claim to
detect a VPN or a proxy's remote peer. Last seen is refreshed at most once a minute and
is not an online-presence promise.

Public device records and owner-attributed enrol/pair/revoke events live in the
workspace store. The daemon's static key does not: it is a private `0600`
`$XDG_STATE_HOME/corral/<store-id>/identity.key` file (fallback `$HOME/.local/state`).
Keep that state directory to preserve browser pins across daemon restarts.

Enrolled native and WebSocket sessions negotiate the same typed app subscriptions,
details, chat and permitted actions. Browser and native authority is identical:
new devices start with `reply`, `act`, `assistant`, `assistant_act`, `admin`, editable
per device. Any admin device can pair/revoke or edit authority, but cannot raise its
own flags. Setting `admin=true` needs fresh device-bound preview/proof. Authority
changes notify active sessions to refresh authority and invalidate confirmations.
Confirm/Admin commands need a tap against the daemon's current preview plus device
proof, not biometric verification. Generic CLI commands, opaque CLI reads, nudges and
shutdown remain refused on WebSocket; existing CLI executable/build checks are unchanged.

### Opt-in Web Push

Web Push is supported for paired browsers with service workers and notification permission.
It is off by default, separately for each device. Generate a private P-256 VAPID PEM outside
the workspace (never commit it), then configure a contact and exact HTTPS push-service
origins. For example, a Firefox subscription uses Mozilla's push service:

```sh
umask 077
openssl ecparam -name prime256v1 -genkey -noout -out "$HOME/.local/state/corral/vapid.pem"
```

```toml
[daemon.push]
vapid_private_key = "/home/owner/.local/state/corral/vapid.pem"
subject = "mailto:owner@example.com"
origins = ["https://updates.push.services.mozilla.com"]
```

The key must be an absolute-path private regular file (0600); restart the daemon after
changing these settings. Add only the exact push-service origin your browser actually
returns (for example `https://fcm.googleapis.com` or `https://web.push.apple.com`).
Non-HTTPS and unlisted subscription endpoints are refused; the sender does not follow
redirects and bounds each HTTP request to five seconds.

In browser Settings › Notifications, explicitly enable push and choose kinds:
needs-you, plan awaiting acceptance, stuck, escalated, or daemon unhealthy. Permission
is requested from that gesture. The paired, authenticated device stores its own
subscription and filters through `push.subscribe`; `push.unsubscribe` deletes it.
Other devices cannot replace it. Revoking a device removes its subscription and prevents
future sends; an already accepted/in-flight content-free notification may still arrive.

The worker is the app's shipped wasm-bindgen JS module, registered with module scope
inside its own directory, not a root-scope cache/proxy. Host the scripts on the app's
same origin; a cross-origin `--asset-url` CDN cannot host this worker. Browser/platform
restrictions still apply (including installed-web-app requirements on iOS).
Notifications say only “Corral: Something needs you”; no task ids, titles, text, kinds
or counts go to the notification service. RFC 8291 ECE encrypts the fixed payload and
VAPID authenticates the daemon. Opening a notification focuses/opens the app; it must
reconnect through Noise to fetch facts or act.

The daemon watches the shared canonical Inbox projection only while opted-in subscriptions
exist. Open owner moves, not fixture counts or PM/worker waits, trigger a push; identical
attention sets are suppressed. Sends are reserved durably and limited to one per device
per minute, with a five-minute TTL and one collapse topic. `last_delivered` means the
push service accepted the message, **not proof the browser displayed or read it**.
Failures remain visible through `push.status`; endpoint URLs and key material never
appear in that response or events.

This is not a relay: the browser must still reach the configured Noise endpoint when
opened. Web Push depends on the browser vendor's external push service and network
availability. It can announce degradation while corrald runs; a stopped daemon cannot
send its own “unreachable” notification. That requires an independently hosted dead-man
service. Native APNs/FCM app tokens, publisher credentials, ticket-authenticated rendezvous,
outbound daemon multiplexing and hosted TLS are not supplied by this direct-listener
implementation.


Typed app topics publish coherent Store snapshots even if another writer commits
while they are read; the next refresh catches up. Lease and command-journal
bookkeeping does not invalidate cached topics.

The `agents` topic lists the sessions running now and those that ended within
`[ui] agents_ended_window` (default one hour), not every session the store remembers.
A resumable assistant session with no host is on the `assistant` topic only. Each row
carries the last four lines of its recorded transcript, or of a running gate's log.
Those inputs and the window move without a store write, so a watched `agents` topic is
read again every two seconds; it is published only when a row changed. A session detail
is one bounded window of its transcript: `older` is the cursor of the window before it
and stays valid after later writes, and the reply never has a continuation.

Pane-backed sessions negotiate `stream.terminal.v1`: acknowledged, scoped
`Topic::Terminal` replacement windows with bounded scrollback and visible screen.
Updates publish at most once per second; at most 200 rows and 52 KiB of line/style
arrays fit inside the 60 KiB CONTROL envelope. Parallel styled runs retain colour,
bold, dim, italic, underline, strike and inverse; over-budget styling falls back
to plain lines. Slow consumers coalesce and closing a view unsubscribes.
The reader verifies pane ownership against the configured herdr server. Session
Send/Cancel use Act authority; raw Keys use Admin plus fresh device-bound proof.
Input never queues offline. Daemon-hosted assistants have no raw terminal; a gate's
terminal remains its recorded log.

See [the app guide](../../apps/corral-app/README.md#live-data) for enrolment and recovery.


## Incremental passes

The daemon remembers, in memory only, what each sync step last saw: the store change
feed position, the herdr pane inventory, Git ref and index file stats of open tasks'
worktrees and of project repositories, resource admission, herdr availability, and
when each step and task last ran or comes due. A pass then runs a step only when one
of its inputs changed or a time it depends on came due, and runs the per-task steps
only for tasks whose state, panes, worktree, task-keyed records or deadlines changed.
Steps whose inputs live outside the store (transcripts, gate results, PM health,
harness files) still run at least every 20 or 60 seconds, the sandbox sweep every five
minutes. Usage refresh, PM supervision and database maintenance follow the same
rules after the pass; the heartbeat is touched on every pass.

A quiet pass is one where nothing changed and nothing came due: it only checks its
inputs (one feed scan, one herdr pane listing, a few file stats per worktree and
repository, no Git subprocess, no store write) and costs tens of milliseconds instead
of the second or more a full pass takes.

A pass is full, running every step for every open task as `corral sync` does without a
daemon, when:

- the daemon has just started (memory is empty);
- `corral sync`, `corral daemon nudge` or an explicit board refresh asked for it
  (hook and PM nudges only start an incremental pass early);
- the configuration or the session binding changed, or herdr became available or
  unavailable;
- the change feed has a gap or more than 4,096 entries since the previous pass;
- the clock went backwards or jumped forward by more than ten minutes;
- ten minutes passed since the last full pass. This bounds how late a step can react
  to an input the rules do not watch.

Pacing: after a quiet incremental pass the daemon waits 19 times the pass's CPU time,
clamped between 5 and 60 seconds; after a full pass, or an incremental pass that did
work, it waits 19 times the CPU time, clamped between 2 and 60 seconds, as the loop did
before passes were incremental. A step or task coming due never shortens the wait, so
paced passes use at most about 5% of a core; anything due is served by the next pass.
A nudge starts the next pass at once. Expect one extra "echo" pass after any pass that
wrote: it sees those writes, reruns the affected steps once and settles.

The pass mode is set persistently by `[daemon] pass` in `.corral/corral.toml`
(`"incremental"`, the default, `"audit"` or `"full"`; see
[configuration](configuration.md#daemon)). The daemon reads it from the config it reloads
every pass, so an edit applies from the next pass, survives restarts and reinstalls, and
needs no restart:

```toml
[daemon]
pass = "audit"
```

Two switch files beside the daemon socket can make the mode more conservative for a
running daemon without a config edit, never less: the effective mode is the most
conservative of the setting and the files (full > audit > incremental). They live in the
runtime directory, so a reboot removes them. They are checked at the start of every pass
like the timing switch ([timing guide](timing.md)); no restart is needed. `"full"` behaves
as `sync-full-on`, `"audit"` as `sync-audit-on`:

- `sync-full-on` turns skipping off: every pass is full. Use it to rule out the
  incremental planner when something reacts late, or as a rollback switch.
- `sync-audit-on` makes every pass full while still deciding what an incremental pass
  would have skipped, on the schedule an incremental pass would keep (a step's
  "every N seconds" cadence restarts only when its rule fired). A step or task that would
  have been skipped but wrote to the store or reported a change is printed to the daemon's
  stderr (the service journal) as
  `corrald sync audit: <Step>[ task <id>] would have been skipped but acted`, with one of
  two explanations appended when they hold:
  - `; raced: an input its rule lists changed during the pass`: another process or an
    earlier step of the same pass wrote it, or the config file was edited mid-pass. The
    next pass sees that change and runs the step, so nothing is lost.
  - `; delayed: its every <N>s cadence would have run it`: the input is not in the store
    (a process, a file, a herdr pane), and the step's cadence would have run it at most
    N seconds later.

  If both files exist, `sync-full-on` wins and nothing is audited. `corral daemon status`
  prints `sync passes: <mode> (<source>)` while the effective mode is not `incremental`,
  naming `[daemon] pass` or the switch file that set it.

```sh
touch "$(dirname "$(corral daemon socket)")/sync-audit-on"
rm "$(dirname "$(corral daemon socket)")/sync-audit-on"
```

With timing on, each pass record shows what the planner did, as span counts (`n`):

| Span | `n` in one pass record |
|---|---|
| `sync.quiet` | 1 when the pass ran no gated step |
| `sync.skip` | steps and tasks the pass skipped |
| `sync.full` | 1 when the pass was full |
| `sync.full.<reason>` | 1 for the reason of a full pass: `start`, `owner` (a `cli`/`board` nudge), `switch` (`sync-full-on` or `pass = "full"`), `periodic`, `config`, `session`, `herdr`, `feed_gap`, `clock_jump` or `audit` (`sync-audit-on` or `pass = "audit"`) |
| `sync.audit.pass` | 1 when the pass was audited |
| `sync.audit.step` | steps and tasks the audited pass decided |
| `sync.audit.would_skip` | of those, the ones the rules would have skipped |
| `sync.audit.raced` | of those, the ones that acted after an input their rule lists changed during the pass |
| `sync.audit.delayed` | of those, the ones that acted on an input only their `every N` cadence covers |
| `sync.audit.wrong_skip` | of those, the ones that acted with no listed input changed and no cadence covering them |

An audited pass is one whose only reason to be full is audit mode; a pass that is
full for another reason (start, nudge, periodic, ...) is not audited. `sync.audit.pass`
proves audit ran, and `sync.audit.would_skip` that it had something to check.
`sync.audit.raced` is expected while other processes write, and `sync.audit.delayed` while
agents, gates and hooks run outside the store. `sync.audit.wrong_skip` should stay absent or
zero; any finding names a rule that misses an input, so report the printed step and task.

## Projected board

The daemon folds the store change feed into a rebuildable in-memory board on a
separate thread. Pass completion and nudges wake it; sequence polling also catches
direct CLI writes, every 500 ms with board subscribers and every two seconds otherwise.
The cache (`boardview-v1.bin` under the workspace cache root) is saved at most once
every ten seconds while running and again on orderly shutdown. A missing or invalid
cache rebuilds with a new epoch; subscribers then resubscribe from the start.

`corral list --view` reads this projection over the daemon socket without running a
pass. It groups tasks by who acts next and prints the run/gate/review/land rail,
current activity and title. `corral list --view --json` returns the board rows.
The view conflicts with classic listing's `--query`, `--status`, `--project`,
`--all`, `--done`, `--since` and `--columns`; it does not fall back to store reads
when the daemon is absent (exit 2; inspect `corral daemon status`).

Socket topics `board` and `health` push bounded CONTROL pages with one page in flight
until `sub.ack`. Slow subscribers retain only a cursor, never a backlog of rows.
`welcome.caps` negotiates `board.v1`; `welcome.daemon.epoch` identifies the projection.
`welcome.daemon.store` is the store directory path (the workspace root's `.corral`).
Health deltas reflect condition or completed-pass changes, not elapsed-age ticks.
Topic `classic` (capability `classic.v1`) carries only a change notice for the classic
board frame: epoch, revision, store sequence and time. One notice is in flight until
`sub.ack`, the latest revision wins, and a different epoch resets the topic.

## Served reads

The socket's `read.v1` capability serves PM next/brief readouts, the workspace PM
agent's bounded `read_next` / `read_brief` documents, and the classic board's frame.
A dedicated reader thread computes them without holding up sync. PM and agent PM
documents reuse their last result only while it is younger than ten seconds and no
non-lease store change occurred; agent PM documents are never warmed in the background.

The classic board's frame (`classic_frame`: tasks, counts, pins, plan progress,
gauges, waits, attention and operating status) is computed by the daemon on the reader
thread while a board subscribes to `classic`, and for 120 seconds after the latest frame
read. It recomputes only the parts whose inputs changed, at most every 500 ms, and
publishes a new revision only when the content changed; every board receives the same
encoded document. A full pass asked for by the owner recomputes every part. Queued client
requests take priority. Lease bookkeeping does not affect readouts; bounded change-feed
checks ignore it. The frame reuses the last pass's attention under the same non-lease
rule. Without a daemon, or when the read is refused, the board computes the same frame
itself.

Readouts contain executable-bound commands and action IDs, so a request must match
both the daemon's executable path and build identity; mismatches are refused.
Clients silently compute locally when the daemon is absent, refuses, times out or
cannot provide a valid document. PM `--fresh` always computes locally.
Agent reads retain authentication, live-generation checks, payload validation,
result-size checks and their ephemeral response receipts in the calling process.
Neither path persists a read receipt or journal entry. Only the default workspace
PM scope is served; explicit project scopes, `read_list` (including its pagination
and filters), and `read_lessons` remain local. Legacy preparation runs in the daemon
for served documents, and in the caller on fallback, as for CLI PM reads.

## Commands through the daemon

Store-writing owner and agent commands (`comment`, `move`, `question answer`,
`queue add`, `inbox mark` and the rest of the `cmd.v1` verb table) are submitted to the
daemon, which runs them on one of two command workers and replies with exactly the
output and exit status the command prints in the calling process. PM shells and the
classic board's moves, comments and review waivers take the same path. Verbs that open
an editor, ask on the terminal, launch or focus panes, use Git worktrees, or depend on
the caller's herdr pane stay in the calling process, as do reads.

A command runs in the calling process only when the daemon provably did not accept it:

- `CORRAL_DAEMON_COMMANDS=0` is set, the command carries a claimed owner action, or
  the clock is pinned for a test;
- no daemon answers (socket missing, connection refused, connect timeout), hello fails,
  or the daemon does not offer `cmd.v1`;
- the request exceeds one 60 KiB control frame;
- the daemon refuses it (`refused`, including a different executable or build,
  `busy`, `expired`, `method_not_found` or `unavailable`);
- the answer was lost and a follow-up `cmd.result` recorded the id as abandoned, so a
  late copy of the submit is refused.

Once the submit is written and the daemon did not refuse it, the command never runs a
second time locally. Invalid parameters are reported as
`error: daemon rejected command: <message>` (exit 1).

Waits are bounded. A CLI submit expires 120 seconds after it is issued; while the
command runs, the client re-queries the same connection in waits of at most 25 seconds.
After a lost connection it reconnects for at most 30 seconds and asks for the result
again. If no result arrives before the submit expires, it prints
`error: corrald accepted command <id> but its result is unknown; inspect the task before retrying`
and exits 1. The board allows two seconds to connect, expires its submits after ten
seconds and waits at most five seconds in total; it then reports
`corrald is still running command <id>; check the task before repeating`.

Two switches turn routing off: `CORRAL_DAEMON_COMMANDS=0` in the caller's environment
keeps that process's commands local, and `[daemon] commands = false` in
`.corral/corral.toml` makes the daemon refuse every command, so all clients run them
locally. Routing prints nothing on the command's output. When a reachable daemon could
not take a command, the client records it in `<runtime dir>/command-fallback.json` and
`corral daemon status` (text form) adds the line
`commands: last local fallback <command> (<reason>) at <time>`. A missing daemon, the
environment switch, owner actions and pinned clocks are not recorded.

## Install and enable

Run `corral daemon install --print` to inspect the definition, then
`corral daemon install`. The installer uses the absolute executable and workspace
paths, the recorded store-owned herdr socket as `HERDR_SOCKET_PATH` when present,
and the installing shell's `PATH` filtered to existing absolute directories,
deduplicated in order. These environment settings matter because a user manager's
default PATH lacks `herdr`. Reinstall after changing paths or the socket binding.

On Linux the file is `~/.config/systemd/user/corrald@<store-id>.service`.
Run the printed commands:

```sh
systemctl --user daemon-reload
systemctl --user enable --now corrald@<store-id>.service
journalctl --user -u corrald@<store-id>.service -f
```

The systemd unit uses `Restart=on-failure` and `RestartSec=2`: crashes and upgrade
exit 75 restart after two seconds; a clean stop or already-running exit (both 0)
stays stopped. After a deliberate stop, run
`systemctl --user start corrald@<store-id>.service` to start it again. Starting before
interactive login requires the separate decision `loginctl enable-linger <user>`.
On macOS the installer writes
`~/Library/LaunchAgents/dev.corral.corrald.<store-id>.plist` with `RunAtLoad` and a
`KeepAlive` dictionary whose `SuccessfulExit` is false: failures restart, clean
stops do not. It prints the `launchctl bootstrap` command without executing it.
After a deliberate stop, use `launchctl kickstart gui/$(id -u)/dev.corral.corrald.<store-id>`.
Without a supported service manager, run `corral daemon run` in the foreground.
The daemon never starts the herdr server: start the store-owned server yourself.

## Health and upgrades

`corral daemon status` prints daemon state, reasons, runtime reachability and PM
state (`live`, `relaunching`, or `absent`). The client reports `down` when no daemon
answers on the socket; it does not read the heartbeat file. The daemon touches
`<runtime dir>/heartbeat` at the end of every pass without a store write, providing
a liveness signal for external monitors. The board shows degraded/down health in
its status area; health probes happen off the UI thread, no more often than every
ten seconds. `corral up` prints installed/running health or the installation hint.

Health reasons identify the next investigation, not permission to break locks:

| Reason | Meaning / action |
|---|---|
| `sync stalled <age>` | No pass completed recently; inspect the service journal and runtime. |
| `lock <name> held <age> by pid <n>` | A lock is continuously unavailable; inspect its holder, never remove its file. |
| `foreign runtime` | Selected runtime is not store-owned; fix the socket selection/binding. |
| `pm stores <pct>` | A PM record family remains at its retention high-water mark: 15,360 of 16,384 receipts (93.75%) or 56 of 64 generations (87.5%). Retention could not reclaim enough; inspect which current generation or pending record pins the receipts or generations. |
| `restarting for upgrade` | The running executable was replaced; the daemon is finishing its pass. |
| `unsupervised` | No service manager is supervising this foreground process. |

Replacing the `corral` binary makes the daemon finish its pass and exit 75; the
service manager restarts the installed binary. An open classic board still needs
`q` and a restart. For a coordinated upgrade, stop with `corral daemon stop --drain`,
install, then start the service with `systemctl --user start corrald@<store-id>.service`
(or the launchd kickstart command above). For rollback, disable the service (`systemctl --user
disable --now corrald@<store-id>.service`, or unload the launch agent); once the
lease is released, the board drives sync again.

## PM recovery

Recovery is opt-in in `.corral/corral.toml`:

```toml
[profiles.pi]
harness = "corral-pi"
model = "gpt-6.1-sol"
vendor = "openai"

[pm]
agent = "pi"
supervise = true
```

`pm.supervise` defaults to false; it permits recovery, not heartbeat and lock
diagnostics. The supervisor's heartbeat record is written only when its content changes,
so its `at` is the last change; `pm brief` health reports `age_secs` from the later of
that and `<runtime dir>/heartbeat`. For a store-owned, available herdr server and a
recorded corral-pi PM, recovery relaunches a verified dead bound host when unfinished,
unobserved inbox work exists; replaces a live host whose wake remains unobserved beyond
`pm.attention_secs`; and replaces a generation that refused PM input with
`input_limit` (4,096 keys). The refusal is a definite non-delivery for that
generation; later passes hold it without another bridge call. Relaunch requeues
unobserved messages for the successor (`delivery_requeued`).

Relaunch continues the saved conversation without taking focus. Attempts are
recorded before launch and separated by at least five minutes. Launch acceptance
alone does not demonstrate recovery. Three undemonstrated attempts emit one
`pm_relaunch_failed` event and stop automatic recovery. Reset requires the expected
successor's hello, a live bound host and an Observed receipt for watched work at
that successor endpoint. Old-generation receipts do not qualify. A verified live
external generation change can also reset the counter. Resets retain the last
attempt's five-minute cooldown, including across daemon restarts.

Recovery never starts herdr, kills unrelated processes or panes, removes locks,
dispatches tasks itself, resolves owner decisions or proves observed work was
handled. Only verified normal PM replacement may stop the old PM host. Foreign or
unknown sockets permit no recovery. A cache-guard pause alone never causes relaunch.
Set `pm.supervise = false` to disable automatic PM recovery without disabling sync.
