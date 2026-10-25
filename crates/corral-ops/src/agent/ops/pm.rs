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

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Section {
    #[default]
    Overview,
    Spec,
    Questions,
    Comments,
    Plans,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Show {
    task: String,
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

fn scope(task: &Task, project: &str) -> Result<()> {
    if task.project.as_deref() != Some(project) {
        return Err(
            Rejection::new(RejectCode::OutOfScope, "target is outside the PM project").into(),
        );
    }
    Ok(())
}

fn target(ctx: &Ctx, id: &str, project: &str) -> Result<Task> {
    let task = ctx.store.get(&Store::normalize_id(id)?)?;
    scope(&task, project)?;
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
            let p: Capture = super::payload(envelope, "project, title, body, status?")?;
            text(&p.title)?;
            let status = p.status.unwrap_or(Status::Backlog);
            if !matches!(status, Status::Idea | Status::Backlog) {
                return Err(bad("capture may file only idea or backlog tasks"));
            }
            if p.project != project {
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
            match ctx.store.list()?.into_iter().find(|task| {
                task.agent_origin
                    .as_ref()
                    .is_some_and(|mark| mark.request_id == origin.request_id)
            }) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::receipts::FAILPOINT;
    use crate::agent::{self, BindTarget, test_support as ts};
    use corral_core::agent::{ContainerRef, pm_state};

    fn fixture() -> (ts::Fx, ts::Host) {
        let mut fx = ts::fx();
        for project in ["corral", "other"] {
            fx.ctx.store.config.projects.insert(
                project.into(),
                serde_json::from_value(json!({"path":fx.ctx.store.root()})).unwrap(),
            );
        }
        fx.ctx.store.save_config().unwrap();
        let host = ts::bind(
            &fx,
            &BindTarget::Pm {
                project: "corral".into(),
            },
            "pm-pane",
            "pm-session",
        );
        ts::committed(&ts::hello(&fx, &host));
        (fx, host)
    }

    fn send(fx: &ts::Fx, host: &ts::Host, op: &str, id: &str, payload: Value) -> CallResponse {
        let id = agent::tool_request_id(&host.identity.session_id, id);
        ts::send(fx, &ts::envelope(host, op, &id, payload, false), op)
    }

    fn task(fx: &ts::Fx, project: &str, body: &str) -> Task {
        fx.ctx
            .store
            .create(project, Some(project), Status::Backlog, body)
            .unwrap()
    }

    #[test]
    fn comment_replays_after_fencing_without_a_second_effect() {
        // Given a bound PM and a task in its scope.
        let (fx, host) = fixture();
        let task = task(&fx, "corral", "");
        let payload = json!({"task":task.id,"text":"technical context"});
        // When its acknowledgement is lost and its generation is fenced.
        let first = send(&fx, &host, "comment", "comment", payload.clone());
        agent::fence_generation(&fx.ctx.store, &ContainerRef::Pm, 1, "fixture").unwrap();
        let replay = send(&fx, &host, "comment", "comment", payload);
        // Then the stored task-local receipt is returned and exactly one comment exists.
        assert_eq!(first.receipt(), replay.receipt());
        assert_eq!(fx.ctx.store.get(&task.id).unwrap().comments.len(), 1);
        assert!(matches!(
            replay,
            CallResponse::Receipt { replayed: true, .. }
        ));
    }

    #[test]
    fn capture_replays_its_marker_without_creating_another_task() {
        // Given an authenticated PM and a capture in its project.
        let (fx, host) = fixture();
        let payload = json!({"project":"corral","title":"captured","body":"finite spec"});
        // When the same request is submitted twice.
        let first = send(&fx, &host, "capture", "capture", payload.clone());
        let replay = send(&fx, &host, "capture", "capture", payload);
        // Then the canonical creation marker and one stored result survive the lost ack.
        assert_eq!(first.receipt(), replay.receipt());
        let tasks = fx.ctx.store.list().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(
            tasks[0].agent_origin.as_ref().unwrap().request_id,
            first.receipt().unwrap().request_id
        );
        assert_eq!(tasks[0].status, Status::Backlog);
    }

    #[test]
    fn mutations_refuse_other_projects_and_owner_authority() {
        // Given eligible tasks in two projects and the PM scoped to only one.
        let (fx, host) = fixture();
        let inside = task(&fx, "corral", "");
        let outside = task(&fx, "other", "");
        // When the PM attempts each foreign target and an owner-only status.
        for op in ["comment", "reply"] {
            let reply = send(&fx, &host, op, op, json!({"task":outside.id,"text":"no"}));
            assert_eq!(ts::code(&reply), Some("out_of_scope"));
        }
        let foreign = send(
            &fx,
            &host,
            "capture",
            "foreign",
            json!({"project":"other","title":"no","body":""}),
        );
        let ready = send(
            &fx,
            &host,
            "capture",
            "ready",
            json!({"project":"corral","title":"no","body":"","status":"ready"}),
        );
        // Then both projects and their existing records stay unchanged.
        assert_eq!(ts::code(&foreign), Some("out_of_scope"));
        assert_eq!(ts::code(&ready), Some("bad_request"));
        assert_eq!(fx.ctx.store.list().unwrap().len(), 2);
        assert!(fx.ctx.store.get(&outside.id).unwrap().comments.is_empty());
        assert!(fx.ctx.store.get(&inside.id).unwrap().comments.is_empty());
    }

    #[test]
    fn capture_rechecks_scope_in_the_creation_transaction() {
        // Given admission in project corral, followed by a binding scope change.
        let (fx, host) = fixture();
        let store = fx.ctx.store.clone();
        FAILPOINT.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move |name| {
                if name == "pm_reserved_before_effect" {
                    pm_state::update(&store, |pm| {
                        pm.as_mut().unwrap().state.binding.project = "other".into();
                        Ok(((), true))
                    })
                    .unwrap();
                }
            }))
        });
        // When the capture reaches its initializer under the creation lock.
        let reply = send(
            &fx,
            &host,
            "capture",
            "scope-race",
            json!({"project":"corral","title":"no","body":""}),
        );
        FAILPOINT.with(|slot| *slot.borrow_mut() = None);
        // Then no task is created and the reservation reports the explicit scope refusal.
        let receipt = reply.receipt().unwrap();
        assert_eq!(receipt.state, ReceiptState::Rejected);
        assert_eq!(receipt.result["code"], "out_of_scope");
        assert!(fx.ctx.store.list().unwrap().is_empty());
    }

    #[test]
    fn comment_rechecks_generation_in_its_commit_transaction() {
        // Given admission before an old PM generation is fenced.
        let (fx, host) = fixture();
        let task = task(&fx, "corral", "");
        let store = fx.ctx.store.clone();
        FAILPOINT.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move |name| {
                if name == "pm_admitted_before_commit" {
                    agent::fence_generation(&store, &ContainerRef::Pm, 1, "fixture").unwrap();
                }
            }))
        });
        // When a comment reaches its existing operation's final transaction.
        let reply = send(
            &fx,
            &host,
            "comment",
            "fence-race",
            json!({"task":task.id,"text":"no"}),
        );
        FAILPOINT.with(|slot| *slot.borrow_mut() = None);
        // Then neither the comment nor its task-local receipt commits.
        assert_eq!(ts::code(&reply), Some("agent_fenced"));
        let saved = fx.ctx.store.get(&task.id).unwrap();
        assert!(saved.comments.is_empty() && saved.agent_pm_receipts.is_empty());
    }

    #[test]
    fn typed_reads_are_scoped_bounded_and_do_not_write_receipts() {
        // Given tasks in two projects, and a bound PM with one hello receipt.
        let (fx, host) = fixture();
        let outside = task(&fx, "other", "spec");
        let task = task(&fx, "corral", &"x".repeat(20_000));
        let before = pm_state::load(&fx.ctx.store).unwrap().unwrap();
        let bytes = ts::task_bytes(&fx, &task.id);
        // When the PM lists and inspects its records.
        let listed = send(&fx, &host, "read_list", "list", json!({"limit":1}));
        let shown = send(&fx, &host, "read_show", "show", json!({"task":task.id}));
        let spec = send(
            &fx,
            &host,
            "read_show",
            "spec",
            json!({"task":task.id,"section":"spec","limit":128}),
        );
        let foreign = send(
            &fx,
            &host,
            "read_show",
            "foreign-show",
            json!({"task":outside.id}),
        );
        // Then reads are bounded, foreign records are refused, and canonical state is unchanged.
        assert_eq!(listed.receipt().unwrap().result["total"], 1);
        assert_eq!(shown.receipt().unwrap().result["task"]["id"], task.id);
        assert_eq!(spec.receipt().unwrap().result["next_offset"], 128);
        assert!(
            spec.receipt().unwrap().result["total_chars"]
                .as_u64()
                .unwrap()
                > 20_000
        );
        assert_eq!(ts::code(&foreign), Some("out_of_scope"));
        assert_eq!(before, pm_state::load(&fx.ctx.store).unwrap().unwrap());
        assert_eq!(bytes, ts::task_bytes(&fx, &task.id));
    }
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

fn summary(task: &Task) -> Value {
    json!({"id":task.id,"title":task.title,"status":task.status,"project":task.project,
        "kind":task.kind,"size":task.size,"priority":task.priority})
}

fn page(values: Vec<Value>, offset: usize, limit: usize) -> Result<Value> {
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

fn read(ctx: &Ctx, envelope: &Envelope, op: AgentOp, project: &str) -> Result<Value> {
    match op {
        AgentOp::ReadShow => {
            let p: Show = super::payload(envelope, "task, section?, offset?, limit?")?;
            let task = target(ctx, &p.task, project)?;
            // A run carries many receipts and a specification may exceed a frame. Keep every
            // canonical section reachable without exposing filesystem or shell tools to the PM.
            let content = match p.section {
                Section::Overview => json!({
                    "spec_revision":crate::progress_spec_revision(&task)?,
                    "plan":task.plan,"write_scope":task.write_scope,
                    "run":task.current_run().map(|r| json!({"id":r.id,"outcome":r.outcome,
                        "gate":r.gate,"review":r.review,"blocked_excerpt":r.blocked_excerpt})),
                }),
                Section::Spec => json!(task.body),
                Section::Questions => serde_json::to_value(crate::questions::views(&task))?,
                Section::Comments => serde_json::to_value(&task.comments)?,
                Section::Plans => serde_json::to_value(&task.plans)?,
            };
            let content = serde_json::to_string(&content)?;
            let limit = p.limit.unwrap_or(1024);
            if limit == 0 || limit > 1024 {
                return Err(bad("show limit must be 1..1024 characters"));
            }
            let total = content.chars().count();
            let chunk: String = content.chars().skip(p.offset).take(limit).collect();
            let next = p.offset.saturating_add(chunk.chars().count());
            Ok(
                json!({"task":summary(&task),"content_json":chunk,"total_chars":total,
                "next_offset":(next < total).then_some(next)}),
            )
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
            let p: Search = super::payload(envelope, "query, limit?")?;
            let limit = p.limit.unwrap_or(20);
            let options = crate::content_search::ContentSearch {
                project: Some(project.into()),
                limit,
                ..Default::default()
            };
            let hits = crate::content_search::search_content(ctx, &p.query, &options)?;
            page(
                hits.iter()
                    .map(serde_json::to_value)
                    .collect::<std::result::Result<Vec<_>, _>>()?,
                0,
                limit,
            )
        }
        AgentOp::ReadList => {
            let p: Page = super::payload(envelope, "offset?, limit?, status?")?;
            let tasks = ctx
                .store
                .list()?
                .iter()
                .filter(|t| {
                    t.project.as_deref() == Some(project) && p.status.is_none_or(|s| t.status == s)
                })
                .map(summary)
                .collect();
            page(tasks, p.offset, p.limit.unwrap_or(20))
        }
        AgentOp::ReadBrief | AgentOp::ReadNext | AgentOp::ReadLessons => {
            let _: Empty = super::payload(envelope, "no fields")?;
            if op == AgentOp::ReadLessons {
                return page(
                    ctx.store
                        .lessons(project)?
                        .iter()
                        .map(serde_json::to_value)
                        .collect::<std::result::Result<Vec<_>, _>>()?,
                    0,
                    100,
                );
            }
            let tasks: Vec<_> = ctx
                .store
                .list()?
                .into_iter()
                .filter(|t| t.project.as_deref() == Some(project))
                .collect();
            // Existing read-only observations/policy, never pm_next's workset/action executor.
            let observations = crate::pm_next::observe::observations(ctx)?
                .into_iter()
                .filter(|o| {
                    serde_json::to_value(o)
                        .ok()
                        .and_then(|v| v["task"].as_str().map(str::to_owned))
                        .is_some_and(|id| tasks.iter().any(|t| t.id == id))
                })
                .collect::<Vec<_>>();
            let next = crate::pm_next::pm_select(&observations, "corral");
            if op == AgentOp::ReadNext {
                return Ok(serde_json::to_value(next)?);
            }
            let mut counts = std::collections::BTreeMap::new();
            for task in &tasks {
                *counts.entry(task.status.as_str()).or_insert(0usize) += 1;
            }
            let questions: Vec<_> = tasks
                .iter()
                .flat_map(crate::questions::views)
                .filter(|q| q.pending)
                .map(|q| {
                    json!({"task":q.task,"key":q.question.key,
                    "revision":q.question.revision,"kind":q.question.kind,
                    "state":q.effective_state,"blocking":q.question.blocking})
                })
                .collect();
            Ok(json!({"project":project,"counts":counts,"next":next,"questions":questions}))
        }
        _ => Err(super::unavailable(op, OWNER)),
    }
}
