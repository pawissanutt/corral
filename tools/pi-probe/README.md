# Disposable Pi probes (T-136, Bun re-proof T-298)

These tools re-prove T-136's six native criteria (terminal control, busy input,
cancellation, lifecycle receipts, session ownership, restricted tools) for the
exact Pi release in `pins.json`, hosted by **Bun**. They have no production
entry point and change no Corral default. Results: `bun-go.json`, recorded in
`docs/records/pi-native-compatibility.md`.

## Bun runtime, launch and lock

- `package.json` pins `@earendil-works/pi-coding-agent` exactly at `1.3.0`
  (upstream commit `71023aec`), plus the T-136 verification peers and
  TypeScript 6.0.2 as dev dependencies. `trustedDependencies` is empty and every
  install passes `--ignore-scripts`, so no lifecycle script runs.
- `bun.lock` was migrated by `bun install` from T-136's retained npm lock, so it
  keeps that lock's resolutions and integrities. `lock-diff.json` lists every
  difference from it (`bun lock-diff.mjs <T-136 package-lock.json>`).
- `runtime.mjs` refuses to start unless it runs on Bun inside `pins.json`'s
  `bunRange`, `bun.lock`'s raw SHA-256 equals `bunLockSha256`, and the
  installed Pi matches the pinned version and integrity.
- `lock-check.mjs` checks a frozen install: lock bytes unchanged, every locked
  package installed at its version, one copy of each Pi package and `typebox`,
  and each declared peer satisfied (host-provided where the root declares it).
- `bun-fixture.sh` shims `node`, `npm` and `npx` to exit 127, first on PATH,
  and installs the lock frozen into an empty fixture root.

## Commands

Run from the worktree root. Each driver clears its environment, shims Node and
uses private HOME/XDG paths; the herdr drivers use a fresh plugin-free server
from `scripts/test-herdr.sh` and remove it on exit.

```bash
bash tools/pi-probe/lane.sh </dev/null          # frozen install, bun test, typecheck
bash tools/pi-probe/run-native.sh </dev/null    # offline native proof
bash tools/pi-probe/run-fixtures.sh </dev/null  # regenerates fixtures/
bun tools/pi-probe/go-record.mjs tools/pi-probe/output/native-offline-result.json \
  tools/pi-probe/output/native-live-result.json > tools/pi-probe/bun-go.json
shellcheck -x tools/pi-probe/*.sh
```

`run-native.sh` installs the lock into an empty root inside the fixture, copies
the probe sources beside it and hosts upstream `InteractiveMode` in one private
herdr pane, controlled over the disposable Unix bridge (`native-bridge.mjs`).
It writes `output/native-offline-result.json` (ignored): per-criterion results,
eleven ordered steps, the install check, and provider/transport counters.

## Offline mode (default)

- **Synthetic credential.** The launcher writes a Codex-shaped auth file whose
  token has a fixed signature and a synthetic account. It goes through the same
  read-only `readSubscription`/`readOnlyStore` path as a real one; the owner's
  `~/.codex` is never read.
- **Fake transport.** `fake-transport.mjs` answers the pinned Codex adapter's
  injected `fetch` with Responses-API SSE frames, after `request-guard.mjs` has
  validated the real zstd wire request (endpoint, headers, model, 1,024-token
  cap). It holds an open response stream for the busy and cancellation steps,
  answers HTTP 503 for the provider-failure step, and serves scripted usage and
  tool calls for the fixtures. The global `fetch` is replaced by a refusing
  counter, the only path to a provider; GO requires it to stay at zero.
- **Failure classes.** A step failure is `harness_port` (probe, fake transport,
  bridge or fixture plumbing; the default for `ProbeFailure`) or
  `upstream_incompatibility` (an assertion on observed upstream behavior, a
  `ProbeFailure` tagged upstream, or an exception whose first stack frame lies
  under `node_modules/`). The result lists them separately.

## Live mode (not run in T-298)

`PI_PROBE_NATIVE_OFFLINE=0 bash tools/pi-probe/run-native.sh` and
`run-reuse.sh` make provider requests, so both refuse before reading any owner
credential unless `PI_PROBE_LIVE_AUTHORIZATION` names a new recorded owner
authorization (a comment timestamp). T-136's consumed authorization
`2026-10-18T02:05:19.15090102Z` is always refused. Each authorization gets its
own persistent ledger, `output/live-budget-<digest>.json`, capped at 5
requests, 1,024 output tokens each and $0 paid API; never delete it to gain
calls. Without a grant the result is `NOT RUN` and `bun-go.json`'s live column
says so. Live mode keeps T-136's pre-dispatch hold and injected failure.

## Fixtures

`fixtures/session-usage.jsonl` is a Pi 1.3.0 session with three responses of
known synthetic usage; `session-usage.expected.json` gives each response and
the per-category totals, derived from the script alone. `fixtures/session-dangling.jsonl`
is a snapshot taken while one `corral_gate` call and one upstream `edit` call
were in flight, so neither has a result; `session-dangling.expected.json` names
both calls. `fixtures.test.mjs` re-parses both with upstream Pi.

## T-136 Node baseline (historical)

`run.sh` and `preflight.mjs` are T-136's original credential-free Node/npm
prerequisite probe, kept unchanged as evidence (`pins.json`'s `node`/`npm`
fields describe it). They are not part of the Bun proof and fail with Node
shimmed. `verification.md` and the `*.log` files are T-136's ledger and
receipts; the T-136 lock is artifact `T-136:R1:d96d0a73-package-lock.json`.
