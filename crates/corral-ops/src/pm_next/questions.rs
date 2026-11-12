//! Question facts retain their original action identities; only presentation is batched.
use corral_core::Task;
use corral_core::agent::{QuestionKind, QuestionState};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{PmAction, PmActionKind, PmObservation, option};
use crate::gate_script::q;

const DIGEST_QUESTIONS_MAX: usize = 8;
pub(crate) const DIGEST_CARD_MAX_BYTES: usize = 6144;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionDigestOption {
    pub label: String,
    /// The stored string is also its value: questions do not store a separate option ID.
    pub value: String,
    pub label_bytes_omitted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionDetails {
    pub project: Option<String>,
    pub blocking: bool,
    pub asked_at: Timestamp,
    pub needed_by: Option<Timestamp>,
    pub options: Vec<QuestionDigestOption>,
    pub options_omitted: usize,
    pub recommendation: Option<String>,
    pub recommendation_bytes_omitted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionDigestItem {
    pub key: String,
    pub revision: u32,
    pub task: String,
    pub question_kind: String,
    pub text: String,
    pub text_bytes_omitted: usize,
    #[serde(flatten)]
    pub details: QuestionDetails,
    pub age_seconds: i64,
    /// The original per-question options, including IDs, commands and authority flags.
    pub actions: Vec<super::PmOption>,
    pub requires_owner: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionDigest {
    pub questions: Vec<QuestionDigestItem>,
    pub questions_omitted: usize,
}

/// Human-readable presentation of the same question facts and exact action commands.
pub fn question_digest_lines(digest: &QuestionDigest) -> Vec<String> {
    let mut lines = Vec::new();
    for question in &digest.questions {
        let details = &question.details;
        lines.push(format!(
            "Question {} (revision {}): project {}; task {}; {}; age {}s; needed_by {}{}",
            question.key,
            question.revision,
            details.project.as_deref().unwrap_or("none"),
            question.task,
            if details.blocking {
                "blocking"
            } else {
                "non-blocking"
            },
            question.age_seconds,
            details
                .needed_by
                .map_or_else(|| "none".into(), |at| at.to_string()),
            if question.requires_owner {
                "; owner required"
            } else {
                ""
            },
        ));
        lines.push(format!("Text: {}", question.text));
        if question.text_bytes_omitted > 0 {
            lines.push(format!(
                "Text bytes omitted: {}",
                question.text_bytes_omitted
            ));
        }
        for choice in &details.options {
            lines.push(format!("Choice: {} = {}", choice.label, choice.value));
            if choice.label_bytes_omitted > 0 {
                lines.push(format!(
                    "Choice label bytes omitted: {}",
                    choice.label_bytes_omitted
                ));
            }
        }
        if details.options_omitted > 0 {
            lines.push(format!("Choices omitted: {}", details.options_omitted));
        }
        if let Some(recommendation) = &details.recommendation {
            lines.push(format!("Recommendation (context only): {recommendation}"));
        }
        if details.recommendation_bytes_omitted > 0 {
            lines.push(format!(
                "Recommendation bytes omitted: {}",
                details.recommendation_bytes_omitted
            ));
        }
        for action in &question.actions {
            lines.push(format!(
                "Option {}: {} -> {}",
                action.id, action.label, action.command
            ));
        }
    }
    if digest.questions_omitted > 0 {
        lines.push(format!("Questions omitted: {}", digest.questions_omitted));
    }
    lines
}

fn preview(text: &str, bytes: usize) -> (String, usize) {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), text.len() - end)
}

fn details(task: &Task, question: &mut corral_core::agent::AgentQuestion) -> QuestionDetails {
    let (recommendation, recommendation_bytes_omitted) = question
        .recommendation
        .take()
        .map(|text| {
            if text.len() <= 240 {
                return (Some(text), 0);
            }
            let (text, omitted) = preview(&text, 240);
            (Some(text), omitted)
        })
        .unwrap_or_default();
    let options_omitted = question.options.len().saturating_sub(6);
    QuestionDetails {
        project: task.project.clone(),
        blocking: question.blocking,
        asked_at: question.asked.at,
        needed_by: question.needed_by,
        options: question
            .options
            .drain(..)
            .take(6)
            .map(|value| {
                let (label, label_bytes_omitted) = preview(&value, 120);
                QuestionDigestOption {
                    label,
                    value,
                    label_bytes_omitted,
                }
            })
            .collect(),
        options_omitted,
        recommendation,
        recommendation_bytes_omitted,
    }
}

fn kind_name(kind: QuestionKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

pub(super) fn observations(task: &Task) -> Vec<PmObservation> {
    crate::questions::views(task)
        .into_iter()
        .filter(|view| view.pending)
        .map(|view| {
            let mut question = view.question;
            let question_kind = kind_name(question.kind);
            let details = Some(details(task, &mut question));
            if crate::questions::needs_owner(&question) {
                PmObservation::OwnerQuestion {
                    task: view.task,
                    reason: if question.state == QuestionState::Escalated {
                        "escalated".into()
                    } else {
                        "unattended".into()
                    },
                    key: question.key,
                    revision: question.revision,
                    question_kind,
                    text: question.text,
                    details,
                }
            } else {
                PmObservation::AgentQuestion {
                    task: view.task,
                    key: question.key,
                    revision: question.revision,
                    question_kind,
                    blocking: question.blocking,
                    text: question.text,
                    details,
                }
            }
        })
        .collect()
}

pub(super) fn card(observation: &PmObservation, bin: &str) -> PmAction {
    let (kind, task, key, revision, question_kind, text, why, verify, options, owner) =
        match observation {
            PmObservation::AgentQuestion {
                task,
                key,
                revision,
                question_kind,
                blocking,
                text,
                ..
            } => {
                let escalate = option(
                    "escalate",
                    "Escalate to the owner",
                    format!("{bin} question escalate {} --rev {revision}", q(key)),
                    false,
                );
                let options = vec![
                    option(
                        "answer",
                        "Answer with a cited basis inside the approved scope",
                        format!(
                            "{bin} question answer {} --rev {revision} --text <answer> --basis <ref> --scope-note <note>",
                            q(key)
                        ),
                        false,
                    ),
                    escalate,
                ];
                (
                    PmActionKind::AgentQuestion,
                    task,
                    key,
                    revision,
                    question_kind,
                    text,
                    format!(
                        "A corral-pi agent asked a {}{question_kind} question.",
                        if *blocking { "blocking " } else { "" }
                    ),
                    "Answer any kind, citing resolvable basis refs and a scope note; escalate only what needs the owner's own judgment.",
                    options,
                    false,
                )
            }
            PmObservation::OwnerQuestion {
                task,
                key,
                revision,
                question_kind,
                text,
                reason,
                ..
            } => (
                PmActionKind::OwnerQuestion,
                task,
                key,
                revision,
                question_kind,
                text,
                format!(
                    "Owner question {key} (revision {revision}) is {reason}; corral holds it until the owner answers."
                ),
                "Nothing is applied automatically: the owner answers by key and revision, or sets a deadline with an instruction.",
                vec![
                    option(
                        "owner_answer",
                        "Owner answers at this revision",
                        format!(
                            "{bin} question answer {} --rev {revision} --owner --text <answer>",
                            q(key)
                        ),
                        true,
                    ),
                    option(
                        "owner_deadline",
                        "Owner sets a deadline with an instruction",
                        format!(
                            "{bin} question deadline {} --rev {revision} --at <rfc3339> --instruction <instruction>",
                            q(key)
                        ),
                        true,
                    ),
                ],
                true,
            ),
            other => {
                return super::pm_card(
                    &PmObservation::Owner {
                        task: String::new(),
                        reason: format!("not a question observation: {other:?}"),
                    },
                    bin,
                );
            }
        };
    let mut action = PmAction {
        kind,
        task: Some(task.clone()),
        why,
        command: options[0].command.clone(),
        verify: verify.into(),
        evidence: vec![
            format!("question {key} revision {revision} ({question_kind})"),
            text.clone(),
        ],
        recommended: options[0].id.clone(),
        requires_owner: owner,
        options,
        shortlist: vec![],
        question_digest: None,
    };
    let details = match observation {
        PmObservation::AgentQuestion { details, .. }
        | PmObservation::OwnerQuestion { details, .. } => details,
        _ => return action,
    };
    if let Some(details) = details {
        let (text, text_bytes_omitted) = preview(text, 240);
        action.question_digest = Some(QuestionDigest {
            questions: vec![QuestionDigestItem {
                key: key.clone(),
                revision: *revision,
                task: task.clone(),
                question_kind: question_kind.clone(),
                text,
                text_bytes_omitted,
                details: details.clone(),
                age_seconds: 0,
                actions: action.options.clone(),
                requires_owner: owner,
                decision_id: None,
            }],
            questions_omitted: 0,
        });
    }
    action
}

/// Presentation only: leave decision records, IDs and validation inputs untouched.
/// Call after scope filtering and selection; an unrelated action always wins.
pub(crate) fn digest_question_next(
    decisions: &[crate::pm_workset::PmDecision],
    next: &mut PmAction,
    now: Timestamp,
) -> anyhow::Result<()> {
    if !matches!(
        next.kind,
        PmActionKind::AgentQuestion | PmActionKind::OwnerQuestion
    ) {
        return Ok(());
    }
    let mut questions = Vec::new();
    for decision in decisions.iter().filter(|d| d.presentation.is_none()) {
        if !matches!(
            decision.action.kind,
            PmActionKind::AgentQuestion | PmActionKind::OwnerQuestion
        ) {
            continue;
        }
        if let Some(digest) = &decision.action.question_digest {
            for item in &digest.questions {
                let mut item = item.clone();
                item.decision_id = Some(decision.id.clone());
                item.age_seconds = now
                    .as_second()
                    .saturating_sub(item.details.asked_at.as_second())
                    .max(0);
                // Permissions are projected on the per-question card by the existing policy.
                item.actions = decision.action.options.clone();
                item.requires_owner = decision.action.requires_owner;
                questions.push(item);
            }
        }
    }
    if questions.is_empty() {
        return Ok(());
    }
    questions.sort_by(|a, b| {
        b.details
            .blocking
            .cmp(&a.details.blocking)
            .then(a.details.asked_at.cmp(&b.details.asked_at))
            .then(a.key.cmp(&b.key))
    });
    let questions_omitted = questions.len().saturating_sub(DIGEST_QUESTIONS_MAX);
    questions.truncate(DIGEST_QUESTIONS_MAX);
    *next = PmAction {
        kind: if questions.iter().any(|q| q.requires_owner) {
            PmActionKind::OwnerQuestion
        } else {
            PmActionKind::AgentQuestion
        },
        task: None,
        why: "Pending questions; unrelated work continues.".into(),
        command: String::new(),
        verify: "Recommendations are context, not authority. Answer each key at its revision using its unchanged actions; needed_by never authorizes an effect.".into(),
        evidence: vec![],
        options: vec![],
        recommended: String::new(),
        requires_owner: questions.iter().any(|q| q.requires_owner),
        shortlist: vec![],
        question_digest: Some(QuestionDigest {
            questions,
            questions_omitted,
        }),
    };
    fit_digest(next, DIGEST_CARD_MAX_BYTES)
}

/// Trim only presentation. Stored values, question revisions and authority are unchanged.
pub(crate) fn fit_digest(action: &mut PmAction, budget: usize) -> anyhow::Result<()> {
    while serde_json::to_vec(action)?.len() > budget {
        let Some(digest) = action.question_digest.as_mut() else {
            return Ok(());
        };
        // Closed-choice detail is expendable before any question or answer action.
        if let Some(item) = digest
            .questions
            .iter_mut()
            .rev()
            .find(|item| !item.details.options.is_empty())
        {
            item.details.options_omitted += item.details.options.len();
            item.details.options.clear();
            continue;
        }
        // Preserve blocking priority; within a class discard older rows first.
        let oldest = digest
            .questions
            .iter()
            .enumerate()
            .min_by_key(|(_, item)| (item.details.blocking, item.details.asked_at))
            .map(|(index, _)| index);
        let Some(oldest) = oldest else {
            return Ok(());
        };
        digest.questions.remove(oldest);
        digest.questions_omitted += 1;
    }
    Ok(())
}

#[cfg(test)]
#[path = "questions_tests.rs"]
mod tests;
