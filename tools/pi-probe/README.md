# T-136 disposable Pi prerequisite probe

This tool installs the exact release in `pins.json` into a fresh private,
plugin-free herdr fixture and checks its offline CLI, SDK, catalog and
credential metadata. It never signs in, resolves credentials or calls a
model. It has no production entry point and does not change Corral defaults.

Run from the worktree root with Node v24.13.1 and npm 11.8.0:

```bash
bash -n tools/pi-probe/run.sh
node --check tools/pi-probe/preflight.mjs
bash tools/pi-probe/run.sh </dev/null
```

The helper clears inherited provider variables before starting herdr, uses
private HOME/XDG paths, and removes its own server and fixture on exit. The
npm download is not a provider call. The preflight emits JSON metadata, never
credential values. Package lifecycle scripts are disabled. The installed
release's integrity is checked against the independent registry receipt.
The ambient `pi` binary is deliberately not used: it belongs to another
harness and is not evidence about upstream Pi.

The supplied task did not identify an already authenticated owner-controlled
Pi fixture. Copying credentials from other harnesses is forbidden. This
prerequisite probe therefore expects empty credential metadata and stops
with a **no-go** result; exit 0 means the checks ran, not integration passed.
It does not prove that OpenAI sign-in is unsupported or that the account lacks
entitlement. No login URL, account identifier, token or auth file is an artifact.

A later owner-provisioned sign-in and separately reviewed resumption would
need a different, explicitly authorized live driver; this program deliberately
cannot turn into one by adding credentials. Do not weaken its empty-store
assertion or silently add a paid fallback.

## Unexecuted live protocol

Before a live driver, pin the full dependency lock and runtime, verify the
selected subscription route, and arrange native sign-in inside the private
owner-controlled fixture. No owner-server clients may be used. Subscribe
before each action; await exact events with bounded deadlines.

The complete experiment must fit five model requests total, including
automatic continuations, retries, compaction and tool-result follow-ups.
Enforce at the provider request boundary: reject request 6, clamp
`maxTokens <= 1024`, disable automatic retry/cache warming/compaction calls,
and forbid every provider except `openai-codex` and every model except the
pin. If that request boundary cannot be enforced, do not start.

Use at most three calls for visible-terminal idle/busy steering/follow-up and
cancellation, one for typed fixture readback/mutation, and one for a real
trusted owner decision. This is a budget allocation, not a claim the scenarios
will fit; return no-go with missing coverage if they do not. Test handled
input without starting a call. Test disconnect, stale/foreign generation,
tool denial (including nested/deferred tools), reload and shutdown with
non-model actions where possible. Preserve structured command, event,
mutation and owner-action receipts without prompts or credentials.

The transport, production permissions and durable receipt requirements are
in `docs/records/pi-native-protocol.md`. No synthetic fixture result can
replace real native-terminal proof.
