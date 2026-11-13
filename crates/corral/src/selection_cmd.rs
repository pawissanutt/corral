//! File-only selector operations: no Ctx, terminal engine or provider probes.
use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use corral_core::{ProfileRole, Store};
use corral_protocol::cmd::{Command, setup::SetupCommand};

#[derive(Args)]
pub struct SelectionArgs {
    /// Structured output (also the default)
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    pub cmd: SelectionCmd,
}

#[derive(Subcommand)]
pub enum SelectionCmd {
    /// Read current task identities and eligible profile guidance
    Catalog {
        id: String,
        #[arg(long, value_enum)]
        role: SelectionRole,
    },
    /// Validate and save a proposer/PM Auto receipt, never a manual pin
    Save {
        #[arg(long, value_name = "JSON")]
        file: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum SelectionRole {
    Implementation,
    Planning,
    #[value(name = "implementation_review")]
    ImplementationReview,
    #[value(name = "plan_review")]
    PlanReview,
}

impl SelectionCmd {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Catalog { .. } => "catalog",
            Self::Save { .. } => "save",
        }
    }
}

pub fn run(inv: &crate::invocation::Invocation, args: SelectionArgs) -> Result<()> {
    match args.cmd {
        SelectionCmd::Catalog { id, role } => {
            // Catalog keeps its unobserved, unattributed local open.
            let store = Store::open_unobserved(&Store::find_root(&inv.start()?)?)?;
            let role = match role {
                SelectionRole::Implementation => ProfileRole::Implementation,
                SelectionRole::Planning => ProfileRole::Planning,
                SelectionRole::ImplementationReview => ProfileRole::ImplementationReview,
                SelectionRole::PlanReview => ProfileRole::PlanReview,
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&corral_ops::selection_catalog(&store, &id, role)?)?
            );
        }
        SelectionCmd::Save { file } => {
            return inv.submit(Command::Setup(SetupCommand::SelectionSave {
                file: crate::setup_cmd::read_command_input(&file)?,
            }));
        }
    }
    Ok(())
}
