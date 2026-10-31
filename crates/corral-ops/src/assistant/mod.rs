//! Assistant-session operations that run from the CLI rather than the agent host channel.
mod focus;
pub mod owner_action;
pub mod scratch;

pub use focus::{BRIEF_BYTES, task_brief};
