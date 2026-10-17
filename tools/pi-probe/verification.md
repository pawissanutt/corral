# T-136 verification coverage

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
