//! Project-scoped PM reads and mutations. No operation runs the board's sync or action
//! executor. Single-phase receipts commit with their target; capture and legacy delivery
//! reserve in AgentPm and never replay an interrupted effect (T-252 F1.c).
use anyhow::{Context, Result, ensure};
use corral_core::agent::{
    AGENT_RECORD_VERSION, AgentOrigin, RESULT_MAX_BYTES, ReceiptState, RequestOrigin,
    RequestReceipt,
};
use corral_core::inbox::{Inbox, MessageBinding, Post, Urgency};
use corral_core::{Origin, ProfileChoice, Status, Store, Task};
use jiff::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::admission::{self, Admission, Mode};
use super::super::call::{AgentOp, CallResponse, RejectCode, Rejection};
use super::super::effects::{self, Home};
use super::super::envelope::Envelope;
use super::worker::{answer, origin_of};
use crate::Ctx;

/// Continue a written PM session; an unprompted host has no file to resume.
pub(crate) fn recovery_session(
    file: Option<std::path::PathBuf>,
    session_dir: &mut std::path::PathBuf,
    fresh_session: &mut bool,
) -> Result<Option<std::path::PathBuf>> {
    let Some(file) = file else {
        // No generation ever bound a file (e.g. launches that failed before hello).
        *fresh_session = true;
        return Ok(None);
    };
    if std::fs::metadata(&file).is_ok_and(|meta| meta.is_file() && meta.len() > 0) {
        return Ok(Some(file));
    }
    *session_dir = file
        .parent()
        .context("the bound Pi session file has no directory")?
        .to_path_buf();
    *fresh_session = true;
    Ok(None)
}

/// The T-252 child that lands these handlers.
pub const OWNER: &str = "pm-role";

pub(crate) fn lands(op: AgentOp) -> bool {
    matches!(
        op,
        AgentOp::Reply
            | AgentOp::Comment
            | AgentOp::Capture
            | AgentOp::ReadBrief
            | AgentOp::ReadNext
            | AgentOp::ReadShow
            | AgentOp::ReadList
            | AgentOp::ReadSearch
            | AgentOp::ReadRoute
            | AgentOp::ReadArtifacts
            | AgentOp::ReadLessons
            | AgentOp::InboxRead
            | AgentOp::InboxResolve
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    task: String,
}

#[derive(Default, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Section {
    #[default]
    Overview,
    Spec,
    Questions,
    Comments,
    Plans,
    Automation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Show {
    task: String,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    section: Section,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    task: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    project: String,
    title: String,
    body: String,
    #[serde(default)]
    status: Option<Status>,
    #[serde(default)]
    difficulty: Option<corral_core::TaskDifficulty>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    status: Option<Status>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

fn bad(reason: &str) -> anyhow::Error {
    Rejection::new(RejectCode::BadRequest, reason).into()
}

fn scope(ctx: &Ctx, task: &Task, project: &str) -> Result<()> {
    if !super::super::hooks::pm::contains(&ctx.store, project, task.project.as_deref())? {
        return Err(
            Rejection::new(RejectCode::OutOfScope, "target is outside the PM project").into(),
        );
    }
    Ok(())
}

fn target(ctx: &Ctx, id: &str, project: &str) -> Result<Task> {
    let task = ctx.store.get(&Store::normalize_id(id)?)?;
    scope(ctx, &task, project)?;
    Ok(task)
}

/// Authentication and receipt lookup precede liveness. Task-local receipts are replayed by
/// their operation, which repeats PM liveness and project scope inside its final transaction.
pub(crate) fn call(ctx: &Ctx, envelope: &Envelope, op: AgentOp) -> Result<CallResponse> {
    if !lands(op) {
        return Err(super::unavailable(op, OWNER));
    }
    let admitted = admission::admit(ctx, envelope, Mode::Lookup)?;
    if let Admission::Replay(stored) = admitted {
        let receipt = if stored.state == ReceiptState::Reserved {
            effects::reconcile_pm(ctx, &stored.request_id)?
        } else {
            *stored
        };
        return Ok(CallResponse::Receipt {
            receipt,
            replayed: true,
        });
    }
    let origin = origin_of(envelope);
    if let Some((task, _)) = effects::pm_task_receipt(&ctx.store, &origin)? {
        return effects::swept(ctx, &origin, Home::PmTask(task), || unreachable!()).map(answer);
    }
    let project = envelope.identity.project.as_str();
    effects::failpoint("pm_admitted_before_commit");
    match op {
        AgentOp::Comment => {
            let p: Text = super::payload(envelope, "task, text")?;
            text(&p.text)?;
            // comment_request checks scope under the task lock, including on a fence race.
            crate::comments::comment_request(ctx, &p.task, &p.text, &origin).map(answer)
        }
        AgentOp::Capture => {
            let p: Capture =
                super::payload(envelope, "project, title, body, status?, difficulty?")?;
            text(&p.title)?;
            let status = p.status.unwrap_or(Status::Backlog);
            if !matches!(status, Status::Idea | Status::Backlog) {
                return Err(bad("capture may file only idea or backlog tasks"));
            }
            if !super::super::hooks::pm::contains(&ctx.store, project, Some(&p.project))? {
                return Err(Rejection::new(
                    RejectCode::OutOfScope,
                    "capture names another project",
                )
                .into());
            }
            two_phase(ctx, &origin, "capture", &p.project, || {
                let task = crate::tasks::new_task_initialized(
                    ctx,
                    &p.title,
                    Some(&p.project),
                    ctx.store.root(),
                    status,
                    &p.body,
                    |task| {
                        effects::recheck(
                            &ctx.store,
                            task,
                            &origin,
                            &Home::PmTask(task.id.clone()),
                        )?;
                        task.origin = Some(Origin::Agent);
                        task.difficulty = p.difficulty;
                        task.agent_origin = Some(AgentOrigin {
                            request_id: origin.request_id.clone(),
                            session_id: origin.identity.session_id.clone(),
                            generation: origin.binding.generation,
                            at: Timestamp::now(),
                        });
                        Ok(())
                    },
                )?;
                Ok(json!({"outcome": "captured", "task": task.id}))
            })
        }
        AgentOp::Reply => {
            let p: Text = super::payload(envelope, "task, text")?;
            text(&p.text)?;
            reply(ctx, &origin, &p)
        }
        AgentOp::InboxResolve => {
            let p: super::pm_inbox::Resolve =
                super::payload(envelope, super::pm_inbox::RESOLVE_FIELDS)?;
            two_phase(ctx, &origin, "pm_inbox", &p.id, || {
                super::pm_inbox::resolve(ctx, &p, &origin.request_id)
            })
        }
        _ => {
            // Reads require a live PM, but never store a receipt or advance board policy.
            admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })?;
            let result = read(ctx, envelope, op, project)?;
            if serde_json::to_vec(&result)?.len() > RESULT_MAX_BYTES {
                return Err(bad(
                    "read exceeds 8 KiB; use a smaller page or a narrower search",
                ));
            }
            let now = Timestamp::now();
            Ok(CallResponse::Receipt {
                receipt: RequestReceipt {
                    version: AGENT_RECORD_VERSION,
                    request_id: origin.request_id,
                    op: origin.op,
                    payload_sha256: origin.payload_sha256,
                    identity: origin.identity,
                    state: ReceiptState::Committed,
                    result,
                    executor: None,
                    effect: None,
                    evidence: vec![],
                    post_pending: vec![],
                    at: now,
                    committed_at: None,
                },
                replayed: false,
            })
        }
    }
}

fn text(value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(bad("text must not be empty"));
    }
    Ok(())
}

fn two_phase(
    ctx: &Ctx,
    origin: &RequestOrigin,
    kind: &str,
    target: &str,
    apply: impl FnOnce() -> Result<Value>,
) -> Result<CallResponse> {
    let reserved = effects::pm_reserve(&ctx.store, origin, kind, target)?;
    if reserved.replayed {
        return Ok(CallResponse::Receipt {
            receipt: if reserved.receipt.state == ReceiptState::Reserved {
                effects::reconcile_pm(ctx, &origin.request_id)?
            } else {
                reserved.receipt
            },
            replayed: true,
        });
    }
    effects::failpoint("pm_reserved_before_effect");
    let outcome = apply();
    effects::failpoint("pm_effect_before_receipt");
    let (state, result) = match outcome {
        Ok(result) => (ReceiptState::Committed, result),
        Err(error) if error.downcast_ref::<Rejection>().is_some() => {
            let rejection = error
                .downcast_ref::<Rejection>()
                .context("PM refusal disappeared")?;
            (
                ReceiptState::Rejected,
                json!({"code":rejection.code.as_str(),"reason":rejection.reason}),
            )
        }
        Err(error)
            if error
                .downcast_ref::<corral_core::agent::AgentRefusal>()
                .is_some() =>
        {
            let refusal = error
                .downcast_ref::<corral_core::agent::AgentRefusal>()
                .context("PM fence refusal disappeared")?;
            let rejection = Rejection::from(refusal.clone());
            (
                ReceiptState::Rejected,
                json!({"code":rejection.code.as_str(),"reason":rejection.reason}),
            )
        }
        Err(error) if kind == "capture" => {
            // Creation can succeed before its event write fails. The marker, not the return
            // code, is the evidence of a capture, exactly as after a killed executor.
            match ctx
                .store
                .heads_page(
                    &corral_core::backend::HeadQuery::EdgeTo {
                        kind: "agent_origin".into(),
                        other: origin.request_id.clone(),
                    },
                    1,
                )?
                .into_iter()
                .next()
            {
                Some(task) => (
                    ReceiptState::Committed,
                    json!({"outcome":"captured","task":task.id}),
                ),
                None => (
                    ReceiptState::Rejected,
                    json!({"outcome":"not_started","reason":format!("{error:#}")}),
                ),
            }
        }
        Err(error) => (
            ReceiptState::Uncertain,
            json!({"outcome":"outcome_unknown","reason":format!("{error:#}")}),
        ),
    };
    let receipt = effects::pm_settle(&ctx.store, &reserved.receipt, state, result)?;
    effects::failpoint("pm_receipt_before_ack");
    Ok(CallResponse::Receipt {
        receipt,
        replayed: false,
    })
}

fn live_reply(task: &Task, run: &str) -> Result<()> {
    ensure!(
        matches!(task.status, Status::Running | Status::NeedsMe),
        "reply requires a running task"
    );
    ensure!(
        task.current_run().is_some_and(|r| r.id == run
            && r.outcome.is_none()
            && r.ended.is_none()
            && r.herdr.is_some()),
        "reply target run changed"
    );
    Ok(())
}

fn reply(ctx: &Ctx, origin: &RequestOrigin, p: &Text) -> Result<CallResponse> {
    let task = target(ctx, &p.task, &origin.identity.project)?;
    let home = Home::PmTask(task.id.clone());
    let run = task.current_run().context("reply target has no run")?;
    let run_id = run.id.clone();
    live_reply(&task, &run_id)?;
    let _delivery = ctx
        .store
        .try_lock(&format!("delivery-{}", task.id))?
        .context("another reply is being delivered")?;
    if let Some(recipient) = crate::delivery::owner::live_run(&task) {
        let inbox = Inbox::new(&ctx.store, recipient.clone())?;
        let sent = effects::swept(ctx, origin, home.clone(), || {
            let _inbox = inbox.lock()?;
            let mut receipt = None;
            ctx.store.update(&task.id, |current| {
                effects::recheck(&ctx.store, current, origin, &home)?;
                live_reply(current, &run_id)?;
                // Native OmO questions retain their exact request-bound owner route.
                let native = current
                    .current_run()
                    .is_some_and(|run| run.harness.as_deref().unwrap_or(&run.agent) == "omo");
                let question = if native {
                    crate::question_reply::route(ctx, current)?
                } else {
                    None
                };
                let message = inbox.reserve_in(
                    current,
                    Post {
                        kind: "owner_reply",
                        urgency: Urgency::Wake,
                        author: "pm",
                        body: &p.text,
                        binding: MessageBinding {
                            run: Some(run_id.clone()),
                            question_request: question,
                            ..Default::default()
                        },
                        subject: Some(json!({"request_id":origin.request_id})),
                        max_payload: corral_core::inbox::MAX_PAYLOAD,
                    },
                )?;
                current
                    .current_run_mut()
                    .context("reply run disappeared")?
                    .pending_reply = Some(p.text.clone());
                receipt = Some(effects::record(
                    current,
                    origin,
                    &home,
                    effects::Commit {
                        result: json!({"task":task.id,"message":message.id,"seq":message.seq}),
                        effect: Some(("message", message.id)),
                        owed: vec![],
                    },
                )?);
                Ok(())
            })?;
            Ok(receipt)
        })?;
        let seq = sent.receipt.result["seq"]
            .as_u64()
            .context("reply receipt has no sequence")?;
        let message = inbox.message(seq)?;
        if let Some(request) = &message.binding.question_request {
            let current = ctx.store.get(&task.id)?;
            let delivery = crate::question_reply::answer(
                ctx,
                &current,
                &p.text,
                true,
                &crate::question_reply::Bound {
                    request,
                    message: &message.id,
                },
            )?;
            if delivery.submitted {
                inbox.record_receipt(
                    seq,
                    corral_core::inbox::ReceiptLevel::Observed,
                    &corral_core::inbox::Receipt {
                        envelope: None,
                        binding: None,
                        observed_at: Timestamp::now(),
                        via: "omo_question".into(),
                        detail: json!({"request":request}),
                    },
                )?;
            }
        } else if let Err(reason) = crate::delivery::owner::kick(ctx, &recipient) {
            crate::messages::report(Some(&task.id), "PM reply", reason);
        }
        Ok(answer(sent))
    } else {
        two_phase(ctx, origin, "legacy_reply", &task.id, || {
            let mut result = None;
            // This operation's own target transaction serializes the last scope/fence check
            // with delivery. It never wraps dispatch::reply or another public operation.
            ctx.store.update(&task.id, |current| {
                effects::recheck(&ctx.store, current, origin, &home)?;
                live_reply(current, &run_id)?;
                ensure!(
                    crate::session_scope::owns_task(ctx, current)?,
                    "selected herdr session does not own the reply pane"
                );
                let run = current.current_run_mut().context("reply run disappeared")?;
                ensure!(
                    run.pending_reply.is_none(),
                    "a prior reply requires owner recovery"
                );
                run.pending_reply = Some(p.text.clone());
                if run.stuck.is_some() {
                    result = Some(json!({"outcome":"queued","task":task.id}));
                    return Ok(());
                }
                let delivery = crate::question_reply::deliver(ctx, current, &p.text, false)?;
                let run = current.current_run_mut().context("reply run disappeared")?;
                if delivery.submitted {
                    run.pending_reply = None;
                    run.blocked_excerpt = None;
                    run.seen_working = false;
                    run.idle_since = None;
                    run.idle_no_report = false;
                    current.status = Status::Running;
                }
                result = Some(
                    json!({"outcome":if delivery.submitted {"sent"} else {"outcome_unknown"},
                    "task":task.id,"submitted":delivery.submitted,"error":delivery.error}),
                );
                Ok(())
            })?;
            let result = result.context("reply produced no result")?;
            if result["outcome"] == "outcome_unknown" {
                anyhow::bail!("legacy reply was not acknowledged");
            }
            Ok(result)
        })
    }
}

fn summary(task: &impl corral_core::TaskRef) -> Value {
    json!({"id":task.id(),"title":task.title(),"status":task.status(),"project":task.project(),
        "kind":task.kind(),"size":task.size(),"difficulty":task.difficulty(),"priority":task.priority()})
}

pub(super) fn page(values: Vec<Value>, offset: usize, limit: usize) -> Result<Value> {
    if limit == 0 || limit > 100 {
        return Err(bad("limit must be between 1 and 100"));
    }
    let total = values.len();
    let mut items = Vec::new();
    for value in values.into_iter().skip(offset).take(limit) {
        items.push(value);
        if serde_json::to_vec(&items)?.len() > RESULT_MAX_BYTES - 256 {
            items.pop();
            break;
        }
    }
    if items.is_empty() && offset < total {
        return Err(bad("one item exceeds 8 KiB; use a narrower search"));
    }
    let next = offset + items.len();
    Ok(json!({"items":items,"total":total,"next_offset":(next < total).then_some(next)}))
}

// The final wire JSON, not just source characters, must fit the existing 8 KiB contract.
// Opt-in 4K pages fit ordinary prose; escapes and Unicode can reduce the actual page.
fn show_page(
    task: &Task,
    p: &Show,
    content: &str,
    limit: usize,
    revision: Option<u32>,
) -> Result<Value> {
    let diagnostic = if matches!(p.section, Section::Automation) {
        let value: Value = serde_json::from_str(content)?;
        let stage = value["autopilot"]["stage"]
            .as_str()
            .unwrap_or("not_started");
        let reason = value["blocker"]
            .as_str()
            .or(value["autopilot"]["reason"].as_str());
        Some(
            format!(
                "{stage}{}",
                reason.map(|r| format!(": {r}")).unwrap_or_default()
            )
            .chars()
            .take(160)
            .collect::<String>(),
        )
    } else {
        None
    };
    let total = content.chars().count();
    let mut chunk: String = content.chars().skip(p.offset).take(limit).collect();
    loop {
        let next = p.offset.saturating_add(chunk.chars().count());
        let result = json!({"task":summary(task),"section":p.section,"key":p.key,
            "revision":revision,"diagnostic":diagnostic,"offset":p.offset,"end_offset":next,
            "content_json":chunk,"total_chars":total,"truncated":next < total,
            "next_offset":(next < total).then_some(next)});
        if chunk.is_empty() && p.offset < total {
            return Err(bad("task metadata leaves no room for content"));
        }
        if serde_json::to_vec(&result)?.len() <= RESULT_MAX_BYTES {
            return Ok(result);
        }
        if chunk.pop().is_none() {
            return Err(bad("task metadata exceeds read bound"));
        }
    }
}

fn read(ctx: &Ctx, envelope: &Envelope, op: AgentOp, project: &str) -> Result<Value> {
    let projects = if project == super::super::hooks::pm::WORKSPACE {
        super::super::hooks::pm::projects(&ctx.store)?
    } else {
        [project.to_owned()].into()
    };
    match op {
        AgentOp::InboxRead => {
            super::pm_inbox::read(ctx, super::payload(envelope, super::pm_inbox::READ_FIELDS)?)
        }
        AgentOp::ReadShow => {
            let _timing = corral_core::timing::span("agent.read_show");
            let p: Show = super::payload(envelope, "task, section?, key?, offset?, limit?")?;
            let task = target(ctx, &p.task, project)?;
            // A run carries many receipts and a specification may exceed a frame. Keep every
            // canonical section reachable without exposing filesystem or shell tools to the PM.
            if p.key.is_some() && !matches!(p.section, Section::Questions) {
                return Err(bad("key requires section=questions"));
            }
            let mut revision = None;
            let content = match p.section {
                Section::Overview => json!({
                    "spec_revision":crate::progress_spec_revision(&task)?,
                    "plan":task.plan,"write_scope":task.write_scope,
                    "run":task.current_run().map(|r| json!({"id":r.id,"outcome":r.outcome,
                        "gate":r.gate,"review":r.review,"blocked_excerpt":r.blocked_excerpt})),
                }),
                Section::Spec => json!(task.body),
                Section::Questions => {
                    let views = crate::questions::views(&task);
                    if let Some(key) = &p.key {
                        let view = views
                            .into_iter()
                            .find(|v| &v.question.key == key)
                            .ok_or_else(|| {
                                bad("question key not found in this task; use its canonical key")
                            })?;
                        revision = Some(view.question.revision);
                        serde_json::to_value(view)?
                    } else {
                        serde_json::to_value(views)?
                    }
                }
                Section::Automation => json!({
                    "run":task.current_run().map(|r| &r.id),
                    "autopilot":task.current_run().and_then(|r| r.autopilot.as_ref()),
                    "infra_wait":task.infra_wait,"park":task.park,"landed":task.landed,
                    "retry":task.current_run().and_then(|r| r.retry.as_ref()),
                    "operations":crate::post_land_operations(ctx, task.project.as_deref())?
                        .into_iter().filter(|op| op.task == task.id &&
                            task.current_run().is_some_and(|r| r.id == op.run))
                        .map(|op| json!({"key":op.key,"run":op.run,"landed_sha":op.landed_sha,
                            "landed":op.landed,"abandoned":op.abandoned,"ready":op.ready(),
                            "blocker":op.blocker(),"hooks":op.hooks.iter().map(|h|
                                json!({"name":h.hook.name,"state":h.state,"reason":h.reason,
                                    "attempts":h.attempts.len(),"latest":h.attempts.last().map(|a|
                                        json!({"number":a.number,"started":a.started,"finished":a.finished,
                                            "exit_code":a.exit_code,"timed_out":a.timed_out,
                                            "error":a.error,"log":a.log}))})).collect::<Vec<_>>() }))
                        .collect::<Vec<_>>(),
                    "blocker":crate::automation_blocker(ctx, &task)?,
                    "events":ctx.store.events_shared()?.iter().rev().filter(|e|
                        e.task.as_deref() == Some(task.id.as_str()) &&
                        matches!(e.kind.as_str(), "autopilot_error" | "automation_blocked" |
                            "automation_resumed" | "landing_blocked" | "autopilot"))
                        .take(8).collect::<Vec<_>>(),
                }),
                Section::Comments => serde_json::to_value(&task.comments)?,
                Section::Plans => serde_json::to_value(&task.plans)?,
            };
            let content = serde_json::to_string(&content)?;
            let limit = p.limit.unwrap_or(1024);
            if limit == 0 || limit > 4096 {
                return Err(bad("show limit must be 1..4096 characters"));
            }
            show_page(&task, &p, &content, limit, revision)
        }
        AgentOp::ReadRoute | AgentOp::ReadArtifacts => {
            let p: Target = super::payload(envelope, "task")?;
            let task = target(ctx, &p.task, project)?;
            match op {
                AgentOp::ReadRoute => Ok(serde_json::to_value(crate::routing::route_snapshot(
                    ctx,
                    &task,
                    &ProfileChoice::default(),
                    false,
                )?)?),
                _ => page(
                    task.artifacts
                        .iter()
                        .map(serde_json::to_value)
                        .collect::<std::result::Result<Vec<_>, _>>()?,
                    0,
                    100,
                ),
            }
        }
        AgentOp::ReadSearch => {
            let _timing = corral_core::timing::span("agent.read_search");
            let p: Search = super::payload(envelope, "query, limit?")?;
            let limit = p.limit.unwrap_or(20);
            let mut hits = Vec::new();
            for project in &projects {
                let options = crate::content_search::ContentSearch {
                    project: Some(project.clone()),
                    limit,
                    ..Default::default()
                };
                hits.extend(crate::content_search::search_content(
                    ctx, &p.query, &options,
                )?);
            }
            page(
                hits.iter()
                    .map(serde_json::to_value)
                    .collect::<std::result::Result<Vec<_>, _>>()?,
                0,
                limit,
            )
        }
        AgentOp::ReadList => {
            let _timing = corral_core::timing::span("agent.read_list");
            let p: Page = super::payload(envelope, "offset?, limit?, status?")?;
            let mut heads = Vec::new();
            for project in &projects {
                heads.extend(ctx.store.heads(
                    &corral_core::backend::HeadQuery::ProjectUpdated {
                        project: project.clone(),
                    },
                )?);
            }
            // Match Store::list order, not the project query's updated order.
            heads.sort_by(|a, b| (a.num, &a.id).cmp(&(b.num, &b.id)));
            let tasks = heads
                .iter()
                .filter(|head| p.status.is_none_or(|status| head.status == status))
                .map(summary)
                .collect();
            page(tasks, p.offset, p.limit.unwrap_or(20))
        }
        AgentOp::ReadBrief | AgentOp::ReadNext | AgentOp::ReadLessons => {
            let _timing = match op {
                AgentOp::ReadBrief => Some(corral_core::timing::span("agent.read_brief")),
                AgentOp::ReadNext => Some(corral_core::timing::span("agent.read_next")),
                _ => None,
            };
            let _: Empty = super::payload(envelope, "no fields")?;
            if op == AgentOp::ReadLessons {
                let mut lessons = Vec::new();
                for project in &projects {
                    lessons.extend(
                        ctx.store
                            .lessons(project)?
                            .iter()
                            .map(serde_json::to_value)
                            .collect::<std::result::Result<Vec<_>, _>>()?,
                    );
                }
                return page(lessons, 0, 100);
            }
            // Share CLI priority and effective authority, but select within approved projects.
            // This projects a readback only; it never invokes pm_act or sync.
            let next = crate::pm_workset::next_for_projects(ctx, &projects)?;
            if op == AgentOp::ReadNext {
                return super::pm_bound::bounded_next(&next, corral_core::agent::RESULT_MAX_BYTES);
            }
            let mut counts = std::collections::BTreeMap::new();
            let status_counts = ctx
                .store
                .view()?
                .aggregates(&[corral_core::backend::AggregateName::StatusCounts])?
                .status_counts;
            for count in status_counts {
                if count.n > 0 && projects.contains(&count.project) {
                    *counts.entry(count.status).or_insert(0u64) += count.n;
                }
            }
            // This is the same pending projection as questions::list(..., false):
            // native owners only, semantic closure applied, no task-history hydration.
            let mut heads = ctx.store.unfinished_native_heads()?;
            heads.retain(|head| head.project.as_ref().is_some_and(|p| projects.contains(p)));
            heads.sort_by(|a, b| (a.num, &a.id).cmp(&(b.num, &b.id)));
            let mut pending = Vec::new();
            for head in heads {
                pending.extend(crate::questions::recorded_pending_views_ref(&head)?);
            }
            let questions: Vec<_> = pending
                .into_iter()
                .filter(|q| q.pending)
                .map(|q| {
                    json!({"task":q.task,"key":q.question.key,
                    "revision":q.question.revision,"kind":q.question.kind,
                    "state":q.effective_state,"blocking":q.question.blocking})
                })
                .collect();
            let brief = super::pm_bound::bounded_brief(
                &next,
                json!({"project":project,"counts":counts,"questions":questions}),
                corral_core::agent::RESULT_MAX_BYTES,
            )?;
            super::pm_shadow::with_brief_digest(brief, &next)
        }
        _ => Err(super::unavailable(op, OWNER)),
    }
}

#[cfg(test)]
#[path = "pm_tests.rs"]
mod tests;
