# T-053: authorized dispatch queue and shared capacity

Implemented on October 15, 2026. Includes the owner's additional requirement that gates and reviewers
consume global and project slots, with finishing work before new coding dispatches.

## Contracts

- `queue add` authorizes without launching; `queue admit PROJECT` opts only that project's Ready tasks
  into the existing sync drain. `queue remove` prevents automatic readmission. `queue pause/resume`
  is independent of review autopilot. Idea/Backlog require explicit authorization.
- P0 through P3, creation timestamp, then ID determine coding order. Done dependencies qualify;
  Review does not. Cycles and missing dependencies produce visible explanations.
- Queue-time routing uses the existing precedence and exact-model checks, with the selected profile,
  model, effort, mode, base override and route provenance persisted alongside the retry choice.
  Restart does not reroute the queue. Unknown discovery is explicitly warned; a known missing
  pinned model never falls back.
- All board/config writes and coding/review admissions share a workspace transaction lock.
  Kernel-owned launch locks exclude same-task races. Explicit unlock avoids inherited-descriptor
  timing races during concurrent subprocess creation.
- A persisted launch phase reserves capacity before worktree/pane creation. Recovery reuses the
  run, branch, worktree and uniquely labelled workspace. Failed preparation releases its reservation.
- Running and Needs me tasks each consume one slot. Each running gate and active reviewer consumes
  another by default; reservations bridge the external-launch window without double counting.
  `limits.count_reviews` and `limits.count_gates` default true. `limits.review` is an additional,
  independent combined review-process bound, not an exemption from global/project limits.
- Autopilot capacity waits remain in Review with a visible reason. Eligible finishing work has
  priority even when a concurrent manual dispatch checks capacity. Saturated projects do not
  block eligible work in other projects. Header and autopilot status expose the component counts.

## Acceptance evidence

The integration entry point is `cargo test -p corral --test dispatch_queue`.
Fixtures execute the actual CLI, store, Git worktrees and herdr protocol adapter.
Competing service calls synchronize on barriers; crash tests kill the actual CLI process at the
external boundary rather than assuming that an error return simulates a crash.

| Requirement | Evidence |
|---|---|
| Explicit authorization and project scope | `admission_is_project_scoped_ready_only_and_dequeue_survives_sync`, `manual_default_never_admits_ready_work` |
| Dependency approval, not merely Review | `review_dependency_waits_but_approval_refills_the_slot` |
| Enqueue-only, independent pause, dequeue | `enqueue_is_side_effect_free_and_pause_is_independent_of_review`, `dequeue_of_failed_launch_closes_its_pane_and_releases_authorization` |
| Shared manual dispatch capacity | `request_changes_uses_the_same_capacity_as_queued_dispatch`, existing retry/escalation tests |
| Competing sync/manual and same-task exclusion | `competing_sync_and_manual_dispatch_launch_exactly_one_with_one_slot`, `same_task_barrier_race_cannot_create_two_runs_or_panes` |
| Failure/crash recovery | `crashed_external_boundaries_recover_without_duplicate_run_or_pane`, `failed_launch_releases_capacity_and_resumes_the_same_reserved_run`, `reservation_precedes_worktree_and_recovers_after_git_process_crash` |
| Pinned overrides and routing | `explicit_route_and_base_survive_reload_without_silent_model_replacement`, `ui_routes_to_opus_and_backend_to_gpt_at_admission_and_stays_pinned` |
| No silent missing-model substitution | `known_missing_explicit_model_is_rejected_before_queue_or_git`, `disappearing_pinned_model_is_visible_and_never_falls_back_at_claim` |
| Deterministic priority, Needs me, caps and invalid graphs | `priority_then_creation_then_id_order_is_deterministic`, `needs_me_and_reservations_count_but_project_saturation_does_not_starve_others`, `malformed_dependencies_are_visible_and_do_not_stop_other_projects` |
| Gates/reviewers share global and project slots | `full_global_capacity_blocks_manual_gate_and_reviewer_before_pane_creation`, `project_cap_includes_gate_and_does_not_block_other_project`, `gate_and_dispatch_barrier_contenders_share_one_atomic_reservation` |
| Finishing work first and release | `finished_reviewer_frees_slot_for_waiting_gate_before_ready_dispatch`, `reviewer_pane_disappearance_releases_shared_capacity` |
| Explicit counting switches and bounded review budget | `counting_switches_are_opt_out_and_review_budget_remains_bounded`; 12 `review_budget::tests` |
| Deterministic lock release | `dropping_owner_unlocks_even_when_child_keeps_an_inherited_descriptor` |
| Isolated generic fixture | `isolated_corral_test_fills_and_refills_authorized_slots` uses real `corral-test` panes, fills two slots, waits through Review, refills after approval, and checks three claim/release/dispatch receipts |
| Board display | `shared_slot_header_renders_at_wide_and_narrow_terminal_widths` renders actual ratatui widgets at 160x45 and 80x30 without reading terminal input; screenshots come from those rendered cells |

The evidence bundle under `.omo/evidence/T-053/` contains test logs, the live fixture's event records,
board renders, and full-gate logs. The required gate is:

```sh
cargo fmt --all -- --check &&
cargo clippy --workspace --all-targets -- -D warnings &&
cargo test --workspace
```

All gate commands receive null stdin; herdr checks target `corral-test`. Documentation is checked
with `scripts/check-docs.sh target/debug/corral`. File-size evidence counts all nonblank lines
except standalone `//` comments, a conservative upper bound that also counts block comments.

## Explicit recovery boundary

Arbitrary shell submission cannot be made exactly-once by retrying an IPC call. If corral dies
after recording submission intent, it never sends the same command again. The attached run/pane
is retained in Needs me for inspection, with no leaked temporary reservation. The crash tests
distinguish the zero-submission boundary before the call from the one-submission boundary after it.
They do not misrepresent an uncertain command as automatically retried or prove that arbitrary
agent commands are idempotent. No production agent/model calls are required by the generic fixture.
