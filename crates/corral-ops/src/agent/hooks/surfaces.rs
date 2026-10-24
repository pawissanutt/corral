//! Instruction-surface seam: the Pi contract assets live in `PI_ASSETS`, and the T-251 drift
//! test (crates/corral-ops/tests/instruction_surfaces.rs) scans every Markdown file there, so a
//! contract asset added later is covered without editing the test. The worker's gate section is
//! rendered here from the run's GateContract readback, the same resolver `corral show --json`
//! reports, and reaches the worker in its run prompt message (`wake::kickoff`).
//! Owner: T-252 child worker-role.
use anyhow::Result;
use corral_core::Task;
use corral_core::agent::paths::sha256_hex;

use crate::Ctx;
use crate::gate_tier::{DestinationKind, GateContract};

/// The Pi contract asset directory, relative to the repository root.
pub const PI_ASSETS: &str = "packages/corral-pi/assets";

/// The worker's gate section and the SHA-256 of exactly that text, the `contract_sha256` its
/// AgentReport carries for audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateSection {
    pub text: String,
    pub contract_sha256: String,
}

fn listed(items: &[String], empty: &str) -> String {
    if items.is_empty() {
        return format!(" {empty}");
    }
    items.iter().map(|i| format!("\n  - `{i}`")).collect()
}

fn landing(contract: &GateContract) -> String {
    let branch = &contract.destination.branch;
    match contract.destination.kind {
        DestinationKind::PlanTarget => format!(
            "this member lands on its Plan target `{branch}` on the {} tier, and corral gates the \
             aggregate itself at plan verify",
            contract.landing_tier
        ),
        DestinationKind::Main | DestinationKind::Other => format!(
            "corral itself runs the {} tier when this candidate lands on `{branch}`",
            contract.landing_tier
        ),
    }
}

/// The Pi worker's one gate instruction for `contract`. `commands` lists what the tier runs, for
/// reference; a message too large for the host's input bound renders without them.
pub fn render_gate(contract: &GateContract, commands: bool) -> String {
    let expected = if contract.expected_failures.is_empty() {
        " (none)".to_owned()
    } else {
        contract
            .expected_failures
            .iter()
            .map(|f| format!("\n  - `{}`: {}", f.test, f.reason))
            .collect()
    };
    let commands = if commands {
        format!(
            "\n- Commands the {} tier runs (reference only; corral runs them):{}",
            contract.tier,
            listed(&contract.commands, "(none)")
        )
    } else {
        String::new()
    };
    format!(
        "## Gate (authoritative)\n\n\
         This is the only gate instruction for this run. It replaces every gate command the prompt \
         file names: you never run `corral gate` or a gate command yourself.\n\n\
         - Tier: {tier} (source: {source})\n\
         - Gate: call `corral_gate` exactly once per round, after the final commit and rebase. It \
         answers `started` with the attempt; end your turn and corral wakes you with the result \
         and the `gate_ref` your `corral_done` carries. A rejected or uncertain answer never \
         started a gate.{commands}\n\
         - Module scope:{scope}\n\
         - Declared tests:{tests}\n\
         - Expected failures:{expected}\n\
         - Landing: {landing}; do not start it yourself.\n\n\
         Fix failures inside the module scope that are not expected failures, commit, and call \
         `corral_gate` again. Report a failure outside the module scope, or an expected failure, \
         under `Not verified:` in the done summary instead of fixing it, and ask with `corral_ask` \
         when it keeps the tier from passing.\n",
        tier = contract.tier,
        source = contract.source,
        scope = listed(&contract.scope, "(the task declares no write scope)"),
        tests = listed(&contract.tests, "(none: the tier's own selection)"),
        landing = landing(contract),
    )
}

/// The current run's gate section from its GateContract readback, or the readback's reason when
/// the contract cannot be resolved now.
pub fn worker_gate_section(
    ctx: &Ctx,
    task: &Task,
    commands: bool,
) -> Result<Result<GateSection, String>> {
    let readback = crate::gate_tier::gate_readback(ctx, task)?;
    Ok(match readback.gate_contract {
        Some(contract) => {
            let text = render_gate(&contract, commands);
            Ok(GateSection {
                contract_sha256: sha256_hex(text.as_bytes()),
                text,
            })
        }
        None => Err(readback
            .gate_contract_error
            .unwrap_or_else(|| "the task has no current run".into())),
    })
}
