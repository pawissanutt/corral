use anyhow::{Context, Result, ensure};
use corral_core::records::RecordArea;
use corral_core::{Config, ProfileChoice, ResolvedProfile};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Ctx;
use crate::prompt::{corral_bin, render_template};

const STATE: RecordArea<PmState> = RecordArea::new("pm");

#[derive(Serialize, Deserialize)]
struct PmState {
    pane_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bootstrap: Option<SetupConnection>,
    /// What the PM session actually runs; absent on sessions opened before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution: Option<PmExecution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PmExecution {
    pub harness: String,
    pub model: Option<String>,
    pub vendor: Option<String>,
}

/// The live PM session, if any, with its recorded execution identity.
pub(crate) enum PmSession {
    None,
    Live(Option<PmExecution>),
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
        connection_identity: None,
        connection: None,
        candidate: None,
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
/// - The PM's pane id is remembered in the canonical `pm/state` record; reuse it when `pane_get` finds it.
/// - Otherwise write `prompts/pm.md` (role: workspace PM for corral; lists the projects from config and the
///   exact corral CLI verbs with the absolute binary path: list --json, show, new -p <project> [--idea] [--body],
///   move, dispatch, artifact list; rules: plan and file tasks, never edit project code itself,
///   obey current effective policy),
///   create a herdr workspace "corral PM" at the workspace root, run the configured pm agent with that prompt
///   (render the agent template with {prompt_file}=prompts/pm.md, {name}="corral PM", {task}="PM",
///   {worktree}=root), report metadata title "corral PM", save the PM record, focus it.
pub fn open_pm(ctx: &Ctx) -> Result<String> {
    open_pm_with(ctx, None)
}

fn open_pm_with(
    ctx: &Ctx,
    bootstrap: Option<(&SetupConnection, ResolvedProfile)>,
) -> Result<String> {
    // Validate a corral-pi PM before any policy delivery, workspace or pane.
    crate::agent::hooks::pm::preflight(ctx, bootstrap.as_ref().map(|(_, r)| r))?;
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
            let resolved = config.resolve_profile(&choice, None, None)?;
            let resolved = crate::availability::resolve_before_start(
                ctx,
                corral_core::ProfileRole::Pm,
                resolved,
                None,
            )?;
            (None, resolved)
        }
    };
    let agent = resolved.profile.clone();
    crate::agent::hooks::pm::preflight(ctx, Some(&resolved))?;
    let command = if corral_core::is_corral_pi(&resolved.harness) {
        None
    } else {
        Some(render_template(
            &resolved,
            &[
                ("prompt_file", prompt_file.display().to_string()),
                ("name", "corral PM".to_string()),
                ("task", "PM".to_string()),
                ("worktree", root.display().to_string()),
            ],
        )?)
    };
    let created = ctx.herdr.workspace_create(&root, "corral PM", false)?;
    // prepare opens the generation under its own dispatch boundary.
    let command = match command {
        Some(command) => command,
        None => crate::agent::hooks::pm::command(ctx, &created.pane_id, &resolved)?,
    };
    // A fresh non-Pi PM replaces the old host's mutation authority, not its records.
    // Fencing owns the dispatch boundary; do it before startup takes that boundary.
    if !corral_core::is_corral_pi(&resolved.harness)
        && let Some(pm) = corral_core::agent::pm_state::load_current(&ctx.store)?
        && pm.state.binding.generation > 0
    {
        crate::agent::fence_generation(
            &ctx.store,
            &corral_core::agent::ContainerRef::Pm,
            pm.state.binding.generation,
            "pm_replaced",
        )?;
    }
    let _board = ctx
        .store
        .try_lock("dispatch")?
        .context("operating policy is being updated; retry opening the PM")?;
    let policy = crate::pm_policy::snapshot(ctx)?;
    save_prompt(ctx, &policy)?;
    let state = PmState {
        pane_id: created.pane_id.clone(),
        bootstrap: connection.clone(),
        execution: Some(PmExecution {
            harness: resolved.harness.clone(),
            model: resolved.model.clone(),
            vendor: resolved.vendor.clone(),
        }),
    };
    crate::pm_policy::begin_startup(ctx, &created.pane_id, &policy)?;
    STATE.save(&ctx.store, "state", &state)?;
    let command = crate::launch_exit::arm(ctx, &command, &pm_receipt(ctx, &created.pane_id)?)?;
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

/// The live PM session and what it actually runs, from the canonical PM record.
pub(crate) fn pm_session(ctx: &Ctx) -> Result<PmSession> {
    if let Some(state) = STATE.load(&ctx.store, "state")?
        && ctx.herdr.pane_get(&state.pane_id)?.is_some()
        && live(ctx, &state.pane_id)?
    {
        return Ok(PmSession::Live(state.execution));
    }
    Ok(PmSession::None)
}

fn pm_receipt(ctx: &Ctx, pane: &str) -> Result<std::path::PathBuf> {
    crate::launch_exit::receipt(ctx, "PM", &pane.replace(':', "_"))
}

/// The owner's pane adopted by `corral pi` is the PM's only while its host is verifiably alive,
/// and is never closed; any other PM pane is live until its agent returns to the shell.
fn live(ctx: &Ctx, pane: &str) -> Result<bool> {
    match crate::agent::pi::adopted_alive(ctx, pane)? {
        Some(alive) => Ok(alive),
        None => Ok(!exited(ctx, pane)?),
    }
}

/// `corral pi`: the owner's `pane` becomes the PM pane. Under `open_pm`'s pm-policy and board
/// boundaries it refuses a live non-Pi PM, saves the prompt, runs `start` (the checks, the
/// generation and its bootstrap) and records the pane with a confirmed startup: the host launches
/// in-process, so nothing is typed into the pane.
pub(crate) fn adopt_pm<T>(
    ctx: &Ctx,
    pane: &str,
    resolved: &ResolvedProfile,
    start: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let _delivery = ctx
        .store
        .try_lock("pm-policy")?
        .context("PM policy delivery is in progress; retry `corral pi`")?;
    let board = || -> Result<_> {
        ctx.store
            .try_lock("dispatch")?
            .context("operating policy is being updated; retry `corral pi`")
    };
    if let Some(state) = STATE.load(&ctx.store, "state")?
        && state.pane_id != pane
        && !state
            .execution
            .is_some_and(|e| corral_core::is_corral_pi(&e.harness))
        && live_pm_pane(ctx)?.is_some()
    {
        return Err(crate::agent::pi::refuse(
            "pm_alive",
            format!(
                "a non-corral-pi PM session is open in pane {}; close it before `corral pi`",
                state.pane_id
            ),
        ));
    }
    {
        let _board = board()?;
        save_prompt(ctx, &crate::pm_policy::snapshot(ctx)?)?;
    }
    let started = start()?;
    let _board = board()?;
    let policy = crate::pm_policy::snapshot(ctx)?;
    save_prompt(ctx, &policy)?;
    let state = PmState {
        pane_id: pane.into(),
        bootstrap: None,
        execution: Some(PmExecution {
            harness: resolved.harness.clone(),
            model: resolved.model.clone(),
            vendor: resolved.vendor.clone(),
        }),
    };
    crate::pm_policy::begin_startup(ctx, pane, &policy)?;
    STATE.save(&ctx.store, "state", &state)?;
    crate::pm_policy::confirm_startup(ctx, pane)?;
    drop(_board);
    corral_core::inbox::Inbox::new(&ctx.store, corral_core::inbox::Recipient::Pm)?
        .record_pm_launch()?;
    ctx.store.log_event(
        None,
        "pm_opened",
        json!({ "pane": pane, "agent": resolved.profile, "adopted": true }),
    )?;
    Ok(started)
}

/// A pane whose PM agent returned to its shell is only a shell: report why once, close it and
/// forget it, so the next open launches a fresh PM instead of typing into the shell.
fn exited(ctx: &Ctx, pane: &str) -> Result<bool> {
    let receipt = pm_receipt(ctx, pane)?;
    if !crate::launch_exit::exited(&receipt) {
        return Ok(false);
    }
    let reason = crate::launch_exit::reason(ctx, "The PM agent", Some(pane));
    ctx.herdr.pane_close(pane)?;
    STATE.delete(&ctx.store, "state")?;
    std::fs::remove_file(&receipt).with_context(|| format!("removing {}", receipt.display()))?;
    crate::messages::report(None, "pm", reason.clone());
    ctx.store
        .log_event(None, "pm_exited", json!({ "pane": pane, "reason": reason }))?;
    Ok(true)
}

/// Remembered PM ownership is protected even when its liveness is uncertain.
pub(crate) fn recorded_pane(ctx: &Ctx) -> Result<Option<String>> {
    Ok(STATE.load(&ctx.store, "state")?.map(|state| state.pane_id))
}

/// The PM pane remembered in the canonical PM record, when herdr still has it.
pub(crate) fn live_pm_pane(ctx: &Ctx) -> Result<Option<String>> {
    if let Some(state) = STATE.load(&ctx.store, "state")?
        && ctx.herdr.pane_get(&state.pane_id)?.is_some()
        && live(ctx, &state.pane_id)?
    {
        return Ok(Some(state.pane_id));
    }
    Ok(None)
}

pub(crate) fn save_prompt(
    ctx: &Ctx,
    policy: &crate::pm_policy::PolicySnapshot,
) -> Result<std::path::PathBuf> {
    let path = prompt_file(ctx)?;
    crate::pm_policy::write_atomic(&path, pm_prompt(ctx, policy)?.as_bytes())?;
    Ok(path)
}

pub(crate) fn prompt_file(ctx: &Ctx) -> Result<std::path::PathBuf> {
    Ok(ctx
        .store
        .scratch_spool(corral_core::backend::ScratchKind::Prompt, "pm")?
        .join("pm.md"))
}

/// The Pi role assembles its installed instructions with this bounded data snapshot.
/// Project guidance is loaded separately by the host under its recorded trust settings.
pub(crate) fn save_native_context(ctx: &Ctx) -> Result<std::path::PathBuf> {
    let projects = crate::agent::hooks::pm::projects(&ctx.store)?
        .into_iter()
        .map(|name| Ok((name.clone(), ctx.store.project_path(&name)?)))
        .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
    let bytes = serde_json::to_vec_pretty(&json!({
        "version": 1,
        "workspace": ctx.store.root(),
        "projects": projects,
        "policy": crate::pm_policy::native_summary(&crate::pm_policy::snapshot(ctx)?),
    }))?;
    ensure!(
        bytes.len() <= 64 * 1024,
        "Pi PM operating context exceeds 64 KiB"
    );
    let path = ctx
        .store
        .scratch_spool(corral_core::backend::ScratchKind::Prompt, "pm")?
        .join("pm-pi.json");
    crate::pm_policy::write_atomic(&path, &bytes)?;
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
         - Start and accept work only under current effective policy or an explicit owner action. \
         You act as the owner for it: run `plan accept`, `plan add-member`, `plan move-member`, \
         question answers of any kind, decision receipts and `--owner-ok` yourself. Gates, \
         independent review and the approve verdict for landing still apply.\n",
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

#[cfg(test)]
mod tests {
    #[test]
    fn pm_prompt_and_installed_skill_share_the_complete_watch_runbook() {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let ctx = crate::Ctx::from_store(corral_core::Store::init(root.path()).unwrap());
        let policy = crate::pm_policy::snapshot(&ctx).unwrap();
        let prompt_path = super::save_prompt(&ctx, &policy).unwrap();
        let prompt = std::fs::read_to_string(prompt_path).unwrap();
        let bin = crate::prompt::corral_bin().unwrap().display().to_string();
        let runbook = crate::skill::pm_runbook("corral");
        assert!(prompt.contains(crate::skill::pm_runbook(&bin).trim()));
        std::fs::create_dir(home.path().join(".agents")).unwrap();
        crate::ensure_skills(home.path(), "agents").unwrap();
        let installed =
            std::fs::read_to_string(home.path().join(".agents/skills/corral/SKILL.md")).unwrap();
        assert!(installed.contains(runbook.trim()));
        assert_eq!(installed, crate::role_skill_markdown("corral").unwrap());
    }
}
