//! A tiered review takes its gate only from corral's Gate contract (review) section, so a
//! `.corral/prompts/review.md` override or the selected reviewer profile's instructions that
//! hardcode a full-gate directive refuse the launch before a pane starts or review state
//! changes. Legacy runs and untiered projects keep today's review prompt and never refuse.
use anyhow::{Context as _, Result};
use corral_core::{Run, Task};

use crate::Ctx;
use crate::gate_tier::{GateInstructionConflict, scan_gate_instructions};

#[cfg(test)]
#[path = "instruction_guard_tests.rs"]
mod tests;

/// Refuses the launch on the first conflict; an automatic launch reports it once, and a clean
/// scan resumes that report.
pub(super) fn check(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    profiles: &[&str],
    automatic: bool,
) -> Result<()> {
    let Some(conflict) = conflict(ctx, task, run, profiles)? else {
        return crate::gate_tier::resolve_blocked(ctx, task);
    };
    if automatic {
        crate::gate_tier::report_blocked(ctx, task, &conflict)?;
    }
    Err(conflict.into())
}

/// The review template override, then each named profile's `metadata.instructions`.
pub(crate) fn conflict(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    profiles: &[&str],
) -> Result<Option<GateInstructionConflict>> {
    let config = &ctx.store.config;
    let policy = config.task_review_policy(task);
    if run.gate_rules.is_legacy() || policy.gate_tiers.is_empty() {
        return Ok(None);
    }
    let scan = |surface: &str, path: String, text: &str| {
        scan_gate_instructions(text, &policy.gate).map(|(line, phrase)| GateInstructionConflict {
            surface: surface.to_owned(),
            path,
            line,
            phrase,
        })
    };
    let template = ctx.store.prompts_dir().join("review.md");
    match std::fs::read_to_string(&template) {
        Ok(text) => {
            if let Some(found) = scan("review_template", template.display().to_string(), &text) {
                return Ok(Some(found));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("reading {}", template.display())),
    }
    let mut seen: Vec<&str> = Vec::new();
    for profile in profiles {
        if seen.contains(profile) {
            continue;
        }
        seen.push(profile);
        let Some(instructions) = config
            .profiles
            .get(*profile)
            .and_then(|p| p.metadata.as_ref())
            .map(|m| m.instructions.as_str())
        else {
            continue;
        };
        let path = format!("profiles.{profile}.metadata.instructions");
        if let Some(found) = scan("reviewer_instructions", path, instructions) {
            return Ok(Some(found));
        }
    }
    Ok(None)
}
