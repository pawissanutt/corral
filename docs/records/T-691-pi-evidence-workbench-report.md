# T-691 — Candidate-bound Changes and Evidence

## Delivered

Pi's existing native workbench now defaults task/run sessions to the **committed candidate** comparison. The canonical reader resolves the integration merge base and candidate HEAD; bounded Git reads use those immutable SHAs for the changed-path list, rename/binary-aware diffstat and selected hunks. Both identities are displayed in full in the main reading area. Selected-file status and diffstat are captured with the retained patch, so refreshes cannot attribute a newer candidate’s metadata to an older displayed hunk. Keyboard and pointer file selection use the same store. Working/index comparisons remain independently available through `c`; changing repository scope does not rebind the host or its evidence.

F10 and `/panels evidence` open an integrated read-only criterion/evidence view. Wide matrices and narrow stacked rows keep implemented, tested, reviewed and accepted separate:

- Implemented means self-reported canonical progress, not independent proof.
- Latest observations remain `worker_assertion`, including their reported pass/fail/not-run status, capture provenance, artifacts and recorded candidate/manifest identities.
- Canonical completion admission checks—not UI SHA comparisons—report missing/stale/failed/not-live/invalid-artifact proof. Authorized evidence carries use the existing completion contract.
- Gate receipt result, tier, freshness and log validation are separate from candidate-wide review verdict/freshness/notes. Historical findings retain run/attempt/HEAD and superseded labels.
- There is no per-criterion acceptance record in the current model. The UI says **not recorded**, never derives acceptance from progress, an assertion, a gate, a review or a task lifecycle label.
- Legacy/no-manifest, unavailable reads, omitted metadata, stale retained rows and worker summaries are explicit.

The new `corral run inspect <task> --run <run>` machine read loads one task, projects only the selected run, and writes no receipts, events, progress or acceptance. It reuses completion and verification functions. It does not build a review packet, hydrate full history into Pi, parse CLI tables or read on render. Input bounds: 100 criteria, 2,000 observations, 500 run artifacts. Output: at most 20 historical findings and 100 artifact metadata rows, excerpted prose and 128 KiB total. Limit errors are unavailable state, not empty success. UI reads coalesce, cache for five seconds, validate the explicit task/run identity and discard invalidated in-flight responses. Git subprocesses retain their existing 256 KiB/3-second bounds.

## Verification and evidence map

| Acceptance | Proof |
|---|---|
| Exact base/candidate, real selected file/diffstat/hunks | `inspection.test.ts` committed-candidate Git fixture covers clean/dirty working trees, later HEAD, renames, binary stats and captured identities. Real-input Changes captures use this task's canonical record and actual Git objects. |
| Canonical criterion/observation/artifact/gate/review matrix; distinct stages | Five Rust adapter tests plus Pi presentation tests. No `accepted` boolean is introduced. |
| Explicit legacy, missing, failed, stale, superseded and assertion states | Rust and Pi tests cover these states, including legacy approval without identity and failed gate. T-691's actual no-manifest state is retained in the real-input Evidence captures. |
| Bounded read adapters, no per-frame hydration | Rust limit/read-only tests; Pi read-count, malformed/wrong-run, cache, invalidation and overlapping-response tests. |
| Changed candidate/manifest/artifact identity; narrow/wide inspection | Rust changes committed HEAD, manifest expectation, snapshot bytes and registered artifact SHA independently. Pi tests render at 52/80/140 columns and exercise keyboard/pointer selection. Real-input renderer captures 80x34 and 140x46. |

Checks run:

- Baseline regression: committed candidate disappears from the former uncommitted-only store when the worktree is clean — **fails as expected**; the same reproducer against the new store **passes**. Both raw logs are attached.
- `cargo test -p corral-ops workbench::tests --lib` — **5 passed**.
- `cargo build -p corral` — **passed**.
- Package `bun run typecheck` — **passed** after frozen dependency installation.
- Package `bun test src/ui/ src/roles/worker/worker.test.ts src/roles/reviewer/reviewer.test.ts` — **92 passed** after the F1 rendering regression. These include bound-role restrictions and the unchanged real host lifecycle with offline providers.
- The assigned post-rebase quick gate has its own canonical receipt/log; that receipt, not this report, determines its result.

## Review round 1 — F1: retained diff metadata

Reproduced the reviewer’s A→B mismatch before the fix: the main pane retained A’s SHA and `+first` hunk while displaying B’s `+3 −1` diffstat. The new rendering regression fails on that exact mismatch in the red log. `PatchSnapshot` now captures a copy of selected-file status and diffstat before the asynchronous diff read; applying the patch installs that metadata together with the retained text and identities. The refreshed sidebar remains independently bound to its current candidate.

The green regression covers both an edit (+1/−1 → +3/−1) and a deletion (Modified → Deleted, +0/−1), at 80 and 140 columns. It checks initial A, retained A after canonical/Git refresh to B, and explicit `r` application of B. All 82 assertions pass. Attached ANSI/screenshot proof uses disposable real Git repositories through the production inspection/rendering seam, explicitly not a live bound host. The original canonical adapter/authority scope is unchanged.

## Capture method and boundaries

`packages/corral-pi/test/support/capture-inspection.ts` reads the real canonical run through the built candidate CLI and real Git objects, then renders production native components in a private in-memory terminal. Captures are explicitly labeled **read-only QA / no live generation**: no host binding, conversation, editor, provider, delivery or owner acceptance is simulated. It exercises keyboard file selection, production pointer hit routing and Evidence navigation. ANSI/text frames, the canonical source JSON and the capture result accompany rendered screenshots. This does not drive any owner/PM/worker pane or start a browser service.

The live task is intentionally not given a manifest or fabricated observations for a screenshot. Rich matrix states are verified with isolated canonical-store tests and presentation fixtures; the real-input captures show the actual legacy/no-manifest state. No paid/provider calls, uploads, host load or extra gates are used.

## Remaining limitations

- Unbound PM/assistant sessions report unavailable task/run evidence; there is no new arbitrary-task picker or session rebinding control.
- Artifact content opening and persistent inline annotation editing are not added. Artifact references, registered hashes, validation failures and review notes are inspectable text.
- Capture verification covers the production rendering/hit-testing seams, not a newly launched live bound host or comprehensive terminal/accessibility certification.
