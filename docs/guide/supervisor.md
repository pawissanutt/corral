# Supervised sync

`corral sync --watch` reconciles every two seconds and then runs the PM supervisor.
One-shot sync, board sync and plugin hooks do not run it. The watch driver writes a
heartbeat even with recovery disabled. A systemd **user** service can restart the
watch driver after an exit; nothing installs or enables it automatically.

## Recovery switch

In the workspace's `.corral/corral.toml`:

```toml
[pm]
supervise = true
```

The default is `false`. This switch permits recovery actions, not heartbeat or lock
diagnostics. Restart the watch service after changing its configuration.

For a store-owned, available herdr server and a recorded corral-pi PM, recovery:

- relaunches a verified dead bound host when unobserved, non-terminal inbox work exists;
- replaces a live host when a wake has remained unobserved longer than
  `pm.attention_secs`;
- replaces a generation whose bridge refused PM input with `input_limit` (4,096 keys).
  The refusal is recorded against the envelope as a definite non-delivery for that
  generation: later passes hold it without another bridge call, and the relaunch
  requeues its unobserved messages for the next generation (`delivery_requeued`). The
  marker is cleared once the generation advances.

Relaunch continues the saved conversation through the normal recovery entry point,
without taking focus. Attempts are recorded before recovery starts and spaced at
least five minutes apart. Three consecutive failures open the circuit and emit one
`pm_relaunch_failed` event. A successful observation of watched inbox work or an
external PM generation change resets the failure circuit. A successful relaunch
retains its five-minute cooldown.

The supervisor never starts a herdr server, kills unrelated panes or processes, or
removes lock files. Only normal verified PM replacement may stop the old PM host.
It does not dispatch tasks, resolve owner decisions, or prove that observed work was
handled. Unowned (foreign or unknown) sockets are reported as unhealthy and permit
no supervisor action.

The lock watchdog probes `artifact-storage`, `pm-question-wake-outbox`, `dispatch`
and `pm-events` without waiting. A continuously unavailable lock gets one
`lock_held` event and one diagnostic after five minutes. Availability resets the
watch. Linux holder PID/argv come only from the matching device/inode in
`/proc/locks` and `/proc/<pid>/cmdline`; unavailable identity is `unknown`. There is
no lock breaking. The watchdog runs after sync returns: a sync call blocked inside
a lock cannot itself advance this driver's heartbeat.

## Install and manage the user unit

1. Copy `scripts/corral-sync.service` to
   `~/.config/systemd/user/corral-sync.service` (create that directory if needed).
2. Edit the copy: replace the executable placeholder with the absolute path to the
   installed `corral` binary and `<WORKSPACE>` with the absolute workspace root.
3. Select the **existing store-owned** server explicitly with the commented
   `Environment=CORRAL_HERDR_SESSION=...` or `Environment=HERDR_SOCKET_PATH=...`
   line appropriate to your server. Do not leave conflicting selectors. A wrong
   selector is not equivalent to a healthy idle board.
4. Start the herdr server yourself using your existing setup, then run:

```sh
systemctl --user daemon-reload
systemctl --user enable --now corral-sync.service
systemctl --user status corral-sync.service
journalctl --user -u corral-sync.service -f
```

`Restart=always` and `RestartSec=5` restart an exited driver; they do not detect a
hung process. This unit deliberately creates no server or system-wide service.
For startup without an interactive login, user-manager lingering is an optional,
separate operator decision (`loginctl enable-linger "$USER"`).

## Health and rollback

```sh
corral --root /absolute/workspace pm brief --json
```

Read `health.supervisor`: `null` means no heartbeat was ever written. Otherwise it
contains `at`, `age_secs`, driver `pid`, `owned`, `herdr_available`, `status`,
`last_action`, `last_error`, attempt state and lock first-seen timestamps. Normal
heartbeat writes are at most once per ten seconds; status transitions and recovery
attempts are recorded immediately. Increasing age indicates a stopped or blocked
watch driver; inspect service status and its journal. Health reads do not write.
Inbox inspection uses fixed-size pages of the unfinished index, never settled
message history; exceptionally large pending queues are examined across passes.

To roll back the service and automatic recovery:

```sh
systemctl --user disable --now corral-sync.service
```

Set `[pm] supervise = false` in `.corral/corral.toml` before starting any watch
driver again. Board, hooks and one-shot sync retain their existing behavior.
