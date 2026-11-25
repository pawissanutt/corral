use std::{
    io::{self, Read, Write},
    path::Path,
};

use anyhow::{Context, Result};
use clap::Subcommand;
use corral_engine::{
    Engine,
    board_plan::{BoardPlanRefusal, MAX_PLAN_BYTES},
};

use crate::output::Output;

#[derive(Subcommand)]
pub(crate) enum Command {
    Check,
    File {
        #[arg(long)]
        id: String,
        #[arg(long)]
        start: bool,
    },
}

pub(crate) fn run(root: &Path, command: Command, out: &mut Output<'_>) -> Result<u8> {
    let mut payload = Vec::new();
    io::stdin()
        .lock()
        .take((MAX_PLAN_BYTES + 1) as u64)
        .read_to_end(&mut payload)
        .context("read board plan from stdin")?;
    let result = match command {
        Command::Check => Engine::check_board_plan(root, &payload)
            .and_then(|()| out.value(&(), |out, _| writeln!(out, "Board plan is valid"))),
        Command::File { id, start } => Engine::file_board_plan(root, &id, start, &payload)
            .and_then(|receipt| {
                out.value(&receipt, |out, receipt| {
                    for parent in &receipt.parents {
                        writeln!(out, "{} ({}): board plan parent", parent.id, parent.project)?;
                    }
                    for task in &receipt.tasks {
                        writeln!(
                            out,
                            "{} ({}): {} [{}]",
                            task.id,
                            task.project,
                            task.key,
                            task.status.as_str()
                        )?;
                    }
                    Ok(())
                })
            }),
    };
    match result {
        Ok(()) => Ok(0),
        Err(error) => {
            if let Some(refusal) = error.downcast_ref::<BoardPlanRefusal>() {
                out.value(refusal, |_, refusal| {
                    writeln!(io::stderr().lock(), "refused: {refusal}")
                })?;
                Ok(2)
            } else {
                Err(error)
            }
        }
    }
}
