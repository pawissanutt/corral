//! Diagnostics for command-success receipts that no longer bind to the candidate.
use crate::{Ctx, verification};
use anyhow::Result;
use corral_core::{CandidateIdentity, GateRecord, RunHead, TaskRef};

pub(super) fn log_passed<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    record: &GateRecord,
) -> Result<bool> {
    let Some(identity) = &record.identity else {
        return Ok(false);
    };
    let Some(log) = super::evidence::attempt_log(ctx, task.id(), &run.id, &identity.attempt)?
    else {
        return Ok(false);
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

pub(super) fn reason<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
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
