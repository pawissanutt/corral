//! Handlers behind every v1 `corral agent` subcommand other than `call`. The CLI parses the
//! complete surface once (request-receipts); each handler delegates to its owner's module
//! (`launch`, `install`, `credentials`, `recovery`), so the owning child fills that module and
//! never edits this file or the CLI crate. Every subcommand has landed; `OpUnavailable` names
//! the owner a subcommand answered with before its child landed.
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::Ctx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCommand {
    Launch,
    Install,
    Login,
    Doctor,
    State,
    Cancel,
    Resume,
}

impl AgentCommand {
    pub fn name(self) -> &'static str {
        match self {
            AgentCommand::Launch => "launch",
            AgentCommand::Install => "install",
            AgentCommand::Login => "login",
            AgentCommand::Doctor => "doctor",
            AgentCommand::State => "state",
            AgentCommand::Cancel => "cancel",
            AgentCommand::Resume => "resume",
        }
    }

    /// The T-252 child that lands this subcommand.
    pub fn owner(self) -> &'static str {
        match self {
            AgentCommand::Launch => "host-bridge",
            AgentCommand::Install
            | AgentCommand::Login
            | AgentCommand::Doctor
            | AgentCommand::State => "host-install",
            AgentCommand::Cancel | AgentCommand::Resume => "recovery",
        }
    }
}

/// A subcommand whose owning child has not landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpUnavailable {
    pub command: AgentCommand,
    pub owner_child: &'static str,
}

impl std::fmt::Display for OpUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "op_unavailable: `corral agent {}` lands with T-252 child {}",
            self.command.name(),
            self.owner_child
        )
    }
}

impl std::error::Error for OpUnavailable {}

/// The container a state, cancel or resume command names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTarget {
    /// A worker run (`run` absent: the task's current run) or one of its review children.
    Task {
        task: String,
        run: Option<String>,
        review: Option<String>,
    },
    Pm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchArgs {
    pub bootstrap: PathBuf,
    pub assets: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallArgs {
    pub from: Option<PathBuf>,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateArgs {
    pub target: AgentTarget,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelArgs {
    pub target: AgentTarget,
    pub turn: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeArgs {
    pub target: AgentTarget,
    pub replace: bool,
}

/// Runs in the pane: consumes the one-shot bootstrap and execs the Bun host.
pub fn launch(start: &Path, args: &LaunchArgs) -> Result<()> {
    super::launch::launch(start, args)
}

pub fn install(start: &Path, args: &InstallArgs) -> Result<()> {
    super::install::install(start, args)
}

/// One interactive owner login into the corral-owned Pi auth directory (OD8): `provider`, or
/// the one the owner picks on a terminal.
pub fn login(start: &Path, provider: Option<&str>) -> Result<()> {
    super::credentials::login(start, provider)
}

pub fn doctor(ctx: &Ctx, json: bool) -> Result<()> {
    super::install::doctor(ctx, json)
}

pub fn state(ctx: &Ctx, args: &StateArgs) -> Result<()> {
    super::install::state(ctx, args)
}

/// Cancels a turn; the run, its questions and its receipts stay live.
pub fn cancel(ctx: &Ctx, args: &CancelArgs) -> Result<()> {
    super::recovery::cancel(ctx, args)
}

/// Fences a lost generation and launches the next one in the same pane.
pub fn resume(ctx: &Ctx, args: &ResumeArgs) -> Result<()> {
    super::recovery::resume(ctx, args)
}
