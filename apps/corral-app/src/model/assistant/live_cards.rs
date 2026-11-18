//! Served `EntryCard`s as the typed cards the transcript draws: filed Ideas with their engine
//! Edit and Drop operations, owner-action receipts as the thread recorded them, ask_user
//! questions and permission or confirmation prompts, each showing the true state of any answer
//! this device sent.

use corral_protocol::assistant::{
    ContextRef, EntryCard, MessageState as WireState, PromptChoice, ThreadEntry,
};
use corral_protocol::detail::question::{QuestionAnswer, QuestionOption};
use serde_json::Value;

use super::views::button;
use super::{
    Action, ActionCard, ActionClass, ActionOutcome, AskOption, AskUserCard, Body, Chip, DiffLine,
    Editing, FiledCard, IdeaFields, PermissionCard, PermissionChoice, PromptCard, PromptPane,
    State, choice_label, clock, subject,
};
use crate::model::act::{self, ActStatus};
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
    let parts = [field(command, "family"), args.and_then(|args| field(args, "verb"))]
        .into_iter()
        .chain(named.into_iter().map(|name| args.and_then(|args| field(args, name))))
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

/// The assistant's ask_user question with its options and how it was answered. Read-only:
/// answering an assistant's question returns with the chat host.
pub(super) fn question_card(
    feed: &Feed,
    entry: &ThreadEntry,
    key: &str,
    options: &[QuestionOption],
    answer: Option<&QuestionAnswer>,
) -> AskUserCard {
    let options = options
        .iter()
        .map(|option| {
            let mut consequence = option.next.clone().unwrap_or_default();
            if option.recommended {
                if !consequence.is_empty() {
                    consequence.push_str(" · ");
                }
                consequence.push_str("recommended");
            }
            AskOption {
                label: option.label.clone(),
                consequence,
                selected: answer.and_then(|answer| answer.option.as_deref())
                    == Some(option.label.as_str()),
                enabled: false,
                action: None,
            }
        })
        .collect();
    let answered = answer.map(|answer| {
        vec![
            span(format!("Answered by {} · ", answer.by)),
            span(answer.via.clone()).mono(),
            span(format!(" · {}", clock(&answer.at, feed))),
        ]
    });
    AskUserCard {
        heading: format!("assistant asks · ask_user · {}", clock(&entry.at, feed)),
        question: vec![span(entry.text.clone().unwrap_or_else(|| key.to_owned()))],
        options,
        answer: answered,
    }
}

/// A permission or confirmation prompt. Confirm/Decline prompts draw as an owner action
/// waiting on this device; grant prompts as the permission card. A press answers at once.
#[allow(clippy::too_many_arguments)]
pub(super) fn prompt_body(
    state: &State,
    feed: &Feed,
    entry: &ThreadEntry,
    key: &str,
    rev: u32,
    tool: Option<&String>,
    path: Option<&String>,
    preview: &str,
    choices: &[PromptChoice],
) -> Body {
    let remembered = subject("prompt", key);
    let status = act_chip(state, feed, &remembered, "Your answer");
    let settled = choices.is_empty() || entry.state.is_some();
    let open = !settled && !busy(state, feed, &remembered);
    let choice = |choice: PromptChoice| {
        let mut control = button(
            choice_label(choice),
            Action::Answer {
                key: key.to_owned(),
                rev,
                choice,
            },
        );
        control.enabled = open;
        control
    };
    if choices.contains(&PromptChoice::Confirm) {
        let mut buttons = Vec::new();
        if open {
            let mut confirm = choice(PromptChoice::Confirm);
            confirm.primary = true;
            buttons.push(confirm);
            if choices.contains(&PromptChoice::Decline) {
                buttons.push(choice(PromptChoice::Decline));
            }
        }
        let outcome = if open {
            ActionOutcome::Waiting {
                explanation: vec![
                    span("Nothing runs until you confirm here; Decline records a "),
                    span("declined").bold(),
                    span(" receipt."),
                ],
                buttons,
            }
        } else if let Some(chip) = status.filter(|_| !settled) {
            ActionOutcome::Pending(chip.label)
        } else {
            ActionOutcome::Executed(entry.text.clone().unwrap_or_else(|| "answered".into()))
        };
        return Body::Action(ActionCard {
            heading: "Owner action",
            command: preview.to_owned(),
            class: ActionClass::Confirm,
            why: tool.cloned(),
            input: None,
            confirmed: None,
            outcome,
        });
    }
    let grants = [
        PromptChoice::Once,
        PromptChoice::Hour,
        PromptChoice::Session,
        PromptChoice::Deny,
    ];
    let choices = if open {
        grants
            .into_iter()
            .filter(|grant| choices.contains(grant))
            .map(|grant| {
                let mut control = choice(grant);
                control.primary = grant == PromptChoice::Once;
                control.danger = grant == PromptChoice::Deny;
                PermissionChoice { button: control }
            })
            .collect()
    } else {
        Vec::new()
    };
    let state_chip = if settled {
        Chip::new(
            entry.text.clone().unwrap_or_else(|| "answered".into()),
            Hue::Neutral,
        )
    } else if let Some(chip) = status {
        chip
    } else {
        Chip::new("waiting on you", Hue::You)
    };
    Body::Permission(PermissionCard {
        heading: format!(
            "assistant wants to {} · {}",
            tool.map_or("act", String::as_str),
            clock(&entry.at, feed)
        ),
        state: state_chip,
        path: path.cloned().unwrap_or_else(|| "path not stated".into()),
        lines: preview
            .lines()
            .map(|line| {
                let (marker, text) = match line.chars().next() {
                    Some('+') => ("+", &line[1..]),
                    Some('-') => ("-", &line[1..]),
                    _ => ("", line),
                };
                DiffLine {
                    line: None,
                    marker,
                    text: text.to_owned(),
                }
            })
            .collect(),
        choices,
        timer_note: (!settled).then(|| {
            "This prompt stays answerable here on every device; one answer wins.".into()
        }),
        illustrative: None,
    })
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

/// An assistant prompt as a detail pane shows it, from the thread that carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LivePrompt {
    /// The thread is not loaded yet.
    Loading,
    /// The thread is loaded and has no such prompt, or cannot be loaded.
    Missing(String),
    Pane(PromptPane),
}

/// Builds the pane of prompt `key` in thread `scope` with its real answer controls.
pub fn live_prompt_pane(state: &State, feed: &Feed, scope: &str, key: &str) -> LivePrompt {
    let Some(thread) = feed.threads.get(scope) else {
        return match feed
            .topics
            .get(&format!("thread:{scope}"))
            .and_then(|topic| topic.error.as_ref())
        {
            Some(error) => {
                LivePrompt::Missing(format!("Its thread is unavailable: {}", error.message))
            }
            None => LivePrompt::Loading,
        };
    };
    let found = thread.entries.values().find_map(|entry| match &entry.card {
        Some(EntryCard::Prompt {
            key: id,
            rev,
            tool,
            path,
            preview,
            choices,
            ..
        }) if id == key => Some((
            entry,
            prompt_body(
                state,
                feed,
                entry,
                key,
                *rev,
                tool.as_ref(),
                path.as_ref(),
                preview,
                choices,
            ),
            choices.is_empty() || entry.state.is_some(),
        )),
        _ => None,
    });
    let Some((entry, body, settled)) = found else {
        return if thread.loading {
            LivePrompt::Loading
        } else {
            LivePrompt::Missing("This prompt is no longer in its thread.".into())
        };
    };
    let (title, card) = match body {
        Body::Permission(card) => (
            "Assistant asks permission".to_owned(),
            PromptCard::Permission(card),
        ),
        Body::Action(card) => (
            format!("Confirm {}", card.command),
            PromptCard::Confirmation(card),
        ),
        _ => return LivePrompt::Missing("This prompt is no longer in its thread.".into()),
    };
    let author = super::row(feed, scope).map_or_else(|| scope.to_owned(), |row| row.id.clone());
    let place = if scope == super::QUICK {
        format!("from the quick thread · {author}")
    } else {
        format!("from assistant session {author}")
    };
    LivePrompt::Pane(PromptPane {
        state: if settled {
            Chip::new("answered", Hue::Neutral)
        } else {
            Chip::new("needs you", Hue::You)
        },
        origin: format!("{place} · {}", clock(&entry.at, feed)),
        title,
        card,
        note: "One answer wins: answering here also settles it in the thread on every device.",
        open: button(
            "Open the thread",
            Action::OpenPrompt {
                thread: super::ThreadRef::Live(Id::from(scope)),
                key: key.to_owned(),
            },
        ),
    })
}
