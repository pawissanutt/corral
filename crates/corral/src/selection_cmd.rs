//! File-only selector operations: no Ctx, terminal engine or provider probes.
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use corral_core::{AutoProfileSelection, ProfileRole, Store};

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
}

impl SelectionCmd {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Catalog { .. } => "catalog",
            Self::Save { .. } => "save",
        }
    }
}

pub fn run(start: &Path, args: SelectionArgs) -> Result<()> {
    let store = Store::open_unobserved(&Store::find_root(start)?)?;
    match args.cmd {
        SelectionCmd::Catalog { id, role } => {
            let role = match role {
                SelectionRole::Implementation => ProfileRole::Implementation,
                SelectionRole::Planning => ProfileRole::Planning,
                SelectionRole::ImplementationReview => ProfileRole::ImplementationReview,
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&corral_ops::selection_catalog(&store, &id, role)?)?
            );
        }
        SelectionCmd::Save { file } => {
            let receipt: AutoProfileSelection =
                crate::setup_cmd::read_input(&file, store.config.artifacts.max_bytes)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&corral_ops::selection_save(&store, &receipt)?)?
            );
        }
    }
    Ok(())
}
