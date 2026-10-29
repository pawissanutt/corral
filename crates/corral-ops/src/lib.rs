//! Application services shared by the CLI and the TUI. Every state change goes through these functions,
//! which go through `Store::update` and log an event.

mod acceptance;
pub mod agent;
pub mod analytics;
mod artifact_gc;
mod artifact_search;
mod artifact_server;
mod artifact_storage;
mod artifacts;
pub mod attention;
mod automation_controls;
mod automation_status;
mod autopilot;
mod availability;
mod cleanup;
mod cleanup_worktree;
mod comments;
mod completion;
mod content_index;
mod content_refresh;
mod content_scan;
mod content_search;
mod ctx;
pub mod dashboard;
pub mod delivery;
mod desktop_notifications;
mod digest;
mod dispatch;
mod dispatch_policy;
mod escalation;
mod gate;
mod gate_probe;
mod gate_script;
mod gate_tier;
mod harness;
mod history;
mod hook_command;
mod hook_process;
mod hooks;
mod ideas;
mod import;
mod importance;
mod integration;
mod launch;
mod launch_exit;
mod lesson_distill;
mod lesson_signals;
mod lessons;
mod movement;
#[cfg(not(target_os = "linux"))]
mod native_process;
mod obligations;
mod obligations_sync;
mod omo;
mod operating;
mod owner_notice;
mod pane_liveness;
pub mod park;
mod parked;
pub mod phases;
mod plan;
mod plan_acceptance;
mod plan_decompose;
mod plan_dispatch;
mod plan_lifecycle;
mod plan_pilot;
mod plan_review;
mod plan_review_offer;
mod plan_sandbox;
mod plan_target;
mod pm;
pub mod pm_bench;
mod pm_decisions;
mod pm_events;
mod pm_health;
mod pm_next;
mod pm_policy;
mod pm_questions;
mod pm_workset;
mod pricing;
mod profile_selector;
mod progress;
mod promotion;
mod prompt;
mod provider_session;
mod provider_stall;
mod question_reply;
pub mod questions;
mod queue;
mod queue_drain;
mod reply_ack;
mod resource_escalation;
pub mod resources;
mod resume;
mod revert;
mod review;
mod review_budget;
mod reviewer;
mod role_session;
mod round_notice;
pub use role_session::refresh_role_sessions;
mod routing;
mod search;
mod session_scope;
mod setup;
mod setup_discovery;
mod setup_structure;
mod setup_validation;
mod skill;
mod slots;
mod sprint;
mod stats;
mod sync;
mod tasks;
pub mod throughput;
mod thumbnails;
pub mod timeline;
mod tracker;
mod usage;
mod verdict_carry;
mod verification;
pub mod viewers;
pub mod watch;
mod watchdog;

#[cfg(test)]
mod native_runtime_tests;
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;

use corral_core::messages;

pub use analytics::PhaseTotals;
pub use analytics::{ANALYTICS_SCHEMA_VERSION, Analytics, collect_analytics};
pub use artifact_gc::{
    GcAction, GcReport, StorageStats, TaskStorageStats, artifact_gc, storage_stats,
};
pub use artifact_search::ArtifactSearch;
pub use artifact_server::{
    DEFAULT_IDLE, Served, ServerInfo, artifact_server_used, artifact_url_path, ensure_server,
    run_server, running_server, serve_artifact, serve_index, stop_server,
};
pub use artifact_storage::{
    artifact_path, artifact_read, artifact_reader, resolve_artifact, set_artifact_tier,
};
pub use artifacts::{
    ArtifactRow, Preview, add_artifact, all_artifacts, artifact_document, artifact_preview,
    scan_artifacts,
};
pub use automation_controls::{automation_confirm, automation_preview, require_automation_owner};
pub use automation_status::{automation_blocker, automation_summary};
pub use comments::comment;
pub use completion::{
    completion_check, completion_check_task, completion_spec_digest, evidence_failures,
    evidence_list, record_evidence, register_criteria,
};
pub use content_refresh::{refresh_search_index, refresh_search_index_with_tasks};
pub use content_search::{ContentHit, ContentSearch, search_content};
pub use ctx::Ctx;
pub use digest::{
    Digest, DigestEntry, DigestReport, collect_digest, digest, digest_at, scheduled_digest_at,
};
pub use dispatch::{
    DispatchOpts, DispatchOutcome, dispatch, jump, live_tail, reply, request_changes,
    request_changes_escalated, resend_reply, retry, retry_run,
};
pub use dispatch_policy::{AutomaticDispatchStatus, automatic_dispatch_status};
pub use gate::{
    GatePaths, gate_collect, gate_paths, gate_start, gate_start_with_force, gate_wait,
    worktree_head,
};
#[doc(hidden)]
pub use gate_probe::runtime_probe;
pub use gate_tier::{
    DestinationKind, GateContract, GateDestination, GateInstructionConflict, GatePhase,
    GateReadback, GateRef, GateRequirement, gate_readback, scan_gate_instructions,
};
pub use harness::{
    CLAUDE_MODEL_ALIASES, HARNESS_CACHE_FILE, HarnessInfo, HarnessReport, ModelSource,
    cached_harness_report, detect_harnesses, harness_report, models_for_profile,
};
pub use history::{Finish, Landed, finished_tasks, finishes};
pub use hooks::{
    HookAttempt, HookReceipt, HookState, PostLandOperation, operational_blocker,
    post_land_operations, reconcile_post_land, retry_post_land,
};
pub use ideas::{IdeaDecisionOptions, decide_idea};
pub use import::{ImportResult, amend_plan_scope, import_plan};
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
pub use phases::{CommandSpan, Phase, Span, task_phases};
#[doc(hidden)]
pub use plan::member_add::{PlanMemberAdded, plan_add_member, plan_add_member_from};
pub use plan::member_gate::plan_member_gate_from;
pub use plan::member_gate::{MemberGateOutcome, MemberGateRequest, OwnerEntry, plan_member_gate};
#[doc(hidden)]
pub use plan::member_move::{MoveGate, PlanMemberMoved, plan_move_member, plan_move_member_from};
pub use plan::{approved_plan, plan_policy_set, plan_spec_revision, plan_submit};
pub use plan_lifecycle::plan_cancel;
pub use plan_review::{PlanVerdict, plan_review, plan_verdict, plan_verdict_report};
pub use plan_review_offer::{
    PlanReviewOffer, plan_review_offer, plan_review_retryable, plan_review_wait,
};
pub use pm::{SetupConnection, open_pm, open_setup_pm, setup_connection};
pub use pm_decisions::{pm_present, pm_resolve};
pub use pm_health::pm_health;
pub use pm_next::{PmAction, PmActionKind, PmObservation, PmOption, pm_card, pm_next, pm_select};
pub use pm_policy::{pm_policy_health, reconcile_pm_policy};
#[doc(hidden)]
pub use pm_questions::set_transcript_settle_window;
pub use pm_workset::{
    OperatingStatus, PmActionOutcome, PmActionResult, PmDecision, PmExecutable, PmWorkset,
    ScopedProgress, pm_act, pm_workset, pm_workset_with_policy,
};
pub use pricing::{
    CacheRateFallback, PriceShadow, PricingCheck, UnpricedModel, pricing_check, pricing_coverage,
};
pub use profile_selector::{SelectionCatalog, SelectionProfile, selection_catalog, selection_save};
pub use progress::{progress_readback, progress_spec_revision, save_checkpoint, update_progress};
pub use promotion::{PromotionCandidate, promotion_shortlist};
pub use prompt::{PREVIEW_TASK, preview_command, render_command, task_prompt};
pub use queue::{QueueEntry, admit_ready, dequeue, dispatch_pause, dispatch_queue, enqueue};
pub use reply_ack::mark_reply_delivered;
pub use resume::{ResumePacket, ResumeStatus, resume_packet};
pub use revert::{RevertResult, revert};
pub use review::{
    ApproveStrategy, ReviewInfo, approve, discard, discard_with_decision, discard_with_options,
    done, done_with_result, review_info,
};
pub use reviewer::{
    reconcile_review_at, review_notes_path, review_prompt, review_run, verdict, verdict_at,
    verdict_bound,
};
pub use routing::{implementer_set, recorded_route, reviewer_set, route_snapshot};
pub use setup::{
    SETUP_TEMPLATE, SetupCatalog, SetupProposalInput, SetupStatus, setup_answers, setup_apply,
    setup_approve, setup_begin, setup_cancel, setup_catalog, setup_diff, setup_propose,
    setup_propose_at, setup_propose_draft, setup_status, setup_validate, setup_validate_draft,
};
pub use setup_discovery::{
    CandidateObservation, CandidateSettings, DiscoveredHarness, SetupDiscovery, setup_discover,
};
pub use setup_structure::{SetupInputDraft, parse_setup_input};
pub use setup_validation::{SetupValidationError, SetupValidationErrors};
mod review_routing;
pub use plan_acceptance::{
    AggregateProgress, AggregateVerdict, CheckpointProgress, PlanCheckpoint, plan_accept,
    plan_aggregate_review, plan_aggregate_verdict, plan_checkpoint, plan_checkpoint_automatic,
    plan_evidence, plan_verify,
};
pub use review_routing::ReviewMapping;
pub mod review_contracts;
pub mod review_packet;
pub mod review_receipts;
pub use parked::ParkedRun;
pub use reviewer::{StrongerOutcome, request_stronger_review, review_callback_token};
pub use routing::{
    PlannerMapping, PlannerSource, ReviewExplanation, RouteExplanation, route, route_with_plan,
};
pub use search::{SearchData, search_tasks};
pub use skill::{
    MODEL_SELECTION_PROCEDURE, MODEL_SELECTION_VERSION, PLANNER_SKILL, PM_SKILL, ROLE_SKILLS,
    SKILL_HARNESSES, SKILL_MARKER, SKILL_SHARED, SUBMIT_SKILL, SkillAction, SkillWrite, VERBS,
    WORKER_SKILL, duplicate_skills, ensure_skills, install_harness_skills, install_role_skills,
    role_skill_markdown, skill_markdown, skills_dir_for, verbs_block,
};
pub use slots::{SlotUsage, slot_usage, soft_target};
pub use sprint::{
    SprintChange, SprintDecision, SprintNumbers, SprintReview, SprintSections, SprintTask,
    SprintTotals, collect_sprint_review, review_sprint,
};
pub use stats::{StatsBy, StatsRow, stats};
pub use sync::{Change, sync, sync_at, sync_at_with_processes, sync_hook, sync_with_attention};
pub use tasks::{
    infer_project, move_task, move_task_with_decision, new_task, new_task_initialized,
    new_task_with_origin, slots,
};
pub use thumbnails::{artifact_thumbnail, capture_html_thumbnails, is_visual_artifact};
pub use timeline::{EntryClass, TimelineEntry, TimelineFilter, Wait, timeline};
pub use tracker::{
    PlanCoverage, PlanPrerequisite, PlanProgress, PlanWork, PlanWorkCounts, PlanWorkState,
    plan_progress,
};
pub use usage::{refresh_active_usage, refresh_usage};
pub use viewers::{Opened, open_artifact, open_artifact_live, open_path};
pub use watchdog::{ProcessControl, ProcessSnapshot, SystemProcesses, unstick, unstick_with};
