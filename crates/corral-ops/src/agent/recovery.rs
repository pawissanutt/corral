//! Host loss, `corral agent cancel` and `resume [--replace]`: fence generation n, drain its
//! executors, launch n+1 in the same pane and deliver one recovery_notice; provider errors from
//! the host feed provider_stall directly. Owner: T-252 child recovery.
use anyhow::Result;

use super::cli::{AgentCommand, CancelArgs, ResumeArgs, unavailable};
use crate::Ctx;

/// The T-252 child that lands recovery.
pub const OWNER: &str = "recovery";

pub fn cancel(_ctx: &Ctx, _args: &CancelArgs) -> Result<()> {
    Err(unavailable(AgentCommand::Cancel))
}

pub fn resume(_ctx: &Ctx, _args: &ResumeArgs) -> Result<()> {
    Err(unavailable(AgentCommand::Resume))
}
