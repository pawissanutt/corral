use anyhow::{Context, Result};
use corral_core::ProfileChoice;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Ctx;
use crate::prompt::{corral_bin, render_template};

#[derive(Serialize, Deserialize)]
struct PmState {
    pane_id: String,
}

/// Ensures the workspace PM pane exists and focuses it; returns its pane id.
/// - The PM's pane id is remembered in `state/pm.json` ({"pane_id": "..."}); reuse it when `pane_get` finds it.
/// - Otherwise write `prompts/pm.md` (role: workspace PM for corral; lists the projects from config and the
///   exact corral CLI verbs with the absolute binary path: list --json, show, new -p <project> [--idea] [--body],
///   move, dispatch, artifact list; rules: plan and file tasks, never edit project code itself, ask before dispatching),
///   create a herdr workspace "corral PM" at the workspace root, run the configured pm agent with that prompt
///   (render the agent template with {prompt_file}=prompts/pm.md, {name}="corral PM", {task}="PM",
///   {worktree}=root), report metadata title "corral PM", save state/pm.json, focus it.
pub fn open_pm(ctx: &Ctx) -> Result<String> {
    if let Some(pane) = live_pm_pane(ctx)? {
        ctx.herdr.pane_focus(&pane)?;
        return Ok(pane);
    }
    let state_dir = ctx.store.state_dir();
    let state_file = state_dir.join("pm.json");

    let root = ctx.store.root().to_path_buf();
    let prompts = ctx.store.prompts_dir();
    std::fs::create_dir_all(&prompts).with_context(|| format!("creating {}", prompts.display()))?;
    let prompt_file = prompts.join("pm.md");
    std::fs::write(&prompt_file, pm_prompt(ctx)?)
        .with_context(|| format!("writing {}", prompt_file.display()))?;

    let agent = ctx.store.config.pm.agent.clone();
    let choice = ProfileChoice {
        profile: Some(agent.clone()),
        ..ProfileChoice::default()
    };
    let resolved = ctx.store.config.resolve_profile(&choice, None, None)?;
    let command = render_template(
        &resolved,
        &[
            ("prompt_file", prompt_file.display().to_string()),
            ("name", "corral PM".to_string()),
            ("task", "PM".to_string()),
            ("worktree", root.display().to_string()),
        ],
    )?;
    let created = ctx.herdr.workspace_create(&root, "corral PM", false)?;
    ctx.herdr.pane_run(&created.pane_id, &command)?;
    ctx.herdr
        .pane_report_metadata(&created.pane_id, "corral PM", &[])?;

    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("creating {}", state_dir.display()))?;
    let state = PmState {
        pane_id: created.pane_id.clone(),
    };
    std::fs::write(&state_file, serde_json::to_string(&state)?)
        .with_context(|| format!("writing {}", state_file.display()))?;
    ctx.herdr.pane_focus(&created.pane_id)?;
    ctx.store.log_event(
        None,
        "pm_opened",
        json!({ "pane": created.pane_id, "agent": agent }),
    )?;
    Ok(created.pane_id)
}

/// The PM pane remembered in `state/pm.json`, when herdr still has it.
pub(crate) fn live_pm_pane(ctx: &Ctx) -> Result<Option<String>> {
    let state_file = ctx.store.state_dir().join("pm.json");
    if let Ok(text) = std::fs::read_to_string(&state_file)
        && let Ok(state) = serde_json::from_str::<PmState>(&text)
        && ctx.herdr.pane_get(&state.pane_id)?.is_some()
    {
        return Ok(Some(state.pane_id));
    }
    Ok(None)
}

fn pm_prompt(ctx: &Ctx) -> Result<String> {
    let corral = corral_bin()?.display().to_string();
    let mut out = String::from("# corral PM\n\n");
    out.push_str(&format!(
        "You are the workspace project manager for corral at {}. You plan work and file it as corral tasks; \
         coding agents dispatched by corral do the implementation.\n\n## Projects\n\n",
        ctx.store.root().display()
    ));
    for (key, project) in &ctx.store.config.projects {
        let path = ctx.store.project_path(key)?;
        out.push_str(&format!(
            "- {key}: {} (configured path {})\n",
            path.display(),
            project.path
        ));
    }
    out.push_str("\n## corral commands (use exactly these)\n\n");
    out.push_str(&crate::skill::verbs_block(&corral));
    out.push_str(&crate::skill::awareness_block(&corral));
    out.push('\n');
    out.push_str(&crate::skill::pm_runbook(&corral));
    out.push_str(
        "\n## Project lessons\n\n\
         `corral pm brief --json` supplies project lessons and due schedules on every wake. \
         Use `corral lessons --for <task>` before filing related work. When the PM action \
         says lessons are due, `corral lessons distill --project <project>` files a separate \
         spike on `[lessons].profile`; dispatch still follows normal owner authorization. \
         Never synthesize lessons in the PM's own context. Lessons are evidence-backed \
         advice, not authority to override an owner decision or waive a check.\n",
    );
    out.push_str(
        "\n## Rules\n\n\
         - Follow pm next decision cards; escalate open-ended design and specification work to a strong-model task.\n\
         - Never edit project code yourself.\n\
         - Ask the user before dispatching any task.\n",
    );
    out.push_str(
        "- Recommend an approved plan only for concrete uncertainty, contract ambiguity or overlapping \
         write scopes, never task size alone. Planning is optional and off by default.\n\
         - Enable with `plan policy <id> required` or `[projects.<project>.review] plan_required = true`. \
         Dispatch with `dispatch <id> --plan`, or flag `design: true` / `plan_required: true` for automatic \
         read-only plan routing. Plan runs use normal slots but create no branch or worktree and run no code gate. \
         The planner submits a structured file with `plan submit`; autopilot launches a different-model reviewer. \
         Approval binds exact plan bytes and task-spec revision, including registered criterion IDs. \
         Owner-only decisions must be resolved by the owner; a plan cannot change accept=owner.\n\
         - Reviewer approval waits in Needs me for the owner. Never use `plan verdict --owner` on the \
         owner's behalf. Only `[review] plan_accept = \"auto\"` delegates final plan acceptance. \
         Owner approval creates linked Backlog children from the plan dependencies, or attaches a single-task \
         plan to its original task. Normal promotion, dependency and WIP policy controls implementation. \
         Do not install ulw-plan or invoke \
         hyperplan automatically, and never treat a touched plan file as approval.\n",
    );
    Ok(out)
}
