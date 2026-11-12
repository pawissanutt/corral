use std::path::Path;

use anyhow::{Context, Result};
use clap::Subcommand;
use corral_core::Store;
use corral_ops::daemon_client;

#[derive(Subcommand)]
pub enum DaemonCmd {
    /// Run the workspace sync daemon in the foreground
    Run {
        /// Complete one pass and exit
        #[arg(long)]
        once: bool,
    },
    /// Read daemon health (exit 0 healthy, 1 degraded, 2 down)
    Status {
        /// Print the health snapshot as JSON
        #[arg(long)]
        json: bool,
    },
    /// Request a pass (exit 2 if no daemon answers)
    Nudge,
    /// Stop the daemon after its current pass
    Stop {
        /// Drain the running pass before stopping
        #[arg(long)]
        drain: bool,
    },
    /// Write a service definition without enabling or starting it
    Install {
        /// Print the service definition without writing it
        #[arg(long)]
        print: bool,
        /// Replace a different existing service definition
        #[arg(long)]
        force: bool,
    },
}

pub fn run(start: &Path, cmd: DaemonCmd, entry: &crate::entry::Entry) -> Result<()> {
    match cmd {
        DaemonCmd::Run { once } => {
            let exit = corral_daemon::run(start, corral_daemon::RunOptions { once })?;
            if matches!(exit, corral_daemon::Exit::AlreadyRunning) {
                println!("corrald is already running for this workspace");
            }
            std::process::exit(i32::from(exit.code()));
        }
        DaemonCmd::Status { json } => {
            let store = entry.open(start)?.store;
            let health = daemon_client::health(&store);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&health).context("encoding daemon health")?
                );
            } else {
                println!("{}", health.line());
            }
            std::process::exit(health_code(&health));
        }
        DaemonCmd::Nudge => {
            let store = entry.open(start)?.store;
            let answered = daemon_client::nudge(&store, "cli");
            println!(
                "{}",
                if answered {
                    "corrald answered; pass requested"
                } else {
                    "no daemon answered"
                }
            );
            if !answered {
                std::process::exit(2);
            }
            Ok(())
        }
        DaemonCmd::Stop { drain } => {
            let store = entry.open(start)?.store;
            let Some(mut client) = daemon_client::Client::connect(&store)? else {
                println!("corrald is not running");
                std::process::exit(2);
            };
            client.stop(drain)?;
            println!("corrald stop requested");
            Ok(())
        }
        DaemonCmd::Install { print, force } => {
            let store = entry.open(start)?.store;
            crate::daemon_unit::install(&store, print, force)
        }
    }
}

fn health_code(health: &corral_protocol::Health) -> i32 {
    match health.daemon {
        corral_protocol::DaemonState::Healthy => 0,
        corral_protocol::DaemonState::Down => 2,
        _ => 1,
    }
}

pub(crate) fn startup_health(store: &Store) -> Result<()> {
    let health = daemon_client::health(store);
    if health_code(&health) != 2 || crate::daemon_unit::installed(store)? {
        println!("{}", health.line());
    } else {
        println!("For always-on sync, run `corral daemon install`.");
    }
    Ok(())
}
