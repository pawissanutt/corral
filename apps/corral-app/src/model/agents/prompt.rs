//! The question a chat's host is waiting on the owner for (`ask_user`), named by its row's
//! `question_id` and drawn from the transcript's newest `corral_owner_question` snapshot of it,
//! and answered in place through `prompt.answer`: a listed option, the owner's own words, or both
//! where the question takes a note. Before the transcript holding the snapshot is read again,
//! the row's wait text is asked in words. An answer is read back under a key made from its
//! session and question, so any surface that shows the question shows how its answer went.

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

/// The question the host of `session` waits on the owner for, as its row names it.
pub fn prompt(feed: &Feed, session: &str) -> Option<Prompt> {
    let row = crate::model::assistant::row(feed, session)?;
    let question = row.question_id.clone()?;
    let (text, options, free_text) = match snapshot(feed, session, &question) {
        // The host answers a question of one choice here; one with several is answered in
        // its pane.
        Some(record) => {
            let [choice] = <[Choice; 1]>::try_from(record.questions).ok()?;
            (
                choice.question,
                choice.options,
                choice.free_text != Some(false),
            )
        }
        None => (row.wait.as_ref()?.text.clone(), Vec::new(), true),
    };
    let (status, sending) = answer_status(feed, &answer_key(session, &question));
    Some(Prompt {
        session: Id::from(session),
        question,
        text,
        options,
        free_text,
        status,
        sending,
    })
}

/// The newest snapshot of `question` in the loaded transcript of `session`.
fn snapshot(feed: &Feed, session: &str, question: &str) -> Option<Record> {
    let target = DetailRef::Session {
        id: session.to_owned(),
    };
    let Some(DetailBody::Session(page)) = feed.detail(&target).and_then(|cache| cache.body())
    else {
        return None;
    };
    page.messages
        .iter()
        .flat_map(|message| &message.items)
        .filter_map(|item| match item {
            SessionItem::Card {
                title,
                data: Some(data),
                ..
            } if title == QUESTION_CARD => serde_json::from_str::<Record>(data).ok(),
            _ => None,
        })
        .filter(|record| record.id == question)
        .last()
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
        DetailBody, DetailRef, HostPresence, SessionItem, SessionMessage, SessionPage,
    };
    use corral_protocol::rows::{
        AssistantKind, AssistantLocation, AssistantRow, AssistantState, WaitReason,
    };

    use super::{QUESTION_CARD, prompt};
    use crate::model::daemon::{DetailCache, Feed, detail_key};

    fn card(options: &str) -> SessionItem {
        SessionItem::Card {
            title: QUESTION_CARD.into(),
            text: String::new(),
            data: Some(format!(
                r#"{{"version":1,"id":"q-1","session":"A-1","questions":[{{"question":"Which branch?","options":[{options}]}}],"expires":0,"status":"pending"}}"#
            )),
        }
    }

    fn feed(question: Option<&str>, items: Vec<SessionItem>) -> Feed {
        let target = DetailRef::Session { id: "A-1".into() };
        let mut feed = Feed::default();
        feed.assistants.insert(
            "A-1".into(),
            AssistantRow {
                id: "A-1".into(),
                kind: AssistantKind::Session,
                title: "A-1".into(),
                state: AssistantState::Waiting,
                live_in: AssistantLocation::Daemon,
                profile: None,
                connection: None,
                focus: None,
                pending: 0,
                turn: Some(1),
                cost_usd: None,
                cost_basis: None,
                wait: Some(WaitReason {
                    code: "owner_question".into(),
                    text: "Which branch?".into(),
                    retry_at: None,
                }),
                question_id: question.map(str::to_owned),
                rev: 1,
                model: None,
                effort: None,
                created: None,
                updated: None,
                build: None,
            },
        );
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
                        id: String::new(),
                        author: "custom".into(),
                        at: None,
                        items,
                        model: None,
                        stop_reason: None,
                        usage: None,
                        input: None,
                        attachments: Vec::new(),
                    }],
                    older: None,
                    replyable: true,
                    reply_refusal: None,
                    host: HostPresence::Running,
                })),
            },
        );
        feed
    }

    /// The row's question id says what is pending; the newest snapshot of it gives its options,
    /// the wait's words stand in until the transcript holds one, and no id asks nothing.
    #[test]
    fn the_rows_question_id_names_the_pending_question() {
        let newest = feed(
            Some("q-1"),
            vec![card(r#""main""#), card(r#""main","dev""#)],
        );
        let asked = prompt(&newest, "A-1");
        assert_eq!(
            asked.as_ref().map(|asked| asked.question.as_str()),
            Some("q-1")
        );
        assert_eq!(asked.map(|asked| asked.options.len()), Some(2));
        let unread = prompt(&feed(Some("q-1"), Vec::new()), "A-1");
        assert_eq!(unread.map(|asked| asked.options.len()), Some(0));
        assert!(prompt(&feed(None, vec![card(r#""main""#)]), "A-1").is_none());
    }
}
