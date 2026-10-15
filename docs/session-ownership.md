# Herdr session ownership

Pane IDs are scoped to a herdr server, not to the machine. New dispatches save
`runs[].herdr.socket` alongside the workspace and pane IDs. Plan-review and reviewer
pane references also save the socket they were launched against.
The dispatch reservation first saves `launch.socket` atomically with the run's
capacity claim, before external worktree or pane creation. That journal protects
recovery even when a process dies after pane creation but before saving its IDs.

The identity is the socket path selected by herdr: an explicit named session
overrides the inherited socket; otherwise `HERDR_SOCKET_PATH` wins, followed by
`HERDR_SESSION`, then the default server. Socket directories use
`XDG_CONFIG_HOME/herdr` or `HOME/.config/herdr`. The named `default` session and
the unnamed default server identify the same socket. `HERDR_CONFIG_PATH` changes
the config file, not the socket directory.

Before any reconciliation, engine call, state write, event, cleanup, or queued
dispatch, `sync` checks the selected socket against the store's saved pane
references and launch reservations, including reviewer children and plan reviews. A foreign invocation
returns success without output or writes. This also protects `hook`, which calls
the same sync path. Shared inventory probes and the public reviewer watchdog
apply the ownership guard too.

CLI sync and hook open the store without observing configuration edits first.
Only an owning sync may record a pending configuration change. Hooks also check
the invoking server's inherited `HERDR_SOCKET_PATH` or `HERDR_SESSION` before
honoring a client override, so a foreign event cannot be retargeted into a real
store reconciliation by that store's configuration.

Legacy pane references and launch reservations without a saved socket have
**unknown ownership**. Neither the current `[herdr].session` nor environment
selectors are launch evidence. Hook and manual sync remain silent, successful,
read-only no-ops until ownership is explicitly established. They do not sample
panes, observe config changes, release slots, or recover interrupted launches.

To migrate an old store, first identify its original server from retained launch
logs or its original plugin registration. Record that evidence in a text file,
including the socket and why it owns all existing legacy references. Then run:

```sh
corral --root /path/to/store bind-session \
  --socket /absolute/path/to/original/herdr.sock \
  --launch-record /path/to/original-launch-record.txt
```

This is an explicit operator attestation, not automatic discovery or validation
against live pane IDs (which can collide across servers). The command snapshots
the evidence, socket, and binding timestamp into `.corral/herdr-session.json`.
It takes the board writer lock, refuses conflicts with saved worker, reviewer,
plan-review, or launch sockets, and atomically installs the binding. Repeating the
same binding leaves its original evidence untouched; changing its socket is
refused. Keep this durable file with the store: it is not rebuildable `state/`
data. Hooks and sync never create or change it.

Both historical launch defaults are supported by explicit binding: inside herdr,
the default server uses `<config>/herdr/herdr.sock`; outside herdr, corral's
dedicated `corral` session uses `<config>/herdr/sessions/corral/herdr.sock`.
Use the original launch evidence to choose, not the current shell or config.
Once bound, select that same server for sync; normal startup grace and two-miss
disappearance detection apply. If provenance is unavailable or ambiguous, leave
the store unbound and read-only rather than guessing an owner.

A store with conflicting saved socket identities fails closed: no one session
can reconcile it. Keep separate stores for separate herdr sessions. Changing a
session override or configuration does not transfer existing panes or their
ownership to a new server. Dispatch, gate and reviewer launches reject a foreign
session once a store has saved pane ownership. A store without saved pane
references or explicit legacy binding remains unbound, so its first queued
dispatch and local-only gate collection still work.
