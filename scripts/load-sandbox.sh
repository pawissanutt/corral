#!/usr/bin/env bash
# All intentional load belongs inside this single resource-capped container.
set -euo pipefail
usage() {
    echo 'Usage: scripts/load-sandbox.sh [--cpus N] [--memory SIZE] [--pids-limit N] [--burners N] [--image IMAGE] -- COMMAND [ARG...]'
    echo 'Defaults: 2 CPUs (maximum 4), 2g memory, 256 PIDs, no burners, docker.io/library/rust:1.94-trixie.'
}
die() { echo "load-sandbox: $*" >&2; exit 2; }
cpus=2 memory=2g pids=256 burners=0 image=docker.io/library/rust:1.94-trixie
while (($#)); do
    case "$1" in
        --help|-h) usage; exit 0 ;;
        --cpus|--memory|--pids-limit|--burners|--image)
            (($# >= 2)) || die "missing value for $1"
            case "$1" in
                --cpus) cpus=$2 ;; --memory) memory=$2 ;;
                --pids-limit) pids=$2 ;; --burners) burners=$2 ;; --image) image=$2 ;;
            esac
            shift 2 ;;
        --) shift; break ;;
        *) die "unknown option: $1 (use -- before the command)" ;;
    esac
done
# Validate before invoking Podman, including NaN, exponents and unlimited values.
[[ $cpus =~ ^[0-9]+([.][0-9]+)?$ ]] || die '--cpus must be a number in (0, 4]'
awk -v n="$cpus" 'BEGIN { exit !(n > 0 && n <= 4) }' || die '--cpus must be in (0, 4]'
[[ $memory =~ ^[1-9][0-9]*[bkmgBKMG]?$ ]] || die '--memory must be a positive size (e.g. 2g)'
[[ $pids =~ ^[1-9][0-9]{0,5}$ ]] || die '--pids-limit must be positive (maximum 999999)'
[[ $burners =~ ^(0|[1-9][0-9]?)$ ]] || die '--burners must be an integer from 0 to 99'
(($#)) || die 'a command is required after --'
command -v podman >/dev/null || die 'podman is required; never fall back to host load'
root=$(git rev-parse --show-toplevel)
cargo_home=${CARGO_HOME:-$HOME/.cargo}
rustup_home=${RUSTUP_HOME:-$HOME/.rustup}
bun_binary=$(command -v bun || true)
name=corral-load-$(id -u)-$$-$RANDOM
client=
cleanup() {
    local status=$?
    trap - EXIT INT TERM HUP
    # Remove by preselected name even if create was interrupted before returning its ID.
    if podman container exists "$name"; then
        podman rm --force --time 0 "$name" >/dev/null || status=1
    fi
    if [[ -n $client ]]; then wait "$client" 2>/dev/null || true; fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP
mounts=(--volume "$root:$root:rw")
path=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
if [[ -d $cargo_home/bin && -d $rustup_home ]]; then
    mounts+=(--volume "$cargo_home/bin:$cargo_home/bin:ro" --volume "$rustup_home:$rustup_home:ro")
    path=$cargo_home/bin:$path
fi
if [[ -n $bun_binary ]]; then
    # npm-installed Bun is often a symlink outside ~/.bun; mount the real binary.
    bun_binary=$(readlink -f "$bun_binary")
    mounts+=(--volume "$bun_binary:/opt/corral-bun/bin/bun:ro")
    path=/opt/corral-bun/bin:$path
fi
# No host PID namespace, sockets, privileges, or writable toolchains. Cargo caches and
# build artifacts are container-local; only the selected worktree is writable.
podman create --name "$name" --label corral.load-sandbox=true \
    --cpus "$cpus" --memory "$memory" --memory-swap "$memory" --pids-limit "$pids" \
    --userns keep-id --cap-drop ALL --security-opt no-new-privileges \
    "${mounts[@]}" --workdir "$root" \
    --env HOME=/tmp/load-home --env CARGO_HOME=/tmp/load-home/.cargo \
    --env CARGO_TARGET_DIR=/tmp/load-target --env RUSTUP_HOME="$rustup_home" \
    --env RUSTUP_AUTO_INSTALL=0 --env PATH="$path" --entrypoint /bin/bash "$image" \
    -c 'set -e
        mkdir -p "$HOME" "$CARGO_HOME"
        count=$1; shift
        for ((i=0; i<count; i++)); do
            /bin/bash -c "while :; do :; done" &
        done
        "$@"
    ' load-sandbox "$burners" "$@" >/dev/null </dev/null
# A separate wait lets signal traps promptly remove the container, including all
# burners and daemonized test children. Nothing generates load in this shell.
podman start --attach "$name" </dev/null &
client=$!
status=0
wait "$client" || status=$?
client=
exit "$status"
