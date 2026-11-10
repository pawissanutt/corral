# T-928 R1 — evidence-bound recovery and managed shutdown

Approved scope: P1. Implementation base: `c7ef5815be29ad56e8f8662bffbb15d835081b95`.
This report records targeted verification. The contracted quick gate's separately attached
Corral receipt is authoritative for the committed, rebased candidate.

## Delivered

- Separate, versioned executor evidence and owner-terminal-only
  `assistant effects preview/apply/receipt/outcome`. Ordinary `Settle` remains generation-bound.
- Bounded authoritative task association from retained source/session/scratch, current
  driver heads and run locations. A known association requires an explicit matching task;
  focus prose is not authority. Incomplete, conflicting, ambiguous or over-bound inventories
  block application, never imply workspace-only scope. Bounds: 1,024 heads / 4,096 runs.
- Exact previews pin source/current bindings, task contract/driver, repair history,
  complete admission set, evidence versions/digests and retained blockers. Apply holds
  dispatch → normalized task → profile leases through the atomic record unit. Only selected
  proved rows change; immutable per-row outcomes, proof blobs, receipt and audit accompany it.
- MCP/LSP close admission, join local startup/transmission work, retain owned handles and
  verify actual exit/group clearance before settlement. Close/reload/planning share one
  lifecycle; repeated close reuses its outcome rather than repeating shutdown.
- MCP request identities are journaled before protocol dispatch. Validated results,
  matching RPC errors, positive not-started fences and unknown outcomes are distinct.
  Malformed responses, unanswered requests and unsupported async-task acknowledgements
  cannot become HTTP shutdown proof. Available response digests/task IDs remain retained.
- Complete original-host terminal/seal journals recover lost canonical publication. The
  narrow legacy verifier reads bounded regular canonical source files, validates exact
  direct-bash call/job/generation/parent ancestry, exit/group-clear and consumed facts,
  and cross-checks retained job metadata when available. No supplied excerpt is authority.
- Operator guidance and generated help for the new command only. No binary was installed.

## Acceptance evidence

All artifact references below are under `artifacts/T-928/R1/`.

| Invariant | Proof |
| --- | --- |
| Exact legacy subset, immutable receipt/outcome, no execution replay | `76afd84c-preflight-recovery-tests.log` |
| Unknown/mismatched MCP/LSP remain outstanding | Same service tests; `9c6ed5d8-final-pi-tests.log` |
| Real MCP/LSP shutdown precedes settlement | `9c6ed5d8-final-pi-tests.log`; failing-first `39418fa0-lifecycle-red.log` originally observed zero settlements for two live resource admissions |
| Lost acknowledgement/publication uses retained facts, not shutdown retry | Real HostSession fixture plus complete lifecycle-journal service fixture in those passing logs |
| Stale source/task/evidence/CAS and omitted/mismatched association refuse without mutation | Service tests; failing-first `aeb79068-association-red.log` |
| RPC response alone is not terminal work; queued sends remain fenced | Passing low-level MCP variants; failing-first `e1afbf47-mcp-terminal-red.log` reproduced async acknowledgement falsely clearing the resource |
| No replacement handoff proof or authority/budget change | `59abc506-final-boundary-tests.log`, `a43ceb98-final-source-boundary-tests.log`, and unchanged source/task assertions in service tests |
| Reviewed exact preview required before any live incident repair | `docs/guide/assistants.md`, CLI help and opaque unmanaged controlling-terminal confirmation; no live application performed |

Q1 provided the byte-exact retained incident packet/provenance:
`995b923b-T928-A77-j2-61-exact.jsonl` and
`66c04122-T928-A77-j2-61-provenance.json`. The generic small fixture covers its composite
pipe-separated call ID, repeated preparation records, non-root model parent and
`toolName: bash`. The packet's missing earlier ancestry is not substituted for the complete
canonical production source. No standalone retained job metadata was supplied; none was
invented. Q2 selected the conservative bounded association contract implemented above.

## Checks and test cost

- `cargo test -p corral-ops --lib assistant_effect_recovery_`: **3 pass**, 0.13 s execution.
- `cargo test -p corral-ops --lib assistant_boundary_`: **2 pass**, 0.08 s.
- `cargo test -p corral-ops --lib assistant::workflow::handoff::source_boundary::tests`:
  **3 pass**, 0.12 s; missing actual native proof still cannot publish or reassign ownership.
- `cargo build -p corral`: **pass**, no warnings (`dfe61d87-preflight-cli-build.log`).
  Local candidate generated CLI help; unrelated existing reference content was retained.
- `bun run typecheck` in `packages/corral-pi`: **pass**
  (`70685b4d-checkpoint-pi-typecheck.log`, content-deduplicated with the final run).
- Six targeted Bun files: **27 pass**, 5.46 s execution:
  `assistant-effect-lifecycle`, `assistant-planning-policy`, `assistant-planning-host`,
  `assistant-planning-shell`, `mcp`, and `extensions/lsp/lsp.test.ts`.
- `cargo fmt --all -- --check` and `git diff --check`: **pass**.
- Changed-file diagnostics were requested. Reported paths had no errors; several Rust/TS
  requests timed out during indexing. Those are **not** clean diagnostic results; targeted
  Rust compilation/tests and package typecheck are the verified fallback
  (`30f33338-preflight-diagnostics.json` plus retained later timeout logs).

**Five added tests:** three Rust contract tests, one real HostSession lifecycle journey,
and one cheaper MCP manager contract test. Protocol variants stay at manager level.
Added execution is approximately **1.2 seconds**, excluding compilation. No quarantine
entry names these tests. No load, provider, large-history or overlapping new E2E matrix.

## Research and limits

Both original generation-1 read-only helpers' retained final messages were collected
(`64a55b59`, `42d2a30b`, each completed). Their persistence/HTTP/LSP coverage gaps were
checked against record-unit APIs, pinned SDK transport/client source and production paths.
Generation-2 lifecycle research completed within its stated wrapper coverage; recovery
research was partial at its turn limit. Helpers confer no independent review or approval.
The SDK inspection additionally found the async-acknowledgement defect reproduced above.

No live A-77 reconciliation, end/resume, signals or restart; no owner-terminal ceremony,
installed CLI/host compatibility or deployment was exercised. Historical MCP/LSP shutdown
remains unproved and may legitimately keep handoff blocked. Unsupported proof/platform
states fail closed. T-855 startup readback remains independently owned; no candidate was
borrowed or installed. Actual `handoff_drain`/NativeProof and normal independent review
remain mandatory. Original history, scratch, takeover approval and T-924's two spent repairs
plus its separately approved **ONE extra third cycle** are unchanged; recovery grants no
work, checks, diagnostics, credit or consent.
