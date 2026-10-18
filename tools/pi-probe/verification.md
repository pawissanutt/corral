# October 18, 2026 verification ledger

The current authorization is the durable owner comment at
`2026-10-18T02:05:19.15090102Z`. It supersedes the old no-reuse boundary
only for this bounded probe. Historical gate/review results do not verify
the rebased resumption.

## Probe receipts

| Receipt | Observed result |
|---|---|
| `output/reuse-route-2026-10-18.log` | Exit 0; existing Codex subscription reused read-only/in-memory. Request 1 HTTP 200, 7 output tokens; wire cap 1024 accepted. Owner credential bytes/inode/mode unchanged; no fixture auth file; cleanup completed. |
| `output/native-live-2026-10-18.log` | Exit 0; `bounded-native-proof`, all seven grouped scenario assertions passed across the six native criteria. Requests 2-4 HTTP 200, outputs 7/7/6. Native cancellation and injected failure occurred before HTTP. Native reload, pinned model reselection, session replacement, deferred denial and exact fixture decision/lost-ack replay passed. |
| `output/live-budget.json` | Four reservations under the one authorization, never reset. At most 1024 output tokens each, total reported output 27; $0 paid API. No fifth request, retry, refresh or compaction. |
| `output/native-fixture-deferred.log` | Exit 0; model-free native terminal/tool/decision checks passed. `nativeProofComplete=false` correctly distinguishes this from live proof. |
| `output/offline-preserved-2026-10-18.log` | Exit 0; original offline probe unchanged, no credentials/provider requests, six capabilities `not_run`, private cleanup completed. |
| `output/runtime/package-lock.json` | Complete tested graph retained; raw SHA-256 `d96d0a7303245992b782b25787e13e4bd0576ec3f1d00b3e22ceb39e1648472a`. |
| `output/reuse-owner-authorization.json` | Full current canonical owner authorization, no credentials. |
| `output/pre-reuse-review2-notes.md` | Full previous independent escalation notes, preserved, not reused as approval. |

The native tool fixture labels its synthetic assistant parent explicitly
as not provider-generated. The terminal decision actor is the synthetic
private fixture driver, not the real owner. The increment and receipt cache
are disposable; production durable state, real owner decisions, remote
cancellation, restart recovery and installed packaging are not verified.

## Local checks

All commands below exited 0 on the resumption source:

```bash
node --test tools/pi-probe/credentials.test.mjs \
  tools/pi-probe/request-guard.test.mjs tools/pi-probe/native-bridge.test.mjs
shellcheck -x tools/pi-probe/run*.sh
tsc --noEmit --allowJs --checkJs --module esnext --moduleResolution bundler \
  --resolveJsonModule --target es2024 --types node \
  --typeRoots "$PWD/tools/pi-probe/output/runtime/node_modules/@types" \
  tools/pi-probe/*.mjs
```

Eleven tests passed, zero failures/skips. Coverage includes read-only access,
expired-access refusal, symlink rejection, secret-safe parse errors, denied
refresh callbacks, compressed wire cap enforcement, persistent five-call
budget, paid-destination refusal, scope/generation fencing, lost-ack replay,
conflicting payload rejection, in-flight deduplication and receipt capacity.
The tests use synthetic credentials and no provider requests.

TypeScript 6.0.2 uses the pinned runtime declarations and exact verification
peers `@types/node@24.10.0`, `@modelcontextprotocol/sdk@1.25.2`. No
`skipLibCheck`, ignored type error or weakened test is used. Automatic LSP
diagnostics repeatedly timed out after 3000 ms; checked-JS compilation is
the completed diagnostics evidence. ShellCheck is 0.10.0.

The model-free fixture's failed development receipts remain separate:
`native-fixture-only.log`, `native-fixture-dialog.log`,
`native-fixture-pipeline.log`, `native-fixture-baseline.log`. They exposed
the wrong dialog event, missing synthetic assistant parent and deferred
tools filtered from the registry. None is counted as a pass or made a model
request. The corrected fixture requires an allowed nested baseline before
accepting denial evidence and observes the deferred sentinel's real hook.

The full configured Corral gate and fresh independent verdict are collected
by the board after the final source commit. Their canonical identities and
complete logs, not this prose or an older gate, determine review readiness.

Current canonical raw artifact references:

- `T-136:R1:5162e6f0-reuse-route-2026-10-18.log`
- `T-136:R1:88422cb1-native-live-2026-10-18.log`
- `T-136:R1:9580949c-native-fixture-deferred.log`
- `T-136:R1:5ccc5189-offline-preserved-2026-10-18.log`
- `T-136:R1:04c419d3-live-budget.json`
- `T-136:R1:d96d0a73-package-lock.json`
- `T-136:R1:b08f6904-reuse-owner-authorization.json`
- `T-136:R1:acce0a91-review-notes.md` (historical escalation, not approval)

## Historical October 17 verification coverage

The following original ledger describes the pre-reuse baseline only. Its
missing-live findings are superseded by the October 18 measurements above.

October 17, 2026. This ledger distinguishes a successful offline prerequisite
check from a successful native integration. The decision is **no-go**.

| Scenario | Exact check | Result | Evidence |
|---|---|---|---|
| Approved scope before probing | `corral plan show T-102 --json`; `sha256sum /home/pawis/miti-workspace/.corral/state/plans/T-102/D4/plan.md` | P1 snapshot matched; PR3 and PR4 approve the bounded spike only. | `provenance.json`; compatibility report |
| Current main and actual PM needs | `git rev-parse HEAD main`; `corral list --all --json`; source review | Inspected main `911486b4347027f522e155c69812405f10cb3759`; existing readbacks/authority reused conceptually, no production changes. | Compatibility matrix |
| Disposable private prerequisite check | `bash tools/pi-probe/run.sh </dev/null` | Exit 0 on the final typed driver: package, integrity, CLI version, SDK catalog and empty private auth metadata checked. Integration remains no-go. | `preflight-final.log`; preceding successful `preflight.log` retained |
| Initial failed prerequisite check | Same command, initial internal-export implementation | Exit 1: `ReadOnlyAuthStorage` is not a public root export. Corrected before the successful check. | `failed-preflight.log` (exception excerpt) |
| Missing fixture authentication | Public `ModelRuntime.listCredentials()` in the probe | Count 0; no credential resolution, login or model request; stopped. | `preflight-final.log` prerequisite receipt |
| Cleanup of all attempts | `test ! -e /tmp/corral-herdr-9kYp6x`; `test ! -e /tmp/corral-herdr-Tq7Dwv`; `test ! -e /tmp/corral-herdr-FHmGdE` | All exit 0; no server/fixture retained for sign-in. | Successful cleanup receipt; command results recorded in session |
| Script syntax | `bash -n tools/pi-probe/run.sh`; `node --check tools/pi-probe/preflight.mjs` | Both exit 0. | Session command results |
| Static language-server check | `lsp_diagnostics` on `preflight.mjs` | Unavailable: fresh diagnostics timed out after 3,000 ms. Syntax/runtime checks are not a static typecheck. | Session diagnostic result |
| Local JavaScript static check | `tsc --noEmit --allowJs --checkJs --module nodenext --target es2024 --types node --typeRoots /home/pawis/.nvm/versions/node/v24.13.1/lib/node_modules/lighthouse/node_modules/@types tools/pi-probe/preflight.mjs` | Exit 0 with existing TypeScript 6.0.2 and Node types 26.2.0. An earlier type-root lookup failed; the actual check then found undefined HOME and implicit receipt parameter types, fixed without suppression. The SDK is dynamically imported, so its export compatibility is covered by runtime assertions, not this static check. | `typecheck.json` |
| Shell lint | `shellcheck -x tools/pi-probe/run.sh` | Exit 0 with ShellCheck 0.10.0. Initial invocation without `-x` reported the unfollowed sourced helper; rerun follows it without suppressions. | Session command results |
| Live control, busy input, cancellation, lifecycle, ownership, restricted tools | No live driver invoked | **NOT VERIFIED**; each emitted `not_run`. No invented negative capability result. | `preflight-final.log` coverage receipt |
| Scope and whitespace | `git diff --check`; inspect new paths and staged diff | Initial whitespace check passed; final staged/candidate checks belong to delivery receipts. | Session command results |

The full repository gate and independent review are separate delivery
artifacts, not passes implied by this ledger. Final `corral done` summary
must name their actual results and retain these missing-evidence boundaries.
No canonical criterion IDs were registered by this spike, and no progress
entry is claimed as acceptance evidence.
