#!/usr/bin/env bash
# Live check (T-207): owner replies reach an idle, a busy and a drafting OmO pane exactly once
# through the thread gateway, and corral observes each one. Runs only inside a fresh isolated
# herdr server (scripts/test-herdr.sh) with a private OmO agent directory: the owner's
# sessions, gateway store and herdr server are never touched. Spends a few small model turns.
# Pane state comes from the session's own control endpoint (`get_state`), never from text.
# The receipt binds the result to the committed candidate and the binary that ran.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OWNER_AGENT="${OMO_CODING_AGENT_DIR:-$HOME/.omo/agent}"
OMO_BIN="${CORRAL_LIVE_OMO:-$(command -v omo)}"
EVIDENCE="${CORRAL_LIVE_EVIDENCE:-$REPO/.omo/evidence/inbox-live}"
CORRAL="$(cd "$REPO" && cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/debug/corral"
rm -rf "$EVIDENCE"
mkdir -p "$EVIDENCE"
LOG="$EVIDENCE/live-check.log"
log() { printf '%s\n' "$*" | tee -a "$LOG"; }
fail() { log "FAIL: $*"; exit 1; }

HEAD=$(git -C "$REPO" rev-parse HEAD)
TREE=$(git -C "$REPO" rev-parse 'HEAD^{tree}')
[ -z "$(git -C "$REPO" status --porcelain --untracked-files=no)" ] ||
  fail "tracked changes are uncommitted; the live check must run on a committed candidate"
STARTED=$(date -u +%Y-%m-%dT%H:%M:%SZ)
SESSION_ID=""
receipt() {
  local status=$?
  jq -n --arg head "$HEAD" --arg tree "$TREE" --arg started "$STARTED" \
    --arg finished "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --arg binary "${BINARY_SHA:-}" \
    --arg omo "${OMO_VERSION:-}" --arg session "$SESSION_ID" --arg root "${CORRAL_TEST_HERDR_ROOT:-}" \
    --argjson exit "$status" \
    '{candidate: {head: $head, tree: $tree}, corral_binary_sha256: $binary, omo: $omo,
      isolated_herdr_root: $root, omo_session: $session, started: $started,
      finished: $finished, exit_status: $exit}' >"$EVIDENCE/receipt.json"
  corral_test_herdr_cleanup
}

source "$REPO/scripts/test-herdr.sh"
corral_test_herdr_start || exit 1
trap receipt EXIT
H=(herdr --session corral-test)
# The isolated server shadows harnesses with failing shims; this check needs the real OmO,
# confined to a private agent directory that holds only copied credentials and settings.
AG="$CORRAL_TEST_HERDR_ROOT/omo-agent"
mkdir -p "$AG"
for file in auth.json settings.json models.json; do
  [ -f "$OWNER_AGENT/$file" ] && cp "$OWNER_AGENT/$file" "$AG/"
done
export OMO_CODING_AGENT_DIR="$AG" CORRAL_OMO_PROGRAM="$OMO_BIN"
OMO_VERSION=$("$OMO_BIN" --version </dev/null)

(cd "$REPO" && cargo build -q -p corral) || fail "cargo build"
BINARY_SHA=$(sha256sum "$CORRAL" | cut -d' ' -f1)
log "candidate $HEAD tree $TREE; corral sha256 $BINARY_SHA; $OMO_VERSION"
log "isolated herdr root: $CORRAL_TEST_HERDR_ROOT"
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

row() {
  "$OMO_BIN" thread list --all-scope --json </dev/null |
    jq -c --arg cwd "$WT" 'if type == "array" then [.[] | select(.cwd == $cwd and .status == "live")][0] // empty else empty end'
}
deadline=$((SECONDS + 90))
until ROW=$(row) && [ -n "$ROW" ]; do
  (( SECONDS < deadline )) || fail "OmO session never appeared in $WT"
  sleep 1
done
SF=$(jq -r .sessionPath <<<"$ROW")
SOCKET=$(jq -r .socket <<<"$ROW")
SESSION_ID=$(jq -r .durableSessionId <<<"$ROW")
log "bound OmO session $SESSION_ID (pane $PANE, recipient $KEY)"

# The session's own control endpoint: {isStreaming, editor_has_draft, turn_epoch, ...}.
state() {
  python3 - "$SOCKET" <<'PY'
import json, socket, sys
path = sys.argv[1]
sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.settimeout(5)
sock.connect(path)
sock.sendall(open(path + ".secret", "rb").read())
sock.sendall(b'{"type":"get_state","id":"live-check"}\n')
data = b""
while not data.endswith(b"\n"):
    chunk = sock.recv(65536)
    if not chunk:
        break
    data += chunk
reply = json.loads(data)
assert reply.get("success"), reply
d = reply["data"]
print(json.dumps({k: d.get(k) for k in ("isStreaming", "editor_has_draft", "turn_epoch", "pendingMessageCount")}))
PY
}
wait_state() {
  local filter=$1 limit=$2 deadline=$((SECONDS + $2)) now
  until now=$(state) && jq -e "$filter" <<<"$now" >/dev/null; do
    (( SECONDS < deadline )) || fail "pane never reached $filter within ${limit}s (last $now)"
    sleep 0.5
  done
  log "pane state: $now"
}

long_message() {
  printf '%s live check. Do not run tools or edit files; reply with one short line.\n' "$1"
  for i in $(seq 1 70); do
    printf 'filler line %02d: "quoted" text, a tab\there, unicode \xe2\x80\x94 %s\n' "$i" "$1"
  done
}
message_for() {
  jq -rs --arg text "$1" '[.[] | select(.kind == "replied" and .detail.text == $text) | .detail.message][-1]' \
    "$W/.corral/events.jsonl"
}
entry() { "${C[@]}" inbox show "$1" --json </dev/null; }
observed() { entry "$1" | jq -e '.observed != null' >/dev/null; }
wait_observed() {
  local id=$1 deadline=$((SECONDS + $2))
  until observed "$id"; do
    (( SECONDS < deadline )) || return 1
    "${C[@]}" sync </dev/null >/dev/null 2>&1 || true
    sleep 2
  done
}
check_once() {
  local id=$1 label=$2 payload hash count
  payload=$(entry "$id" | jq -r '.envelope.payload_path')
  hash=$(entry "$id" | jq -r '.envelope.payload_sha256')
  [ -f "$payload" ] || fail "$label: no frozen envelope for $id"
  [ "$(sha256sum "$payload" | cut -d' ' -f1)" = "$hash" ] || fail "$label: frozen payload hash mismatch"
  count=$(python3 - "$SF" "$payload" <<'PY'
import json, sys
path, payload = sys.argv[1], open(sys.argv[2]).read()
print(sum(1 for line in open(path)
          if (entry := json.loads(line)).get("customType") == "session_control_delivery"
          and entry["details"].get("display_text") == payload))
PY
)
  log "$label: $id observed; transcript entries carrying the complete envelope (sha256 $hash): $count"
  [ "$count" = 1 ] || fail "$label: expected exactly one delivery, saw $count"
  entry "$id" | jq -c '{id: .message.id, admitted: .admitted.detail.delivery, observed: .observed.detail}' | tee -a "$LOG"
}
admission() { entry "$1" | jq -r '.admitted.detail.delivery.kind'; }

log "=== idle pane"
wait_state '.isStreaming == false and .editor_has_draft == false' 60
IDLE=$(long_message "Idle")
"${C[@]}" reply "$T" "$IDLE" </dev/null >/dev/null
ID_IDLE=$(message_for "$IDLE")
wait_observed "$ID_IDLE" 60 || fail "idle message never observed"
[ "$(admission "$ID_IDLE")" = started ] || fail "idle admission was $(admission "$ID_IDLE")"
check_once "$ID_IDLE" idle

log "=== busy pane"
wait_state '.isStreaming == false' 120
"${C[@]}" reply "$T" "Live check: run the shell command 'sleep 45' with your bash tool, then reply with one short line." </dev/null >/dev/null
wait_state '.isStreaming == true' 60
BUSY=$(long_message "Busy")
"${C[@]}" reply "$T" "$BUSY" </dev/null >/dev/null
ID_BUSY=$(message_for "$BUSY")
log "busy admission: $(admission "$ID_BUSY"); state after send: $(state)"
[ "$(admission "$ID_BUSY")" = queued ] || fail "busy admission was $(admission "$ID_BUSY")"
wait_observed "$ID_BUSY" 180 || fail "busy message never observed"
check_once "$ID_BUSY" busy

log "=== drafting pane"
wait_state '.isStreaming == false' 180
"${H[@]}" pane send-text "$PANE" "owner draft that must not be submitted" </dev/null >/dev/null
wait_state '.editor_has_draft == true' 20
DRAFT=$(long_message "Draft")
"${C[@]}" reply "$T" "$DRAFT" </dev/null >/dev/null
ID_DRAFT=$(message_for "$DRAFT")
log "draft admission: $(entry "$ID_DRAFT" | jq -c '.admitted.detail | {effective_mode, delivery}')"
for _ in 1 2 3 4 5; do
  "${C[@]}" sync </dev/null >/dev/null 2>&1 || true
  observed "$ID_DRAFT" && fail "message was delivered over the user's draft"
  sleep 2
done
wait_state '.editor_has_draft == true and .isStreaming == false' 5
log "held while the user drafts: unobserved across 5 sync passes"
"${H[@]}" pane send-keys "$PANE" ctrl+c </dev/null >/dev/null
wait_state '.editor_has_draft == false' 20
wait_observed "$ID_DRAFT" 90 || fail "draft-held message never observed after the draft cleared"
check_once "$ID_DRAFT" drafting

log "=== board state"
"${C[@]}" inbox --for "$KEY" --all --json </dev/null >"$EVIDENCE/inbox.json"
jq -c '.[] | {id: .message.id, kind: .message.kind, observed: (.observed != null)}' "$EVIDENCE/inbox.json" | tee -a "$LOG"
cp -r "$W/.corral/inbox/$KEY" "$EVIDENCE/inbox-records"
grep -E '"kind":"(message_posted|envelope_attempt|message_admitted|message_observed|replied)"' \
  "$W/.corral/events.jsonl" >"$EVIDENCE/events.jsonl" || true
cp "$SF" "$EVIDENCE/session-transcript.jsonl"
"${H[@]}" pane close "$PANE" </dev/null >/dev/null || true
log "PASS: idle, busy and drafting deliveries each observed exactly once"
