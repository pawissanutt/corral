//! Session backends: herdr panes, detached shell processes, and an in-memory fake.
//!
//! A process session owns its run directory. Its `exit_code` is the durable completion
//! marker, including when the command outlives the caller that launched it.

use std::path::PathBuf;

use anyhow::Result;
use corral_types::Origin;
use serde::{Deserialize, Serialize};

mod fake;
mod herdr;
pub mod pi;
mod process;

pub use corral_types::{SessionRef, SessionStatus};
pub use fake::{FakeRunner, SendRecord};
pub use herdr::HerdrRunner;
pub use process::ProcessRunner;
pub(crate) use process::detach;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub label: String,
    pub cwd: PathBuf,
    pub command: String,
    pub env: Vec<(String, String)>,
    pub dir: PathBuf,
    pub tokens: Vec<(String, String)>,
}

pub trait Runner: Send + Sync {
    fn start(&self, spec: &LaunchSpec) -> Result<SessionRef>;
    fn status(&self, session: &SessionRef) -> Result<SessionStatus>;
    /// Owner replies and automatic input share the transport. Runners that can
    /// pause a session may defer automatic input.
    fn send(&self, session: &SessionRef, text: &str, origin: Origin) -> Result<()>;
    fn stop(&self, session: &SessionRef) -> Result<()>;
    fn focus(&self, session: &SessionRef) -> Result<()>;
}

#[cfg(all(test, unix))]
#[path = "runner/herdr_tests.rs"]
mod herdr_tests;

#[cfg(test)]
#[path = "runner/runner_tests.rs"]
mod tests;
