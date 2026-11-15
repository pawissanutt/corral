//! CLI routing for session controls.
use anyhow::Result;
use clap::{Args, Subcommand};
use corral_protocol::cmd::{Command, session::SessionCommand};

use crate::invocation::Invocation;

#[derive(Args)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub(crate) cmd: SessionCmd,
}

#[derive(Subcommand)]
pub(crate) enum SessionCmd {
    /// Cancel the current turn without ending the session
    Cancel {
        id: String,
        #[arg(long)]
        generation: Option<u32>,
        #[arg(long)]
        turn: Option<u64>,
        #[arg(long)]
        json: bool,
    },
    /// Pause automated input while keeping the host alive
    Pause {
        id: String,
        #[arg(long)]
        generation: Option<u32>,
        #[arg(long)]
        json: bool,
    },
    /// Resume automated input
    Resume {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop the host and end the session
    Stop {
        id: String,
        #[arg(long)]
        generation: Option<u32>,
        #[arg(long)]
        json: bool,
    },
    /// Start the next generation of a session
    Relaunch {
        id: String,
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        json: bool,
    },
    /// Open the PM when none is live
    PmOpen {
        #[arg(long)]
        json: bool,
    },
    /// Send owner words, lifting any pause
    Send {
        id: String,
        text: String,
        #[arg(long, default_value = "follow-up", value_parser = ["steer", "follow-up"])]
        policy: String,
        #[arg(long)]
        json: bool,
    },
    /// Send keys and optional text to a foreign-harness pane
    Keys {
        id: String,
        keys: Vec<String>,
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run(inv: &Invocation<'_>, args: SessionArgs) -> Result<()> {
    let command = match args.cmd {
        SessionCmd::Cancel {
            id,
            generation,
            turn,
            json,
        } => SessionCommand::Cancel {
            id,
            generation,
            turn,
            json,
        },
        SessionCmd::Pause {
            id,
            generation,
            json,
        } => SessionCommand::Pause {
            id,
            generation,
            json,
        },
        SessionCmd::Resume { id, json } => SessionCommand::Resume { id, json },
        SessionCmd::Stop {
            id,
            generation,
            json,
        } => SessionCommand::Stop {
            id,
            generation,
            json,
        },
        SessionCmd::Relaunch { id, replace, json } => {
            SessionCommand::Relaunch { id, replace, json }
        }
        SessionCmd::PmOpen { json } => SessionCommand::PmOpen { json },
        SessionCmd::Send {
            id,
            text,
            policy,
            json,
        } => SessionCommand::Send {
            id,
            text,
            policy,
            json,
        },
        SessionCmd::Keys {
            id,
            keys,
            text,
            json,
        } => SessionCommand::Keys {
            id,
            keys,
            text,
            json,
        },
    };
    inv.submit(Command::Session(command))
}
