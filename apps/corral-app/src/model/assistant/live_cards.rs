//! Served `EntryCard`s as the typed cards the transcript draws: filed Ideas with their engine
//! Edit and Drop operations and owner-action receipts as the thread recorded them; and the
//! questions an agent asks in its session, answered in place.

use corral_protocol::assistant::{ContextRef, MessageState as WireState, ThreadEntry};
use serde_json::Value;

use super::views::button;
use super::{
    Action, ActionCard, ActionClass, ActionOutcome, AskOption, AskUserCard, Chip, Editing,
    FiledCard, IdeaFields, State, subject,
};
use crate::model::act::{self, ActStatus};
use crate::model::agents::Prompt;
use crate::model::daemon::Feed;
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::live::Live;
use crate::model::text::span;

/// The truthful status of a card operation this device sent, as a chip.
pub(super) fn act_chip(state: &State, feed: &Feed, remembered: &str, what: &str) -> Option<Chip> {
    let key = state.live.acts.get(remembered)?;
    let status = act::status(feed, key.as_str())?;
    let hue = match status {
        ActStatus::Done => Hue::Ok,
        ActStatus::Failed(_) | ActStatus::Refused(_) => Hue::Fail,
        ActStatus::Pending | ActStatus::Running => Hue::Corral,
    };
    Some(Chip::new(format!("{what} · {}", status.line()), hue))
}

/// Whether a card operation is in flight or done, so its controls stay down.
fn busy(state: &State, feed: &Feed, remembered: &str) -> bool {
    state
        .live
        .acts
        .get(remembered)
        .and_then(|key| act::status(feed, key.as_str()))
        .is_some_and(|status| !matches!(status, ActStatus::Failed(_) | ActStatus::Refused(_)))
}

/// A string field of a recorded JSON value, or nothing.
fn field<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(Value::as_str)
}

/// The CLI line a recorded command corresponded to: its family and verb, then the records it
/// names. The record is history: its shape is read as it was stored, never submitted again.
pub fn command_line(command: &Value) -> String {
    let args = command.get("args");
    let mut line = "corral".to_owned();
    let named = ["id", "key", "task", "entry", "session", "into"];
    let parts = [
        field(command, "family"),
        args.and_then(|args| field(args, "verb")),
    ]
    .into_iter()
    .chain(
        named
            .into_iter()
            .map(|name| args.and_then(|args| field(args, name))),
    )
    .flatten();
    for part in parts {
        line.push(' ');
        line.push_str(part);
    }
    line
}

/// An owner action the assistant ran or proposed, with the receipt the thread recorded.
pub(super) fn action_card(
    entry: &ThreadEntry,
    command: &Value,
    class: &str,
    outcome: Option<&Value>,
    input: Option<&String>,
) -> ActionCard {
    let class = match (class, &entry.state) {
        (_, Some(WireState::Late)) => ActionClass::Late,
        ("confirm" | "admin", _) => ActionClass::Confirm,
        _ => ActionClass::Run,
    };
    let text = |value: &Value, name: &str| field(value, name).unwrap_or("").to_owned();
    let message = |result: &Value| match result.get("error") {
        Some(error) => text(error, "message"),
        None => text(result, "cmd_id"),
    };
    let outcome = match (class, outcome) {
        (ActionClass::Late, _) => ActionOutcome::Late {
            typed: String::new(),
            delivered: String::new(),
            explanation: entry.text.clone().unwrap_or_else(|| {
                "Nothing ran: this input reached corrald more than 15 min after it was typed, so it cannot instruct. Say it again to act.".into()
            }),
        },
        (_, None) => ActionOutcome::Pending("no receipt recorded".into()),
        (_, Some(result)) => match field(result, "state").unwrap_or("") {
            "done" => ActionOutcome::Executed(
                entry
                    .text
                    .clone()
                    .unwrap_or_else(|| format!("done · {}", text(result, "cmd_id"))),
            ),
            "failed" | "busy" => ActionOutcome::Refused(message(result)),
            "conflict" => ActionOutcome::Refused(format!("changed meanwhile: {}", message(result))),
            "running" => ActionOutcome::Pending("running when recorded".into()),
            "abandoned" => ActionOutcome::Declined("not run · corrald never executed it".into()),
            _ => ActionOutcome::Unknown("result unknown · it may have run".into()),
        },
    };
    ActionCard {
        heading: "Owner action",
        command: command_line(command),
        class,
        why: None,
        input: input.cloned(),
        confirmed: None,
        outcome,
    }
}

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

/// What a filed Idea's card needs from its entry.
pub(super) struct IdeaEntry<'a> {
    pub entry: &'a ThreadEntry,
    pub task: &'a str,
    pub title: &'a str,
    pub labels: &'a [String],
    pub criteria: &'a [String],
    pub duplicates: &'a [ContextRef],
    pub words: &'a str,
    /// The thread's assistant id, for the attribution.
    pub author: &'a str,
    /// The owner's message was filed as written, with no model.
    pub as_written: bool,
}

/// A filed Idea: the owner's words verbatim, the generated fields, and the engine's Edit (title
/// only) and Drop operations with their status.
pub(super) fn idea_card(state: &State, feed: &Feed, idea: IdeaEntry<'_>) -> FiledCard {
    let IdeaEntry {
        entry,
        task,
        title,
        labels,
        criteria,
        duplicates,
        words,
        author,
        as_written,
    } = idea;
    let editing = state.live.editing.as_deref() == Some(task);
    let title = super::idea_title(feed, task).unwrap_or_else(|| title.to_owned());
    let project = feed
        .board
        .task(task)
        .and_then(|row| row.project().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into());
    let status = [("discard", "Discard"), ("edit", "Retitle")]
        .into_iter()
        .find_map(|(kind, what)| act_chip(state, feed, &subject(kind, task), what));
    let busy = ["discard", "edit"]
        .into_iter()
        .any(|kind| busy(state, feed, &subject(kind, task)));
    let buttons = if editing {
        let mut save = button(
            "Save",
            Action::SaveIdea {
                task: task.to_owned(),
                title: title.clone(),
            },
        );
        save.primary = true;
        vec![save, button("Cancel", Action::EditIdea(None))]
    } else {
        let mut discard = button("Discard", Action::DiscardIdea(task.to_owned()));
        discard.danger = true;
        let mut controls = vec![
            button("Edit", Action::EditIdea(Some(task.to_owned()))),
            discard,
        ];
        for control in &mut controls {
            control.enabled = !busy;
        }
        controls
    };
    let via = entry.via.clone().unwrap_or_else(|| "via unknown".into());
    let related = (!duplicates.is_empty()).then(|| {
        let mut runs = vec![span("· related ").dim()];
        for (at, duplicate) in duplicates.iter().enumerate() {
            if at > 0 {
                runs.push(span(", "));
            }
            runs.push(
                span(duplicate.id.clone())
                    .mono()
                    .live(Live::Task(Id::from(duplicate.id.as_str()))),
            );
        }
        runs
    });
    FiledCard {
        chip: Chip::new(format!("filed · {task}"), Hue::Ok),
        idea: IdeaFields {
            id: task.to_owned(),
            words: words.to_owned(),
            title,
            labels: labels.to_vec(),
            criteria: criteria.to_vec(),
            // corrald sends no duplicate search with the card yet; nothing served is no claim.
            duplicates: if duplicates.is_empty() {
                String::new()
            } else {
                format!(
                    "close: {}",
                    duplicates
                        .iter()
                        .map(|duplicate| duplicate.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        },
        provenance: vec![
            span("Idea · project "),
            span(project.clone()).bold(),
            span(" · "),
            span(via).mono(),
        ],
        project,
        // Provenance from what the thread recorded, never inferred: an entry written by a model
        // names the assistant; one saved as written says no AI filed it; otherwise neutral.
        attribution: if as_written {
            "filed from your message, as written · no AI used to file".to_owned()
        } else if entry.model.is_some() {
            format!(
                "filed by assistant {author} from your message · your words kept as the request"
            )
        } else {
            format!("filed from your message in {author} · your words kept as the request")
        },
        quote_label: "your words, unchanged",
        editing: if editing { Editing::Title } else { Editing::No },
        related,
        buttons,
        discard_note: (!editing).then_some("Discard asks on this device"),
        status,
        attached: false,
    }
}
