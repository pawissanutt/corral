//! The questions an agent asks in its session, answered in place, as the typed card the
//! transcript draws.

use super::{Action, AskOption, AskUserCard};
use crate::model::agents::Prompt;
use crate::model::text::span;

/// A question the agent asked in its session, answered here: each option answers at once, and
/// a question that takes the owner's own words has a field for them.
pub fn question_card(prompt: &Prompt) -> AskUserCard {
    let open = !prompt.sending;
    AskUserCard {
        heading: format!("{} asks · waiting on you", prompt.session),
        question: vec![span(prompt.text.clone())],
        options: prompt
            .options
            .iter()
            .map(|option| AskOption {
                label: option.clone(),
                consequence: String::new(),
                selected: false,
                enabled: open,
                action: open.then(|| Action::AnswerQuestion {
                    session: prompt.session.to_string(),
                    question: prompt.question.clone(),
                    option: Some(option.clone()),
                    text: None,
                }),
            })
            .collect(),
        words: (prompt.free_text && open).then(|| super::WordsAnswer {
            session: prompt.session.to_string(),
            question: prompt.question.clone(),
            placeholder: if prompt.options.is_empty() {
                "Your answer…".into()
            } else {
                "Or answer in your own words…".into()
            },
        }),
        answer: prompt
            .status
            .as_ref()
            .map(|(_, line)| vec![span(line.clone())]),
    }
}
