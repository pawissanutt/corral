#!/bin/sh
# Build the release Pi-native host: packages/corral-pi compiled by Bun into one executable that
# embeds Bun, every dependency from the frozen lock, and the role, delivery and auth modules.
# It never autoloads .env or bunfig.toml at runtime. Run on the target platform (no cross build:
# native add-ons ship per platform).
# Usage: scripts/build-pi-host.sh <output path>
set -eu

output=${1:?usage: scripts/build-pi-host.sh <output path>}
repo=$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)
package=$repo/packages/corral-pi
range=$(awk -F'"' '/"range"/ { print $4; exit }' "$package/compat.json")
version=$(bun --version)
[ "$version" = "$range" ] || {
    echo "build-pi-host: bun $version is not the GO manifest's pinned $range" >&2
    exit 1
}
mkdir -p "$(dirname "$output")"
(
    cd "$package"
    bun install --frozen-lockfile
    bun build --compile --no-compile-autoload-dotenv --no-compile-autoload-bunfig \
        src/host/compiled.ts src/extensions/codemode/worker.ts --outfile "$output"
)
probe=$("$output" --version </dev/null)
case "$probe" in
"corral-pi-host pi "*" bun $range") echo "built $output: $probe" ;;
*)
    echo "build-pi-host: $output --version printed: $probe" >&2
    exit 1
    ;;
esac
