//! PM and owner triage by key and revision: answer (and owner supersede), escalate, cancel and the
//! owner's deadline. Each action is one task transaction that takes the recipient's
//! `Inbox::lock()` first and reserves its message with `reserve_in`, so the question state, a
//! Pi-native PM's receipt and the message commit together (deliver -> inbox -> task order).
use anyhow::{Context, Result};
use corral_core::agent::{
    self, AGENT_RECORD_VERSION, ANSWER_TEXT_MAX_BYTES, AgentQuestion, AnswerBy, ContainerRef,
    QUESTION_CONTEXT_REFS_MAX, QuestionActor, QuestionAnswer, QuestionDeadline, QuestionKind,
    QuestionState, RESULT_MAX_BYTES, ReceiptEffect, ReceiptState, RequestOrigin, RequestReceipt,
};
use corral_core::inbox::{Inbox, Message, MessageBinding, Post, Urgency};
use corral_core::{Status, Task};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    NOTE_MAX_BYTES, QuestionKey, QuestionRefusal, REF_MAX_BYTES, RefusalKind, bounded, refuse,
    transition, triage,
};
use crate::Ctx;

/// A Pi-native PM request: its admitted origin and the PM liveness, fence and project-scope
/// check that the commit transaction repeats on the target task.
pub struct PmRequest<'a> {
    pub origin: &'a RequestOrigin,
    pub guard: &'a dyn Fn(&Task) -> Result<()>,
}

/// Who acts. `Pm(None)` is the PM on the CLI; `Owner` is trust-based like other `--owner` verbs.
pub enum Actor<'a> {
    Pm(Option<PmRequest<'a>>),
    Owner,
}

impl Actor<'_> {
    fn by(&self) -> QuestionActor {
        match self {
            Actor::Pm(_) => QuestionActor::Pm,
            Actor::Owner => QuestionActor::Owner,
        }
    }

    fn pm_request(&self) -> Option<&PmRequest<'_>> {
        match self {
            Actor::Pm(request) => request.as_ref(),
            Actor::Owner => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerInput {
    pub text: String,
    #[serde(default)]
    pub option: Option<String>,
    #[serde(default)]
    pub basis_refs: Vec<String>,
    #[serde(default)]
    pub scope_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Outcome {
    pub key: String,
    pub revision: u32,
    pub state: QuestionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// `queued` (reserved in the recipient's inbox), `pending_recipient` (a review child's
    /// recipient lands with integration-seams) or `none`.
    pub delivery: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Applied {
    Fresh {
        outcome: Outcome,
        receipt: Option<RequestReceipt>,
    },
    /// A Pi-native PM replayed a committed request; its stored receipt is returned unchanged.
    Replayed(RequestReceipt),
}

impl Applied {
    pub fn outcome(&self) -> Option<&Outcome> {
        match self {
            Applied::Fresh { outcome, .. } => Some(outcome),
            Applied::Replayed(_) => None,
        }
    }
}

pub(super) enum NeedsMe {
    Unchanged,
    Raise(&'static str),
    Settle,
}

pub(super) struct Change {
    pub question: AgentQuestion,
    pub message: Option<(&'static str, String)>,
    pub needs_me: NeedsMe,
}

pub(super) const EXCERPT_PREFIX: &str = agent::QUESTION_HOLD_PREFIX;

fn excerpt(question: &AgentQuestion) -> String {
    format!(
        "{EXCERPT_PREFIX}{} (revision {}, {:?}): {}",
        question.key, question.revision, question.kind, question.text
    )
}

/// Returns a task to Running (worker) or Review (review child) once no question of the
/// container waits on the owner, keeping any block that a question did not raise. Withdrawal
/// and terminalization call it in their own transactions.
pub fn settle_needs_me(task: &mut Task, container: &ContainerRef) {
    apply_needs_me(task, container, &NeedsMe::Settle);
}

/// Holds Needs me for every open container's pending question that waits on the owner (an
/// escalation or an unattended mark), however it got there, so sync never leaves one
/// unraised. Returns whether the task changed.
pub fn hold_owner_attention(ctx: &Ctx, task_id: &str) -> Result<bool> {
    let mut notice = None;
    let held = ctx.store.try_update(task_id, |task| {
        let mut changed = false;
        for container in super::containers(task) {
            if agent::container_closure(task, &container).is_some() {
                continue;
            }
            changed |= apply_needs_me(task, &container, &NeedsMe::Raise("question_attention"));
        }
        if changed {
            notice = Some(record_entry_notice(ctx, task, Timestamp::now())?);
        }
        Ok(changed)
    })?;
    if held.is_some() {
        crate::obligations::transition_with_notice(
            ctx,
            task_id,
            "needs_me",
            json!({"reason": "question_attention"}),
            notice,
        )?;
    }
    Ok(held.is_some())
}

/// Moves the task to Needs me for a question that waits on the owner, from Running for a
/// worker run or planner, or from Review for a review child or plan reviewer (whose binding stays
/// live through `agent::question_hold` or its open plan review), and back once none of the
/// container's questions waits on the owner, mirroring `register_decision`. The hold lives in the
/// run's `blocked_excerpt`, or a planning container's own `question_hold`, so only a hold that a
/// question raised is ever cleared.
fn apply_needs_me(task: &mut Task, container: &ContainerRef, needs_me: &NeedsMe) -> bool {
    let resting = match container {
        ContainerRef::Worker { .. } | ContainerRef::Planner { .. } => Status::Running,
        ContainerRef::Reviewer { .. } | ContainerRef::PlanReviewer { .. } => Status::Review,
        ContainerRef::Pm | ContainerRef::Assistant { .. } => return false,
    };
    let waiting = agent::container_state(task, container).and_then(|state| {
        state
            .questions
            .iter()
            .find(|q| q.recorded_pending() && super::needs_owner(q))
            .map(excerpt)
    });
    let status = task.status;
    let (slot, decision_pending) = match container {
        ContainerRef::Worker { run, .. } | ContainerRef::Reviewer { run, .. } => {
            let Some(record) = task.runs.iter_mut().find(|r| r.id == *run) else {
                return false;
            };
            let pending = record.obligations.pending_decision().is_some();
            (&mut record.blocked_excerpt, pending)
        }
        ContainerRef::Planner { .. } | ContainerRef::PlanReviewer { .. } => {
            let Some(state) = agent::container_state_mut(task, container) else {
                return false;
            };
            (&mut state.question_hold, false)
        }
        ContainerRef::Pm | ContainerRef::Assistant { .. } => return false,
    };
    let held = slot
        .as_deref()
        .is_some_and(|e| e.starts_with(EXCERPT_PREFIX));
    let (raised, next) = match needs_me {
        NeedsMe::Raise(_) if status == resting && waiting.is_some() => {
            *slot = waiting;
            (true, Some(Status::NeedsMe))
        }
        NeedsMe::Raise(_) | NeedsMe::Settle if status == Status::NeedsMe && held => match waiting {
            Some(next) => {
                *slot = Some(next);
                (false, None)
            }
            None if !decision_pending => {
                *slot = None;
                (false, Some(resting))
            }
            None => (false, None),
        },
        _ => (false, None),
    };
    if let Some(next) = next {
        task.status = next;
    }
    raised
}

fn record_entry_notice(
    ctx: &Ctx,
    task: &mut Task,
    now: Timestamp,
) -> Result<crate::owner_notice::PendingNotice> {
    let exact = super::containers(task)
        .iter()
        .filter_map(|container| agent::container_state(task, container))
        .flat_map(|state| &state.questions)
        .filter(|question| question.recorded_pending() && super::needs_owner(question))
        .map(|question| {
            format!(
                "{}/{}/{}@{}",
                task.id,
                if question.unattended_at.is_some() {
                    "question_unattended"
                } else {
                    "question_escalated"
                },
                question.key,
                question.revision
            )
        })
        .collect();
    crate::owner_notice::record_entry(ctx, task, "question_needs_me", now, exact)
}

fn refuse_closed(task: &Task, container: &ContainerRef, question: &AgentQuestion) -> Result<()> {
    // Container closure governs every new action, even on an already withdrawn question.
    // Committed PM receipt replays are handled by transact before reaching this check.
    if let Some(reason) = agent::container_closure(task, container) {
        return Err(refuse(
            RefusalKind::Closed,
            format!(
                "{} belongs to a closed container ({})",
                question.key,
                reason.as_str()
            ),
        ));
    }
    if question.state == QuestionState::Withdrawn {
        return Err(refuse(
            RefusalKind::Withdrawn,
            format!("{} was withdrawn", question.key),
        ));
    }
    if let Some(terminal) = &question.terminal {
        return Err(refuse(
            RefusalKind::Closed,
            format!("{} was closed ({})", question.key, terminal.reason),
        ));
    }
    Ok(())
}

/// A PM request ID is unique across the PM container: its AgentPm receipts and every task-held
/// receipt. Runs under the dispatch lock that every PM receipt writer holds, so the scan is
/// serialized with them. An identical replay returns the target task's stored receipt.
fn pm_replay(ctx: &Ctx, task: &Task, origin: &RequestOrigin) -> Result<Option<RequestReceipt>> {
    let same = |r: &&RequestReceipt| r.request_id == origin.request_id;
    let mismatch = || {
        refuse(
            RefusalKind::PayloadMismatch,
            "request_id was already used with a different op or payload",
        )
    };
    if let Some(stored) = task.agent_pm_receipts.iter().find(same) {
        if stored.op != origin.op || stored.payload_sha256 != origin.payload_sha256 {
            return Err(mismatch());
        }
        return Ok(Some(stored.clone()));
    }
    let in_pm = agent::pm_state::load(&ctx.store)?
        .is_some_and(|pm| pm.state.receipts.iter().any(|r| same(&r)));
    let in_task = ctx
        .store
        .list_shared()?
        .iter()
        .filter(|t| t.id != task.id)
        .any(|t| t.agent_pm_receipts.iter().any(|r| same(&r)));
    if in_pm || in_task {
        return Err(mismatch());
    }
    Ok(None)
}

fn pm_receipt(origin: &RequestOrigin, outcome: &Outcome, at: Timestamp) -> Result<RequestReceipt> {
    let result = serde_json::to_value(outcome)?;
    let bytes = serde_json::to_vec(&result)?.len();
    anyhow::ensure!(
        bytes <= RESULT_MAX_BYTES,
        "receipt result exceeds {RESULT_MAX_BYTES} bytes"
    );
    Ok(RequestReceipt {
        version: AGENT_RECORD_VERSION,
        request_id: origin.request_id.clone(),
        op: origin.op.clone(),
        payload_sha256: origin.payload_sha256.clone(),
        identity: origin.identity.clone(),
        state: ReceiptState::Committed,
        result,
        executor: None,
        effect: Some(ReceiptEffect {
            kind: "question".into(),
            reference: outcome.key.clone(),
        }),
        evidence: Vec::new(),
        post_pending: Vec::new(),
        at,
        committed_at: Some(at),
    })
}

/// One question action. `plan` computes the next record from the current one and the task, or
/// `None` to leave everything untouched (sweeps whose condition no longer holds).
pub(super) fn transact(
    ctx: &Ctx,
    key: &str,
    revision: Option<u32>,
    actor: &Actor<'_>,
    by: QuestionActor,
    now: Timestamp,
    plan: impl FnOnce(&Ctx, &Task, &AgentQuestion, Timestamp) -> Result<Option<Change>>,
) -> Result<Option<Applied>> {
    let parsed = QuestionKey::parse(key)?;
    let container = parsed.container.clone();
    if ctx.store.get(parsed.task()).is_err() {
        return Err(refuse(
            RefusalKind::NotFound,
            format!("question {key} not found"),
        ));
    }
    let inbox = super::recipient(&container)
        .map(|r| Inbox::new(&ctx.store, r))
        .transpose()?;
    let _inbox_lock = inbox.as_ref().map(Inbox::lock).transpose()?;
    let mut applied = None;
    let mut posted: Option<Message> = None;
    let mut raised: Option<&'static str> = None;
    let mut notice = None;
    let mut transitioned = false;
    ctx.store.try_update(parsed.task(), |task| {
        if let Some(pm) = actor.pm_request() {
            if let Some(stored) = pm_replay(ctx, task, pm.origin)? {
                applied = Some(Applied::Replayed(stored));
                return Ok(false);
            }
            (pm.guard)(task)?;
        }
        let current = agent::container_state(task, &container)
            .and_then(|state| state.questions.iter().find(|q| q.key == key))
            .cloned()
            .ok_or_else(|| refuse(RefusalKind::NotFound, format!("question {key} not found")))?;
        refuse_closed(task, &container, &current)?;
        if let Some(revision) = revision
            && revision != current.revision
        {
            return Err(refuse(
                RefusalKind::StaleRevision,
                format!("{key} is at revision {}, not {revision}", current.revision),
            ));
        }
        let Some(change) = plan(ctx, task, &current, now)? else {
            return Ok(false);
        };
        let mut question = change.question;
        transitioned = question.revision != current.revision;
        super::bound_triage(&ctx.store, &mut question)?;
        let message = match (&inbox, change.message) {
            (Some(inbox), Some((kind, body))) => {
                let message = inbox.reserve_in(
                    task,
                    Post {
                        kind,
                        urgency: Urgency::Wake,
                        author: by_name(by),
                        body: &body,
                        binding: MessageBinding {
                            run: match &container {
                                ContainerRef::Worker { run, .. }
                                | ContainerRef::Planner { run, .. } => Some(run.clone()),
                                ContainerRef::Reviewer { review, .. }
                                | ContainerRef::PlanReviewer { review, .. } => Some(review.clone()),
                                ContainerRef::Pm | ContainerRef::Assistant { .. } => None,
                            },
                            // Not question-bound: that convention marks an OmO native answer
                            // (`dispatch/inbox_reply.rs`) which the inbox never packs. A triage
                            // answer is a `pi_native` message and travels in an envelope.
                            ..MessageBinding::default()
                        },
                        subject: Some(json!({"key": key, "revision": question.revision})),
                        max_payload: corral_core::inbox::MAX_PAYLOAD,
                    },
                )?;
                if let Some(answer) = question.answer.as_mut() {
                    answer.message_id = Some(message.id.clone());
                }
                Some(message)
            }
            _ => None,
        };
        let delivery = if message.is_some() { "queued" } else { "none" };
        let outcome = Outcome {
            key: key.into(),
            revision: question.revision,
            state: question.state,
            message_id: message.as_ref().map(|m| m.id.clone()),
            delivery,
        };
        let slot = agent::container_state_mut(task, &container)
            .and_then(|state| super::find_mut(state, key))
            .context("the question disappeared inside its transaction")?;
        *slot = question;
        if apply_needs_me(task, &container, &change.needs_me)
            && let NeedsMe::Raise(reason) = change.needs_me
        {
            raised = Some(reason);
            notice = Some(record_entry_notice(ctx, task, now)?);
        }
        let receipt = match actor.pm_request() {
            Some(pm) => {
                let receipt = pm_receipt(pm.origin, &outcome, now)?;
                task.agent_pm_receipts.push(receipt.clone());
                Some(receipt)
            }
            None => None,
        };
        posted = message;
        applied = Some(Applied::Fresh { outcome, receipt });
        Ok(true)
    })?;
    if let Some(message) = &posted
        && let Some(inbox) = &inbox
    {
        inbox.announce(message)?;
    }
    if applied.is_some() {
        super::events::reconcile_question(&ctx.store, key)?;
    }
    // The asking host's herdr projection follows escalations and cancellations it did not make.
    if transitioned && matches!(applied, Some(Applied::Fresh { .. })) {
        crate::agent::herdr_status::push_after_transition(ctx, &container, key);
    }
    if let (Some(reason), Some(Applied::Fresh { outcome, .. })) = (raised, &applied) {
        crate::obligations::transition_with_notice(
            ctx,
            parsed.task(),
            "needs_me",
            json!({"run": container_run(&container), "question": key,
                   "revision": outcome.revision, "reason": reason}),
            notice,
        )?;
    }
    Ok(applied)
}

fn container_run(container: &ContainerRef) -> Option<&str> {
    match container {
        ContainerRef::Worker { run, .. } | ContainerRef::Reviewer { run, .. } => Some(run),
        ContainerRef::Pm
        | ContainerRef::Assistant { .. }
        | ContainerRef::Planner { .. }
        | ContainerRef::PlanReviewer { .. } => None,
    }
}

fn by_name(by: QuestionActor) -> &'static str {
    match by {
        QuestionActor::Agent => "agent",
        QuestionActor::Pm => "pm",
        QuestionActor::Owner => "owner",
        QuestionActor::System => "corral",
    }
}

fn rule(reason: impl Into<String>) -> anyhow::Error {
    refuse(RefusalKind::Rule, reason)
}

fn state_error(question: &AgentQuestion, wanted: &str) -> anyhow::Error {
    refuse(
        RefusalKind::State,
        format!("{} is {:?}; {wanted}", question.key, question.state),
    )
}

fn validate_answer(question: &AgentQuestion, input: &AnswerInput) -> Result<(), QuestionRefusal> {
    bounded(&input.text, ANSWER_TEXT_MAX_BYTES, "answer text")?;
    if let Some(option) = &input.option
        && !question.options.contains(option)
    {
        return Err(QuestionRefusal::new(
            RefusalKind::Bounds,
            format!("{option:?} is not one of the question's options"),
        ));
    }
    if input.basis_refs.len() > QUESTION_CONTEXT_REFS_MAX {
        return Err(QuestionRefusal::new(
            RefusalKind::Bounds,
            format!("at most {QUESTION_CONTEXT_REFS_MAX} basis refs"),
        ));
    }
    for reference in &input.basis_refs {
        bounded(reference, REF_MAX_BYTES, "basis ref")?;
    }
    if let Some(note) = &input.scope_note {
        bounded(note, NOTE_MAX_BYTES, "scope note")?;
    }
    Ok(())
}

pub(super) fn answer_body(question: &AgentQuestion, answer: &QuestionAnswer, kind: &str) -> String {
    let mut body = format!(
        "{} {} (revision {}) by {:?}:\n{}",
        if kind == "question_correction" {
            "Corrected answer to"
        } else {
            "Answer to"
        },
        question.key,
        question.revision,
        answer.by,
        answer.text
    );
    if let Some(option) = &answer.option {
        body.push_str(&format!("\nChosen option: {option}"));
    }
    body
}

/// The PM answers any pending question of any kind, citing at least one resolvable basis ref and a
/// scope note. The owner answers without them and may supersede an answer, which queues a
/// `question_correction`.
pub fn answer(
    ctx: &Ctx,
    key: &str,
    revision: u32,
    actor: Actor<'_>,
    input: AnswerInput,
) -> Result<Applied> {
    let by = actor.by();
    let pm = matches!(actor, Actor::Pm(_));
    let applied = transact(
        ctx,
        key,
        Some(revision),
        &actor,
        by,
        Timestamp::now(),
        |ctx, task, current, now| {
            validate_answer(current, &input)?;
            let superseding = matches!(
                current.state,
                QuestionState::Answered | QuestionState::Expired
            );
            if pm {
                if !current.state.pending() {
                    return Err(state_error(
                        current,
                        "the PM answers pending questions; only the owner supersedes",
                    ));
                }
                if input.basis_refs.is_empty() {
                    return Err(rule("a PM answer cites at least one basis ref"));
                }
                for reference in &input.basis_refs {
                    super::basis::resolve(ctx, task, reference).map_err(|why| {
                        rule(format!("basis ref {reference} does not resolve: {why}"))
                    })?;
                }
                if input.scope_note.is_none() {
                    return Err(rule(
                        "a PM answer states its scope note (approved write scope and acceptance)",
                    ));
                }
            } else if !current.state.pending() && !superseding {
                return Err(state_error(
                    current,
                    "only pending or answered questions take an owner answer",
                ));
            }
            let mut question = current.clone();
            let previous = current.answer.as_ref().map(|a| (current.revision, a.by));
            let answer = QuestionAnswer {
                by: if pm { AnswerBy::Pm } else { AnswerBy::Owner },
                text: input.text.clone(),
                option: input.option.clone(),
                basis_refs: input.basis_refs.clone(),
                scope_note: input.scope_note.clone(),
                at: now,
                message_id: None,
                superseded_by: None,
            };
            let (action, kind, note) = match previous.filter(|_| superseding) {
                Some((rev, prior)) => (
                    "supersede",
                    "question_correction",
                    Some(format!("supersedes the {prior:?} answer of revision {rev}")),
                ),
                None => ("answer", "question_answer", None),
            };
            transition(
                &mut question,
                QuestionState::Answered,
                action,
                by,
                note,
                now,
            );
            let body = answer_body(&question, &answer, kind);
            question.answer = Some(answer);
            Ok(Some(Change {
                question,
                message: Some((kind, body)),
                needs_me: NeedsMe::Settle,
            }))
        },
    )?;
    applied.context("the answer produced no change")
}

fn rank(kind: QuestionKind) -> u8 {
    match kind {
        QuestionKind::Clarification => 0,
        QuestionKind::TechnicalChoice => 1,
        QuestionKind::Permission => 2,
        QuestionKind::OwnerOnly => 3,
    }
}

/// Hands an open question to the owner, optionally reclassifying it upward (never downward). An
/// escalated question is held indefinitely; nothing auto-applies without an owner deadline.
pub fn escalate(
    ctx: &Ctx,
    key: &str,
    revision: u32,
    actor: Actor<'_>,
    note: Option<String>,
    kind: Option<QuestionKind>,
) -> Result<Applied> {
    let by = actor.by();
    let applied = transact(
        ctx,
        key,
        Some(revision),
        &actor,
        by,
        Timestamp::now(),
        |_, _, current, now| {
            if let Some(note) = &note {
                bounded(note, NOTE_MAX_BYTES, "note")?;
            }
            if current.state != QuestionState::Open {
                return Err(state_error(current, "only open questions escalate"));
            }
            let mut question = current.clone();
            if let Some(kind) = kind {
                if rank(kind) < rank(current.kind) {
                    return Err(rule(format!(
                        "reclassification goes upward only ({:?} -> {kind:?})",
                        current.kind
                    )));
                }
                question.kind = kind;
            }
            transition(
                &mut question,
                QuestionState::Escalated,
                "escalate",
                by,
                note.clone(),
                now,
            );
            Ok(Some(Change {
                question,
                message: None,
                needs_me: NeedsMe::Raise("question_escalated"),
            }))
        },
    )?;
    applied.context("the escalation produced no change")
}

pub fn cancel(
    ctx: &Ctx,
    key: &str,
    revision: u32,
    actor: Actor<'_>,
    reason: &str,
) -> Result<Applied> {
    let by = actor.by();
    let applied = transact(
        ctx,
        key,
        Some(revision),
        &actor,
        by,
        Timestamp::now(),
        |_, _, current, now| {
            bounded(reason, NOTE_MAX_BYTES, "reason")?;
            if !current.state.pending() {
                return Err(state_error(current, "only pending questions are cancelled"));
            }
            let mut question = current.clone();
            transition(
                &mut question,
                QuestionState::Cancelled,
                "cancel",
                by,
                Some(reason.into()),
                now,
            );
            Ok(Some(Change {
                question,
                message: None,
                needs_me: NeedsMe::Settle,
            }))
        },
    )?;
    applied.context("the cancellation produced no change")
}

/// Owner only: at `at`, if the revision is still `revision`, the question expires with an
/// `owner_deadline` answer carrying `instruction`. Setting it is not a transition, so the
/// revision is unchanged; any later transition voids it.
pub fn deadline(
    ctx: &Ctx,
    key: &str,
    revision: u32,
    at: Timestamp,
    instruction: &str,
) -> Result<Applied> {
    let actor = Actor::Owner;
    let applied = transact(
        ctx,
        key,
        Some(revision),
        &actor,
        QuestionActor::Owner,
        Timestamp::now(),
        |_, _, current, now| {
            bounded(instruction, ANSWER_TEXT_MAX_BYTES, "instruction")?;
            if !current.state.pending() {
                return Err(state_error(
                    current,
                    "only pending questions take a deadline",
                ));
            }
            let mut question = current.clone();
            question.deadline = Some(QuestionDeadline {
                set_by: QuestionActor::Owner,
                at,
                instruction: instruction.into(),
                for_revision: current.revision,
            });
            triage(
                &mut question,
                "deadline",
                QuestionActor::Owner,
                Some(at.to_string()),
                now,
            );
            Ok(Some(Change {
                question,
                message: None,
                needs_me: NeedsMe::Unchanged,
            }))
        },
    )?;
    applied.context("the deadline produced no change")
}
