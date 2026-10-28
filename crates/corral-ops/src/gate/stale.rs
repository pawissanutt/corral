//! Diagnostics for command-success receipts that no longer bind to the candidate.
use crate::{Ctx, verification};
use anyhow::Result;
use corral_core::{CandidateIdentity, GateRecord, Run, Task};

pub(super) fn log_passed(ctx: &Ctx, task: &Task, run: &Run, record: &GateRecord) -> Result<bool> {
    let path = super::gate_paths(ctx, &task.id, &run.id).log;
    let log = match std::fs::read(path) {
        Ok(log) => log,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let commands: Vec<_> = record.commands.iter().map(|r| r.command.clone()).collect();
    Ok(
        record.log_sha256.as_deref() == Some(verification::digest(&log).as_str())
            && super::command_log_valid(
                &log,
                &commands,
                &record.commands,
                corral_core::GateResult::Pass,
            ),
    )
}

pub(super) fn reason(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    record: &GateRecord,
    reported: Option<&CandidateIdentity>,
) -> Result<String> {
    let mut fields = Vec::new();
    if let Some(expected) = &record.identity {
        let current = verification::identity(ctx, task, run, expected)?;
        for actual in std::iter::once(&current).chain(reported) {
            for change in expected.differences(actual) {
                if !fields.contains(&change.field) {
                    fields.push(change.field);
                }
            }
        }
    }
    if fields.is_empty() {
        fields.push("gate binding or log proof".into());
    }
    Ok(format!(
        "stale_result: changed {}; rebase onto current main, then run the gate once on the new base",
        fields.join(", ")
    ))
}
