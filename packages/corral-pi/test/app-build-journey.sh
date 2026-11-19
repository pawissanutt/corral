#!/usr/bin/env bash
set -euo pipefail

if ! command -v bun >/dev/null; then
    echo 'SKIP app build journey: Bun is not installed'
    exit 0
fi

cd "$(dirname -- "${BASH_SOURCE[0]}")/../../.."
if [[ ! -d packages/corral-pi/node_modules ]]; then
    (cd packages/corral-pi && bun install --frozen-lockfile)
fi

if [[ $# == 0 ]]; then
    target=${CARGO_TARGET_DIR:-target}
    cargo build -j 2 -p corral-next -p corral-daemon --target-dir "$target"
    set -- "$target/debug/corral-next" "$target/debug/corrald-next"
fi
if [[ $# != 2 ]]; then
    echo 'usage: app-build-journey.sh [<corral-next> <corrald-next>]' >&2
    exit 2
fi
exec bun --no-env-file packages/corral-pi/test/app-build-journey.ts "$@"
