//! Application services shared by the CLI and the TUI. Every state change goes through these functions,
//! which go through `Store::update` and log an event.

mod acceptance;
mod artifacts;
mod autopilot;
mod ctx;
mod dispatch;
mod escalation;
mod gate;
mod harness;
mod history;
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

pub use artifacts::{
    ArtifactRow, Preview, add_artifact, all_artifacts, artifact_preview, scan_artifacts,
};
pub use ctx::Ctx;
pub use dispatch::{
    DispatchOpts, DispatchOutcome, dispatch, jump, live_tail, reply, request_changes,
    request_changes_escalated, resend_reply, retry,
};
pub use gate::{GatePaths, gate_collect, gate_paths, gate_start, gate_wait, worktree_head};
pub use harness::{
    CLAUDE_MODEL_ALIASES, HARNESS_CACHE_FILE, HarnessInfo, HarnessReport, ModelSource,
    cached_harness_report, detect_harnesses, harness_report,
};
pub use history::{Finish, Landed, finished_tasks, finishes};
pub use pm::open_pm;
pub use prompt::{PREVIEW_TASK, preview_command, render_command, task_prompt};
pub use review::{ApproveStrategy, ReviewInfo, approve, discard, done, review_info};
pub use reviewer::{review_notes_path, review_prompt, review_run, verdict};
pub use routing::route;
pub use skill::{SKILL_HARNESSES, SKILL_MARKER, VERBS, ensure_skill, skill_markdown, verbs_block};
pub use stats::{StatsBy, StatsRow, stats};
pub use sync::{Change, sync, sync_at, sync_at_with_processes};
pub use tasks::{infer_project, move_task, new_task, slots};
pub use usage::refresh_usage;
pub use viewers::{Opened, open_artifact, open_path};
pub use watchdog::{ProcessControl, ProcessSnapshot, SystemProcesses, unstick, unstick_with};
