#!/usr/bin/env bash
# Profiles the live board paths (frames, sync tick, keys, detail/review/settings) against a
# private copy of a real workspace store and prints work counts plus informational timings.
#
# Usage: scripts/qa/board-work-profile.sh SOURCE_WORKSPACE OUT_DIR [TEST_BINARY]
#
# Safety: the test runs in bubblewrap with the whole filesystem read-only except OUT_DIR, no
# network, private /tmp and /run/user, and the owner's herdr config directory hidden. herdr is a
# recording fake (scripts/qa/fake-herdr.sh) serving a synthesized inventory in which every open
# pane is still working, so no real server, pane or agent is ever contacted. The board runs as
# if inside the owner's default server (HERDR_ENV, socket path) so session ownership matches the
# store's binding; that socket is hidden by a tmpfs and only the fake binary is ever executed.
set -euo pipefail
exec </dev/null
src=$(realpath "$1")
out=$(realpath -m "$2")
cd "$(dirname "$0")/../.."
test -d "$src/.corral"
case "$out" in "$src"*) echo "OUT_DIR must be outside the source workspace" >&2; exit 2 ;; esac

rm -rf "$out"
mkdir -p "$out/ws/.corral" "$out/herdr"
# A live store's temporary files may vanish mid-copy (rsync status 24).
rsync -a --exclude /artifacts --exclude /lock "$src/.corral/" "$out/ws/.corral/" || [[ $? == 24 ]]
mkdir -p "$out/ws/.corral/artifacts" "$out/ws/.corral/lock"
# Landing needs the read-only project repositories; only the copy's review autopilot is turned
# off, so everything else in the tick (reconcile, movement, dispatch, search index) still runs.
sed -i '/^\[review\]/,/^\[/s/^autopilot = true/autopilot = false/' "$out/ws/.corral/corral.toml"
python3 -I scripts/qa/fake_herdr_inventory.py "$out/ws/.corral/tasks" "$out/herdr"
cp scripts/qa/fake-herdr.sh "$out/herdr/herdr"
chmod +x "$out/herdr/herdr"

binary=${3:-$(cargo test -p corral-tui --lib --no-run --message-format=json 2>/dev/null |
  jq -r 'select(.executable != null and .target.name == "corral_tui") | .executable' | tail -1)}
test -x "$binary"
# The sandbox has a private /tmp, so run a copy kept inside OUT_DIR.
cp "$binary" "$out/corral-tui-tests"
binary="$out/corral-tui-tests"

# Project repositories stay visible (read-only) at the copied workspace's relative paths.
binds=(--ro-bind "$src/.corral/artifacts" "$out/ws/.corral/artifacts")
while IFS= read -r project; do
  if [[ -d "$src/$project" ]]; then
    mkdir -p "$out/ws/$project"
    binds+=(--ro-bind "$src/$project" "$out/ws/$project")
    # Sync takes the landing lock even when nothing lands; give it a private stand-in.
    if [[ -f "$src/$project/.git/corral-landing.lock" ]]; then
      mkdir -p "$out/locks"
      : > "$out/locks/${project//\//-}.lock"
      binds+=(--bind "$out/locks/${project//\//-}.lock" "$out/ws/$project/.git/corral-landing.lock")
    fi
  fi
done < <(python3 -I -c 'import sys, tomllib
for p in tomllib.load(open(sys.argv[1], "rb")).get("projects", {}).values():
    path = p.get("path", "")
    if path and not path.startswith("/"): print(path)' "$src/.corral/corral.toml")

# PROFILE_WRAPPER (for example "strace -f -e trace=openat -o OUT_DIR/trace") wraps the sandbox.
${PROFILE_WRAPPER:-} bwrap --ro-bind / / --dev /dev --proc /proc --unshare-net --die-with-parent \
  --tmpfs /tmp --tmpfs /run/user --tmpfs "$HOME/.config/herdr" \
  --bind "$out" "$out" "${binds[@]}" \
  env -i HOME="$HOME" PATH="$out/herdr:/usr/bin:/bin" LANG=C.UTF-8 \
    HERDR_BIN_PATH="$out/herdr/herdr" FAKE_HERDR="$out/herdr" \
    HERDR_ENV=1 HERDR_SOCKET_PATH="$HOME/.config/herdr/herdr.sock" \
    CORRAL_PROFILE_STORE="$out/ws" \
    ${CORRAL_PROFILE_DETAIL:+CORRAL_PROFILE_DETAIL="$CORRAL_PROFILE_DETAIL"} \
    ${CORRAL_PROFILE_REVIEW:+CORRAL_PROFILE_REVIEW="$CORRAL_PROFILE_REVIEW"} \
  "$binary" --ignored --exact app::work_paths::profile_live_paths --nocapture --test-threads=1
printf 'fake herdr calls: %s\n' "$(wc -l < "$out/herdr/calls" 2>/dev/null || echo 0)"
