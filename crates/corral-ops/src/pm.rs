use anyhow::{Context, Result, ensure};
use corral_core::{Config, ProfileChoice, ResolvedProfile};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Ctx;
use crate::prompt::{corral_bin, render_template};

#[derive(Serialize, Deserialize)]
struct PmState {
    pane_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bootstrap: Option<SetupConnection>,
}

/// One owner-selected existing harness connection, not a catalog recommendation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupConnection {
    pub harness: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// Resolve bootstrap independently of the default profile or PM assignment.
pub fn setup_connection(config: &Config, connection: &SetupConnection) -> Result<ResolvedProfile> {
    let (command, flags) = config.harness(&connection.harness)?;
    let model_args = flags.render(
        &connection.harness,
        connection.model.as_deref(),
        connection.effort.as_deref(),
    )?;
    ensure!(
        model_args.is_empty() || command.contains("{model_args}"),
        "bootstrap harness needs a model_args placeholder for model/effort settings"
    );
    Ok(ResolvedProfile {
        profile: "setup-connection".into(),
        harness: connection.harness.clone(),
        command,
        model: connection.model.clone(),
        vendor: connection
            .model
            .as_deref()
            .and_then(corral_core::infer_vendor),
        effort: connection
            .effort
            .as_ref()
            .map(|value| value.trim().to_ascii_lowercase()),
        model_args,
    })
}

/// Start or reuse the setup assistant only after the owner confirms their connection.
/// Launch is not an authentication probe or approval of future profile mappings.
pub fn open_setup_pm(ctx: &Ctx, connection: &SetupConnection) -> Result<String> {
    let config = ctx.store.config_layers()?.config()?;
    let resolved = setup_connection(&config, connection)?;
    let current = ctx.store.profile_setup()?;
    crate::setup_begin(&ctx.store, current.revision)?;
    open_pm_with(ctx, Some((connection, resolved)))
}

/// Ensures the workspace PM pane exists and focuses it; returns its pane id.
/// - The PM's pane id is remembered in `state/pm.json` ({"pane_id": "..."}); reuse it when `pane_get` finds it.
/// - Otherwise write `prompts/pm.md` (role: workspace PM for corral; lists the projects from config and the
///   exact corral CLI verbs with the absolute binary path: list --json, show, new -p <project> [--idea] [--body],
///   move, dispatch, artifact list; rules: plan and file tasks, never edit project code itself,
///   obey current effective policy),
///   create a herdr workspace "corral PM" at the workspace root, run the configured pm agent with that prompt
///   (render the agent template with {prompt_file}=prompts/pm.md, {name}="corral PM", {task}="PM",
///   {worktree}=root), report metadata title "corral PM", save state/pm.json, focus it.
pub fn open_pm(ctx: &Ctx) -> Result<String> {
    open_pm_with(ctx, None)
}

fn open_pm_with(
    ctx: &Ctx,
    bootstrap: Option<(&SetupConnection, ResolvedProfile)>,
) -> Result<String> {
    let _delivery = ctx
        .store
        .try_lock("pm-policy")?
        .context("PM policy delivery is in progress; retry opening the PM")?;
    let _board = ctx
        .store
        .try_lock("dispatch")?
        .context("operating policy is being updated; retry opening the PM")?;
    let policy = crate::pm_policy::snapshot(ctx)?;
    if let Some(pane) = live_pm_pane(ctx)? {
        crate::pm_policy::deliver(ctx, &pane, &policy)?;
        drop(_board);
        ctx.herdr.pane_focus(&pane)?;
        return Ok(pane);
    }
    let prompt_file = save_prompt(ctx, &policy)?;
    drop(_board);
    let state_dir = ctx.store.state_dir();
    let state_file = state_dir.join("pm.json");

    let root = ctx.store.root().to_path_buf();
    let config = ctx.store.config_layers()?.config()?;
    let (connection, resolved) = match bootstrap {
        Some((connection, resolved)) => (Some(connection.clone()), resolved),
        None => {
            let choice = ProfileChoice {
                profile: Some(config.pm.agent.clone()),
                ..ProfileChoice::default()
            };
            config.check_profile_eligibility(
                &config.pm.agent,
                corral_core::ProfileRole::Pm,
                None,
                false,
            )?;
            (None, config.resolve_profile(&choice, None, None)?)
        }
    };
    let agent = resolved.profile.clone();
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
    let _board = ctx
        .store
        .try_lock("dispatch")?
        .context("operating policy is being updated; retry opening the PM")?;
    let policy = crate::pm_policy::snapshot(ctx)?;
    save_prompt(ctx, &policy)?;
    let state = PmState {
        pane_id: created.pane_id.clone(),
        bootstrap: connection.clone(),
    };
    crate::pm_policy::begin_startup(ctx, &created.pane_id, &policy)?;
    crate::pm_policy::write_atomic(&state_file, &serde_json::to_vec(&state)?)?;
    ctx.herdr
        .pane_run(&created.pane_id, &format!("export CORRAL_PM=1; {command}"))?;
    crate::pm_policy::confirm_startup(ctx, &created.pane_id)?;
    // Release the board boundary before metadata, focus and event accounting.
    drop(_board);
    corral_core::inbox::Inbox::new(&ctx.store, corral_core::inbox::Recipient::Pm)?
        .record_pm_launch()?;
    ctx.herdr
        .pane_report_metadata(&created.pane_id, "corral PM", &[])?;
    ctx.herdr.pane_focus(&created.pane_id)?;
    ctx.store.log_event(
        None,
        "pm_opened",
        json!({ "pane": created.pane_id, "agent": agent, "bootstrap": connection }),
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

pub(crate) fn save_prompt(
    ctx: &Ctx,
    policy: &crate::pm_policy::PolicySnapshot,
) -> Result<std::path::PathBuf> {
    let path = ctx.store.prompts_dir().join("pm.md");
    crate::pm_policy::write_atomic(&path, pm_prompt(ctx, policy)?.as_bytes())?;
    Ok(path)
}

fn pm_prompt(ctx: &Ctx, policy: &crate::pm_policy::PolicySnapshot) -> Result<String> {
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
    out.push_str("\n\n## Setup state at launch (reload on every wake)\n\n```json\n");
    out.push_str(&serde_json::to_string_pretty(&crate::setup_status(
        &ctx.store,
    )?)?);
    out.push_str("\n```\n");
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
         - Start and accept work only under current effective policy or an explicit owner action.\n",
    );
    out.push_str(
        "- Recommend an approved plan only for concrete uncertainty, contract ambiguity or overlapping \
         write scopes, never task size alone. Planning is optional and off by default.\n\
         - Enable with `plan policy <id> required` or `[projects.<project>.review] plan_required = true`. \
         Dispatch with `dispatch <id> --plan`, or flag `design: true` / `plan_required: true` for automatic \
         read-only plan routing. Plan runs use normal slots but create no branch or worktree and run no code gate. \
         The planner submits a structured file with `plan submit`; autopilot launches a reviewer under the effective review policy. \
         Approval binds exact plan bytes and task-spec revision, including registered criterion IDs. \
         Owner-only decisions must be resolved by the owner; a plan cannot change accept=owner.\n\
         - Never use `plan verdict --owner` on the owner's behalf. Final plan acceptance requires \
         current scoped plan-acceptance delegation without owner exceptions, or explicit owner approval. \
         Owner approval creates linked Backlog children from the plan dependencies, or attaches a single-task \
         plan to its original task. Normal promotion, dependency and WIP policy controls implementation. \
         Do not install ulw-plan or invoke \
         hyperplan automatically, and never treat a touched plan file as approval.\n",
    );
    out.push_str(&crate::pm_policy::instructions(policy)?);
    Ok(out)
}
