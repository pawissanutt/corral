# Sprint review and owner ideas

Build a deterministic review from recorded task events, run summaries, artifacts,
usage, and lessons:

```sh
corral review-sprint
corral review-sprint last --json
corral review-sprint --since 2026-10-16T00:00:00Z
corral review-sprint --mark
```

The report contains landed outcomes, sent-back or failed work, dropped/rejected/
escalated items, in-flight work, owner ideas, decisions needed, and numbers.
It is stored as a Markdown report artifact and pinned for the existing artifact
viewer. The JSON form includes structured data and the artifact reference.
No model call or interactive agent session is required.

Without `--since`, or with `last`, history starts strictly after the last review
mark. `--mark` builds and pins the report before recording its covered-through
point. Ordinary report generation does not move that point. Current owner ideas
and in-flight work remain visible even when no new outcomes have occurred.
Scheduled triggers and interactive walkthroughs are not part of this version.

## Protecting owner intent

Human CLI submissions default to `origin: owner`, including non-Idea tasks.
PMs and agents should identify their own submissions explicitly:

```sh
corral new "Consider caching" --idea
corral new "Split the parser implementation" --origin pm
corral new "Investigate parser allocation" --origin agent
```

Legacy Idea records without an origin are inferred as owner-filed, and keep that
origin when scheduled. Other historical records are not reclassified.

Dropping, declining, deferring, or replacing an owner specification requires
an explicit owner decision. `--force` does not bypass it. Because code cannot
reliably distinguish narrowing from expansion, any replacement of an owner
task's title or Markdown specification requires approval. The normal store
write boundary enforces this across CLI and board operations; manual edits to
the underlying files remain outside that boundary.

```sh
corral move T-001 dropped --owner-ok --reason "Use the replacement"
corral discard T-001 --owner-ok --reason "No longer wanted"
corral idea T-001 decline --reason "No longer wanted" --owner-ok
corral idea T-001 defer --reason "Revisit next sprint" --owner-ok
corral idea T-001 narrow --body-file revised-spec.md --reason "Keep the first use case" --owner-ok
corral idea T-001 merge --into T-002 --reason "Same outcome" --owner-ok
corral edit T-001 --owner-ok
```

Merges retain the original task, its destination, a source link on the
destination task, and an event for the next review. Reopening an idea does not
require permission to reduce it.

The owner may instead leave an explicit, action-specific approval comment:

```sh
corral comment T-001 "owner-ok: decline Use task two instead"
corral idea T-001 decline --reason "Use task two instead"
```

Only owner-authored comments with the `owner-ok: <action> <reason>` syntax grant
permission. Ordinary comments and PM comments do not. Consent is tied to the
current title, specification, status, and disposition, and is consumed by the
protected change. A changed specification or reopened task needs fresh consent.
The resulting audit records the owner and reason. Owner reductions lead the
dropped section and retain the “needs your OK” callout alongside approval state,
so legacy unapproved reductions cannot disappear among routine agent outcomes.
