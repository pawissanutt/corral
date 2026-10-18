//! Setup is configuration review, not dispatch or terminal-engine authority.
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use corral_core::{ProfileSetupApproval, Store};
use corral_ops::SetupProposalInput;
use serde::{Serialize, de::DeserializeOwned};

#[derive(Args)]
pub struct SetupArgs {
    /// Explicit structured output (also the default)
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    pub cmd: SetupCmd,
}

#[derive(Subcommand)]
pub enum SetupCmd {
    /// Read persisted setup state without observing configuration edits
    Status,
    /// Read the current profile catalog
    Catalog,
    /// Run local harness inventory probes; never makes paid model calls
    Discover,
    /// Begin a new setup conversation
    Begin {
        #[arg(long)]
        expected_revision: u64,
    },
    /// Validate a proposal and its saved discovery without writing
    Validate {
        #[arg(long, value_name = "JSON")]
        file: PathBuf,
    },
    /// Save a proposal and its discovery for owner review
    Propose {
        #[arg(long, value_name = "JSON")]
        file: PathBuf,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Approve the exact proposal, including any provisional acknowledgement
    Approve {
        #[arg(long, value_name = "JSON")]
        file: PathBuf,
        #[arg(long)]
        expected_revision: u64,
    },
    /// Preview the persisted proposal's configuration changes
    Diff,
    /// Apply only the persisted, approved proposal and discovery
    Apply,
    /// Cancel the current setup without changing configured profiles
    Cancel {
        #[arg(long)]
        expected_revision: u64,
    },
}

impl SetupCmd {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Catalog => "catalog",
            Self::Discover => "discover",
            Self::Begin { .. } => "begin",
            Self::Validate { .. } => "validate",
            Self::Propose { .. } => "propose",
            Self::Approve { .. } => "approve",
            Self::Diff => "diff",
            Self::Apply => "apply",
            Self::Cancel { .. } => "cancel",
        }
    }
}

fn read_input<T: DeserializeOwned>(path: &Path, max: u64) -> Result<T> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("inspecting JSON input {}", path.display()))?;
    if !meta.is_file() {
        bail!(
            "{} must be an ordinary regular JSON file (not a symlink or special file)",
            path.display()
        );
    }
    if meta.len() > max {
        bail!(
            "{} exceeds artifacts.max_bytes ({max} bytes)",
            path.display()
        );
    }
    let mut file =
        File::open(path).with_context(|| format!("opening JSON input {}", path.display()))?;
    let meta = file
        .metadata()
        .with_context(|| format!("inspecting opened JSON input {}", path.display()))?;
    if !meta.is_file() {
        bail!("{} is not a regular JSON file", path.display());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(max)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading bounded JSON input {}", path.display()))?;
    let mut extra = [0];
    if file
        .read(&mut extra)
        .with_context(|| format!("checking JSON input size {}", path.display()))?
        != 0
    {
        bail!(
            "{} exceeds artifacts.max_bytes ({max} bytes)",
            path.display()
        );
    }
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing typed JSON input {}", path.display()))
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn run(start: &Path, args: SetupArgs) -> Result<()> {
    // Even rejected mutations must not record unrelated raw configuration edits.
    // Do not construct Ctx/Herdr or use Entry::open on this path.
    let store = Store::open_unobserved(&Store::find_root(start)?)?;
    let max = store.config.artifacts.max_bytes;
    match args.cmd {
        SetupCmd::Status => print_json(&corral_ops::setup_status(&store)?),
        SetupCmd::Catalog => print_json(&corral_ops::setup_catalog(&store)?),
        SetupCmd::Discover => print_json(&corral_ops::setup_discover(&store.config)),
        SetupCmd::Begin { expected_revision } => {
            print_json(&corral_ops::setup_begin(&store, expected_revision)?)
        }
        SetupCmd::Validate { file } => {
            let input: SetupProposalInput = read_input(&file, max)?;
            print_json(&corral_ops::setup_validate(&store, &input)?)
        }
        SetupCmd::Propose {
            file,
            expected_revision,
        } => {
            let input: SetupProposalInput = read_input(&file, max)?;
            print_json(&corral_ops::setup_propose(
                &store,
                &input,
                expected_revision,
            )?)
        }
        SetupCmd::Approve {
            file,
            expected_revision,
        } => {
            let input: ProfileSetupApproval = read_input(&file, max)?;
            print_json(&corral_ops::setup_approve(
                &store,
                &input,
                expected_revision,
            )?)
        }
        SetupCmd::Diff => print_json(&corral_ops::setup_diff(&store)?),
        SetupCmd::Apply => print_json(&corral_ops::setup_apply(&store)?),
        SetupCmd::Cancel { expected_revision } => {
            print_json(&corral_ops::setup_cancel(&store, expected_revision)?)
        }
    }
}
