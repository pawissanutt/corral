# Day-to-day commands for a source checkout: the installed `corral` binary and its daemon,
# the corral-pi agent host, and the app in apps/corral-app. `just` lists them.
#
# Daemon and agent recipes act on the workspace `corral` discovers from this directory (the
# nearest `.corral/` above it). The stop, start, restart, logs and service recipes drive the
# systemd user service that `corral daemon install` writes.
#
# A checkout outside the workspace, or a worktree (where corral refuses discovery), names the
# workspace: `just root=/path/to/workspace daemon-status`.

set shell := ["bash", "-euo", "pipefail", "-c"]

root := ''
corral := if root == '' { 'corral' } else { 'corral --root ' + quote(root) }
just := quote(just_executable()) + " --justfile " + quote(justfile()) + " root=" + quote(root)

[private]
default:
    @{{ just }} --list

# Build and install `corral` from this checkout, then wait for the daemon to restart on it
[group('corral')]
install:
    #!/usr/bin/env bash
    set -euo pipefail
    old=$({{ just }} _pid)
    cargo install --path crates/corral --locked
    if [ -z "$old" ]; then
        echo "no daemon was running; start one with: just daemon-start"
        exit 0
    fi
    # A daemon whose executable was replaced finishes its pass and exits 75; the service restarts it.
    echo "waiting for daemon $old to finish its pass and restart"
    for _ in $(seq 180); do
        pid=$({{ just }} _pid)
        if [ -n "$pid" ] && [ "$pid" != "$old" ]; then
            {{ corral }} daemon status || [ $? -eq 1 ]
            exit 0
        fi
        sleep 1
    done
    echo "daemon $old has not restarted after 180s; see: just daemon-logs" >&2
    exit 1

# `install`, then rebuild and install the corral-pi host from the same commit
[group('corral')]
upgrade: install agent-install

# Daemon health (exit 0 healthy, 1 degraded, 2 down)
[group('daemon')]
daemon-status:
    {{ corral }} daemon status

# Drain the running pass, stop the daemon, and wait until its service is inactive
[group('daemon')]
daemon-stop:
    #!/usr/bin/env bash
    set -euo pipefail
    unit=$({{ just }} _unit)
    code=0
    {{ corral }} daemon stop --drain || code=$?
    # 2: no daemon answered, so there is nothing to stop.
    [ "$code" -eq 0 ] || [ "$code" -eq 2 ] || exit "$code"
    # The request returns at once; a start issued before the old process exits would be lost.
    for _ in $(seq 180); do
        systemctl --user is-active --quiet "$unit" || exit 0
        sleep 1
    done
    echo "$unit is still active after 180s; see: just daemon-logs" >&2
    exit 1

# Start the daemon's service and wait until it answers
[group('daemon')]
daemon-start:
    #!/usr/bin/env bash
    set -euo pipefail
    unit=$({{ just }} _unit)
    systemctl --user start "$unit"
    for _ in $(seq 30); do
        # 1 is degraded: it answers, and the status line carries the reasons.
        if out=$({{ corral }} daemon status 2>&1) || [ $? -eq 1 ]; then
            echo "$out"
            exit 0
        fi
        sleep 1
    done
    echo "$out" >&2
    exit 1

# Restart the daemon on the installed binary (after a config change, for example)
[group('daemon')]
daemon-restart: daemon-stop daemon-start

# Follow the daemon's journal
[group('daemon')]
daemon-logs *args='-n 50 -f':
    unit=$({{ just }} _unit); journalctl --user -u "$unit" {{ args }}

# Write the service definition (pass --force to replace a different one), then enable and start it
[group('daemon')]
daemon-service *args:
    {{ corral }} daemon install {{ args }}
    systemctl --user daemon-reload
    unit=$({{ just }} _unit); systemctl --user enable --now "$unit"

# Rebuild the corral-pi host from this checkout and install it; `corral` must be built from HEAD
[group('agent')]
agent-install:
    {{ corral }} agent install

# The corral-pi host's runtime, assets, roles and credentials
[group('agent')]
agent-doctor:
    {{ corral }} agent doctor

# Run the app against the workspace daemon: `just app` in this terminal, `just app web` in a browser
[group('app')]
[working-directory('apps/corral-app')]
app target='tui' *args:
    socket=$({{ corral }} daemon socket); CORRAL_SOCKET="$socket" nsh run --target {{ target }} {{ args }}

# Check the app
[group('app')]
[working-directory('apps/corral-app')]
app-check:
    nsh check

# The app's unit and headless shell tests
[group('app')]
[working-directory('apps/corral-app')]
app-test *args:
    cargo test {{ args }}

# Package the static web site into apps/corral-app/.nsh/out/Corral
[group('app')]
[working-directory('apps/corral-app')]
app-package:
    nsh package --target web

# Serve the packaged site on loopback
[group('app')]
[working-directory('apps/corral-app')]
app-serve port='4173':
    python3 -m http.server --bind 127.0.0.1 --directory .nsh/out/Corral {{ port }}

# The service's unit name: the store id is the directory that holds the daemon socket.
[private]
_unit:
    @socket=$({{ corral }} daemon socket); echo "corrald@$(basename "$(dirname "$socket")").service"

# The answering daemon's process id; empty when none answers or `corral` is not installed yet.
[private]
_pid:
    @{{ corral }} daemon status --json 2>/dev/null | sed -n 's/.*"pid": *\([0-9][0-9]*\).*/\1/p' || true
