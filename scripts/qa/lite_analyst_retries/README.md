# T-2071 · offline Lite analyst retry probe

This is a **synthetic contract exercise**, not a Corral endpoint, Pi role, artifact
store, or Lite integration. It uses the pinned [T-2069 bundle](../../../docs/contracts/lite-analyst-v1/README.md).
Run on Linux with Python 3.10+ and its standard library (including SQLite with
JSON functions). No daemon, Git operations, packages, vault, provider credentials,
services or network are needed.

From the Corral checkout:

```sh
mkdir -p .evidence
python3 -B scripts/qa/lite_analyst_retries/run.py \
  --report-out .evidence/T-2071-sealed-report.json \
  > .evidence/T-2071-receipt.json
```

Exit 0 and `status: pass` mean all checks passed. On test failure the exit is
nonzero, the JSON identifies the failed case, and stderr contains the traceback.
Omit `--report-out` for a receipt only; it must name a new file if used. The
report is the exact fetched byte sequence (no trailing newline), not the receipt
or a pretty-printed copy. The host tool `finish_report` has no path argument;
`--report-out` is only the harness's local way to export evidence. Repeated runs
should produce byte-identical reports and receipts.

| Probe | Observable checks |
|---|---|
| Lost start reply | Canonical request replay after deadline returns the current run; changed digest conflicts. A single durable run, slot and launch claim exist in private non-repo scratch. Launch is modeled, not a real Pi worker. |
| Turn holds and permits | Lite and Corral keep independent SQLite journals. Deny each side's cap independently, keep unresolved holds across UTC midnight, replay the same reserve only within its lease, and refuse stale, expired or disconnected/rebound dispatch and changed request bytes. |
| Ambiguous provider response | A third journal records every real call to the scripted fake provider, with no deduplication. Lose its reply after the send record commits, reopen the journals with no in-memory usage, then prove one send total, no next paid step, and one worst-case debit on each side despite settlement receipt loss. |
| Pending result and finish | First paid turn asks for February evidence and the predefined calculation. Host polling observes pending then polls again, loses the terminal reply, and recovers it by exact invocation replay without new compute or provider send. A revoked grant cannot replay that cached result. Report validation rejects absent/unreceived handles, broken UTF-8 spans, stale generation and an extra path argument. A second paid turn delivers `finish_report` using the replies actually accepted. The committed offer seals the invocation receipt ledger, not preloaded handles or a transcript. |
| Finish/cancel and abrupt exit | Event barriers hold the finish call after blob staging or after row commit, forcing both race outcomes without sleeps. A separate process exits without unwinding before/after blob publication and before/after SQLite row commit. Reopen to verify whether an offer and its ledger are visible; pre-commit staged bytes are unreferenced. After commit, recover by read/fetch, replay the same finish, deny a conflicting finish, and leave the offer intact on cancellation. |
| Delivery | The initiating fake Lite link fetches twice, verifies and imports once, reads the stored bytes back, then acknowledges twice. Acknowledgement doesn't remove the original. Another link with valid identifiers cannot read, cancel, fetch or acknowledge. |

This probes selected orderings with local journals, a synthetic clock and injected
responses. It does **not** demonstrate process/network isolation (T-2070), actual
MCP negotiation, authenticated transport, independent clock skew, real spawn/reap
or provider behavior, power-loss durability, retention/deletion, or the full v1
state machine. The separate lifecycle and turn probes are composed for the happy
path, not a production shared supervisor. Passing is not permission to enable the
integration or use private data.
