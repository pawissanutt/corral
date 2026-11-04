//! Assistant-session operations that run from the CLI rather than the agent host channel.
mod focus;
pub mod host;
pub mod owner_action;
pub mod permissions;
mod project;
pub mod scratch;
pub mod workflow;

pub use focus::{BRIEF_BYTES, task_brief};
pub use project::{Directory, directory, project_at};
