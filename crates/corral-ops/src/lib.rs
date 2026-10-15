//! Application services shared by the CLI and the TUI. Every state change goes through these functions,
//! which go through `Store::update` and log an event.

mod acceptance;
mod artifacts;
mod autopilot;
mod ctx;
mod dispatch;
mod gate;
mod harness;
mod history;
mod omo;
mod pm;
mod prompt;
mod review;
mod reviewer;
mod skill;
mod stats;
mod sync;
mod tasks;
mod usage;
pub mod viewers;

pub use artifacts::{
    ArtifactRow, Preview, add_artifact, all_artifacts, artifact_preview, scan_artifacts,
};
pub use ctx::Ctx;
pub use dispatch::{
    DispatchOpts, DispatchOutcome, dispatch, jump, live_tail, reply, request_changes, retry,
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
pub use skill::{SKILL_HARNESSES, SKILL_MARKER, VERBS, ensure_skill, skill_markdown, verbs_block};
pub use stats::{StatsBy, StatsRow, stats};
pub use sync::{Change, sync, sync_at};
pub use tasks::{infer_project, move_task, new_task, slots};
pub use viewers::{Opened, open_artifact, open_path};
