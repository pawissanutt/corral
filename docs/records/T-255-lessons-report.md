# T-255: corral lessons synthesis

Applied an evidence-bound collection of 11 lessons: all nine existing lessons
unchanged and two narrowly scoped additions at confidence 0.40. No project code
was edited and no other agents were dispatched.

## Question and method

Distill the captured corral history into useful project-local guidance without
inventing evidence, inflating recurrence, refreshing old timestamps, or
overwriting owner edits. Read the complete request at
`.corral/state/lessons/T-255.json`, including nine existing lessons and all 256
deterministic signal clusters. Search prior artifacts with
`corral search "lessons" --in all --json` before further investigation.

The clusters comprise 3 conflict, 181 reviewer-note and 72 size-ceiling matches.
Every signal references T-093. Repeated review lines, transcript-envelope
fragments and file unions are not independent incidents or proof that the
named file exceeded its ceiling. Read the prior T-225 synthesis and the T-093
R2/R3 source reviews supporting the additions.

## Findings and decisions

- Preserve all nine existing IDs, rules, explanations, evidence, scopes,
  confidence values, flags and `last_seen` timestamps exactly. Retain the
  existing T-177 evidence even though the new signal collection only contains
  T-093. No recurring new incident justifies refreshing those lessons.
- Add `legacy-session-ownership-needs-immutable-provenance`: T-093 R2 reproduced
  foreign reconciliation after a configuration edit and legitimate legacy
  reconciliation blocked by a guessed default. R3 recognized immutable
  operator-attested migration as the chosen design. Evidence:
  `artifacts/T-093/R2/1f6b1acc-review-notes.md` and
  `artifacts/T-093/R3/60393c0c-review-notes.md`.
- Add `herdr-live-checks-use-private-plugin-free-server`: R3 independently
  verified helper-based startup, absent plugins, an actual live success rather
  than a skip, and owned cleanup. Evidence:
  `artifacts/T-093/R3/60393c0c-review-notes.md`.
- Both additions are single-task observations at confidence 0.40, restricted to
  their relevant files and backend, logic or test kinds. Profile and harness
  observations do not establish model-specific rules. New `last_seen` values
  use the exact source timestamp, `2026-10-15T23:12:34.995281797Z`.
- No captured evidence contradicts an existing rule; all `contradicted` flags
  remain false. The request and current document contain no pinned lessons.
  Moving a concurrency fix to T-094 changes task responsibility, not the
  underlying ownership invariant.

The PM comment identifies newer review/gate invalidation, slot deadlock,
question timeout, idle reviewer, repeated review variant, missing-gate and
gate-count pitfalls. The captured signals do not substantiate those incidents
or the stated counts. Do not promote them into lessons from this request.

## Validation and application

Proposal: `docs/records/T-255-lessons-proposal.json`.

Request digest copied exactly:
`ab6921d26de7f4d49402162dc3d204571b8d7ac386b635d8be68b1e7ab8a8831`.

Validated request identity, unique IDs, the 100-lesson limit, one-line rules,
all scope dimensions, bounded confidence, exact captured evidence triples,
evidence-backed timestamps and unchanged existing lessons. The current owner
collection matched the captured request before application. The guarded
`corral lessons apply --project corral --file docs/records/T-255-lessons-proposal.json`
command, with the workspace root explicitly selected, exited 0 without a stale
owner rejection. An authoritative `corral lessons --project corral --json`
readback matched all 11 proposed lessons structurally.

Recommend retaining these conservative rules and obtaining captured evidence
for the newer PM observations in a future distillation. Validation evidence and
the required pre/post-rebase isolated gate logs are attached separately to
T-255. Gate results verify this candidate, not historical defect resolution.
Historical reproducers and causal effects of lesson exposure were not rerun.
