//! JSON-only local transport for the terminal's workspace startup preference.
use std::io::{self, Read, Write};

use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use corral_engine::pi_startup;
use corral_protocol::config::PiStartupConfig;
use serde::Deserialize;

use crate::output::Output;

const MAX_REQUEST: u64 = 2048;

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Print the workspace preference and its current revision.
    Read,
    /// Read {"revision":"…","model":null or "provider/model"} from stdin.
    Write,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteRequest {
    revision: String,
    // Missing is a malformed request; only explicit null clears the preference.
    #[serde(deserialize_with = "Deserialize::deserialize")]
    model: Option<String>,
}

pub(crate) fn run(command: Command, out: &mut Output<'_>) -> Result<()> {
    // No discovery, environment fallback, config load, DB open, or runner selection.
    let root = out.root.context("pi-startup requires an explicit --root")?;
    ensure!(
        !root.as_os_str().is_empty(),
        "pi-startup requires a workspace root"
    );
    let root = root.canonicalize().context("resolve Pi workspace root")?;
    let preference = match command {
        Command::Read => pi_startup::read(&root)?,
        Command::Write => {
            let mut bytes = Vec::new();
            io::stdin()
                .lock()
                .take(MAX_REQUEST + 1)
                .read_to_end(&mut bytes)
                .context("read Pi startup request")?;
            ensure!(
                bytes.len() as u64 <= MAX_REQUEST,
                "Pi startup request is too large"
            );
            let request: WriteRequest = serde_json::from_slice(&bytes)
                .map_err(|_| anyhow::anyhow!("invalid Pi startup request"))?;
            pi_startup::save(&root, &request.revision, request.model)?
        }
    };
    let response = PiStartupConfig {
        revision: preference.revision,
        model: preference.model,
    };
    serde_json::to_writer(&mut out.stdout, &response).context("write Pi startup response")?;
    writeln!(out.stdout).context("finish Pi startup response")
}
