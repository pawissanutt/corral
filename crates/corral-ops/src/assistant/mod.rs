//! Assistant-session operations that run from the CLI rather than the agent host channel.
pub(crate) mod continuation;
mod focus;
pub mod host;
pub mod owner_action;
pub mod permissions;
mod project;
pub mod scratch;
pub mod show;
pub mod workflow;

pub use focus::{BRIEF_BYTES, task_brief};
pub use project::{Directory, directory, project_at};

#[cfg(all(test, target_os = "linux"))]
mod continuation_tests;
