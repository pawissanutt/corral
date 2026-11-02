#!/usr/bin/env bash
# End-to-end source install regression. Run from a committed checkout; uses an isolated clone,
# HOME and data directory. Bun is stubbed but the actual build-pi-host.sh and CLI are exercised.
set -euo pipefail
repo=$(git rev-parse --show-toplevel)
tmp=$(mktemp -d "$repo/.evidence/source-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/home" "$tmp/bin" "$tmp/data" "$tmp/tools"
git clone -q --no-hardlinks "$repo" "$tmp/checkout"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export HOME="$tmp/home" XDG_DATA_HOME="$tmp/data" XDG_CONFIG_HOME="$tmp/config"
export CARGO_TARGET_DIR="$repo/target/source-install-e2e"
(cd "$tmp/checkout" && cargo build -q -p corral --bin corral </dev/null)
cp "$CARGO_TARGET_DIR/debug/corral" "$tmp/bin/corral"
compat="$tmp/checkout/packages/corral-pi/compat.json"
export TEST_COMPAT="$compat" TEST_BUN_LOG="$tmp/bun.log"
cat >"$tmp/tools/bun" <<'BUN'
#!/bin/sh
set -eu
echo "$*" >> "$TEST_BUN_LOG"
case "$1" in
  --version) echo 1.4.2 ;;
  install) [ "$2" = --frozen-lockfile ] ;;
  build)
    shift
    out=
    while [ "$#" -gt 0 ]; do
      if [ "$1" = --outfile ]; then out=$2; shift; fi
      shift
    done
    [ -n "$out" ]
    cat >"$out" <<EOF
#!/bin/sh
case "\$1" in
 --version) echo 'corral-pi-host pi 1.3.0 bun 1.4.2' ;;
 --write-install) mkdir -p "\$2/assets"; cp '$TEST_COMPAT' "\$2/compat.json"; echo fresh > "\$2/assets/worker-contract.md" ;;
 *) exit 1 ;;
esac
EOF
    chmod +x "$out" ;;
  *) exit 1 ;;
esac
BUN
chmod +x "$tmp/tools/bun"
export PATH="$tmp/tools:$PATH"
cat >"$tmp/bin/corral-pi-host" <<EOF
#!/bin/sh
case "\$1" in
 --version) echo 'corral-pi-host pi 1.0 bun 1.4.2' ;;
 --write-install) mkdir -p "\$2/assets"; cp '$compat' "\$2/compat.json"; echo stale > "\$2/assets/worker-contract.md" ;;
 *) exit 1 ;;
esac
EOF
chmod +x "$tmp/bin/corral-pi-host"
cli="$tmp/bin/corral"
record="$tmp/data/corral/pi-native/installed.json"
digest() { python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["digest"])' "$record"; }
"$cli" agent install --no-build </dev/null
old=$(digest)
"$cli" agent install </dev/null
fresh=$(digest)
[ "$old" != "$fresh" ]
grep -q '^install --frozen-lockfile$' "$TEST_BUN_LOG"
grep -q '^build --compile --root . --no-compile-autoload-dotenv --no-compile-autoload-bunfig ' "$TEST_BUN_LOG"
"$cli" agent install --from "$tmp/bin/corral-pi-host" </dev/null
[ "$(digest)" = "$fresh" ]
# Missing Bun must not fall back to the sibling, nor change the installation.
if PATH=/usr/bin:/bin "$cli" agent install </dev/null >"$tmp/error" 2>&1; then echo 'missing bun accepted' >&2; exit 1; fi
grep -q bun_unavailable "$tmp/error"
[ "$(digest)" = "$fresh" ]
# A replacement failure must not publish the staged installation.
rm "$tmp/bin/corral-pi-host"
mkdir "$tmp/bin/corral-pi-host"
record_before=$(sha256sum "$record")
if "$cli" agent install </dev/null >"$tmp/error" 2>&1; then echo 'replacement failure accepted' >&2; exit 1; fi
grep -q 'replacing ' "$tmp/error"
[ "$(sha256sum "$record")" = "$record_before" ]
rmdir "$tmp/bin/corral-pi-host"
# A new checkout commit must be refused before modifying either sibling or record.
cp "$CARGO_TARGET_DIR/debug/corral" "$tmp/bin/corral-pi-host"
before=$(sha256sum "$tmp/bin/corral-pi-host")
git -C "$tmp/checkout" -c user.name=Test -c user.email=test@example.com commit --allow-empty -qm changed
head=$(git -C "$tmp/checkout" rev-parse HEAD)
built=$(git -C "$repo" rev-parse HEAD)
if "$cli" agent install </dev/null >"$tmp/error" 2>&1; then echo 'mismatched HEAD accepted' >&2; exit 1; fi
grep -q "$head" "$tmp/error"; grep -q "$built" "$tmp/error"; grep -q -- '--from' "$tmp/error"
[ "$before" = "$(sha256sum "$tmp/bin/corral-pi-host")" ]
[ "$(digest)" = "$fresh" ]
# With the checkout gone, the sibling remains usable as in releases.
rm "$tmp/bin/corral-pi-host"
# Recover the installed host before removing the checkout (fixture references compat there).
cp "$tmp/data/corral/pi-native/assets/$fresh/corral-pi-host" "$tmp/bin/corral-pi-host"
# Give the fixture host its own retained compat file outside the checkout.
cp "$compat" "$tmp/compat.json"
python3 - "$tmp/bin/corral-pi-host" "$compat" "$tmp/compat.json" <<'PY'
import sys
p, old, new = sys.argv[1:]
s = open(p).read().replace(old, new)
open(p, 'w').write(s)
PY
mv "$tmp/checkout" "$tmp/checkout-removed"
"$cli" agent install </dev/null
printf 'PASS: real CLI and build script, digest, overrides, missing Bun, failed replacement, mismatched HEAD, release fallback\n'
