# The workspace daemon

`corrald` is the always-on sync loop for one workspace, run through `corral daemon`.
It reconciles runs, advances autopilot, refreshes usage, maintains the database and
runs PM recovery. It is not the only store writer: ordinary CLI commands still
read and write the store directly, including while the daemon is unavailable.

A kernel-held lease allows only one loop owner. A fence at the sync entry point
prevents other processes from running a competing pass. A crash releases the lease;
no lock-file deletion is needed or safe.

## Commands

- `corral daemon run [--once]`: run in the foreground, or complete one pass and exit.
  A second daemon prints that it is already running and exits successfully.
  One-shot pass failures return an error; a runtime upgrade latch exits 75.
- `corral daemon status [--json]`: read health; exit 0 healthy, 1 degraded, 2 down.
- `corral daemon nudge`: request a pass; exit 2 if no daemon answers.
- `corral daemon stop [--drain]`: stop after the running pass; exit 2 if absent.
- `corral daemon install [--print] [--force]`: render or write the workspace service.
  It never starts or enables services. A different existing file needs `--force`.

`run`, `stop` and `install` require the owner, not a PM or agent shell. Anyone may
read `status`; the PM may `nudge` and should report a down daemon to the owner.

With a daemon owning the loop, `corral sync` requests a pass, waits at most 20 seconds
and reports whether it completed. It exits successfully even if that wait expires.
The hook nudges without opening the store. The board reads state back on its periodic
ticks without nudging, so the daemon keeps its own pace; an explicit board refresh asks
for a pass and waits up to five seconds for it.
The daemon performs usage refresh and database maintenance instead of the board.
Without a daemon, sync runs one pass, hooks retain coalesced passes, and the board
retains its periodic sync and maintenance. Read-only PM commands never drive sync.

The daemon keeps the board's pacing rule: wait 19 times the CPU time used by a
pass, clamped between 2 and 60 seconds. A nudge starts the next pass at once.
PM, runtime and store gauges are sampled about every 15 seconds.
Connections are capped at 64, require hello within five seconds, and have absolute
per-frame I/O deadlines. Client connect is nonblocking and deadline-bounded.
Orderly shutdown allows at most two seconds for outstanding connection writers
after closing the loop, so a draining stop's reply is written before process exit.
Interrupted or aborted accepts retry; descriptor exhaustion retries after 500 ms.
Other listener failures drain and exit nonzero so the service manager can restart.

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
Health deltas reflect condition or completed-pass changes, not elapsed-age ticks.


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
diagnostics. For a store-owned, available herdr server and a recorded corral-pi PM,
recovery relaunches a verified dead bound host when unfinished, unobserved inbox
work exists; replaces a live host whose wake remains unobserved beyond
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
