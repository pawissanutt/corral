//! Read-only, workspace-local navigation. Routes are instructions, never executable actions.
use anyhow::Result;
use corral_core::{ReceiptOutcome, Task};
use jiff::Timestamp;
use serde::Serialize;

use crate::Ctx;
use crate::questions::{self, QuestionView};

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
    let views = questions::views(task);
    for view in &views {
        match view.question.group.as_deref() {
            // A batched ask is one item under its first member; the others fold into it.
            Some(group) if group == view.question.key => {
                let members: Vec<&QuestionView> = views
                    .iter()
                    .filter(|v| v.question.group.as_deref() == Some(group))
                    .collect();
                items.push(group_item(task, bin, group, &members));
            }
            Some(_) => {}
            None => items.push(question_item(task, bin, view)),
        }
    }
}

/// One question group: every member's content and exact answer identity (key and revision).
fn group_item(task: &Task, bin: &str, group: &str, members: &[&QuestionView]) -> Item {
    let pending: Vec<&&QuestionView> = members.iter().filter(|v| v.pending).collect();
    let mut detail = vec![format!(
        "Question group {group}: {} questions from one batched ask, {} pending",
        members.len(),
        pending.len()
    )];
    for view in members {
        let q = &view.question;
        detail.push(format!(
            "── {} rev {} ({}): {}",
            q.key,
            q.revision,
            format!("{:?}", view.effective_state).to_lowercase(),
            q.text
        ));
        detail.extend(member_detail(view));
    }
    let answers = pending
        .iter()
        .map(|v| {
            format!(
                "{{\"key\":\"{}\",\"rev\":{}}}",
                v.question.key, v.question.revision
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let response = (!pending.is_empty()).then(|| {
        format!(
            "In a separate trusted owner terminal: {bin} question answer-group {} --owner --text <answer> (or --answers '[{answers}]' with per-member text/option)\nPM: use the authenticated corral_question_answer tool with key {group} and answers [{answers}] (per-member text/option), canonical basis_refs and scope_note; every member commits in one receipt. Rust validates authority, scope and each revision again.",
            q(group)
        )
    });
    Item {
        kind: "question_group",
        task: task.id.clone(),
        container: Some(members[0].container.clone()),
        key: group.into(),
        revision: members
            .iter()
            .map(|v| format!("{}@{}", v.question.key, v.question.revision))
            .collect::<Vec<_>>()
            .join(","),
        revision_kind: "question_group_revisions",
        state: if pending.is_empty() {
            "settled"
        } else {
            "pending"
        }
        .into(),
        pending: !pending.is_empty(),
        scope: task.project.clone().unwrap_or_else(|| "unassigned".into()),
        reason: members
            .iter()
            .map(|v| v.question.text.lines().next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" | "),
        responsible: if pending.iter().any(|v| questions::needs_owner(&v.question)) {
            "owner"
        } else {
            "pm_or_owner"
        }
        .into(),
        detail,
        inspect: format!("{bin} question show {} --json", q(group)),
        response,
        pm_eligible: !pending.is_empty(),
    }
}

fn question_item(task: &Task, bin: &str, view: &QuestionView) -> Item {
    let question = &view.question;
    let mut detail = vec![format!(
        "{:?}; blocking={}",
        question.kind, question.blocking
    )];
    detail.extend(member_detail(view));
    let response = view.pending.then(|| {
        let owner = format!(
            "In a separate trusted owner terminal: {bin} question answer {} --rev {} --owner --text <answer>",
            q(&question.key), question.revision
        );
        format!("{owner}\nPM: use the existing authenticated corral_question_answer tool with this key and revision, canonical basis_refs and scope_note. Rust validates authority and scope again; the panel supplies no basis or permission.")
    });
    Item {
        kind: "question",
        task: task.id.clone(),
        container: Some(view.container.clone()),
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
        // Existing canonical triage allows a PM answer of any pending kind, but only with
        // resolvable basis refs and an approved scope note. Kind is not permission authority.
        pm_eligible: view.pending,
    }
}

/// Closure, terminal, answer, recommendation, deadline, triage, options and context of one question.
fn member_detail(view: &QuestionView) -> Vec<String> {
    let question = &view.question;
    let mut detail = Vec::new();
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
    detail
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
        "question" | "question_group" => 0,
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
