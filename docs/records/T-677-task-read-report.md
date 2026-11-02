# T-677 — bounded PM task reads and compact cards

## Delivered

The existing authenticated `read_show` operation and Pi `corral_show` tool now accept
`section: questions` with an exact canonical `key`, e.g. `T-337/R1/Q16`.
The task is authorized before lookup, and lookup is confined to that task's canonical
question views (including effective state/container closure). The key and revision
remain available both in the canonical content and in page metadata. This grants no
answer, dispatch, acceptance, replay or recovery authority. Unknown keys and keys on
other sections are refused. Answer operations retain their existing revision checks.

The default remains 1,024 characters. Explicit limits up to 4,096 are supported;
the serialized **result** still cannot exceed the existing 8 KiB protocol boundary.
Escaping and multibyte text can shorten a page. `offset`, `end_offset`, `total_chars`,
`truncated`, and `next_offset` describe actual character boundaries, never guessed
limit boundaries. Reassemble `content_json` chunks before parsing JSON. Continue with
the same task, section and key. Reads are observations, not frozen snapshots: if a
question revision changes between pages, restart rather than joining revisions.

`section: automation` provides current run/autopilot stage, exact persisted reason,
SHA, infrastructure hold, park, retry, landing receipt, recent task automation events,
and task/current-run post-land operations. Operation keys, hook states, attempt counts,
latest exit/error/log and readiness/blockers support diagnosis without running hooks.
Hook environments and command arguments are deliberately not exposed. Events are
recent historical evidence, not a claim of current failure; current state is separate.
A bounded stage/refusal diagnostic is visible in the compact result.

Cards identify section, canonical question key and requested offset/limit. Successful
results show actual ranges, completion/continuation and question revision; failures
retain their distinct error mark and actionable reason. Expanded read cards expose
the entire bounded raw result, without the generic 40-line renderer cutoff.

## Compactness and grouping decision

Retain separate one-line calls and one-line collapsed results using existing Pi Text
renderers. Each consecutive read retains its own call identity, expansion and error.
A merged/grouped card would couple independently completing calls and can obscure
one refusal among successes; no session-global grouping state or new UI framework
is justified. The representative screenshot shows a consecutive multi-page pair
alongside exact-question, error and automation cards. This is a rendered fixture,
not a claim of a live Pi-session screenshot; the HTML invokes production renderers.

## Evidence and verification

- `cargo test -p corral-ops --lib agent::ops::pm::tests::`: 10 passed. Includes exact
  question/revision selection among unrelated history, missing key/invalid section,
  automation stage/refusal and pending-operation identity, unchanged task/operation
  records, default limit, excessive limit, escaping/Unicode and multi-page reassembly.
  Existing tests verify project isolation, PM state immutability and receipt behavior.
  These are disposable fake-client fixtures, not owner herdr activity.
- `bun test src/ui/ui.test.ts src/roles/pm/reads.test.ts`: 10 passed, 53 assertions.
  Schema rejects authority inputs and oversize pages; render tests cover question,
  overview, range, continuation, expansion and failure.
- `bun run typecheck`: passed. Changed Rust and renderer LSP diagnostics: zero.
- New UI/schema regressions run against baseline source: each failed as expected;
  temporary baseline files were removed. Attached red and green logs preserve this.
- `git diff --check`: passed. The run's receipted quick gate is attached separately.

The read-only T-337 snapshot at implementation time has 148,941 projected question
history characters (closure metadata omitted conservatively). R1/Q16 revision 2 is
3,153 characters, ending at history character 45,606. Sequential old reads through
that question require 45 calls; even with exact boundaries already known, reading
its contents takes at least four old 1K pages. Exact selection with an explicit 4K
limit takes one page. The estimated wire result for that selection is about 3.4 KiB,
well inside the unchanged result cap. This is a size-based workflow comparison using
real canonical question content, not a live authorized PM invocation or a timing
benchmark. No question text is committed in this report.

## Discovery and limitations

`cxm context` exceeded its configured 128 MiB Corral-output limit. The required
cross-task search for `tool rendering pagination` timed out at 30 seconds; targeted
source inspection located the PM API and renderer directly. T-676's Code Mode
exposure/policy remains separate and untouched. No live-owner PM session, automated
landing/recovery, paid model calls, host load, or full gate was run by this worker.
Automation pages can require continuation; absence of a state/operation is explicit
null/empty rather than synthesized recovery advice. The screenshot's pagination
and blocked-landing data are labeled synthetic; Q16 content comes from the read-only
snapshot. Protocol checks and API state checks are independent of that presentation.
