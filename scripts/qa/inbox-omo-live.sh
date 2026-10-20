#!/usr/bin/env bash
# Live check (T-207): owner replies reach an idle, a busy and a drafting OmO pane exactly once
# through the thread gateway, and corral observes each one. Runs only inside a fresh isolated
# herdr server (scripts/test-herdr.sh) with a private OmO agent directory: the owner's
# sessions, gateway store and herdr server are never touched. Spends a few small model turns.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OWNER_AGENT="${OMO_CODING_AGENT_DIR:-$HOME/.omo/agent}"
OMO_BIN="${CORRAL_LIVE_OMO:-$(command -v omo)}"
EVIDENCE="${CORRAL_LIVE_EVIDENCE:-$REPO/.omo/evidence/inbox-live}"
CORRAL="$(cd "$REPO" && cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/debug/corral"
mkdir -p "$EVIDENCE"
LOG="$EVIDENCE/live-check.log"
: >"$LOG"
log() { printf '%s\n' "$*" | tee -a "$LOG"; }
fail() { log "FAIL: $*"; exit 1; }

source "$REPO/scripts/test-herdr.sh"
corral_test_herdr_start || exit 1
H=(herdr --session corral-test)
# The isolated server shadows harnesses with failing shims; this check needs the real OmO,
# confined to a private agent directory that holds only copied credentials and settings.
AG="$CORRAL_TEST_HERDR_ROOT/omo-agent"
mkdir -p "$AG"
for file in auth.json settings.json models.json; do
  [ -f "$OWNER_AGENT/$file" ] && cp "$OWNER_AGENT/$file" "$AG/"
done
export OMO_CODING_AGENT_DIR="$AG" CORRAL_OMO_PROGRAM="$OMO_BIN"
log "isolated herdr root: $CORRAL_TEST_HERDR_ROOT; omo: $OMO_BIN ($("$OMO_BIN" --version </dev/null))"

(cd "$REPO" && cargo build -q -p corral) || fail "cargo build"
W="$CORRAL_TEST_HERDR_ROOT/board"
mkdir -p "$W/app"
git -C "$W/app" init -q -b main
echo "# app" >"$W/app/README.md"
git -C "$W/app" add README.md
git -C "$W/app" commit -q -m "initial commit"
"$CORRAL" --root "$W" init </dev/null >/dev/null
# Launch an idle OmO (the dispatch prompt is not sent) so each case controls the pane state.
python3 - "$W/.corral/corral.toml" "$AG" "$OMO_BIN" <<'PY'
import re, sys
path, agent, omo = sys.argv[1:]
text = open(path).read()
command = f"command = \": '{{prompt_file}}'; exec env OMO_CODING_AGENT_DIR='{agent}' '{omo}'\""
text, n = re.subn(r"(\[harnesses\.omo\]\n)command = .*\n", lambda m: m.group(1) + command + "\n", text)
assert n == 1, "harnesses.omo not found"
open(path, "w").write(text)
PY
C=("$CORRAL" --root "$W")
"${C[@]}" new "Inbox live check" -p app --ready </dev/null >/dev/null
T=T-001
"${C[@]}" dispatch "$T" --profile omo </dev/null | tee -a "$LOG"
field() { "${C[@]}" show "$T" --json </dev/null | jq -r "$1"; }
[ "$(field '.runs[-1].delivery')" = inbox ] || fail "OmO run was not launched with inbox delivery"
PANE=$(field '.runs[-1].herdr.pane_id')
KEY="${T}_$(field '.runs[-1].id')"
WT="$W/app/.worktrees/$T"

session_file() {
  "$OMO_BIN" thread list --all-scope --json </dev/null |
    jq -r --arg cwd "$WT" 'if type == "array" then [.[] | select(.cwd == $cwd and .status == "live")][0].sessionPath // empty else empty end'
}
deadline=$((SECONDS + 90))
until SF=$(session_file) && [ -n "$SF" ]; do
  (( SECONDS < deadline )) || fail "OmO session never appeared in $WT"
  sleep 1
done
log "bound OmO session transcript: $SF (pane $PANE, recipient $KEY)"

long_message() {
  printf '%s live check. Do not run tools or edit files; reply with exactly: %s\n' "$1" "$2"
  for i in $(seq 1 70); do
    printf 'filler line %02d: "quoted" text, a tab\there, unicode \xe2\x80\x94 %s\n' "$i" "$1"
  done
}
message_id() { jq -r --arg text "$1" '[.[] | select(.text == $text)][0].message' <<<"$(replied_json)"; }
replied_json() { jq -s '[.[] | select(.kind == "replied") | .detail]' "$W/.corral/events.jsonl"; }
observed() {
  "${C[@]}" inbox show "$1" --json </dev/null | jq -e '.observed != null' >/dev/null
}
wait_observed() {
  local id=$1 deadline=$((SECONDS + $2))
  until observed "$id"; do
    (( SECONDS < deadline )) || return 1
    "${C[@]}" sync </dev/null >/dev/null 2>&1 || true
    sleep 2
  done
}
deliveries_of() {
  python3 - "$SF" "$1" <<'PY'
import json, sys
path, payload = sys.argv[1], open(sys.argv[2]).read()
count = 0
for line in open(path):
    entry = json.loads(line)
    if entry.get("customType") == "session_control_delivery" and entry["details"].get("display_text") == payload:
        count += 1
print(count)
PY
}
check_once() {
  local id=$1 label=$2 seq envelope payload
  seq=${id##*#}
  envelope=$("${C[@]}" inbox show "$id" --json </dev/null | jq -r '.envelope.payload_path')
  [ -f "$envelope" ] || fail "$label: no frozen envelope for $id"
  local count
  count=$(deliveries_of "$envelope")
  log "$label: $id observed; typed gateway entries carrying the complete envelope: $count"
  [ "$count" = 1 ] || fail "$label: expected exactly one delivery, saw $count"
  "${C[@]}" inbox show "$id" --json </dev/null | jq '{id: .message.id, admitted: .admitted.detail.delivery, observed: .observed}' | tee -a "$LOG"
}

log "=== idle pane"
IDLE=$(long_message "Idle" "IDLE-OK")
"${C[@]}" reply "$T" "$IDLE" </dev/null | tail -n 1 >/dev/null
ID_IDLE=$(message_id "$IDLE")
wait_observed "$ID_IDLE" 60 || fail "idle message never observed"
check_once "$ID_IDLE" idle

log "=== busy pane"
"${C[@]}" reply "$T" "Live check: run the shell command 'sleep 40' with your bash tool, then reply with exactly: BUSY-WORK-DONE" </dev/null >/dev/null
deadline=$((SECONDS + 60))
until "${H[@]}" pane read "$PANE" --lines 40 </dev/null | grep -q 'sleep 40'; do
  (( SECONDS < deadline )) || fail "agent never started the busy command"
  sleep 1
done
BUSY=$(long_message "Busy" "BUSY-OK")
"${C[@]}" reply "$T" "$BUSY" </dev/null >/dev/null
ID_BUSY=$(message_id "$BUSY")
log "busy reply admission: $("${C[@]}" inbox show "$ID_BUSY" --json </dev/null | jq -c '.admitted.detail.delivery')"
wait_observed "$ID_BUSY" 120 || fail "busy message never observed"
check_once "$ID_BUSY" busy

log "=== drafting pane"
deadline=$((SECONDS + 120))
until "${H[@]}" pane read "$PANE" --lines 60 </dev/null | grep -q 'BUSY-OK'; do
  (( SECONDS < deadline )) || fail "agent never finished the busy turn"
  sleep 2
done
sleep 3
"${H[@]}" pane send-text "$PANE" "owner draft that must not be submitted" </dev/null >/dev/null
DRAFT=$(long_message "Draft" "DRAFT-OK")
"${C[@]}" reply "$T" "$DRAFT" </dev/null >/dev/null
ID_DRAFT=$(message_id "$DRAFT")
log "draft reply admission: $("${C[@]}" inbox show "$ID_DRAFT" --json </dev/null | jq -c '.admitted.detail | {effective_mode, delivery}')"
for _ in 1 2 3 4 5; do
  "${C[@]}" sync </dev/null >/dev/null 2>&1 || true
  sleep 2
done
observed "$ID_DRAFT" && fail "message was delivered over the user's draft"
log "held while the user drafts: not observed after 10s"
"${H[@]}" pane read "$PANE" --lines 6 </dev/null | tee -a "$LOG"
"${H[@]}" pane send-keys "$PANE" ctrl+c </dev/null >/dev/null
wait_observed "$ID_DRAFT" 60 || fail "draft-held message never observed after the draft cleared"
check_once "$ID_DRAFT" drafting

log "=== board state"
"${C[@]}" inbox --for "$KEY" --all --json </dev/null | tee "$EVIDENCE/inbox.json" >/dev/null
jq -c '.[] | {id: .message.id, kind: .message.kind, observed: (.observed != null)}' "$EVIDENCE/inbox.json" | tee -a "$LOG"
grep -E '"kind":"(message_observed|envelope_attempt|replied)"' "$W/.corral/events.jsonl" >"$EVIDENCE/events.jsonl" || true
cp "$SF" "$EVIDENCE/session-transcript.jsonl"
"${H[@]}" pane close "$PANE" </dev/null >/dev/null || true
log "PASS: idle, busy and drafting deliveries each observed exactly once"
