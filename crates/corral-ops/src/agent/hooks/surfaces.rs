//! Instruction-surface seam: the Pi contract assets live in `PI_ASSETS`, and the T-251 drift
//! test (crates/corral-ops/tests/instruction_surfaces.rs) scans every Markdown file there, so a
//! contract asset added later is covered without editing the test. A Pi worker follows the gate
//! section dispatch rendered into its run prompt from the run's GateContract; its SHA-256 is the
//! `contract_sha256` the worker's AgentReport carries for audit.
//! Owner: T-252 child worker-role.
use anyhow::{Context, Result};
use corral_core::Task;
use corral_core::agent::paths::sha256_hex;

use crate::Ctx;

/// The Pi contract asset directory, relative to the repository root.
pub const PI_ASSETS: &str = "packages/corral-pi/assets";
/// The heading of the gate section a tiered run prompt carries.
pub const PROMPT_GATE_HEADING: &str = "## Gate contract (authoritative)";

/// The gate section of a run prompt and the SHA-256 of exactly that text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateSection {
    pub text: String,
    pub contract_sha256: String,
}

/// The run prompt dispatch wrote, the file the host's bootstrap names.
pub fn prompt_file(ctx: &Ctx, task: &str, run: &str) -> std::path::PathBuf {
    ctx.store.prompts_dir().join(format!("{task}-{run}.md"))
}

/// The gate section of `prompt`: from its heading line up to the next second-level heading,
/// ending in exactly one newline.
pub fn prompt_gate_section(prompt: &str) -> Option<GateSection> {
    let start = prompt
        .match_indices(PROMPT_GATE_HEADING)
        .map(|(at, _)| at)
        .find(|at| *at == 0 || prompt[..*at].ends_with('\n'))?;
    let body = &prompt[start + PROMPT_GATE_HEADING.len()..];
    let end = body.find("\n## ").map_or(body.len(), |at| at + 1);
    let text = format!("{PROMPT_GATE_HEADING}{}\n", body[..end].trim_end());
    Some(GateSection {
        contract_sha256: sha256_hex(text.as_bytes()),
        text,
    })
}

/// The gate section rendered into the current run's prompt; `None` for a run whose prompt has
/// none (a legacy or untiered run) or has not been written.
pub fn worker_gate_section(ctx: &Ctx, task: &Task) -> Result<Option<GateSection>> {
    let Some(run) = task.current_run() else {
        return Ok(None);
    };
    let path = prompt_file(ctx, &task.id, &run.id);
    match std::fs::read_to_string(&path) {
        Ok(prompt) => Ok(prompt_gate_section(&prompt)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}
