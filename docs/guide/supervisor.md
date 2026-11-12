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
least five minutes apart. Launch-command acceptance is not recovery: the heartbeat
retains the source generation, successor generation, attempt time and consecutive
undemonstrated-attempt count. A successor that has not demonstrated recovery by the
end of the cooldown counts toward the three-attempt circuit, even when launching
its command returned success. Three such attempts emit one `pm_relaunch_failed`
event and stop automatic relaunches.

The counter resets only when the expected successor has completed hello, its bound
host is live, and watched inbox work has an Observed receipt bound to that
successor's endpoint; observations from the continued conversation's old generation
do not qualify. Alternatively, a generation change outside the supervisor's recorded
successor resets the counter once that generation's bound host is verified live.
Merely changing to an unbound or dead generation does not reset it. The expected
successor is recorded before launch, so a driver restart during launch does not
mistake that successor for an external recovery. Every reset retains the last
attempt's five-minute cooldown.

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
