//! Pi-native question cards (question-lifecycle): an open question is PM triage, and an
//! escalated or unattended one is an owner card showing its key, revision and text.
use corral_core::Task;
use corral_core::agent::{QuestionKind, QuestionState};

use super::{PmAction, PmActionKind, PmObservation, option};
use crate::gate_script::q;

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
            let question = view.question;
            let question_kind = kind_name(question.kind);
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
                }
            } else {
                PmObservation::AgentQuestion {
                    task: view.task,
                    key: question.key,
                    revision: question.revision,
                    question_kind,
                    blocking: question.blocking,
                    text: question.text,
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
                        "A Pi-native agent asked a {}{question_kind} question.",
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
    PmAction {
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
    }
}
