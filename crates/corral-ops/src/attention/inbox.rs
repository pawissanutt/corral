//! Read-only, workspace-local navigation. Routes are instructions, never executable actions.
use anyhow::Result;
use corral_core::{ReceiptOutcome, Task};
use jiff::Timestamp;
use serde::Serialize;

use crate::{Ctx, questions};

#[derive(Debug, Serialize)]
pub struct Inbox {
    pub version: u32,
    pub workspace: String,
    pub observed_at: Timestamp,
    pub items: Vec<Item>,
}

#[derive(Debug, Serialize)]
pub struct Item {
    pub kind: &'static str,
    pub task: String,
    pub container: Option<String>,
    pub key: String,
    /// Native question revision, otherwise a digest of the canonical source record.
    pub revision: String,
    pub revision_kind: &'static str,
    pub state: String,
    pub pending: bool,
    pub scope: String,
    pub reason: String,
    pub responsible: String,
    pub detail: Vec<String>,
    pub inspect: String,
    /// Existing external answer flow. Never dispatched by the inspector.
    pub response: Option<String>,
    pub pm_eligible: bool,
}

fn q(value: &str) -> String {
    crate::gate_script::q(value)
}

fn digest(value: &impl Serialize) -> Result<String> {
    Ok(crate::verification::digest(&serde_json::to_vec(value)?))
}

fn questions(task: &Task, bin: &str, items: &mut Vec<Item>) {
    for view in questions::views(task) {
        let question = &view.question;
        // Existing canonical triage allows a PM answer of any pending kind, but only with
        // resolvable basis refs and an approved scope note. Kind is not permission authority.
        let pm = view.pending;
        let mut detail = vec![format!(
            "{:?}; blocking={}",
            question.kind, question.blocking
        )];
        if let Some(group) = &question.group {
            detail.push(format!(
                "Group: {group} (one batched ask; answer together with question answer-group)"
            ));
        }
        if let Some(closure) = view.closure {
            detail.push(format!("Container closed: {}", closure.as_str()));
        }
        if let Some(terminal) = &question.terminal {
            detail.push(format!("Terminal reason: {}", terminal.reason));
        }
        if let Some(answer) = &question.answer {
            detail.push(format!("Answer ({:?}): {}", answer.by, answer.text));
            if let Some(scope) = &answer.scope_note {
                detail.push(format!("Answer scope: {scope}"));
            }
        }
        if let Some(recommendation) = &question.recommendation {
            detail.push(format!("Recommendation: {recommendation}"));
        }
        if let Some(deadline) = &question.deadline {
            detail.push(format!(
                "Owner deadline: {} for revision {}: {}",
                deadline.at, deadline.for_revision, deadline.instruction
            ));
        }
        for triage in &question.triage {
            detail.push(format!(
                "{} by {:?} @{}: {}",
                triage.action,
                triage.by,
                triage.revision,
                triage.note.as_deref().unwrap_or("no note")
            ));
        }
        detail.extend(
            question
                .options
                .iter()
                .map(|option| format!("Option: {option}")),
        );
        detail.extend(
            question
                .context_refs
                .iter()
                .map(|reference| format!("Context: {reference}")),
        );
        let response = view.pending.then(|| {
            let owner = format!(
                "In a separate trusted owner terminal: {bin} question answer {} --rev {} --owner --text <answer>",
                q(&question.key), question.revision
            );
            let owner = match &question.group {
                Some(group) => format!(
                    "{owner}\nWhole group: {bin} question answer-group {} --owner --text <answer> (or --answers JSON per member)",
                    q(group)
                ),
                None => owner,
            };
            format!("{owner}\nPM: use the existing authenticated corral_question_answer tool with this key and revision, canonical basis_refs and scope_note. Rust validates authority and scope again; the panel supplies no basis or permission.")
        });
        items.push(Item {
            kind: "question",
            task: task.id.clone(),
            container: Some(view.container),
            key: question.key.clone(),
            revision: question.revision.to_string(),
            revision_kind: "question_revision",
            state: format!("{:?}", view.effective_state).to_lowercase(),
            pending: view.pending,
            scope: task.project.clone().unwrap_or_else(|| "unassigned".into()),
            reason: question.text.clone(),
            responsible: if questions::needs_owner(question) {
                "owner"
            } else {
                "pm_or_owner"
            }
            .into(),
            detail,
            inspect: format!("{bin} question show {} --json", q(&question.key)),
            response,
            pm_eligible: pm,
        });
    }
}

fn portable(task: &Task, bin: &str, items: &mut Vec<Item>) -> Result<()> {
    for run in &task.runs {
        for decision in &run.obligations.decisions {
            let request = &decision.request;
            // Historical portable records are retained as recorded. The existing resume flow also
            // permits exact receipts for the current finished run; do not invent cancellation.
            let current = task
                .current_run()
                .is_some_and(|current| current.id == run.id);
            let pending = decision.pending() && current;
            let state = if let Some(receipt) = &decision.receipt {
                match receipt.outcome {
                    ReceiptOutcome::Answered => "answered",
                    ReceiptOutcome::Cancelled => "cancelled",
                    _ => decision.observation.state.as_str(),
                }
            } else if !current {
                "closed"
            } else {
                decision.observation.state.as_str()
            };
            let mut detail = vec![
                format!("Requested action: {}", request.requested_action),
                decision.observation.detail.clone(),
            ];
            if let Some(receipt) = &decision.receipt {
                detail.push(format!(
                    "Receipt {}: {:?}; delivery {:?}",
                    receipt.id, receipt.outcome, receipt.delivery
                ));
                if let Some(answer) = &receipt.answer {
                    detail.push(format!("Answer: {answer}"));
                }
            }
            items.push(Item {
                kind: "portable_decision", task: task.id.clone(), container: Some(format!("{}/{}", task.id, run.id)),
                key: format!("{}/{}/{}", task.id, run.id, request.id), revision: digest(decision)?, revision_kind: "source_digest",
                state: state.into(), pending, scope: task.project.clone().unwrap_or_else(|| "unassigned".into()),
                reason: request.question.clone(), responsible: request.owner.clone(), detail,
                inspect: format!("{bin} show {} --json", q(&task.id)),
                response: pending.then(|| format!(
                    "In a separate trusted owner terminal, use the established receipt flow: {bin} run resume {} --decision {} --file <receipt.json>\nReceipt version=1, run={}, outcome=answered, nonempty answer, unique id and observed_at inside this registration's lifetime. Inspect run progress/automation documentation for the complete ResumeReceipt. This panel does not resume the run.",
                    q(&task.id), q(&request.id), run.id
                )), pm_eligible: false,
            });
        }
    }
    Ok(())
}

/// Frozen canonical task reads plus the existing read-only attention and PM projections.
/// No reconciliation, acknowledgement, presentation, dispatch or answer writes happen here.
pub fn list(ctx: &Ctx, project: Option<&str>, now: Timestamp) -> Result<Inbox> {
    let ctx = Ctx {
        store: ctx.store.read_only_snapshot()?,
        ..ctx.clone()
    };
    let tasks = ctx.store.list_shared()?;
    let board = super::board(&ctx, now)?;
    let bin = format!(
        "corral --root {}",
        q(&ctx.store.root().display().to_string())
    );
    let mut items = Vec::new();
    for task in tasks
        .iter()
        .filter(|task| project.is_none_or(|project| task.project.as_deref() == Some(project)))
    {
        questions(task, &bin, &mut items);
        portable(task, &bin, &mut items)?;
    }
    for row in &board.tasks {
        if project.is_some_and(|project| row.project.as_deref() != Some(project)) {
            continue;
        }
        for condition in &row.assessment.conditions {
            // Questions and portable decisions above carry the exact answer identities.
            if condition.kind.starts_with("question_") || condition.kind == "decision_pending" {
                continue;
            }
            let revision = digest(&(
                condition.key.as_ref(),
                &condition.kind,
                &condition.reason,
                condition.since,
                &condition.resume,
                &condition.next_action,
                condition.responsible,
            ))?;
            items.push(Item {
                kind: "attention",
                task: row.task.clone(),
                container: None,
                key: condition
                    .key
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| {
                        format!("{}/{}/unkeyed:{revision}", row.task, condition.kind)
                    }),
                revision,
                revision_kind: "source_digest",
                state: condition.kind.clone(),
                pending: true,
                scope: row.project.clone().unwrap_or_else(|| "unassigned".into()),
                reason: condition.reason.clone(),
                responsible: condition.responsible.label().into(),
                detail: vec![
                    if condition.key.is_some() {
                        "Canonical condition key".into()
                    } else {
                        "No canonical condition key; digest identifies this inspection row only"
                            .into()
                    },
                    condition.next_action.description.clone(),
                    format!(
                        "Resume checks: {}",
                        serde_json::to_string(&condition.resume)?
                    ),
                ],
                inspect: format!("{bin} show {} --json", q(&row.task)),
                response: None,
                pm_eligible: false,
            });
        }
    }
    // Retained PM coordination decisions (including answered history), not synthesized owner
    // actions. Native/portable response lifecycles are still represented above, separately.
    items.extend(crate::pm_decisions::inbox(&ctx, project, &tasks, &bin)?);
    let rank = |kind| match kind {
        "question" => 0,
        "portable_decision" => 1,
        "coordination_decision" => 2,
        _ => 3,
    };
    items.sort_by(|a, b| {
        (!a.pending, rank(a.kind), &a.task, &a.key).cmp(&(
            !b.pending,
            rank(b.kind),
            &b.task,
            &b.key,
        ))
    });
    Ok(Inbox {
        version: 1,
        workspace: ctx.store.root().display().to_string(),
        observed_at: now,
        items,
    })
}
