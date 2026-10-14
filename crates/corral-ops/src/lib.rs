//! Application services shared by the CLI and the TUI. Every state change goes through these functions,
//! which go through `Store::update` and log an event.

mod artifacts;
mod ctx;
mod dispatch;
mod pm;
mod prompt;
mod review;
mod sync;
mod tasks;

pub use artifacts::{
    ArtifactRow, Preview, add_artifact, all_artifacts, artifact_preview, scan_artifacts,
};
pub use ctx::Ctx;
pub use dispatch::{
    DispatchOpts, DispatchOutcome, dispatch, jump, live_tail, reply, request_changes, retry,
};
pub use pm::open_pm;
pub use prompt::{render_command, task_prompt};
pub use review::{ApproveStrategy, ReviewInfo, approve, discard, done, review_info};
pub use sync::{Change, sync};
pub use tasks::{infer_project, move_task, new_task, slots};
