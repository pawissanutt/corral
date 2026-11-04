//! A group answer: several members of one question group (one batched ask) answered in one task
//! transaction, so every member's state and message and the single corral-pi PM receipt commit
//! together or not at all. Each member keeps its own revision check and the answer rules of
//! `plan_answer`; a stale or refused member leaves the whole group untouched.
use super::*;
use crate::questions::GroupAnswer;

/// What a group answer applied: one outcome per answered member, and the PM receipt when the
/// corral-pi PM answered (stored and returned unchanged on replay).
#[derive(Debug, Clone, PartialEq)]
pub struct GroupApplied {
    pub group: String,
    pub outcomes: Vec<Outcome>,
    pub receipt: Option<RequestReceipt>,
    pub replayed: bool,
}

fn group_receipt(
    origin: &RequestOrigin,
    group: &str,
    outcomes: &[Outcome],
    at: Timestamp,
) -> Result<RequestReceipt> {
    let result = json!({"key": group, "group": group, "answers": outcomes});
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
            reference: group.into(),
        }),
        evidence: Vec::new(),
        post_pending: Vec::new(),
        at,
        committed_at: Some(at),
    })
}

/// Answers the listed members of the group that `key` (the group key or any member's) belongs to.
/// An entry's `text` falls back to the shared text; basis refs and scope note are shared.
pub fn answer_group(
    ctx: &Ctx,
    key: &str,
    actor: Actor<'_>,
    shared: &AnswerInput,
    answers: &[GroupAnswer],
) -> Result<GroupApplied> {
    if answers.is_empty() || answers.len() > agent::QUESTION_GROUP_MAX {
        return Err(refuse(
            RefusalKind::Bounds,
            format!(
                "{} answers; a group answer names 1 to {}",
                answers.len(),
                agent::QUESTION_GROUP_MAX
            ),
        ));
    }
    let by = actor.by();
    let pm = matches!(actor, Actor::Pm(_));
    let parsed = QuestionKey::parse(key)?;
    let container = parsed.container.clone();
    if ctx.store.get(parsed.task()).is_err() {
        return Err(refuse(
            RefusalKind::NotFound,
            format!("question {key} not found"),
        ));
    }
    let inbox = super::super::recipient(&container)
        .map(|r| Inbox::new(&ctx.store, r))
        .transpose()?;
    let _inbox_lock = inbox.as_ref().map(Inbox::lock).transpose()?;
    let now = corral_core::clock::now();
    let mut applied: Option<GroupApplied> = None;
    let mut posted: Vec<Message> = Vec::new();
    ctx.store.try_update(parsed.task(), |task| {
        if let Some(request) = actor.pm_request() {
            if let Some(stored) = pm_replay(ctx, task, request.origin)? {
                applied = Some(GroupApplied {
                    group: key.into(),
                    outcomes: Vec::new(),
                    receipt: Some(stored),
                    replayed: true,
                });
                return Ok(false);
            }
            (request.guard)(task)?;
        }
        let find = |task: &Task, key: &str| {
            agent::container_state(task, &container)
                .and_then(|state| state.questions.iter().find(|q| q.key == key))
                .cloned()
                .ok_or_else(|| refuse(RefusalKind::NotFound, format!("question {key} not found")))
        };
        let leader = find(task, key)?;
        let group = leader.group.clone().unwrap_or(leader.key);
        let mut outcomes = Vec::new();
        let mut messages = Vec::new();
        for (i, entry) in answers.iter().enumerate() {
            if answers[..i].iter().any(|a| a.key == entry.key) {
                return Err(refuse(
                    RefusalKind::Bounds,
                    format!("{} is answered twice", entry.key),
                ));
            }
            let current = find(task, &entry.key)?;
            if current.group.as_deref().unwrap_or(&current.key) != group {
                return Err(refuse(
                    RefusalKind::NotFound,
                    format!("{} is not in question group {group}", entry.key),
                ));
            }
            refuse_closed(task, &container, &current)?;
            if entry.rev != current.revision {
                return Err(refuse(
                    RefusalKind::StaleRevision,
                    format!(
                        "{} is at revision {}, not {}",
                        entry.key, current.revision, entry.rev
                    ),
                ));
            }
            let input = AnswerInput {
                text: entry.text.clone().unwrap_or_else(|| shared.text.clone()),
                option: entry.option.clone(),
                basis_refs: shared.basis_refs.clone(),
                scope_note: shared.scope_note.clone(),
            };
            let change = plan_answer(ctx, task, &current, &input, pm, by, now)?
                .context("the answer produced no change")?;
            let mut question = change.question;
            super::super::bound_triage(&ctx.store, &mut question)?;
            let message = match (&inbox, change.message) {
                (Some(inbox), Some((kind, body))) => Some(reserve(
                    inbox,
                    task,
                    &container,
                    &entry.key,
                    by,
                    (kind, &body),
                    &mut question,
                )?),
                _ => None,
            };
            outcomes.push(Outcome {
                key: entry.key.clone(),
                revision: question.revision,
                state: question.state,
                message_id: message.as_ref().map(|m| m.id.clone()),
                delivery: if message.is_some() { "queued" } else { "none" },
            });
            messages.extend(message);
            let slot = agent::container_state_mut(task, &container)
                .and_then(|state| super::super::find_mut(state, &entry.key))
                .context("the question disappeared inside its transaction")?;
            *slot = question;
        }
        apply_needs_me(task, &container, &NeedsMe::Settle);
        let receipt = match actor.pm_request() {
            Some(request) => {
                let receipt = group_receipt(request.origin, &group, &outcomes, now)?;
                task.agent_pm_receipts.push(receipt.clone());
                Some(receipt)
            }
            None => None,
        };
        posted = messages;
        applied = Some(GroupApplied {
            group,
            outcomes,
            receipt,
            replayed: false,
        });
        Ok(true)
    })?;
    if let Some(inbox) = &inbox {
        for message in &posted {
            inbox.announce(message)?;
        }
    }
    let applied = applied.context("the group answer produced no change")?;
    for outcome in &applied.outcomes {
        super::super::events::reconcile_question(&ctx.store, &outcome.key)?;
        crate::agent::herdr_status::push_after_transition(ctx, &container, &outcome.key);
    }
    Ok(applied)
}
