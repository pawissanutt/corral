//! Questions an agent asked the owner in its session (`ask_user`), read from the recorded
//! transcript's `corral_owner_question` snapshots, and answered in place through
//! `prompt.answer`: a listed option, the owner's own words, or both where the question takes
//! a note. The transcript records each change of a question; the newest snapshot is its state.
//! An answer is read back under a key made from its session and question, so any surface that
//! shows the question shows how its answer went.

use corral_protocol::detail::{DetailBody, DetailRef, SessionItem};
use corral_protocol::session::{SessionPromptAnswer, SessionPromptAnswered};
use serde::Deserialize;

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

/// The card title the transcript gives a question's snapshot.
pub const QUESTION_CARD: &str = "corral_owner_question";

#[derive(Deserialize)]
struct Record {
    id: String,
    status: String,
    questions: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    question: String,
    options: Vec<String>,
    #[serde(default, rename = "freeText")]
    free_text: Option<bool>,
}

/// A question waiting for the owner, as its card shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub session: Id,
    /// The host's question id, which the answer names.
    pub question: String,
    pub text: String,
    pub options: Vec<String>,
    /// Whether the owner may answer in their own words instead of, or beside, an option.
    pub free_text: bool,
    /// The latest answer sent from here, in its tone.
    pub status: Option<(Hue, String)>,
    /// True while that answer is on its way or accepted: the card takes no second one.
    pub sending: bool,
}

/// The questions still pending in the loaded transcript of `session`, oldest first.
pub fn prompts(feed: &Feed, session: &str) -> Vec<Prompt> {
    let target = DetailRef::Session {
        id: session.to_owned(),
    };
    let Some(DetailBody::Session(page)) = feed.detail(&target).and_then(|cache| cache.body())
    else {
        return Vec::new();
    };
    let mut latest: Vec<Record> = Vec::new();
    for item in page.messages.iter().flat_map(|message| &message.items) {
        let SessionItem::Card {
            title,
            data: Some(data),
            ..
        } = item
        else {
            continue;
        };
        if title != QUESTION_CARD {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Record>(data) else {
            continue;
        };
        latest.retain(|known| known.id != record.id);
        latest.push(record);
    }
    latest
        .into_iter()
        .filter(|record| record.status == "pending")
        .filter_map(|record| {
            // The host answers a question of one choice here; one with several is answered in
            // its pane.
            let [choice] = <[Choice; 1]>::try_from(record.questions).ok()?;
            let (status, sending) = answer_status(feed, &answer_key(session, &record.id));
            Some(Prompt {
                session: Id::from(session),
                question: record.id,
                text: choice.question,
                options: choice.options,
                free_text: choice.free_text != Some(false),
                status,
                sending,
            })
        })
        .collect()
}

/// Whether the loaded transcript of `session` holds a question waiting for the owner.
pub fn asks(feed: &Feed, session: &str) -> bool {
    !prompts(feed, session).is_empty()
}

fn answer_key(session: &str, question: &str) -> Id {
    Id::from(format!("answer:{session}/{question}"))
}

fn answer_status(feed: &Feed, key: &Id) -> (Option<(Hue, String)>, bool) {
    let Some(operation) = feed.operations.get(key.as_str()) else {
        return (None, false);
    };
    if operation.loading {
        return (Some((Hue::Neutral, "Answer sending…".into())), true);
    }
    if let Some(refusal) = &operation.refusal {
        return (
            Some((Hue::Fail, format!("Answer refused: {}", refusal.reason))),
            false,
        );
    }
    if let Some(error) = &operation.error {
        return (
            Some((Hue::Fail, format!("Answer failed: {}", error.message))),
            false,
        );
    }
    let status = operation
        .decode::<SessionPromptAnswered>()
        .map_or_else(|| "answered".to_owned(), |reply| reply.status);
    (Some((Hue::Ok, format!("Answer sent · {status}"))), true)
}

/// Answers `question` of `session` with an option, words, or both; blank words count as none,
/// and an answer with neither sends nothing.
pub fn answer(
    session: &str,
    question: String,
    option: Option<String>,
    text: Option<String>,
) -> Option<Effect> {
    let text = text
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    if option.is_none() && text.is_none() {
        return None;
    }
    Some(Effect::Answer {
        key: answer_key(session, &question),
        answer: SessionPromptAnswer {
            session: session.to_owned(),
            question,
            option,
            text,
        },
    })
}

#[cfg(test)]
mod tests {
    use corral_protocol::detail::{
        DetailBody, DetailRef, SessionItem, SessionMessage, SessionPage,
    };

    use super::{QUESTION_CARD, prompts};
    use crate::model::daemon::{DetailCache, Feed, detail_key};

    fn card(status: &str) -> SessionItem {
        SessionItem::Card {
            title: QUESTION_CARD.into(),
            text: String::new(),
            data: Some(format!(
                r#"{{"version":1,"id":"q-1","session":"A-1","questions":[{{"question":"Which branch?","options":["main","dev"]}}],"expires":0,"status":"{status}"}}"#
            )),
        }
    }

    fn feed(items: Vec<SessionItem>) -> Feed {
        let target = DetailRef::Session { id: "A-1".into() };
        let mut feed = Feed::default();
        feed.details.insert(
            detail_key(&target),
            DetailCache {
                target,
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::Session(SessionPage {
                    id: "A-1".into(),
                    role: "assistant".into(),
                    task: None,
                    run: None,
                    machine: None,
                    messages: vec![SessionMessage {
                        author: "custom".into(),
                        at: None,
                        items,
                        model: None,
                        stop_reason: None,
                        usage: None,
                    }],
                    older: None,
                    replyable: true,
                    reply_refusal: None,
                })),
            },
        );
        feed
    }

    /// The newest snapshot of a question is its state: once the transcript records it
    /// answered, the card no longer asks.
    #[test]
    fn an_answered_snapshot_retires_the_pending_question() {
        let pending = prompts(&feed(vec![card("pending")]), "A-1");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].question, "q-1");
        assert_eq!(pending[0].options, ["main", "dev"]);
        let answered = feed(vec![card("pending"), card("answered")]);
        assert!(prompts(&answered, "A-1").is_empty());
    }
}
