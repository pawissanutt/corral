---
goal: Implement T-048 with fenced per-scope PM leases, project PMs, a digest-only lead inbox, and scoped T-049 decision cards while preserving owner and task-agent contracts.
non_goals:
  - No implementation in T-098; this document is T-048's proposed implementation input.
  - No automatic project-PM activation, model procurement, new budgets, slot-share scheduler, or multi-project task model.
  - No replacement of sync, autopilot, existing approval policy, herdr session ownership, or worker checkpoints.
  - No operating-system sandbox or exactly-once terminal-message delivery.
decisions:
  - Adopt docs/records/team-of-pms-design.md as the proposed technical contract; review both documents together.
  - Use disjoint lead and project lease keys; lead sessions explicitly hold unmanaged project leases, never wildcard authority.
  - Persist lease generations and launch reservations outside rebuildable state; serialize external effects with scope execution guards.
  - Use boot-qualified monotonic deadlines, explicit acquire/renew/release/takeover, and a foreground agent controller for heartbeats.
  - Enforce actor and lease policy inside ops for every mutating entry point, retaining ordinary owner CLI and exact run/reviewer callbacks.
  - Reuse EventReader and corral wait; share scope predicates across push, waits, decision observations and lessons.
  - Keep legacy owner readbacks unscoped by default; managed lead readbacks use only digests and escalations.
  - Preserve existing profiles, limits, owner-required cards and push-off defaults; enable project PMs only by explicit owner configuration.
affected_paths:
  - crates/corral-core/src/config.rs
  - crates/corral-core/src/lib.rs
  - crates/corral-core/src/pm_lease.rs
  - crates/corral-core/src/pm_handoff.rs
  - crates/corral-core/src/model.rs
  - crates/corral-core/src/event_reader.rs
  - crates/corral-core/tests/pm_config.rs
  - crates/corral-core/tests/pm_lease.rs
  - crates/corral-ops/src/ctx.rs
  - crates/corral-ops/src/lib.rs
  - crates/corral-ops/src/pm.rs
  - crates/corral-ops/src/pm_actor.rs
  - crates/corral-ops/src/pm_controller.rs
  - crates/corral-ops/src/pm_digest.rs
  - crates/corral-ops/src/pm_events.rs
  - crates/corral-ops/src/pm_next.rs
  - crates/corral-ops/src/pm_next/
  - crates/corral-ops/src/dispatch.rs
  - crates/corral-ops/src/dispatch/
  - crates/corral-ops/src/launch.rs
  - crates/corral-ops/src/queue.rs
  - crates/corral-ops/src/tasks.rs
  - crates/corral-ops/src/comments.rs
  - crates/corral-ops/src/plan.rs
  - crates/corral-ops/src/plan_review.rs
  - crates/corral-ops/src/gate.rs
  - crates/corral-ops/src/review.rs
  - crates/corral-ops/src/reviewer.rs
  - crates/corral-ops/src/hooks.rs
  - crates/corral-ops/src/sync.rs
  - crates/corral-ops/src/autopilot.rs
  - crates/corral-ops/src/lesson_distill.rs
  - crates/corral-ops/src/skill.rs
  - crates/corral-ops/tests/
  - crates/corral/src/main.rs
  - crates/corral/src/cmds.rs
  - crates/corral/src/pm_cmd.rs
  - crates/corral/src/wait_cmd.rs
  - crates/corral/tests/
  - crates/corral-tui/src/
  - docs/guide/pm.md
  - docs/guide/events.md
  - docs/guide/configuration.md
  - docs/guide/cli.md
  - docs/session-ownership.md
  - docs/records/team-of-pms-design.md
contracts:
  - Lease record version, store/scope/session/generation identity, credential handling, explicit lifecycle and monotonic expiry.
  - Ops actor classification and authorization matrix, including direct and nested CLI/TUI mutation paths.
  - Saved herdr socket and launch-journal ownership remain mandatory and independent of PM authority.
  - Scope execution locks fence external effects; takeover cannot supersede an in-flight guarded operation.
  - Owner CLI remains usable and leases do not grant owner acceptance; task/run and review callbacks retain exact identity checks.
  - Scoped wait/push filtering and per-scope full EventCursor persistence, with bounded reading and existing timeout/rotation semantics.
  - Versioned digest/escalation and handoff records, deterministic source identities and revision guards.
  - PM launch/status/scope selection and T-049 closed card fields, owner requirements, deterministic ranking and read-only behavior.
  - Legacy single-PM config is valid; unknown old pane ownership never grants a lease.
dependencies:
  - task: T-049
    write_scope: []
    depends_on: []
  - task: T-098
    write_scope:
      - docs/records/team-of-pms-design.md
      - docs/records/team-of-pms-plan.md
    depends_on: []
acceptance:
  - criterion: C1
    scenario: Competing and stale PMs attempt every mutation family; owner and own-run callers execute their existing verbs.
    expected: One writer per disjoint scope; explicit lease operations, heartbeat expiry, fenced restart and owner takeover work; refusals name the holder before side effects; owner CLI and exact worker/reviewer callbacks remain usable.
  - criterion: C2
    scenario: Two enabled project PMs receive interleaved project events, restart, and reload their handoff notes.
    expected: One pane/session per project; wait, push, observations and notes are project-scoped, with independent cursors and no unrelated task evidence.
  - criterion: C3
    scenario: Project tasks land or become blocked while lead handles triage and a cross-project dependency proposal.
    expected: Lead receives only stable coalesced digests, escalations and policy notices, including landed SHAs, unresolved owner questions, configured budget alerts and periodic coverage; linked tasks retain project ownership and existing limits.
  - criterion: C4
    scenario: Open lead and project PMs concurrently, inspect status, crash a controller, then take over and reopen.
    expected: Scope-correct launch/focus, lease reservation and filter wiring; read-only status lists scopes, holders, server/panes and heartbeat ages; legacy config still works and unknown old panes require explicit adoption or relaunch.
  - criterion: C5
    scenario: Run scoped next and brief against T-049's seven policy situations plus mixed-project and read-only-contender fixtures.
    expected: Exactly one evidence-backed closed card per selected scope, unchanged owner authorization boundaries, scoped follow-up commands and lessons, no writes or model calls, and no invented cheap-model performance result.
  - criterion: C6
    scenario: Run deterministic hermetic contention, expiry/takeover, crash/fencing and event-filter tests, then the repository gate in its private herdr environment.
    expected: All tests and scripts/gate.sh pass, no owner-session panes are touched, no fixed sleeps determine tests, and every Rust file remains within 800 SLOC.
verification:
  - cargo test -p corral-core --test pm_lease
  - cargo test -p corral-core --test pm_config
  - cargo test -p corral-ops --test pm_leases
  - cargo test -p corral-ops --test pm_events
  - cargo test -p corral-ops --test pm_next
  - cargo test -p corral-ops --test pm_bench
  - cargo test -p corral --test pm_cli
  - Run additional controller, handoff, digest and authorization-inventory tests with fake herdr, subprocess barriers and temporary stores.
  - In a fresh plugin-free test-herdr.sh subshell, exercise the built CLI for two project PMs, lead status/cards, scope refusal, takeover, event delivery and legacy single-PM migration; attach command logs.
  - In the same isolated environment, export CORRAL_SIZE_LIMITS='{"rs":800}' and run scripts/gate.sh with null stdin; attach the full log.
owner_decisions: []
---

# T-048 implementation plan

Prepared by T-098 on October 16, 2026. This file is a proposed plan, not an
implementation or approval. Its C1-C6 entries map in order to T-048's six
October 16 acceptance criteria. If canonical criterion IDs are registered
before implementation, resubmit with those exact IDs rather than assuming
these local labels replace a manifest.

Read the [design report](team-of-pms-design.md) for the complete lifecycle,
command matrix, lock ordering, filtering, compatibility and risk decisions.
The report is part of this review input; keep its exact reviewed contents
with this plan through dispatch. If either technical contract changes,
submit a replacement plan rather than silently changing the reviewed design.
The report snapshot attached to T-048 is
`artifacts/T-048/R0/44bc0da5-team-of-pms-design.md`, SHA-256
`44bc0da55ca7e3e7d5cb6a89a92f2c56e3160980bebc5971614b55dfedad5c58`.
Review and implementation must verify that digest before consuming the report.

## Implementation sequence

### 1. Lease records and actor policy, before multi-PM launch

Add typed scope, holder, generation and lease transitions in core with injected
boot/monotonic clock inputs. Persist authoritative records and tombstones under
`.corral/pm/leases/`; keep holder credentials permission-restricted and out of
logs/prompts. Defaults: heartbeat 15 seconds, TTL 60 seconds, validated as in
the report. Corrupt/unknown state fails closed.

Implement actor resolution in ops, not just the CLI parser. Recognize saved PM
sessions and server-qualified panes; stale claims never become owner calls.
Preserve unmarked ordinary owner CLI and exact task/review-attempt callbacks.
Strip PM identity from worker/reviewer children. Construct system actors only
inside trusted corral automation; a PM-triggered sync retains the PM's scope.

Inventory all mutating CLI/TUI/service entry points against the report's matrix,
including nested queue/autopilot, plan review, settings/config and post-land
hook retries. Gate PM writes with fresh task-derived scope and existing policy.
Add per-scope execution guards before existing board/task locks, sorted for
multiple projects. Retain the guard through subprocess and receipt completion;
allow heartbeat renewal without it. Acquire/takeover cannot cross an in-flight
effect. Never hold the board lock through IPC.

Exit evidence: concurrent admission, stale credentials, exact expiry,
wall-clock rollback/reboot, release/takeover and no-side-effect refusal tests;
owner, worker and reviewer compatibility tests. Complete C1 before allowing a
second managed PM to launch.

### 2. Scoped observations, events and project ownership

Share a pure scope predicate across wait, push and the T-049 observation layer.
Snapshot project provenance in new events; use bounded legacy task lookup and
never broadcast unknown provenance. Preserve ordinary `wait` behavior, adding
explicit lead selection for managed prompts. Intersect project-PM requests
with their scope. Add project-addressed effective-policy notices without
changing workspace config event matching.

Use one full `EventCursor` per scope and independent bounded scans. Preserve
disabled/unavailable EOF behavior and retry-on-unsubmitted-message behavior.
Initialize new cursors at EOF; migration has a separate captured baseline.
Check generation/server/pane immediately before push; delivery accounting is
not an actionable inbox event.

Filter tasks, lessons, due schedules and evidence before selecting `next`.
Every generated follow-up/wait command carries the correct scope/project.
Keep owner unscoped readbacks compatible and the existing card shape/ranking;
lease diagnostics add no write or implicit renewal.

Exit evidence: interleaved two-project wait/push/card fixtures, independent
cursors, rotation/truncation, moved/deleted tasks, failed sends and no foreign
lessons/follow-up task creation; all T-049 replay scenarios stay green (C2/C5).

### 3. Controller, scope-aware UX and migration

Extend the PM CLI with project selection, status, explicit lease lifecycle
operations and revision-guarded handoff read/write. Keep the current unscoped
open function as a wrapper if callers require it; update reviewer and
plan-review routing to the managing PM for the target project.

Reserve launch authority before any pane creation, activate only against the
matching reservation, then run the agent through its foreground controller.
The controller renews on a timer tied to exact child lifetime; it stops on exit
or identity loss. Persist socket/workspace/pane and generation, handle crash
windows using the existing launch-journal pattern, and never infer authority
from pane existence.

Status is read-only and distinguishes lease expiry from unknown pane liveness.
Contending opens focus the actual live scope holder, never launch duplicates;
foreign sockets refuse. Owner-only takeover records its reason and refuses
while a guarded effect is busy.

Keep old configs valid and project PMs disabled by default. A managed lead
explicitly acquires unmanaged project leases. Migrate the old push cursor once
under lock without replaying task history to the lead. Legacy pane-only state
requires owner-attested launch evidence or explicit relaunch, never guessed
adoption. Enabling a project PM requires a deliberate project-lease transfer.

Exit evidence: fake-controller lifecycle and concurrent-open tests, pure status,
handoff revision rejection, reviewer routing and old-config/legacy migration
tests (C2/C4).

### 4. Lead summaries, escalations and coordination

Add code-generated digest aggregation with durable source cursor, pending
summary and stable IDs. Daily summaries are the proposed default; synchronous
read-only digest queries support timestamp windows. Expose coverage lag,
bounded/truncated history and unknown costs. Use existing configured budgets
only; do not invent alerts from a new spending threshold.

Project escalations carry exact question, project/task, urgency, stable source
identity and evidence references. Corral generates corresponding lead envelopes
for unresolved owner blocks. Deduplicate until an answer/decision receipt
resolves the source. Lead inbox/cards contain summaries/escalations/policy only,
not routine raw task transitions. Cross-project coordination uses linked tasks;
actual project mutations still require explicit project authority.

Exit evidence: coalesced full landed SHAs, unresolved question priority, restart
deduplication, periodic coverage, measured/unknown budget facts and absence of
per-task lead chatter (C3).

### 5. Compatibility documentation and final verification

Update PM/event/configuration/CLI guides and session-ownership cross-references
with implemented commands, actor limitations, lease timing, recovery and
migration. Update CLI snapshots/help checks and generated skill text in their
existing locations. No production Rust file or test file may exceed 800 SLOC;
new modules should keep responsibilities separate without broad refactors.

Run the metadata's targeted tests; new names (`pm_lease`, `pm_leases`) are
implementation targets, not claims that those tests already exist. Use
temporary stores, fake engines and injected clocks; subscribe barriers before
triggering concurrent actions. No sleeps, network models or live owner-server
panes. Perform the built-CLI smoke checks only within the repository's fresh
plugin-free `scripts/test-herdr.sh` environment.

Finish with the isolated full gate, preserving gate identity exports and
`CORRAL_SIZE_LIMITS`. Attach its raw output as a log and an acceptance report
mapping C1-C6 to tests and CLI evidence (C6). Review and owner acceptance remain
separate from this plan; do not dispatch T-048 merely because it was submitted.

## Dependencies, owner decisions and changes in scope

T-049 is landed evidence, not an outstanding dependency to reimplement. T-098
provides this design/plan pair and must finish before T-048 consumes it.
The metadata's write scopes describe that dependency boundary; they do not
authorize editing sibling repositories. The affected paths are an implementation
inventory; exact module splits may follow existing neighboring conventions.

There are no unresolved policy decisions in this proposal (`owner_decisions`
is explicitly empty). Retain the owner's current models, review/dispatch
authorization, project limits and opt-ins. Which project PMs to activate and
whether to buy a different model or introduce new budgets are later owner
configuration choices, not guessed implementation defaults. If review requests
new delegated owner powers, automatic activation or a different scope model,
record those decisions explicitly and resubmit before approval.
