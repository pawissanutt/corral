//! Read-only agent projections, including canonical usage from bound assistant transcripts.
//! No reader probes panes, starts hosts, or accepts a caller-supplied filesystem path.
use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use corral_core::agent::{AgentGeneration, ContainerRef, HostHealth, pm_state};
use corral_core::backend::HeadQuery;
use corral_core::{AgentHostHead, GateResult, RunHead, TaskHead, UsageTotals};
use corral_protocol::assistant::{EntryCard, EntryKind};
use corral_protocol::detail::DetailBody;
use corral_protocol::detail::session::{SessionItem, SessionMessage, SessionPage};
use corral_protocol::rows::{
    AgentRow, AgentStatus, AssistantLocation, AssistantRow, AssistantState,
};

use super::{revision, short};
use crate::Ctx;

#[path = "agent_transcript.rs"]
mod transcript;

struct Source {
    row: AgentRow,
    provider_id: Option<String>,
    transcript: Option<PathBuf>,
    replyable: bool,
    gate: Option<(String, String)>,
}

fn blank(id: String, role: &str, title: &str) -> AgentRow {
    AgentRow {
        id,
        role: role.into(),
        task: None,
        run: None,
        title: short(title, 120),
        profile: None,
        runtime: None,
        machine: None,
        pane: None,
        status: AgentStatus::Unavailable,
        now: String::new(),
        since: None,
        tail: Vec::new(),
        cost_usd: None,
        cost_basis: None,
        tokens: None,
        rev: 0,
    }
}

fn basis(value: corral_core::CostBasis) -> corral_protocol::board::CostBasis {
    match value {
        corral_core::CostBasis::Reported => corral_protocol::board::CostBasis::Reported,
        corral_core::CostBasis::Estimated => corral_protocol::board::CostBasis::Estimated,
        corral_core::CostBasis::Mixed => corral_protocol::board::CostBasis::Mixed,
    }
}

fn usage(row: &mut AgentRow, value: Option<&UsageTotals>) {
    if let Some(value) = value {
        row.cost_usd = value.cost_usd;
        row.cost_basis = value.cost_basis.map(basis);
        row.tokens = Some(
            value
                .input
                .saturating_add(value.output)
                .saturating_add(value.cache_read)
                .saturating_add(value.cache_write),
        );
    }
}

fn progress(value: Option<&corral_core::RunProgress>) -> String {
    value
        .into_iter()
        .flat_map(|p| &p.phases)
        .flat_map(|p| &p.steps)
        .find(|step| matches!(step.status, corral_core::StepState::InProgress))
        .map(|step| short(&step.label, 60))
        .unwrap_or_default()
}

fn health(row: &mut AgentRow, host: Option<&AgentHostHead>, ended: bool) {
    if ended {
        row.status = AgentStatus::Ended;
    } else if let Some(host) = host {
        row.runtime = Some("corral-pi".into());
        row.status = match host.binding.health {
            HostHealth::Healthy if host.question_hold.is_some() => AgentStatus::Asking,
            HostHealth::Healthy => AgentStatus::Live,
            HostHealth::Unbound => AgentStatus::Waiting,
            HostHealth::Lost | HostHealth::Fenced => AgentStatus::Unavailable,
        };
        if row.now.is_empty() {
            row.now = progress(host.todo.as_ref());
        }
        if let Some(hold) = &host.question_hold {
            row.now = short(hold, 60);
        }
    }
}

fn native(
    head: &TaskHead,
    container: &ContainerRef,
    host: Option<&AgentHostHead>,
) -> Result<Option<AgentGeneration>> {
    match host.filter(|host| host.binding.generation != 0) {
        Some(host) => head.agent_generation(container, host.binding.generation),
        None => Ok(None),
    }
}

fn source(
    row: AgentRow,
    generation: Option<AgentGeneration>,
    fallback: Option<PathBuf>,
    replyable: bool,
) -> Source {
    let bound = generation.and_then(|generation| generation.bound);
    Source {
        row,
        provider_id: bound.as_ref().map(|bound| bound.session_id.clone()),
        transcript: bound.map(|bound| bound.session_file).or(fallback),
        replyable,
        gate: None,
    }
}

fn run_source(head: &TaskHead, parent: &RunHead, run: &RunHead, reviewer: bool) -> Result<Source> {
    let role = if reviewer { "reviewer" } else { "worker" };
    let id = if reviewer {
        format!("{}/review/{}/{}", head.id, parent.id, run.id)
    } else {
        format!("{}/run/{}", head.id, run.id)
    };
    let mut row = blank(id, role, &head.title);
    row.task = Some(head.id.clone());
    row.run = Some(run.id.clone());
    row.profile = Some(run.profile.as_ref().unwrap_or(&run.agent).clone());
    row.runtime = run
        .harness
        .clone()
        .or_else(|| run.execution.as_ref().map(|e| e.resolved.harness.clone()));
    row.pane = run.herdr.as_ref().map(|pane| pane.pane_id.clone());
    row.since = Some(run.started.to_string());
    row.now = progress(run.progress.as_ref());
    if let Some(text) = &run.blocked_excerpt {
        row.now = short(text, 60);
    }
    let ended = run.ended.is_some() || run.outcome.is_some();
    row.status = if ended {
        AgentStatus::Ended
    } else if run.blocked_excerpt.is_some() {
        AgentStatus::Asking
    } else if run.idle_since.is_some() {
        AgentStatus::Idle
    } else if reviewer {
        AgentStatus::Reviewing
    } else {
        AgentStatus::Working
    };
    health(&mut row, run.agent_host.as_deref(), ended);
    if let Some(text) = run.summary.as_ref().filter(|_| ended) {
        row.now = short(text, 60);
    }
    usage(&mut row, run.usage.as_ref());
    let container = if reviewer {
        ContainerRef::Reviewer {
            task: head.id.clone(),
            run: parent.id.clone(),
            review: run.id.clone(),
        }
    } else {
        ContainerRef::Worker {
            task: head.id.clone(),
            run: run.id.clone(),
        }
    };
    let generation = native(head, &container, run.agent_host.as_deref())?;
    let replyable = !reviewer
        && !ended
        && run.pending_reply.is_none()
        && (run.herdr.is_some() || run.delivery == corral_core::inbox::DeliveryMode::Inbox);
    let mut selected = source(
        row,
        generation,
        run.session.as_ref().and_then(|s| s.transcript.clone()),
        replyable,
    );
    if selected.provider_id.is_none() {
        selected.provider_id = run.session.as_ref().and_then(|s| s.id.clone());
    }
    Ok(selected)
}

fn task_sources(head: &TaskHead, output: &mut Vec<Source>) -> Result<()> {
    if let Some(run) = head.current_run.as_deref() {
        output.push(run_source(head, run, run, false)?);
        if let Some(child) = run.current_child.as_deref() {
            output.push(run_source(head, run, child, true)?);
        }
        if let Some(gate) = &run.gate {
            let mut row = blank(format!("{}/gate/{}", head.id, run.id), "gate", &head.title);
            row.task = Some(head.id.clone());
            row.run = Some(run.id.clone());
            row.pane = gate.pane.clone();
            row.status = if gate.result == GateResult::Running && !gate.process_lost {
                AgentStatus::Gating
            } else if gate.process_lost {
                AgentStatus::Unavailable
            } else {
                AgentStatus::Ended
            };
            row.now = short(
                gate.failed_cmd.as_deref().unwrap_or(gate.result.as_str()),
                60,
            );
            row.since = Some(gate.at.to_string());
            output.push(Source {
                row,
                provider_id: None,
                transcript: None,
                replyable: false,
                gate: Some((head.id.clone(), run.id.clone())),
            });
        }
    }
    if let Some(run) = head.last_plan_run.as_deref() {
        let mut row = blank(
            format!("{}/plan/{}", head.id, run.id),
            "planner",
            &head.title,
        );
        row.task = Some(head.id.clone());
        row.run = Some(run.id.clone());
        row.profile = Some(run.profile.clone());
        row.runtime = Some(run.harness.clone());
        row.pane = run.herdr.as_ref().map(|pane| pane.pane_id.clone());
        row.since = Some(run.started.to_string());
        row.status = if run.ended.is_some() {
            AgentStatus::Ended
        } else {
            AgentStatus::Working
        };
        health(&mut row, run.agent_host.as_deref(), run.ended.is_some());
        let container = ContainerRef::Planner {
            task: head.id.clone(),
            run: run.id.clone(),
        };
        let generation = native(head, &container, run.agent_host.as_deref())?;
        // task.reply chooses the worker first; never claim it addresses this planner then.
        let worker_live = head
            .current_run
            .as_ref()
            .is_some_and(|r| r.outcome.is_none() && r.herdr.is_some());
        let replyable = !worker_live
            && run.ended.is_none()
            && run.pending_reply.is_none()
            && row.pane.is_some();
        output.push(source(row, generation, None, replyable));
    }
    if let Some(plan) = head.last_plan.as_deref()
        && let Some(review) = &plan.current_review
    {
        let mut row = blank(
            format!("{}/plan-review/{}/{}", head.id, plan.id, review.id),
            "plan_reviewer",
            &head.title,
        );
        row.task = Some(head.id.clone());
        row.run = Some(review.id.clone());
        row.profile = Some(review.reviewer.clone());
        row.pane = review.herdr.as_ref().map(|pane| pane.pane_id.clone());
        row.since = Some(review.at.to_string());
        let ended = review.verdict.is_some() || review.cancelled;
        row.status = if ended {
            AgentStatus::Ended
        } else {
            AgentStatus::Reviewing
        };
        row.now = short(review.notes.as_deref().unwrap_or(""), 60);
        health(&mut row, review.agent_host.as_deref(), ended);
        let container = ContainerRef::PlanReviewer {
            task: head.id.clone(),
            plan: plan.id.clone(),
            review: review.id.clone(),
        };
        output.push(source(
            row,
            native(head, &container, review.agent_host.as_deref())?,
            None,
            false,
        ));
    }
    Ok(())
}

fn pm_source(ctx: &Ctx) -> Result<Option<Source>> {
    let Some(pm) = pm_state::load_current(&ctx.store)? else {
        return Ok(None);
    };
    let mut row = blank("pm".into(), "pm", "Project manager");
    row.pane = (!pm.pane.is_empty()).then_some(pm.pane);
    row.now = progress(pm.state.todo.as_ref());
    let host = AgentHostHead::of(&pm.state);
    health(&mut row, Some(&host), pm.state.binding.closing);
    let generation = pm
        .state
        .generations
        .into_iter()
        .find(|g| g.n == host.binding.generation);
    row.since = generation.as_ref().map(|g| g.created.to_string());
    Ok(Some(source(row, generation, None, false)))
}

fn task_heads(ctx: &Ctx) -> Result<Vec<TaskHead>> {
    // Both are maintained head indexes; terminal tasks are included only while cleanup owns a pane.
    let mut heads = BTreeMap::new();
    for query in [
        HeadQuery::Open { project: None },
        HeadQuery::UnreleasedPanes,
    ] {
        for head in ctx.store.heads(&query)? {
            heads.entry(head.id.clone()).or_insert(head);
        }
    }
    Ok(heads.into_values().collect())
}

fn assistant_source(ctx: &Ctx, assistant: AssistantRow, tokens: Option<u64>) -> Result<Source> {
    let session = ctx.store.get_assistant(&assistant.id)?;
    let container = ctx.store.get_assistant_container(&assistant.id)?;
    let generation = container
        .state
        .generations
        .into_iter()
        .find(|g| g.n == container.state.binding.generation);
    let mut row = blank(assistant.id, "assistant", &assistant.title);
    row.profile = assistant.profile;
    row.task = (session.focus != "workspace").then_some(session.focus);
    row.runtime = (container.state.binding.generation != 0).then(|| "corral-pi".into());
    row.pane = container.pane;
    row.since = Some(session.created.to_string());
    row.cost_usd = assistant.cost_usd;
    row.cost_basis = assistant.cost_basis;
    row.tokens = tokens;
    row.status = match assistant.state {
        AssistantState::Working => AgentStatus::Working,
        AssistantState::Ready => AgentStatus::Live,
        AssistantState::Ended => AgentStatus::Ended,
        AssistantState::Failed => AgentStatus::Unavailable,
        _ => AgentStatus::Waiting,
    };
    row.now = assistant
        .wait
        .map(|wait| short(&wait.text, 60))
        .unwrap_or_default();
    if assistant.live_in == AssistantLocation::Daemon {
        row.pane = None;
    }
    Ok(source(row, generation, None, false))
}

/// The canonical runtime owns assistant state; this read never warms or starts it.
pub fn assistant_rows(ctx: &Ctx) -> Result<Vec<AssistantRow>> {
    crate::agent::app_runtime::sessions(ctx)
}

/// Current actual containers, not a pane inventory or a synthetic list of profiles.
pub fn agents_rows(ctx: &Ctx) -> Result<Vec<AgentRow>> {
    let mut sources = Vec::new();
    for head in task_heads(ctx)? {
        task_sources(&head, &mut sources)?;
    }
    if let Some(pm) = pm_source(ctx)? {
        sources.push(pm);
    }
    for (assistant, tokens) in crate::agent::app_runtime::sessions_with_usage(ctx)? {
        sources.push(assistant_source(ctx, assistant, tokens)?);
    }
    sources
        .into_iter()
        .map(|mut source| {
            source.row.rev = revision(&source.row)?;
            Ok(source.row)
        })
        .collect()
}

fn resolve(ctx: &Ctx, id: &str) -> Result<Source> {
    ensure!(
        !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control),
        "invalid session id"
    );
    if id == "pm" {
        return pm_source(ctx)?.context("PM session not found");
    }
    if id.starts_with("A-") || id == "quick" {
        let (assistant, tokens) = crate::agent::app_runtime::sessions_with_usage(ctx)?
            .into_iter()
            .find(|(row, _)| {
                row.id == id
                    || (id == "quick" && row.kind == corral_protocol::rows::AssistantKind::Quick)
            })
            .context("assistant session not found")?;
        return assistant_source(ctx, assistant, tokens);
    }
    let mut sources = Vec::new();
    if let Some((task, _)) = id.split_once('/') {
        let head = ctx.store.head(task)?;
        task_sources(&head, &mut sources)?;
    } else {
        // Board links may contain the provider's recorded session id, not the container key.
        for head in task_heads(ctx)? {
            task_sources(&head, &mut sources)?;
        }
        if let Some(pm) = pm_source(ctx)? {
            sources.push(pm);
        }
    }
    sources
        .into_iter()
        .find(|source| source.row.id == id || source.provider_id.as_deref() == Some(id))
        .context("session not found")
}

fn entry_message(entry: corral_protocol::assistant::ThreadEntry) -> SessionMessage {
    let author = match entry.kind {
        EntryKind::Message | EntryKind::PmMessage | EntryKind::Idea => "owner",
        EntryKind::PmReply => "pm",
        EntryKind::Answer => "assistant",
        _ => "system",
    }
    .into();
    let mut items = Vec::new();
    if let Some(text) = entry.text {
        items.push(SessionItem::Text {
            text: short(&text, 32 * 1024),
        });
    }
    for mut tool in entry.tools.into_iter().take(16) {
        tool.input = short(&tool.input, 4096);
        tool.output = tool.output.map(|text| short(&text, 4096));
        items.push(SessionItem::Tool { tool });
    }
    if let Some(EntryCard::Question {
        key,
        rev,
        options,
        answer,
    }) = entry.card
    {
        items.push(SessionItem::QuestionCard {
            key,
            rev,
            text: String::new(),
            options,
            answer,
        });
    }
    SessionMessage {
        author,
        at: Some(entry.at),
        items,
    }
}

/// `page` is a domain cursor: `thread:<before>` or `transcript:<byte offset>`.
/// Canonical assistants reply through assistant.send, never task.reply or terminal input.
pub(super) fn session(
    ctx: &Ctx,
    id: &str,
    page: Option<&str>,
) -> Result<(DetailBody, Option<String>)> {
    let selected = resolve(ctx, id)?;
    let row = selected.row;
    let (messages, older) = if row.role == "assistant" && ctx.store.app_thread_head(&row.id).is_ok()
    {
        let before = transcript::cursor(page, "thread")?;
        let (entries, next, _) = crate::agent::app_runtime::thread(ctx, &row.id, before, 1)?;
        (
            entries.into_iter().map(entry_message).collect(),
            next.map(|next| format!("thread:{next}")),
        )
    } else if let Some(path) = selected.transcript {
        transcript::messages(&path, page)?
    } else if let Some((task, run)) = selected.gate {
        let snapshot = transcript::gate_snapshot(ctx, &task, &run)?;
        (
            vec![SessionMessage {
                author: "gate".into(),
                at: Some(snapshot.captured_at),
                items: vec![SessionItem::Text {
                    text: snapshot.lines.join("\n"),
                }],
            }],
            None,
        )
    } else {
        ensure!(page.is_none(), "session has no transcript cursor");
        (Vec::new(), None)
    };
    let refusal = if selected.replyable {
        None
    } else {
        Some(
            if row.role == "assistant" {
                "reply_in_assistant_thread"
            } else if row.role == "reviewer" || row.role == "plan_reviewer" {
                "reviewer_session_read_only"
            } else {
                "no_supported_session_reply"
            }
            .into(),
        )
    };
    Ok((
        DetailBody::Session(SessionPage {
            id: row.id,
            role: row.role,
            task: row.task,
            run: row.run,
            machine: row.machine,
            messages,
            older: older.clone(),
            replyable: selected.replyable,
            reply_refusal: refusal,
        }),
        older,
    ))
}

/// Only recorded output can be served without asking a runtime for its terminal contents.
pub(super) fn terminal(ctx: &Ctx, id: &str) -> Result<DetailBody> {
    let selected = resolve(ctx, id)?;
    let Some((task, run)) = selected.gate else {
        bail!(
            "terminal_snapshot_unavailable: no recorded terminal capture; use session transcript"
        );
    };
    Ok(DetailBody::Terminal(transcript::gate_snapshot(
        ctx, &task, &run,
    )?))
}

/// Global summaries remain readable without a live assistant or loaded threads.
/// The canonical facade owns dense-log paging and fails closed before bootstrap.
pub(super) fn filings(ctx: &Ctx, before: Option<u64>) -> Result<DetailBody> {
    Ok(DetailBody::Filings(crate::agent::app_runtime::filings(
        ctx, before,
    )?))
}
