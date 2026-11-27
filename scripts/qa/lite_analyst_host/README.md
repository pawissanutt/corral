# Opt-in Corral host QA

**Component QA only.** The daemon, Pi/Bun executable and Rust MCP shim are real
Corral components. The Lite work/broker peer and scripted HTTP responder here
are synthetic disposable test code; they are not shipped as part of either
product. A passing receipt does **not** prove joined E1, first-party Lite report
import/reopen or approval/revocation, fresh computation, or permission to enable
for other data.

## Run

Use Linux x86_64 as a non-root user with user/PID/mount/network namespaces, the
regular non-setid `/usr/bin/bwrap`, Landlock TCP ABI ≥4, glibc's standard ELF loader,
`/usr/bin/readelf`, and Python 3.11+ (stdlib only). Prefer the pinned archive invocation in the
[C4 package recipe](../lite_analyst_bundle/README.md), which passes the exact raw
package manifest through to the daemon. For local development, prepare the
three trusted executables first. Example from a checkout with the pinned JS
dependencies already installed:

```sh
cargo build --locked -p corral-daemon --bins
pi_build=$(mktemp -d /tmp/corral-c3-pi.XXXXXXXX)
bun build packages/corral-pi/src/lite-analyst/main.ts --compile \
  --no-compile-autoload-dotenv --no-compile-autoload-bunfig \
  --no-compile-autoload-tsconfig --no-compile-autoload-package-json \
  --outfile="$pi_build/corral-pi-lite-analyst"

python3 -B scripts/qa/lite_analyst_host/run.py --enable-public-fixture-e2e \
  --daemon target/debug/corrald-next \
  --pi "$pi_build/corral-pi-lite-analyst" \
  --shim target/debug/corral-lite-mcp-shim \
  --revision "$(git rev-parse HEAD)"
```

The default packet is `.evidence/lite-fixtures/docs/protocol/analyst-february-close-v1.json`.
If it is not present, pass `--packet` with the exact bytes of
`docs/protocol/analyst-february-close-v1.json` from T-2073:3. The runner checks the
packet, prompt, tool policy and [C1 launcher profile](../../../docs/contracts/lite-analyst-launcher-v1/README.md)
against independent raw SHA-256 pins. It hashes the local binaries and runtime
closure into a QA-only manifest/lock and mounts the staged bytes read-only. The
revision is caller-declared baseline metadata, not proof of a clean build or
substitute for the executable digests. There is no build, install, vault, Git
operation, external HTTP, owner service or repository mount at launch.

`--scenario NAME` is repeatable; omit it for all seven. The only output is a
sanitized JSON receipt. Exit 0 means the selected component lanes passed with
teardown; 64 means disabled/invalid inputs, 69 means isolation unavailable, 70
means a fixture check failed, and 74 means teardown was not confirmed (the
private `corral-c3-qa-*` directory is quarantined instead of removed).

## Fault variant canaries

Use the separate [fault package and producer pins](../lite_analyst_bundle/README.md#build-the-separate-corral-fault-variant),
not a local binary or normal C4/E1 archive. Run the same isolated controller:

```sh
python3 -B scripts/qa/lite_analyst_host/run.py \
  --enable-public-fixture-e2e --with-faults --bundle "$archive" \
  --archive-sha256 "$archive_pin" --manifest-sha256 "$manifest_pin" \
  --revision "$revision" --packet "$packet"
```

`--with-faults` selects only these three lanes by default. The controller uses
FD 9 to check the named barrier and return its fixed action. The Corral
component itself acts; the controller never relays or substitutes for Work or
broker traffic. The endpoint peer is still **synthetic Lite**, not the joined
runner or evidence about Lite's behavior.

- **`corral.start.committed` / `drop`**: read the committed Start identity in
  SQLite before release, with no launch submitted or worker present. Observe
  Work EOF without receiving any reply bytes or closing the peer socket.
  Require the exact same run, now cancelled and cleaned; no bind, broker attach,
  permit, report or provider connection.
- **`corral.broker.after_admission` / `drop`**: confirm both real broker routes
  negotiated and one exact permit durably consumed, with no settlement at the
  barrier. Release, then observe EOF on both broker data sockets before
  stopping the daemon. Require one exact indeterminate settlement and observed
  receipt at the full 1000 µUSD charge, fencing and cleanup. The provider TCP
  connection is opened before reserve; require it to close with zero bytes
  and no broker effects.
- **`corral.sandbox.denied` / `continue`**: accept the product barrier only
  after launch submission and broker negotiation. The product emits it only
  after its supervisor and actual sandboxed Pi worker pass their fixed denial
  checks. Require no permit, provider connection, broker effect or report,
  and confirmed fence and physical teardown.

These lanes do not run the joined fault matrix, first-party Lite, report import
or compute. Other Corral fault barriers are not selected by this controller.

## What is measured

- Every run uses a new private, non-repo `/scratch`, `/state`, loopback-only
  network and PID namespace. The disposable outer controller shares the outer
  runtime mount with Corral so it can bind the fixed loopback HTTP port and
  stage the C1 work/broker descriptors; this **is not** the separate Lite
  product sandbox. The actual inner child mounts are inspected during the first
  POST, including distinct PID/mount/net namespaces, only the corresponding
  executable and read-only `/lib`/`/lib64` closure, private `/scratch`, `/dev`,
  `/proc`, capability/seccomp metadata, environment, argv and FD roles. The
  semantic tool gate and `/proc` measurements do not inject arbitrary code into
  Pi to test each syscall. No outer owner canary, state, vault or QA scripts
  may reach the children. Both launcher levels retain the verified namespace
  init pidfd separately from Bubblewrap's waitable child; wrapper exit alone
  cannot free the run slot or authorise scratch removal. Daemon diagnostics
  must be empty or the fixed error.
- **`happy`**: the HTTP peer checks the exact system and approved question,
  ordered registry and prior call IDs, arguments and structured results. It
  asks for search, then reads the live source handle and calculates using the
  visible metric selector, then derives `finish_report` text and UTF-8 spans
  from the actual `5100`, `2800`, captured `2300`, limitations and citations. The
  Pi initiates the private shim's stdio MCP startup before the first POST;
  the mediator relays only pinned JSON-RPC startup frames and verifies the
  three evidence descriptors independently. The owning fake Lite peer reads
  a `finished` offer, verifies exact canonical bytes against the live-derived
  report, rejects a wrong offer or mismatched ack, acks idempotently and can
  re-fetch the same bytes after ack and post-finish cancel. After teardown the
  only original is the exact retained Corral report (mode 0400), with its
  delivery state durable. There is no fourth turn. At each actual POST the
  exact body and permit must already be durably consumed; after teardown the
  three body hashes and 760 µUSD settled/acked charges are checked again.
- **`pending`**: same three POSTs, with a pending metric reply followed by one
  host-only poll at least 1000 ms later. The third POST must contain only the
  terminal metric as the original calculation's result; no model poll/call.
  Verify finish, delivery and retention as in `happy`.
- **`forbidden`**: a first provider response attempts host-only `read_result`;
  no broker invocation and no second send. The POST has an invalid model
  response and settles conservatively, without falling back to another route.
- **`repeated_id`**: the second complete SSE response reuses the first
  turn's call ID, with valid tool arguments. No tool from the second batch
  dispatches; the invalid exchange settles at the full synthetic hold.
- **`registry`**: the fake peer adds `mitimind_read_note` to the evidence
  registry during negotiation. No reserve, provider request or broker effect.
- **`reconnect`**: the first real POST gets an SSE response missing terminal
  data, leaving a consumed indeterminate turn. At the first sealed settlement
  the fake peer drops the ACK and closes its exclusive socket endpoints. After
  confirmed old daemon/worker teardown, the controller re-execs the daemon with
  new endpoints and epoch and the same state/pins. The exact durable settlement
  is replayed and acknowledged once with no new provider send or bind. Exact
  start retry returns the same run, changed bytes conflict, and a new start is
  refused.
- **`crash`**: at the first fully received and durably consumed POST, a
  barrier withholds all response bytes. The controller SIGKILLs the retained
  daemon while both workers are active, proves the old processes/endpoints
  are gone, and only then re-execs at a new epoch. Recovery settles the
  unresolved consumed step at the full synthetic hold; no provider POST or
  broker action is replayed. This is a real process crash, not a power-loss test.

The fake broker releases only a synthetic captured February projection/cell,
with varying selectors and citations to catch hardcoded handles. It never
executes a metric, writes an authored note, or imports a report. All checks read
current component traffic; the packet is not a prerecorded provider transcript.
The controller verifies final absence of child processes, broker closure, and
that the fixed listener port can be rebound before disposing of scratch. Receipts include only report hash/length and custody checks, never report text,
provider body, credential, or Pi transcript.

## C4 packaging handoff

The C1 daemon owns the child launch. Within its outer sandbox it expects the
verified rootless `/usr/bin/bwrap`, a private `/tmp` for nested setup, and
`/fixture/corral/bin/{corral-pi-lite-analyst,corral-lite-mcp-shim}` as exact
entrypoints (no arguments). Stage the measured ELF runtime dependency closure
under `/fixture/corral/runtime/lib` and `/fixture/corral/runtime/lib64`, at the
same relative paths as the child loaders expect. The supervisor mounts only
these two ABI trees and the individual entrypoint read-only; it never mounts
`/state`, the provider credential, the profile metadata, or the full package
into either child. The product runner, not this QA lock, independently verifies the full joint E1
lock. See the [C4 recipe and separate provider handoff](../lite_analyst_bundle/README.md).

## Script checks

To run just the QA script unit tests (no Corral executables, HTTP or Bubblewrap):

```sh
python3 -B -m unittest discover -v -s scripts/qa/lite_analyst_host
```

For that command, set `CORRAL_QA_PACKET` if the pinned packet is not in the default
location. These tests do not count as C3 runtime evidence.
