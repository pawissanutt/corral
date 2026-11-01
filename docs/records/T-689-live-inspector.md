# T-689 — live Pi inspector

## Delivered

The native Context inspector shows active SDK tool names, loaded guidance/resources, generation capability preset/catalog/digest, project-text trust, effective restricted SDK settings and available budget accounting. The composer/footer retain ownership of measured context and current model. Agents inspection adds actual session/bound-session IDs, turn state, retained generation events and observed Corral tool outcomes. Background activity uses the same outcome adapter. No tool result schemas, pagination, shared renderers or authority controls changed.

Plan inspection retains the canonical role-specific ledger, phases, steps and criteria, now including waits. Initial/unavailable ledgers are not presented as empty confirmed plans. An older canonical read keeps the newer receipt-backed revision and marks it stale. Presentation controls only disclose, scroll or select; they cannot mark progress, evidence or gates accepted.

## Acceptance evidence

- **Identity/model/generation/turn and progress:** real SDK `HostSession` tests bind through the isolated fake Corral executable and render wide/inline inspectors before and after hello. Existing host tests drive actual turn events using the fixture transport. Role-specific ledger adapters cover worker, PM and assistant; reviewer/planning roles without a todo capability explicitly show unavailable rather than invented progress.
- **Effective context/provenance/accounting:** guidance-session tests load actual temporary AGENTS files, skills and templates through the SDK loader for worker, reviewer and PM and assert their inspector output. Context counts remain SDK measurements; legacy budget/capability provenance and unavailable values are explicit. Raw capability settings/MCP credentials are not dumped. Per-key settings source history is unavailable and labelled so.
- **No local acceptance authority:** receipt adapters reuse `renderResultText` unchanged. Tests cover committed, replayed, rejected, uncertain, queued and missing/error outcomes, ignore unauthenticated nested/non-Corral results, and demonstrate that rejected/uncertain/agent-asserted progress does not replace canonical progress. A commit is explicitly not gate PASS or criterion acceptance.
- **Stale/disconnected/gaps and rendering:** tests cover older ledger reads, late replay, disconnected/legacy snapshots, bounded receipt retention and event-ring gaps. `.evidence/T-689-host-{bound,disconnected}-render.txt` and the three role guidance render files contain actual native render output. `T-689-adapter-plan-render.txt` additionally renders a canonical-ledger adapter result with its criterion and stale/disconnected state. `T-689-rendered.html`/PNG present the captured bound/worker output, including successive inline scroll positions; they are not mock UI state or evidence of an interactive PTY session.
- **Shared semantics:** `src/ui/render.ts`, Corral tool schemas and corral_show pagination are unchanged.

## Checks

- `bun run --cwd packages/corral-pi typecheck`: passed.
- `bun test --cwd packages/corral-pi src/ui/inspector.test.ts src/ui/panels.test.ts src/ui/todo.test.ts src/ui/workbench.test.ts test/host-session.test.ts test/guidance-session.test.ts`: 59 passed, 770 assertions.
- `bun test --cwd packages/corral-pi test/request-host.test.ts test/request.test.ts`: 46 passed, 181 assertions.
- Screenshot: local browser capture of escaped actual renderer output, no network requests.
- First assigned quick gate failed only two panel-shell assertions whose fixed row index/pager position assumed the old metadata length. Updated tests locate the actual generation row and current pager coordinates; all 7 panel-shell tests pass (153 assertions). The same navigation/resize invariants remain enforced.
- After rebasing onto current main: typecheck passed; inspector/panels/panel-shell/todo/workbench/host/guidance tests passed, 66 tests and 923 assertions.
- Assigned receipted quick gate follows the corrected commit/rebase; its result is recorded separately by Corral.

## Review round 1 — F1/F2

The original implementation incorrectly treated every nested result as untrusted after T-683 enabled authenticated Code Mode mutations. Both receipt activity and the canonical ledger now accept a child only while its host-owned Code Mode execution scope matches its tool, transcript parent and call ID, and the host has not denied it. Receipt-shaped data or a parent ID alone remains insufficient. No renderer, schema, execution-authority or Code Mode API changes were needed.

A new production HostSession regression runs the actual SDK/Code Mode kernel and Corral tool implementations against the isolated receipt executable. It checks backend-supplied committed/replayed/rejected/uncertain child outcomes in rendered Agents and Background activity, a direct progress r1 followed by nested progress r2, a late replayed todo r1, rejected todo, uncertain todo (r2 retained and explicitly stale), and committed todo without progress that reconciles through a canonical snapshot to r3. Parent-bound envelopes and child settlement journals are asserted. Stale status is now a content row so inline layouts cannot hide it by filtering headings. Unit regressions separately refuse unauthenticated, denied, wrong-parent and wrong-tool child results in both adapters.

- Typecheck passed after rework.
- Broader targeted regression set: 118 tests passed across 11 files (before the final additional Background-activity assertions).
- Final nested-host/adapter set: 5 tests passed, 53 assertions, including the Background activity rendering.
- `.evidence/T-689-nested-host-render.txt` records actual wide/inline Plan and Agents rendering plus Background activity from these real host adapters. All backend outcomes are isolated fixture responses, not production board mutations.
- The original review reproductions are the red evidence: `c6b9e3c4-T-689-review-nested.log` (F1) and `737634ec-T-689-review-ledger.log` (F2).

## Boundaries and limitations

No paid calls, owner panes, uploads, load generation or live herdr resources were used. SDK integration tests use isolated fixture transports/executables, not production mutations. Tool receipt history is bounded to 24 observations in this host; earlier sessions/lifecycle receipts and omitted outcomes are explicit. Host event history is bounded by EventLog; gaps demand canonical reconciliation, not inferred completion. Full gate/landing assurance remains Corral's responsibility. `cxm context` failed with a Corral output-size limit, so exact local source reads were used.
