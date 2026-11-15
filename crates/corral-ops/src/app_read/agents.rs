//! Read-only agent projections, including canonical usage from bound assistant transcripts.
//! Topic reads never probe panes; explicit terminal details read one visible screen through herdr.
//! No reader starts hosts or accepts a caller-supplied filesystem path.
use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use corral_core::agent::{AgentGeneration, ContainerRef, HostHealth, pm_state};
use corral_core::backend::HeadQuery;
use corral_core::{AgentHostHead, GateResult, RunHead, TaskHead, UsageTotals};
use corral_protocol::detail::DetailBody;
use corral_protocol::detail::session::{SessionItem, SessionMessage, SessionPage};
use corral_protocol::rows::{
    AgentRow, AgentStatus, AssistantLocation, AssistantRow, AssistantState,
};
use jiff::Timestamp;

use super::{revision, short};
use crate::Ctx;

#[path = "session_replay.rs"]
mod replay;
#[path = "agent_transcript.rs"]
mod transcript;

/// Terminal tasks read per status when looking for sessions that ended recently.
const RECENT_TERMINAL: u32 = 64;

struct Source {
    row: AgentRow,
    container: Option<ContainerRef>,
    provider_id: Option<String>,
    transcript: Option<PathBuf>,
    replyable: bool,
    gate: Option<(String, String)>,
    /// Assistant panes have provenance outside the task ownership summary.
    assistant_socket: Option<PathBuf>,
    /// When an ended session ended; `None` while it has not.
    ended: Option<Timestamp>,
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
        generation: None,
        turn: None,
        controls: Default::default(),
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
    container: Option<ContainerRef>,
    generation: Option<AgentGeneration>,
    fallback: Option<PathBuf>,
    replyable: bool,
) -> Source {
    let bound = generation.and_then(|generation| generation.bound);
    Source {
        row,
        container,
        provider_id: bound.as_ref().map(|bound| bound.session_id.clone()),
        transcript: bound.map(|bound| bound.session_file).or(fallback),
        replyable,
        gate: None,
        assistant_socket: None,
        ended: None,
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
        Some(container),
        generation,
        run.session.as_ref().and_then(|s| s.transcript.clone()),
        replyable,
    );
    if selected.provider_id.is_none() {
        selected.provider_id = run.session.as_ref().and_then(|s| s.id.clone());
    }
    selected.ended = ended.then(|| run.ended.unwrap_or(head.updated));
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
                ended: (row.status == AgentStatus::Ended).then_some(gate.at),
                row,
                container: None,
                provider_id: None,
                transcript: None,
                replyable: false,
                gate: Some((head.id.clone(), run.id.clone())),
                assistant_socket: None,
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
        let mut planner = source(row, Some(container), generation, None, replyable);
        planner.ended = run.ended;
        output.push(planner);
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
        let mut reviewer = source(
            row,
            Some(container.clone()),
            native(head, &container, review.agent_host.as_deref())?,
            None,
            false,
        );
        reviewer.ended = ended.then_some(review.at);
        output.push(reviewer);
    }
    Ok(())
}

fn pm_source(ctx: &Ctx) -> Result<Option<Source>> {
    let pm = pm_state::load_current(&ctx.store)?;
    let recorded = crate::pm::recorded_pane(ctx)?;
    if pm.is_none()
        || recorded.as_ref().is_some_and(|pane| {
            pm.as_ref()
                .is_some_and(|pm| pm.pane != *pane && pm.state.binding.health == HostHealth::Fenced)
        })
    {
        let mut row = blank("pm".into(), "pm", "Project manager");
        row.pane = recorded;
        row.status = if row.pane.is_some() {
            AgentStatus::Live
        } else {
            AgentStatus::Ended
        };
        let ended = row.status == AgentStatus::Ended;
        let mut source = source(row, Some(ContainerRef::Pm), None, None, false);
        source.ended = ended.then(|| ctx.store.now());
        return Ok(Some(source));
    }
    let pm = pm.context("PM session disappeared from stored snapshot")?;
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
    Ok(Some(source(
        row,
        Some(ContainerRef::Pm),
        generation,
        None,
        false,
    )))
}

/// Sessions that ended before this are off the Agents list (`[ui] agents_ended_window`).
fn ended_cutoff(ctx: &Ctx) -> Timestamp {
    corral_core::clock::now()
        .checked_sub(ctx.store.config.ui.agents_ended_window())
        .unwrap_or(Timestamp::MIN)
}

/// Open tasks, and the newest tasks that reached a terminal status since `cutoff`: the heads
/// whose containers can be running or recently ended. Both are maintained head indexes. The
/// cleanup ledger is not consulted: a pane still owed cleanup says nothing about its session.
fn task_heads(ctx: &Ctx, cutoff: Timestamp) -> Result<Vec<TaskHead>> {
    let mut heads = BTreeMap::new();
    for head in ctx.store.heads(&HeadQuery::Open { project: None })? {
        heads.insert(head.id.clone(), head);
    }
    for status in ["done", "dropped"] {
        let recent = ctx.store.heads_page(
            &HeadQuery::RecentByStatus {
                status: status.into(),
            },
            RECENT_TERMINAL,
        )?;
        for head in recent.into_iter().filter(|head| head.updated >= cutoff) {
            heads.insert(head.id.clone(), head);
        }
    }
    Ok(heads.into_values().collect())
}

fn assistant_source(ctx: &Ctx, assistant: AssistantRow, tokens: Option<u64>) -> Result<Source> {
    let controls = crate::session_control::controls::for_assistant(&assistant);
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
    row.generation = Some(container.state.binding.generation).filter(|n| *n != 0);
    row.turn = assistant.turn;
    row.controls = controls;
    row.status = match assistant.state {
        AssistantState::Working => AgentStatus::Working,
        AssistantState::Ready => AgentStatus::Live,
        AssistantState::Ended => AgentStatus::Ended,
        AssistantState::Paused => AgentStatus::Paused,
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
    let mut selected = source(
        row,
        Some(ContainerRef::Assistant {
            session: session.id.clone(),
        }),
        generation,
        None,
        false,
    );
    selected.assistant_socket = container.herdr_socket;
    selected.ended = session.ended;
    Ok(selected)
}

/// The canonical runtime owns assistant state; this read never warms or starts it.
pub fn assistant_rows(ctx: &Ctx) -> Result<Vec<AssistantRow>> {
    crate::agent::app_runtime::sessions(ctx)
}

/// The sessions running now and those that ended within `[ui] agents_ended_window`, each with
/// the last lines of its recorded output. Not a pane inventory, a synthetic list of profiles
/// or a history: an older session is read by id through its task.
pub fn agents_rows(ctx: &Ctx) -> Result<Vec<AgentRow>> {
    let cutoff = ended_cutoff(ctx);
    let mut sources = Vec::new();
    for head in task_heads(ctx, cutoff)? {
        task_sources(&head, &mut sources)?;
    }
    if let Some(pm) = pm_source(ctx)? {
        sources.push(pm);
    }
    let listed = |state: AssistantState, ended: Option<Timestamp>| match state {
        AssistantState::Ended => ended.is_some_and(|at| at >= cutoff),
        // A resumable session with no host runs nowhere; the assistant feed lists it.
        AssistantState::Cold => false,
        _ => true,
    };
    for (assistant, tokens) in crate::agent::app_runtime::sessions_with_usage(ctx, listed)? {
        sources.push(assistant_source(ctx, assistant, tokens)?);
    }
    sources
        .into_iter()
        .filter(|source| {
            source.row.status != AgentStatus::Ended || source.ended.is_some_and(|at| at >= cutoff)
        })
        .map(|mut source| {
            apply_controls(ctx, &mut source)?;
            // A tail that cannot be read leaves the row without one; its other facts stand.
            source.row.tail = match (&source.transcript, &source.gate) {
                (Some(path), _) => transcript::tail(path).unwrap_or_default(),
                (None, Some((task, run))) if source.row.status == AgentStatus::Gating => {
                    transcript::gate_tail(ctx, task, run).unwrap_or_default()
                }
                _ => Vec::new(),
            };
            source.row.rev = revision(&source.row)?;
            Ok(source.row)
        })
        .collect()
}

fn resolve_source(ctx: &Ctx, id: &str) -> Result<Source> {
    ensure!(
        !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control),
        "invalid session id"
    );
    if id == "pm" {
        return pm_source(ctx)?.context("PM session not found");
    }
    if id.starts_with("A-") || id == "quick" {
        let session = if id == "quick" {
            ctx.store
                .app_thread_scope("quick")?
                .context("assistant session not found")?
        } else {
            id.to_owned()
        };
        let (assistant, tokens) = crate::agent::app_runtime::row_with_usage(ctx, &session)
            .context("assistant session not found")?;
        return assistant_source(ctx, assistant, tokens);
    }
    let mut sources = Vec::new();
    if let Some((task, _)) = id.split_once('/') {
        let head = ctx.store.head(task)?;
        task_sources(&head, &mut sources)?;
    } else {
        // Board links may contain the provider's recorded session id, not the container key.
        for head in task_heads(ctx, ended_cutoff(ctx))? {
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

fn resolve(ctx: &Ctx, id: &str) -> Result<Source> {
    let mut source = resolve_source(ctx, id)?;
    apply_controls(ctx, &mut source)?;
    Ok(source)
}

/// The control resolver consumes the projection's identity, not another session-id grammar.
pub(crate) fn control_identity(
    ctx: &Ctx,
    id: &str,
) -> Result<(AgentRow, Option<ContainerRef>, bool)> {
    let selected = resolve_source(ctx, id)?;
    Ok((selected.row, selected.container, selected.replyable))
}

fn apply_controls(ctx: &Ctx, source: &mut Source) -> Result<()> {
    if source.row.role == "assistant" {
        return Ok(());
    }
    let target = crate::session_control::Target::from_row(
        ctx,
        &source.row,
        source.container.clone(),
        source.replyable,
    )?;
    source.row.generation = target.generation;
    source.row.turn = target.turn;
    source.row.controls = crate::session_control::controls::for_target(ctx, &target)?;
    if !target.ended && crate::session_control::pause::is_paused(ctx, &target.id)? {
        source.row.status = AgentStatus::Paused;
    }
    Ok(())
}

fn entry_message(entry: corral_protocol::assistant::ThreadEntry) -> SessionMessage {
    replay::entry_message(entry)
}

/// Canonical assistant words and Pi replay have independent append-only positions.
pub(super) fn session(ctx: &Ctx, id: &str, page: Option<&str>) -> Result<SessionPage> {
    let selected = resolve(ctx, id)?;
    let row = selected.row;
    let (messages, older) = if row.role == "assistant" && ctx.store.app_thread_head(&row.id).is_ok()
    {
        replay::assistant(
            ctx,
            &row.id,
            selected.transcript.as_deref(),
            page,
            entry_message,
        )?
    } else if let Some(path) = selected.transcript.as_deref() {
        transcript::messages(path, &row.id, page)?
    } else if let Some((task, run)) = selected.gate {
        let snapshot = transcript::gate_snapshot(ctx, &task, &run)?;
        (
            vec![SessionMessage {
                author: "gate".into(),
                at: Some(snapshot.captured_at),
                items: vec![SessionItem::Text {
                    text: snapshot.lines.join("\n"),
                }],
                model: None,
                stop_reason: None,
                usage: None,
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
    Ok(SessionPage {
        id: short(&row.id, 512),
        role: short(&row.role, 80),
        task: row.task.map(|task| short(&task, 512)),
        run: row.run.map(|run| short(&run, 512)),
        machine: row.machine.map(|machine| short(&machine, 512)),
        messages,
        older,
        replyable: selected.replyable,
        reply_refusal: refusal,
    })
}

pub(super) fn session_item(ctx: &Ctx, id: &str, raw: &str) -> Result<DetailBody> {
    let token = replay::decode(id, raw)?;
    let selected = resolve(ctx, id)?;
    let load = |source: &replay::Source| -> Result<SessionItem> {
        let message = match source {
            replay::Source::Transcript { start, end, .. } => {
                let path = selected
                    .transcript
                    .as_deref()
                    .context("session has no recorded transcript")?;
                replay::message(&transcript::record(path, *start, *end)?)
                    .context("transcript entry no longer draws content")?
            }
            replay::Source::Thread { before, .. } => {
                let (entries, _, _) = crate::agent::app_runtime::thread(ctx, id, *before, 1)?;
                entry_message(
                    entries
                        .into_iter()
                        .next()
                        .context("thread entry no longer exists")?,
                )
            }
        };
        message
            .items
            .into_iter()
            .nth(source.item())
            .context("transcript item no longer exists")
    };
    let mut item = load(&token.source)?;
    if let Some(pair) = &token.pair {
        replay::merge(&mut item, &load(pair)?);
    }
    Ok(DetailBody::SessionItem(replay::fragment(id, token, item)?))
}

/// Gate captures remain recorded; other terminals require explicit, owning-server reads.
pub(super) fn terminal(ctx: &Ctx, id: &str) -> Result<DetailBody> {
    let selected = resolve(ctx, id)?;
    if let Some((task, run)) = selected.gate {
        return Ok(DetailBody::Terminal(transcript::gate_snapshot(
            ctx, &task, &run,
        )?));
    }
    ensure!(
        selected.ended.is_none() && selected.row.status != AgentStatus::Ended,
        "terminal_snapshot_unavailable: session has ended"
    );
    let pane = selected
        .row
        .pane
        .as_deref()
        .context("terminal_snapshot_unavailable: session has no pane")?;
    let owned = if selected.row.role != "assistant"
        && let Some(task) = selected.row.task.as_deref()
    {
        let head = ctx
            .store
            .head(task)
            .context("reading terminal session owner")?;
        crate::session_scope::owns_head(ctx, &head).context("checking terminal pane ownership")?
    } else {
        let owns_store = crate::session_scope::owns_inventory(ctx)
            .context("checking terminal store ownership")?;
        if selected.row.role == "assistant" {
            let view = ctx
                .store
                .session_view()
                .context("reading terminal ownership evidence")?;
            let actual = ctx
                .herdr
                .socket_path()
                .context("locating terminal herdr server")?;
            owns_store
                && selected
                    .assistant_socket
                    .as_ref()
                    .or(view.binding.as_ref().map(|binding| &binding.socket))
                    == Some(&actual)
        } else {
            // PM panes have no per-pane socket; sync's store ownership is their evidence.
            owns_store
        }
    };
    ensure!(
        owned,
        "terminal_snapshot_unavailable: pane ownership is unknown or belongs to another herdr server"
    );
    let screen = ctx
        .herdr
        .pane_screen(pane)
        .context("reading live terminal screen")?
        .with_context(|| format!("not_found: terminal pane {pane} is gone"))?;
    Ok(DetailBody::Terminal(super::terminal_screen::snapshot(
        &screen,
    )?))
}

/// Global summaries remain readable without a live assistant or loaded threads.
/// The canonical facade owns dense-log paging and fails closed before bootstrap.
pub(super) fn filings(ctx: &Ctx, before: Option<u64>) -> Result<DetailBody> {
    Ok(DetailBody::Filings(crate::agent::app_runtime::filings(
        ctx, before,
    )?))
}
