//! Application services shared by the CLI and the TUI. Every state change goes through these functions,
//! which go through `Store::update` and log an event.

mod acceptance;
mod artifact_gc;
mod artifact_search;
mod artifact_storage;
mod artifacts;
mod autopilot;
mod cleanup;
mod cleanup_worktree;
mod comments;
mod completion;
mod content_index;
mod content_scan;
mod content_search;
mod ctx;
mod dispatch;
mod escalation;
mod gate;
mod gate_script;
mod harness;
mod history;
mod hook_command;
mod hooks;
mod ideas;
mod importance;
mod integration;
mod launch;
mod lesson_distill;
mod lesson_signals;
mod lessons;
mod movement;
mod obligations;
mod obligations_sync;
mod omo;
mod pane_liveness;
mod plan;
mod plan_review;
mod pm;
pub mod pm_bench;
mod pm_events;
mod pm_next;
mod pm_questions;
mod progress;
mod prompt;
mod provider_session;
mod provider_stall;
mod queue;
mod resource_escalation;
pub mod resources;
mod resume;
mod review;
mod review_budget;
mod reviewer;
mod routing;
mod search;
mod session_scope;
mod skill;
mod slots;
mod sprint;
mod stats;
mod sync;
mod tasks;
mod usage;
mod verdict_carry;
mod verification;
pub mod viewers;
pub mod watch;
mod watchdog;

#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;

use corral_core::messages;

pub use artifact_gc::{
    GcAction, GcReport, StorageStats, TaskStorageStats, artifact_gc, storage_stats,
};
pub use artifact_search::ArtifactSearch;
pub use artifact_storage::{
    artifact_path, artifact_read, artifact_reader, resolve_artifact, set_artifact_tier,
};
pub use artifacts::{
    ArtifactRow, Preview, add_artifact, all_artifacts, artifact_document, artifact_preview,
    scan_artifacts,
};
pub use comments::comment;
pub use completion::{
    completion_check, completion_check_task, completion_spec_digest, evidence_failures,
    evidence_list, record_evidence, register_criteria,
};
pub use content_index::refresh_search_index;
pub use content_search::{ContentHit, ContentSearch, search_content};
pub use ctx::Ctx;
pub use dispatch::{
    DispatchOpts, DispatchOutcome, dispatch, jump, live_tail, reply, request_changes,
    request_changes_escalated, resend_reply, retry, retry_run,
};
pub use gate::{
    GatePaths, gate_collect, gate_paths, gate_start, gate_start_with_force, gate_wait,
    worktree_head,
};
pub use harness::{
    CLAUDE_MODEL_ALIASES, HARNESS_CACHE_FILE, HarnessInfo, HarnessReport, ModelSource,
    cached_harness_report, detect_harnesses, harness_report,
};
pub use history::{Finish, Landed, finished_tasks, finishes};
pub use hooks::{
    HookAttempt, HookReceipt, HookState, PostLandOperation, operational_blocker,
    post_land_operations, reconcile_post_land, retry_post_land,
};
pub use ideas::decide_idea;
pub use importance::{
    cmp_importance, excerpt, lead_artifact, order_artifacts, reviewed_set, toggle_reviewed,
};
pub use lesson_distill::{
    LessonDue, LessonProposal, LessonRequest, lessons_apply, lessons_distill, lessons_due,
};
pub use lesson_signals::{LessonSignal, lesson_signals};
pub use lessons::{applied_lessons, lessons_for};
pub use messages::{Message, MessageGuard, capture_messages, report as report_message};
pub use obligations::{ResumeTarget, register_decision, register_wait, resume_run};
pub use plan::{approved_plan, plan_policy_set, plan_spec_revision, plan_submit};
pub use plan_review::{PlanVerdict, plan_review, plan_verdict};
pub use pm::open_pm;
pub use pm_next::{PmAction, PmActionKind, PmObservation, PmOption, pm_card, pm_next, pm_select};
pub use progress::{progress_readback, progress_spec_revision, save_checkpoint, update_progress};
pub use prompt::{PREVIEW_TASK, preview_command, render_command, task_prompt};
pub use queue::{QueueEntry, admit_ready, dequeue, dispatch_pause, dispatch_queue, enqueue};
pub use resume::{ResumePacket, ResumeStatus, resume_packet};
pub use review::{
    ApproveStrategy, ReviewInfo, approve, discard, discard_with_options, done, review_info,
};
pub use reviewer::{
    reconcile_review_at, review_notes_path, review_prompt, review_run, verdict, verdict_bound,
};
pub use routing::route;
pub use search::{SearchData, search_tasks};
pub use skill::{SKILL_HARNESSES, SKILL_MARKER, VERBS, ensure_skill, skill_markdown, verbs_block};
pub use slots::{SlotUsage, slot_usage};
pub use sprint::{
    SprintChange, SprintDecision, SprintNumbers, SprintReview, SprintSections, SprintTask,
    SprintTotals, collect_sprint_review, review_sprint,
};
pub use stats::{StatsBy, StatsRow, stats};
pub use sync::{Change, sync, sync_at, sync_at_with_processes, sync_hook};
pub use tasks::{infer_project, move_task, new_task, new_task_with_origin, slots};
pub use usage::{refresh_active_usage, refresh_usage};
pub use viewers::{Opened, open_artifact, open_path};
pub use watchdog::{ProcessControl, ProcessSnapshot, SystemProcesses, unstick, unstick_with};
