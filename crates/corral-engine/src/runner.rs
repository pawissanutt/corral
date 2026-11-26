//! Session backends: herdr panes, detached shell processes, and an in-memory fake.
//!
//! A process session owns its run directory. Its `exit_code` is the durable completion
//! marker, including when the command outlives the caller that launched it.

use std::{path::PathBuf, process::Command};

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

/// A sender's stable identity and delivery options, separate from the Pi frame ID.
#[derive(Clone, Copy, Debug)]
pub struct InputTag<'a> {
    pub input_id: &'a str,
    pub to: Option<&'a str>,
    pub typed_at: Option<&'a str>,
    pub device: Option<&'a str>,
    pub follow_up: bool,
    pub attachments: &'a [serde_json::Value],
}

pub trait Runner: Send + Sync {
    fn start(&self, spec: &LaunchSpec) -> Result<SessionRef>;
    /// Start an engine-owned command directly rather than evaluating a harness shell line.
    fn start_process(&self, _command: Command, _dir: PathBuf) -> Result<SessionRef> {
        anyhow::bail!("runner does not support direct process placement")
    }
    fn status(&self, session: &SessionRef) -> Result<SessionStatus>;
    /// Unlike the presentation status, an unknown sample is not idle evidence.
    fn reconciliation_status(&self, session: &SessionRef) -> Result<Option<SessionStatus>> {
        self.status(session).map(Some)
    }
    /// Owner replies and automatic input share the transport. Runners that can
    /// pause a session may defer automatic input.
    fn send(&self, session: &SessionRef, text: &str, origin: Origin) -> Result<()>;
    fn send_tagged(
        &self,
        _session: &SessionRef,
        _text: &str,
        _origin: Origin,
        _tag: InputTag<'_>,
    ) -> Result<pi::SubmitReply> {
        Err(crate::refuse(
            "tagged input requires a running Pi host",
            Vec::new(),
        ))
    }
    fn stop(&self, session: &SessionRef) -> Result<()>;
    fn focus(&self, session: &SessionRef) -> Result<()>;
}

#[cfg(all(test, unix))]
#[path = "runner/herdr_tests.rs"]
mod herdr_tests;

#[cfg(all(test, unix))]
#[path = "runner/pi_tests.rs"]
mod pi_tests;

#[cfg(test)]
#[path = "runner/runner_tests.rs"]
mod tests;
