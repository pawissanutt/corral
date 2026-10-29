//! `corral agent`: the complete v1 subcommand and flag surface of the Pi-native host. Handler
//! bodies live in `corral_ops::agent`, so later T-252 children never edit this file.
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use corral_ops::agent::cli::{self, AgentTarget};
use corral_ops::agent::{AgentOp, ENVELOPE_MAX_BYTES};

use crate::entry::Entry;

#[derive(Args)]
pub struct AgentArgs {
    #[command(subcommand)]
    cmd: AgentCmd,
}

#[derive(Subcommand)]
enum AgentCmd {
    /// Run one Pi-native host request: a JSON envelope on stdin, one JSON receipt on stdout
    Call {
        /// Enumerated v1 operation; must equal the envelope's op
        #[arg(long, value_parser = op_names())]
        op: String,
    },
    /// Start the Pi-native host in this pane from a one-shot bootstrap (run by corral)
    Launch {
        /// One-shot bootstrap file written by dispatch, review-run, pm open or resume
        #[arg(long)]
        bootstrap: PathBuf,
        /// Asset directory; a fixture installation is accepted only through [agent] assets_dir
        #[arg(long)]
        assets: Option<PathBuf>,
    },
    /// Install the Pi-native host assets into the user data directory
    Install {
        /// Package directory, or a compiled corral-pi-host, to install from (default: the
        /// corral-pi-host beside this corral, else this build's checkout)
        #[arg(long)]
        from: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Provision the corral-owned Pi credentials with one interactive owner login
    Login {
        /// Provider to log in; without it a terminal shows an arrow-key picker
        #[arg(long, value_parser = ["openai-codex", "anthropic", "amazon-bedrock"])]
        provider: Option<String>,
        /// Named AWS profile for Amazon Bedrock (never ambient credentials)
        #[arg(long)]
        aws_profile: Option<String>,
        /// Standard AWS region for Amazon Bedrock
        #[arg(long)]
        region: Option<String>,
    },
    /// Report the Bun runtime, installed assets, GO manifest, roles and credential presence
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Show a Pi-native binding: generations, host health, receipts, questions and turns
    State {
        #[command(flatten)]
        target: TargetArgs,
        #[arg(long)]
        json: bool,
    },
    /// Cancel a Pi-native host turn; the run, its questions and receipts stay live
    Cancel {
        #[command(flatten)]
        target: TargetArgs,
        /// Turn sequence to cancel (default: the current turn)
        #[arg(long)]
        turn: Option<u64>,
    },
    /// Fence a lost Pi-native generation and start the next one in the same pane
    Resume {
        #[command(flatten)]
        target: TargetArgs,
        /// Replace a host that is still alive after verifying its pid, start time and command
        #[arg(long)]
        replace: bool,
    },
}

#[derive(Args)]
struct TargetArgs {
    /// Task of the worker run or review child
    #[arg(required_unless_present = "pm", conflicts_with = "pm")]
    task: Option<String>,
    /// Run ID (default: the task's current run)
    #[arg(long, requires = "task")]
    run: Option<String>,
    /// Review child ID of the run (default: the worker run itself)
    #[arg(long, requires = "task")]
    review: Option<String>,
    /// The bound Pi-native PM instead of a task
    #[arg(long)]
    pm: bool,
}

impl TargetArgs {
    fn target(self) -> AgentTarget {
        match self.task {
            Some(task) => AgentTarget::Task {
                task,
                run: self.run,
                review: self.review,
            },
            None => AgentTarget::Pm,
        }
    }
}

fn op_names() -> Vec<&'static str> {
    AgentOp::ALL.iter().map(|op| op.as_str()).collect()
}

fn call(start: &Path, entry: &Entry, op: &str) -> Result<()> {
    let ctx = entry.open(start)?;
    let mut input = Vec::new();
    std::io::stdin()
        .take(ENVELOPE_MAX_BYTES as u64 + 1)
        .read_to_end(&mut input)
        .context("reading the request envelope from stdin")?;
    let response = corral_ops::agent::call(&ctx, op, &input)?;
    println!("{}", serde_json::to_string(&response.to_json()?)?);
    Ok(())
}

pub fn run(start: &Path, args: AgentArgs, entry: &Entry) -> Result<()> {
    match args.cmd {
        AgentCmd::Call { op } => call(start, entry, &op),
        AgentCmd::Launch { bootstrap, assets } => {
            cli::launch(start, &cli::LaunchArgs { bootstrap, assets })
        }
        AgentCmd::Install { from, json } => cli::install(start, &cli::InstallArgs { from, json }),
        AgentCmd::Login {
            provider,
            aws_profile,
            region,
        } => match corral_ops::agent::credentials::login_with_aws(
            start,
            provider.as_deref(),
            &corral_ops::agent::credentials::AwsLoginOptions {
                profile: aws_profile.as_deref(),
                region: region.as_deref(),
            },
        ) {
            Err(error) if error.is::<corral_ops::agent::credentials::LoginCancelled>() => {
                eprintln!("{error}");
                std::process::exit(130);
            }
            result => result,
        },
        AgentCmd::Doctor { json } => cli::doctor(&entry.open(start)?, json),
        AgentCmd::State { target, json } => cli::state(
            &entry.open(start)?,
            &cli::StateArgs {
                target: target.target(),
                json,
            },
        ),
        AgentCmd::Cancel { target, turn } => cli::cancel(
            &entry.open(start)?,
            &cli::CancelArgs {
                target: target.target(),
                turn,
            },
        ),
        AgentCmd::Resume { target, replace } => cli::resume(
            &entry.open(start)?,
            &cli::ResumeArgs {
                target: target.target(),
                replace,
            },
        ),
    }
}
