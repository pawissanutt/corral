//! Owner-facing `ask_user` mirrors reuse `AgentRunState.questions` on the authenticated PM
//! or assistant container. Authorship is the container, never `QuestionAnswer.by`: a PM
//! answering a worker question does not author an owner ask. Host dialogs and apps race on
//! the same revision; only the canonical winner may be returned to the asking model.
use serde::{Deserialize, Serialize};

use super::ContainerRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerAskAuthor {
    Pm,
    Assistant,
}

impl OwnerAskAuthor {
    pub fn of(container: &ContainerRef) -> Option<Self> {
        match container {
            ContainerRef::Pm => Some(Self::Pm),
            ContainerRef::Assistant { .. } => Some(Self::Assistant),
            _ => None,
        }
    }
}

/// Parses only workspace owner-ask containers. Task question parsing remains separate.
pub fn owner_question_container(key: &str) -> Option<ContainerRef> {
    let mut parts = key.split('/');
    match (parts.next()?, parts.next()?, parts.next(), parts.next()) {
        ("pm", question, None, None) if question_number(question) => Some(ContainerRef::Pm),
        ("assistant", session, Some(question), None)
            if session
                .strip_prefix("A-")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                && question_number(question) =>
        {
            Some(ContainerRef::Assistant {
                session: session.into(),
            })
        }
        _ => None,
    }
}

fn question_number(value: &str) -> bool {
    value
        .strip_prefix('Q')
        .and_then(|n| n.parse::<u32>().ok())
        .is_some_and(|n| n > 0)
}
