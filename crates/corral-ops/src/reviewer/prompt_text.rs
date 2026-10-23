use super::*;

const DEFAULT_TEMPLATE: &str = include_str!("../../assets/review.md");

/// The review prompt for the task's run, as markdown: `.corral/prompts/review.md` when present, else the
/// built-in template. Placeholders: {{id}} {{title}} {{run}} {{branch}} {{base}} {{worktree}} {{spec}}
/// {{summary}} {{gate}} {{diff_cmd}} {{artifacts}} {{agents_md}} {{notes_file}} {{verdict_cmd}}.
/// A routed review always appends its versioned contract, even under a custom template, and a
/// tiered run's review always appends one authoritative Gate contract (review) section.
pub fn review_prompt(ctx: &Ctx, task: &Task, run: &Run) -> Result<String> {
    let gate_contract = crate::review_packet::gate_contract(ctx, task, run)?;
    let override_file = ctx.store.prompts_dir().join("review.md");
    let template = match std::fs::read_to_string(&override_file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_TEMPLATE.to_string(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", override_file.display())),
    };
    let corral = crate::prompt::shell_path(&corral_bin()?);
    let project_dir = match task.project.as_deref() {
        Some(p) => ctx.store.project_path(p)?,
        None => run.worktree.clone(),
    };
    let spec = if task.body.trim().is_empty() {
        "(the task has no spec beyond its title)".to_string()
    } else {
        task.body.trim_end().to_string()
    };
    let values = [
        ("id", task.id.clone()),
        ("title", task.title.clone()),
        ("run", run.id.clone()),
        ("branch", run.branch.clone()),
        ("base", run.base.clone()),
        ("worktree", run.worktree.display().to_string()),
        ("spec", spec),
        (
            "summary",
            run.summary
                .clone()
                .unwrap_or_else(|| "(no summary)".to_string()),
        ),
        ("gate", {
            let receipt = gate_text(ctx, task, run);
            match &gate_contract {
                Some(_) => format!("{receipt}\n\n{GATE_POINTER}"),
                None => receipt,
            }
        }),
        ("diff_cmd", format!("{corral} review {} --diff", task.id)),
        ("artifacts", artifacts_text(ctx, task, run)),
        (
            "agents_md",
            format!(
                "Follow {}/AGENTS.md (and any deeper AGENTS.md) if it exists.",
                project_dir.display()
            ),
        ),
        (
            "notes_file",
            review_notes_path(ctx, &task.id, &run.id)
                .display()
                .to_string(),
        ),
        ("verdict_cmd", {
            let attempt = run
                .review
                .as_ref()
                .and_then(callback::token)
                .or_else(|| {
                    run.gate
                        .as_ref()
                        .and_then(|g| g.identity.as_ref())
                        .map(|i| i.attempt.clone())
                })
                .map(|a| format!(" --attempt {a}"))
                .unwrap_or_default();
            format!("{corral} verdict {}{attempt}", task.id)
        }),
    ];
    let mut prompt = fill(&template, &values);
    if let Some(contract) = &gate_contract {
        prompt.push_str(&gate_contract_section(contract));
    }
    if let Some(route) = run.review.as_ref().and_then(|r| r.route.as_deref()) {
        prompt.push_str(&crate::review_contracts::contract_section(route));
    }
    prompt.push_str(&crate::lessons::lessons_prompt(ctx, task, run)?);
    prompt.push_str(&crate::comments::owner_comments(task));
    if task.completion.is_some() {
        prompt.push_str(&format!(
            "\n## Criterion evidence (worker assertions, not machine PASS)\n\n```json\n{}\n```\n",
            serde_json::to_string_pretty(&run.completion)?
        ));
    }
    Ok(prompt)
}

const GATE_POINTER: &str = "The Gate contract (review, authoritative) section below states the tier this candidate is judged against.";

fn bullet_list(items: &[String], empty: &str) -> String {
    if items.is_empty() {
        return format!(" {empty}");
    }
    items.iter().map(|i| format!("\n  - `{i}`")).collect()
}

fn gate_contract_section(contract: &crate::review_packet::ReviewGateContract) -> String {
    use crate::gate_tier::DestinationKind;
    let commands = contract
        .commands
        .iter()
        .map(|c| format!("\n      {c}"))
        .collect::<String>();
    let expected = if contract.expected_failures.is_empty() {
        " (none)".to_owned()
    } else {
        contract
            .expected_failures
            .iter()
            .map(|f| format!("\n  - `{}`: {}", f.test, f.reason))
            .collect()
    };
    let receipts: String = contract
        .receipts
        .iter()
        .map(|r| {
            let need = if r.required {
                "required for this review"
            } else {
                "not required for this review"
            };
            let fresh = if r.fresh { "fresh" } else { "not fresh" };
            match &r.attempt {
                Some(attempt) => format!(
                    "\n  - {}: {need}; {} ({fresh}, attempt {attempt})",
                    r.tier, r.status
                ),
                None => format!("\n  - {}: {need}; {}", r.tier, r.status),
            }
        })
        .collect();
    let landing = match contract.destination.kind {
        DestinationKind::PlanTarget => format!(
            "this member lands on its Plan target `{}` on the {} tier; corral runs the full tier itself at plan verify",
            contract.destination.branch, contract.landing_tier
        ),
        DestinationKind::Main | DestinationKind::Other => format!(
            "corral runs the {} tier itself, once, when this candidate lands on `{}`",
            contract.landing_tier, contract.destination.branch
        ),
    };
    format!(
        "\n## Gate contract (review, authoritative)\n\n\
         This is the only gate instruction for this review. It takes precedence over any other gate \
         instruction in this prompt, the reviewer profile guidance, lessons, skills or repository \
         guidance.\n\n\
         - Tier: {} (source: {})\n\
         - Commands, run by corral's receipted gate (reference only):{commands}\n\
         - Module scope:{}\n\
         - Declared tests:{}\n\
         - Expected failures:{expected}\n\
         - Landing: {landing}.\n\
         - Receipts:{receipts}\n\n\
         Judge the candidate against this tier and module scope. Do not run or demand another \
         tier, the full gate or whole-repository evidence. Record failures outside the module \
         scope, and the expected failures above, as non-blocking notes or follow-up proposals. \
         A failure inside the declared scope that is not an expected failure stays blocking.\n",
        contract.tier,
        contract.source,
        bullet_list(&contract.scope, "(the task declares no write scope)"),
        bullet_list(&contract.tests, "(none: the tier's own selection)"),
    )
}

pub(super) fn gate_text(ctx: &Ctx, task: &Task, run: &Run) -> String {
    let Some(g) = &run.gate else {
        return "No gate has run for this commit.".to_string();
    };
    let sha = short(&g.sha);
    let mut text = match g.result {
        GateResult::Running => format!("Gate still running at {sha}."),
        GateResult::Pass => format!("Gate passed at {sha} ({})", g.at),
        GateResult::Fail => format!(
            "Gate FAILED at {sha} ({}): `{}`",
            g.at,
            g.failed_cmd.as_deref().unwrap_or("?")
        ),
    };
    if !verification::gate_fresh(ctx, task, run, g).unwrap_or(false) {
        text.push_str(" (stale: candidate, base, task, policy or inputs changed)");
    }
    if let Some(identity) = &g.identity {
        text.push_str(&format!(
            "\n\nReceipt identity:\n```json\n{}\n```",
            serde_json::to_string_pretty(identity).unwrap_or_default()
        ));
    }
    let log = gate_paths(ctx, &task.id, &run.id).log;
    if log.is_file() {
        text.push_str(&format!("\n\nGate log: {}", log.display()));
    }
    text
}

pub(super) fn artifacts_text(ctx: &Ctx, task: &Task, run: &Run) -> String {
    let rows: Vec<String> = task
        .artifacts
        .iter()
        .filter(|a| a.run == run.id)
        .map(|a| {
            format!(
                "- [{}] {}: {}",
                a.kind.as_str(),
                a.title,
                ctx.store.dir().join(&a.path).display()
            )
        })
        .collect();
    if rows.is_empty() {
        "(none)".to_string()
    } else {
        rows.join("\n")
    }
}
