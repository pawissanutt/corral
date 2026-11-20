# Day-to-day commands for a source checkout: the `corral-next` CLI, the `corrald-next` daemon
# and its service, the corral-pi agent host in packages/corral-pi, and the app in
# apps/corral-app. `just` lists them.
#
# Daemon and app recipes act on the workspace found from this directory: the nearest
# `.corral-next/` above it, which a worktree under the workspace finds too. A checkout
# elsewhere names it: `just root=/path/to/workspace daemon-status`. The daemon recipes drive
# one systemd user service, `corrald-next.service`; a second workspace on the same machine
# names its own: `just root=/other/workspace unit=corrald-next-other.service daemon-service`.
#
# The old engine is not driven from here: its crates are excluded from the Cargo workspace and
# no longer build. Its installed `corral` binary still answers `corral daemon status`.

set shell := ["bash", "-euo", "pipefail", "-c"]

root := ''
unit := 'corrald-next.service'
corrald := if root == '' { 'corrald-next' } else { 'corrald-next --root ' + quote(root) }
just := quote(just_executable()) + " --justfile " + quote(justfile()) + " root=" + quote(root) + " unit=" + quote(unit)
# What the installed system runs from: the corral-pi host's copy and the served web app.
data := env('XDG_DATA_HOME', home_directory() / '.local/share') / 'corral-next'

[private]
default:
    @{{ just }} --list

# Build and install `corral-next` and `corrald-next` from this checkout, then restart a running daemon on them
[group('corral')]
install:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo install --path crates/corral-next --locked
    cargo install --path crates/corral-daemon --locked
    # A running daemon keeps the executable it started with. The session hosts it started run
    # on through the restart (`KillMode=process` in its service).
    if systemctl --user is-active --quiet {{ quote(unit) }}; then
        {{ just }} daemon-restart
    else
        echo "no daemon was running; start one with: just daemon-start"
    fi

# `install`, then the corral-pi host and the web app from the same commit
[group('corral')]
upgrade: install pi-install app-web-install

# Daemon health in one line; it exits 0 whether or not a daemon answers
[group('daemon')]
daemon-status:
    {{ corrald }} status

# Let the running pass finish, stop the daemon, and wait until its service is inactive
[group('daemon')]
daemon-stop:
    #!/usr/bin/env bash
    set -euo pipefail
    unit={{ quote(unit) }}
    if ! systemctl --user is-active --quiet "$unit"; then
        echo "$unit is not running"
        exit 0
    fi
    # The daemon handles no signal: only this request lets a running pass finish.
    if ! {{ corrald }} stop --drain; then
        echo "the daemon did not take the stop request; stopping $unit" >&2
        systemctl --user stop "$unit"
    fi
    # The request returns before the process exits; a start issued until then would be lost.
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
    systemctl --user start {{ quote(unit) }}
    out=
    for _ in $(seq 30); do
        # Degraded answers too, and its line carries the reasons.
        if out=$({{ corrald }} status 2>&1) && [ "$out" != "daemon down" ]; then
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
    journalctl --user -u {{ quote(unit) }} {{ args }}

# Write the daemon's systemd user service for this workspace, then enable and start it
[group('daemon')]
daemon-service:
    #!/usr/bin/env bash
    set -euo pipefail
    root=$({{ just }} _root)
    bin=$(command -v corrald-next) || { echo "corrald-next is not installed; run: just install" >&2; exit 1; }
    dir=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user
    mkdir -p "$dir"
    # The daemon starts git, herdr, bun and corral-pi, so it needs this shell's PATH and herdr
    # server: a service inherits neither. systemd reads `%` as a specifier.
    # `KillMode=process` ends the daemon alone on a stop or restart. systemd's default kills
    # every process the service started, which are the session hosts, in the middle of a turn.
    {
        printf '[Unit]\nDescription=Corral workspace daemon (corrald-next) for %s\n\n' "${root//%/%%}"
        printf '[Service]\nExecStart="%s" --root "%s" run\n' "${bin//%/%%}" "${root//%/%%}"
        printf 'Environment="PATH=%s"\n' "${PATH//%/%%}"
        if [ -n "${HERDR_SOCKET_PATH:-}" ]; then
            printf 'Environment="HERDR_SOCKET_PATH=%s"\n' "${HERDR_SOCKET_PATH//%/%%}"
        fi
        printf 'KillMode=process\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n'
    } > "$dir/{{ unit }}"
    systemctl --user daemon-reload
    systemctl --user enable --now {{ quote(unit) }}

# Pair a browser or phone: prints a link, waits for the device, then asks before enrolling it
[group('daemon')]
pair *args:
    {{ corrald }} pair {{ args }}

# Install the corral-pi host from this checkout: a copy of the package with its dependencies, and the `corral-pi` launcher
[group('pi')]
pi-install:
    #!/usr/bin/env bash
    set -euo pipefail
    dest={{ quote(data / 'corral-pi') }}
    launcher=$HOME/.local/bin/corral-pi
    mkdir -p "$dest" "$(dirname "$launcher")"
    # By checksum, so a second install rewrites nothing under a session that is running.
    rsync -a --delete --checksum --exclude /node_modules --exclude /test packages/corral-pi/ "$dest/"
    (cd "$dest" && bun install --frozen-lockfile)
    git rev-parse --short HEAD > "$dest/SOURCE_COMMIT"
    cat > "$launcher.new" <<'EOF'
    #!/bin/sh
    # The corral-pi agent host, run from the copy of packages/corral-pi that `just pi-install` keeps.
    set -eu
    exec bun --no-env-file "${XDG_DATA_HOME:-$HOME/.local/share}/corral-next/corral-pi/src/cli.ts" "$@"
    EOF
    chmod 755 "$launcher.new"
    mv -f "$launcher.new" "$launcher"
    "$launcher" --version

# Type-check the corral-pi host
[group('pi')]
[working-directory('packages/corral-pi')]
pi-check:
    bun install --frozen-lockfile
    bun run typecheck

# The corral-pi host's tests
[group('pi')]
[working-directory('packages/corral-pi')]
pi-test *args:
    bun install --frozen-lockfile
    bun test {{ args }}

# Stage a private workspace and daemon with the real Pi host and a scripted provider
[group('parity')]
[working-directory('packages/corral-pi')]
stage-parity listen='127.0.0.1:7843' origin='http://127.0.0.1:8893':
    exec bun --no-env-file test/support/stage.ts --listen {{ quote(listen) }} --origin {{ quote(origin) }}

# Stage the same daemon with a scripted Pi host; script paths are relative to packages/corral-pi
[group('parity')]
[working-directory('packages/corral-pi')]
stage-parity-fake script='test/fixtures/sessions/verifying.json' listen='127.0.0.1:7843' origin='http://127.0.0.1:8893':
    exec bun --no-env-file test/support/stage.ts --script {{ quote(script) }} --listen {{ quote(listen) }} --origin {{ quote(origin) }}

# Run the app in this terminal against the workspace daemon (dev port 8891; 8890 serves the web app)
[group('app')]
[working-directory('apps/corral-app')]
app *args:
    root=$({{ just }} _root); CORRAL_SOCKET="$root/.corral-next/daemon.sock" nsh run --target tui --port 8891 {{ args }}

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

# Rebuild the web app: the static site in apps/corral-app/.nsh/out/Corral
[group('app')]
[working-directory('apps/corral-app')]
app-web:
    nsh package --target web

# `app-web`, then copy the site to the directory corral-next-app-web.service serves (port 8890)
[group('app')]
app-web-install: app-web
    #!/usr/bin/env bash
    set -euo pipefail
    dest={{ quote(data / 'web') }}
    mkdir -p "$dest"
    rsync -a --delete apps/corral-app/.nsh/out/Corral/ "$dest/"
    install -m 644 scripts/app_web_serve.py "$dest.serve.py"
    git rev-parse --short HEAD > "$dest.SOURCE_COMMIT"
    echo "the web app in $dest is now $(cat "$dest.SOURCE_COMMIT")"

# Serve the packaged site in the foreground; its origin must be listed in `[daemon] web_origin`
[group('app')]
[working-directory('apps/corral-app')]
app-serve port='8890':
    python3 ../../scripts/app_web_serve.py --directory .nsh/out/Corral {{ quote(port) }}

# The workspace root: `root`, else the nearest directory above this checkout that holds `.corral-next/`.
[private]
_root:
    #!/usr/bin/env bash
    set -euo pipefail
    dir={{ quote(if root == '' { justfile_directory() } else { root }) }}
    while [ ! -f "$dir/.corral-next/corral.toml" ]; do
        if [ -n {{ quote(root) }} ] || [ "$dir" = / ]; then
            echo "no .corral-next/corral.toml in or above $dir; name the workspace: just root=/path/to/workspace ..." >&2
            exit 1
        fi
        dir=$(dirname "$dir")
    done
    echo "$dir"
