//! `corral agent login`: one interactive owner login into the corral-owned Pi auth directory
//! (OD8), refreshed by Pi itself; never ~/.codex or another harness store.
//! Owner: T-252 child host-install.
use std::path::Path;

use anyhow::Result;

use super::cli::{AgentCommand, unavailable};

/// The T-252 child that lands credential provisioning.
pub const OWNER: &str = "host-install";

pub fn login(_start: &Path) -> Result<()> {
    Err(unavailable(AgentCommand::Login))
}
