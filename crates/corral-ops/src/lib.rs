//! Application services shared by the CLI and the TUI. Every state change goes through these functions,
//! which go through `Store::update` and log an event.

mod acceptance;
mod artifacts;
mod autopilot;
mod cleanup;
mod ctx;
mod dispatch;
mod escalation;
mod gate;
mod harness;
mod history;
mod obligations;
mod obligations_sync;
mod omo;
mod pm;
mod pm_events;
mod prompt;
mod review;
mod reviewer;
mod routing;
mod skill;
mod stats;
mod sync;
mod tasks;
mod usage;
pub mod viewers;
mod watchdog;

use corral_core::messages;

pub use artifacts::{
    add_artifact, all_artifacts, artifact_preview, scan_artifacts, ArtifactRow, Preview,
};
pub use ctx::Ctx;
pub use dispatch::{
    dispatch, jump, live_tail, reply, request_changes, request_changes_escalated, resend_reply,
    retry, DispatchOpts, DispatchOutcome,
};
pub use gate::{gate_collect, gate_paths, gate_start, gate_wait, worktree_head, GatePaths};
pub use harness::{
    cached_harness_report, detect_harnesses, harness_report, HarnessInfo, HarnessReport,
    ModelSource, CLAUDE_MODEL_ALIASES, HARNESS_CACHE_FILE,
};
pub use history::{finished_tasks, finishes, Finish, Landed};
pub use messages::{capture_messages, report as report_message, Message, MessageGuard};
pub use obligations::{register_decision, register_wait, resume_run, ResumeTarget};
pub use pm::open_pm;
pub use prompt::{preview_command, render_command, task_prompt, PREVIEW_TASK};
pub use review::{approve, discard, done, review_info, ApproveStrategy, ReviewInfo};
pub use reviewer::{review_notes_path, review_prompt, review_run, verdict};
pub use routing::route;
pub use skill::{ensure_skill, skill_markdown, verbs_block, SKILL_HARNESSES, SKILL_MARKER, VERBS};
pub use stats::{stats, StatsBy, StatsRow};
pub use sync::{sync, sync_at, sync_at_with_processes, Change};
pub use tasks::{infer_project, move_task, new_task, slots};
pub use usage::refresh_usage;
pub use viewers::{open_artifact, open_path, Opened};
pub use watchdog::{unstick, unstick_with, ProcessControl, ProcessSnapshot, SystemProcesses};
