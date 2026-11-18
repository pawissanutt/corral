//! Native preparation of assistant-led local work. One bound owner selection runs the same
//! prepare → preview → commit sequence as the owner shell; nothing here grants authority by
//! itself. Scope comes from the declared plan execution scope, else the scratch's changed
//! paths; a session without a workflow enters from its latest genuine pane request.
use anyhow::{Context as _, Result, ensure};
use corral_core::{
    StepState, Store,
    assistant::{AssistantContainer, OwnerInputSource},
    assistant_workflow::{ExecutionScope, WorkflowPhase},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

use super::super::publication::load_artifact;
use super::{
    Goal, Request,
    context::{Brief, Context, ContextMode},
    lifecycle::Instruction,
    transfer,
};
use crate::Ctx;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalChoice {
    pub project: String,
    pub goal: Goal,
    pub context: ContextMode,
    pub save: bool,
    pub title: String,
    /// Only for a session without a workflow: the pane request it enters from.
    pub entry: Option<String>,
    /// Conversation context only: the visible branch range through a named checkpoint.
    #[serde(default)]
    pub history: Option<super::context::HistorySelection>,
}

/// Refuse before consuming the selection.
pub(super) fn precheck(local: &LocalChoice) -> Result<()> {
    ensure!(
        (local.context == ContextMode::Conversation) == local.history.is_some(),
        "context_incomplete: conversation context needs its selected visible range and checkpoint"
    );
    ensure!(
        !local.title.trim().is_empty() && local.title.len() <= 256,
        "handoff_title: requires a bounded title"
    );
    Ok(())
}

fn bounded(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn changed_paths(path: &Path, base: &str) -> Result<Vec<String>> {
    let lines = |args: &[&str]| -> Result<Vec<String>> {
        Ok(String::from_utf8(super::git::git(path, args, None)?)?
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect())
    };
    let mut paths = lines(&["diff", "--name-only", "--end-of-options", base])?;
    paths.extend(lines(&["ls-files", "--others", "--exclude-standard"])?);
    paths.sort();
    paths.dedup();
    if paths.len() > 64 {
        paths = paths
            .iter()
            .filter_map(|p| p.split('/').next().map(String::from))
            .collect();
        paths.dedup();
    }
    ensure!(
        paths.len() <= 64,
        "handoff_scope: more than 64 changed top-level paths; declare a plan scope"
    );
    Ok(paths)
}

fn brief(c: &AssistantContainer, goal: &str) -> Brief {
    let (mut completed, mut remaining) = (Vec::new(), Vec::new());
    for step in c
        .state
        .todo
        .iter()
        .flat_map(|t| &t.phases)
        .flat_map(|p| &p.steps)
    {
        match &step.status {
            StepState::Done => completed.push(step.label.clone()),
            StepState::Pending | StepState::InProgress => remaining.push(step.label.clone()),
            StepState::Dropped { .. } => {}
        }
    }
    completed.truncate(32);
    remaining.truncate(32);
    let next_action = remaining
        .first()
        .cloned()
        .unwrap_or_else(|| "Continue the goal from the preserved work".into());
    Brief {
        goal: bounded(goal, 4096),
        constraints: Vec::new(),
        decisions: Vec::new(),
        completed,
        remaining,
        findings: Vec::new(),
        failed_approaches: Vec::new(),
        assumptions: Vec::new(),
        evidence: Vec::new(),
        questions: Vec::new(),
        mandatory_blockers: Vec::new(),
        next_action,
    }
}

pub(super) fn run(
    ctx: &Ctx,
    session: &str,
    local: &LocalChoice,
    shown: &str,
    instruction: Instruction,
) -> Result<Value> {
    precheck(local)?;
    let before = ctx.store.get_assistant_container(session)?;
    let seen = super::native_view::view(ctx, session, &before)?;
    ensure!(seen.digest == shown, super::native_view::STALE);
    let assistant = ctx.store.get_assistant(session)?;
    let source = source_scratch(ctx, &before, &assistant, &local.project)?;
    let changed = source
        .map(|s| changed_paths(&s.path, &s.base))
        .transpose()?
        .unwrap_or_default();
    if let Some(entry) = &local.entry {
        // No plan exists yet: refuse before entering a workflow, never leave one behind.
        ensure!(
            !changed.is_empty(),
            "handoff_scope: no declared plan scope or changed scratch paths"
        );
        ensure!(
            super::super::boundary::activities(&ctx.store, session)?.is_empty(),
            "handoff_busy: source effects must settle before preparation"
        );
        ctx.store.settle_assistant_container(session, |c, _| {
            ensure!(
                c.workflow.is_none(),
                "handoff_source: existing workflow must be used without changing its mode"
            );
            super::super::prepare_entry(&ctx.store, session, c, entry)?;
            c.workflow
                .as_mut()
                .context("workflow missing")?
                .current
                .phase = WorkflowPhase::Ordinary;
            Ok(())
        })?;
    }
    let c = ctx.store.get_assistant_container(session)?;
    if local.entry.is_none() {
        // The pin below comes from this same read, so prepare refuses any later change.
        ensure!(
            super::native_view::view(ctx, session, &c)?.digest == shown,
            super::native_view::STALE
        );
    }
    let w = super::super::current(&c)?;
    let expected = super::super::Expected {
        workflow: w.id.clone(),
        state_revision: w.state_revision,
        authority_epoch: w.authority_epoch,
        plan_revision: w.current_plan.as_ref().map(|p| p.revision),
    };
    let planned = match w
        .current_plan
        .as_ref()
        .and_then(|p| p.execution_scope.as_ref())
    {
        Some(artifact) => {
            let scope: ExecutionScope =
                serde_json::from_slice(&load_artifact(&ctx.store, artifact)?)
                    .context("handoff_scope: unreadable declared execution scope")?;
            (scope.project == local.project).then_some(scope.paths)
        }
        None => None,
    };
    let scope = planned.filter(|p| !p.is_empty()).unwrap_or(changed);
    ensure!(
        !scope.is_empty(),
        "handoff_scope: no declared plan scope or changed scratch paths"
    );
    let goal = String::from_utf8(load_artifact(&ctx.store, &w.original_request)?)?;
    let request = Request {
        project: local.project.clone(),
        scope,
        goal: local.goal,
        context: Context {
            mode: local.context,
            brief: brief(&c, &goal),
            history: local.history.clone(),
        },
    };
    let prepared = super::prepare(
        &ctx.store,
        session,
        &c,
        &expected,
        request,
        source.map(|s| (s.path.as_path(), s.base.as_str(), s.branch.as_str())),
    )?;
    // A source edit while the snapshot was captured refuses before anything is committed.
    ensure!(
        super::native_view::sources(ctx, session)? == seen.sources,
        super::native_view::STALE
    );
    let choice = transfer::preview(ctx, prepared.id(), &local.title, None, local.save)?;
    let task = transfer::commit(ctx, &choice, &choice.digest()?, instruction)?;
    Ok(
        json!({"action":"local","package":prepared.id(),"task":task.id,
        "status":task.status,"held":task.dispatch_hold}),
    )
}

/// The one source scratch for `project`: the only one, else the scratch a validated
/// Implement here grant names. Several without a grant refuse rather than guess.
fn source_scratch<'a>(
    ctx: &Ctx,
    container: &AssistantContainer,
    assistant: &'a corral_core::assistant::AssistantSession,
    project: &str,
) -> Result<Option<&'a corral_core::assistant::AssistantScratch>> {
    let sources: Vec<_> = assistant
        .scratch
        .iter()
        .filter(|s| s.project == project)
        .collect();
    if sources.len() <= 1 {
        return Ok(sources.first().copied());
    }
    let granted = match container.workflow.as_ref().map(|i| &i.current) {
        Some(w) if w.phase == WorkflowPhase::Executing => {
            Some(super::native_implement::grant(&ctx.store, w)?.scratch)
        }
        _ => None,
    };
    let chosen = granted.and_then(|slug| sources.iter().copied().find(|s| s.slug == slug));
    ensure!(
        chosen.is_some(),
        "handoff_source: choose one explicit source scratch"
    );
    Ok(chosen)
}

/// Read-only: the exact context, history range, exclusions and scope this choice would
/// package. Prepares, reserves and commits nothing.
pub fn preview(ctx: &Ctx, session: &str, local: &LocalChoice) -> Result<Value> {
    precheck(local)?;
    let c = ctx.store.get_assistant_container(session)?;
    let assistant = ctx.store.get_assistant(session)?;
    let source = source_scratch(ctx, &c, &assistant, &local.project)?;
    let changed = source
        .map(|s| changed_paths(&s.path, &s.base))
        .transpose()?
        .unwrap_or_default();
    let current = c.workflow.as_ref().map(|i| &i.current);
    let (goal, planned) = match &c.workflow {
        Some(index) => {
            let w = &index.current;
            let planned = match w
                .current_plan
                .as_ref()
                .and_then(|p| p.execution_scope.as_ref())
            {
                Some(artifact) => {
                    let scope: ExecutionScope =
                        serde_json::from_slice(&load_artifact(&ctx.store, artifact)?)?;
                    (scope.project == local.project).then_some(scope.paths)
                }
                None => None,
            };
            (
                String::from_utf8(load_artifact(&ctx.store, &w.original_request)?)?,
                planned,
            )
        }
        None => {
            let entry = local.entry.as_deref().context("no assistant workflow")?;
            let input = c.owner_input(entry).context("entry request missing")?;
            (input.text.clone(), None)
        }
    };
    let scope = planned.filter(|p| !p.is_empty()).unwrap_or(changed);
    let context = Context {
        mode: local.context,
        brief: brief(&c, &goal),
        history: local.history.clone(),
    };
    // The same operational identity the committed package's prompt starts with; only the
    // bundle hash is unknown until the snapshot is captured at selection.
    let expected = super::super::Expected {
        workflow: current.map_or_else(|| "(entered at selection)".into(), |w| w.id.clone()),
        state_revision: current.map_or(0, |w| w.state_revision),
        authority_epoch: current.map_or(0, |w| w.authority_epoch),
        plan_revision: current.and_then(|w| w.current_plan.as_ref().map(|p| p.revision)),
    };
    let code = match source {
        Some(s) => {
            super::preview::code_line(&super::git::scan(&s.path, &s.base, &s.branch)?.state, None)
        }
        None => "no code checkpoint".into(),
    };
    let facts = super::preview::Facts {
        assistant: session,
        expected: &expected,
        project: &local.project,
        scope: &scope,
        goal: local.goal,
        plan: current.and_then(|w| w.current_plan.as_ref()),
        code,
        reviews: current.map_or(0, |w| w.reviews.len()),
        execution: current.and_then(|w| w.execution.as_ref()),
    }
    .render();
    let p = super::preview::with_facts(context.preview()?, &facts)?;
    Ok(
        json!({"project": local.project, "scope": scope, "goal": local.goal,
        "plan": current.and_then(|w| w.current_plan.as_ref()).map(|p| json!({
            "revision": p.revision, "digest": p.artifact.blob.hash(), "stage": p.stage})),
        "source": source.map(|s| json!({"slug": s.slug, "branch": s.branch, "base": s.base})),
        "mode": local.context, "native_fork": p.native_fork, "semantics": p.semantics,
        "checkpoint": p.checkpoint, "range": p.range, "excluded": p.excluded,
        "archived_messages": p.archived_messages, "archived_bytes": p.archived_bytes,
        "loaded_messages": p.loaded_messages, "loaded_bytes": p.loaded_bytes,
        "prompt": bounded(&p.prompt, 16 * 1024), "prompt_bytes": p.prompt.len()}),
    )
}

/// What the host may offer for local work. `Null` without a project or an entry request.
pub fn offer(
    store: &Store,
    container: &AssistantContainer,
    project: Option<String>,
) -> Result<Value> {
    let Some(project) = project else {
        return Ok(Value::Null);
    };
    let (entry, goal) = match &container.workflow {
        Some(index) => (
            None,
            String::from_utf8(load_artifact(store, &index.current.original_request)?)?,
        ),
        None => {
            let generation = container.state.binding.generation;
            let Some(input) = container.owner_inputs.iter().rev().find(|i| {
                i.source == OwnerInputSource::Pane && i.generation == generation && !i.truncated
            }) else {
                return Ok(Value::Null);
            };
            (Some(input.id.clone()), input.text.clone())
        }
    };
    let first = goal
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Assistant work");
    Ok(
        json!({"project":project,"entry":entry,"goal_text":bounded(&goal, 512),
        "title":bounded(first, 80)}),
    )
}
