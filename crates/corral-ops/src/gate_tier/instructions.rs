//! In a tiered project the appended Gate contract is the only gate instruction a worker gets,
//! so a run or harness template, or the profile's working instructions, that hardcode a
//! full-gate directive refuse dispatch before any run, worktree or pane exists.
use std::fmt;

use anyhow::Result;
use corral_core::{ExecutionSnapshot, GateRules, Task};
use serde::Serialize;
use serde_json::json;

use crate::Ctx;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateInstructionConflict {
    /// `run_template`, `harness_template` or `worker_instructions`.
    pub surface: String,
    /// The template file, or the profile key whose instructions conflict.
    pub path: String,
    /// One-based line of the directive.
    pub line: usize,
    /// The directive as written.
    pub phrase: String,
}

impl fmt::Display for GateInstructionConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "gate instruction conflict: {} {} line {} says {:?}; tiered runs take their gate only from corral's Gate contract, so remove the directive",
            self.surface, self.path, self.line, self.phrase
        )
    }
}

impl std::error::Error for GateInstructionConflict {}

const DIRECTIVES: [&str; 4] = [
    "run the full gate",
    "run full gate",
    "rerun the gate",
    "re-run the gate",
];
const GATE_SCRIPT: &str = "scripts/gate.sh";

/// The first imperative full-gate directive in `text` as `(line, phrase)`: "run the full gate",
/// "re-run the gate", `scripts/gate.sh` without `--tier`, or a literal full gate command.
/// Matching is ASCII case-insensitive and ignores `{{...}}` placeholders.
pub fn scan_gate_instructions(text: &str, full_commands: &[String]) -> Option<(usize, String)> {
    let commands: Vec<String> = full_commands
        .iter()
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| !c.is_empty())
        .collect();
    let needles: Vec<&str> = DIRECTIVES
        .into_iter()
        .chain(commands.iter().map(String::as_str))
        .collect();
    text.lines().enumerate().find_map(|(index, line)| {
        let lower = without_placeholders(line).to_ascii_lowercase();
        let found = needles
            .iter()
            .find_map(|needle| lower.find(needle).map(|start| (start, needle.len())))
            .or_else(|| bare_gate_script(&lower));
        found.map(|(start, len)| (index + 1, line[start..start + len].to_owned()))
    })
}

/// Blanks every `{{...}}` span byte for byte, so offsets still index the original line.
fn without_placeholders(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find("{{") {
        let Some(close) = rest[open..].find("}}") else {
            break;
        };
        out.push_str(&rest[..open]);
        out.extend(std::iter::repeat_n(' ', close + 2));
        rest = &rest[open + close + 2..];
    }
    out.push_str(rest);
    out
}

fn bare_gate_script(lower: &str) -> Option<(usize, usize)> {
    lower.match_indices(GATE_SCRIPT).find_map(|(start, _)| {
        let after = lower[start + GATE_SCRIPT.len()..].trim_start();
        (!after.starts_with("--tier")).then_some((start, GATE_SCRIPT.len()))
    })
}

/// The run's gate rules, fixed now: tiered only in a tiered project whose selected template and
/// working instructions passed the scan. A conflict is reported once to automation.
pub(crate) fn dispatch_rules(
    ctx: &Ctx,
    task: &Task,
    execution: &ExecutionSnapshot,
    automatic: bool,
) -> Result<GateRules> {
    let policy = ctx
        .store
        .config
        .review_policy(task.project.as_deref(), task.review);
    if policy.gate_tiers.is_empty() {
        return Ok(GateRules::Legacy);
    }
    let (surface, path) = template_source(ctx, &execution.resolved.harness);
    let template = execution.prompt_template.as_deref().unwrap_or_default();
    let conflict = |surface: &str, path: String, text: &str| {
        scan_gate_instructions(text, &policy.gate).map(|(line, phrase)| GateInstructionConflict {
            surface: surface.to_owned(),
            path,
            line,
            phrase,
        })
    };
    let found = conflict(surface, path, template).or_else(|| {
        conflict(
            "worker_instructions",
            format!(
                "profiles.{}.metadata.instructions",
                execution.resolved.profile
            ),
            &execution.instructions,
        )
    });
    let Some(found) = found else {
        return Ok(GateRules::Tiered);
    };
    if automatic {
        report_blocked(ctx, task, &found)?;
    }
    Err(found.into())
}

/// Where `prompt::harness_template` read the run's template from.
fn template_source(ctx: &Ctx, harness: &str) -> (&'static str, String) {
    if let Some(path) = ctx
        .store
        .config
        .harnesses
        .get(harness)
        .and_then(|h| h.prompt.as_deref())
    {
        return (
            "harness_template",
            ctx.store.root().join(path).display().to_string(),
        );
    }
    let file = ctx.store.prompts_dir().join("run.md");
    let path = if file.is_file() {
        file.display().to_string()
    } else {
        "built-in assets/run.md".to_owned()
    };
    ("run_template", path)
}

/// One `automation_blocked` event per run and reason, across concurrent sync callers.
fn report_blocked(ctx: &Ctx, task: &Task, conflict: &GateInstructionConflict) -> Result<()> {
    let Some(_lock) = ctx
        .store
        .try_lock(&format!("autopilot-status-{}", task.id))?
    else {
        return Ok(());
    };
    let run = task.current_run().map(|run| run.id.as_str());
    let reason = conflict.to_string();
    let events = ctx.store.events_shared()?;
    let previous = events.iter().rev().find(|event| {
        event.task.as_deref() == Some(task.id.as_str())
            && event.detail["run"].as_str() == run
            && matches!(
                event.kind.as_str(),
                "automation_blocked" | "automation_resumed"
            )
    });
    if previous.is_none_or(|event| {
        event.kind != "automation_blocked" || event.detail["reason"].as_str() != Some(&reason)
    }) {
        ctx.store.log_event(
            Some(&task.id),
            "automation_blocked",
            json!({"run": run, "reason": reason, "conflict": conflict}),
        )?;
    }
    Ok(())
}
