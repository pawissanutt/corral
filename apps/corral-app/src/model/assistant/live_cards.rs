//! Served `EntryCard`s as the typed cards the transcript draws: filed Ideas with their real
//! Edit/Merge/Discard commands, owner-action receipts, ask_user questions and permission or
//! confirmation prompts, each showing the true state of any answer this device sent.

use corral_protocol::app::ActClass;
use corral_protocol::assistant::{
    ContextRef, EntryCard, MessageState as WireState, PromptChoice, ThreadEntry,
};
use corral_protocol::cmd::{CmdResult, CmdState, Command};
use corral_protocol::detail::question::{QuestionAnswer, QuestionOption};

use super::views::button;
use super::{
    Action, ActionCard, ActionClass, ActionOutcome, AskOption, AskUserCard, Body, Chip, DiffLine,
    Editing, FiledCard, IdeaFields, MergeInput, PermissionCard, PermissionChoice, PromptCard,
    PromptPane, State, choice_label, clock, subject,
};
use crate::model::act::{self, ActStatus};
use crate::model::daemon::Feed;
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::live::Live;
use crate::model::text::span;

/// The truthful status of a card command this device sent, as a chip.
pub(super) fn act_chip(state: &State, feed: &Feed, remembered: &str, what: &str) -> Option<Chip> {
    let key = state.live.acts.get(remembered)?;
    let status = act::status(feed, key.as_str())?;
    let hue = match status {
        ActStatus::Done => Hue::Ok,
        ActStatus::Failed(_) | ActStatus::Refused(_) | ActStatus::Conflict(_) => Hue::Fail,
        ActStatus::Unknown | ActStatus::Queued | ActStatus::Abandoned => Hue::Held,
        _ => Hue::Corral,
    };
    Some(Chip::new(format!("{what} · {}", status.line()), hue))
}

/// Whether a card command is in flight or done, so its controls stay down.
fn busy(state: &State, feed: &Feed, remembered: &str) -> bool {
    state
        .live
        .acts
        .get(remembered)
        .and_then(|key| act::status(feed, key.as_str()))
        .is_some_and(|status| {
            !matches!(
                status,
                ActStatus::Failed(_)
                    | ActStatus::Refused(_)
                    | ActStatus::Conflict(_)
                    | ActStatus::Abandoned
            )
        })
}

/// The CLI line a command corresponds to: its family and verb, then the records it names.
pub fn command_line(command: &Command) -> String {
    let mut line = format!("corral {}", command.name().replace('.', " "));
    if let Ok(serde_json::Value::Object(fields)) = serde_json::to_value(command)
        && let Some(serde_json::Value::Object(args)) = fields.get("args")
    {
        for name in ["id", "key", "task", "entry", "session", "into"] {
            if let Some(serde_json::Value::String(value)) = args.get(name) {
                line.push(' ');
                line.push_str(value);
            }
        }
    }
    line
}

/// An owner action the assistant ran or proposed, with its receipt.
pub(super) fn action_card(
    entry: &ThreadEntry,
    command: &Command,
    class: ActClass,
    outcome: Option<&CmdResult>,
    input: Option<&String>,
) -> ActionCard {
    let refused_by_authority = outcome
        .and_then(|result| result.error.as_ref())
        .is_some_and(|error| error.code.starts_with("authority"));
    let class = match (class, &entry.state) {
        (_, Some(WireState::Late)) => ActionClass::Late,
        (ActClass::Confirm | ActClass::Admin, _) => ActionClass::Confirm,
        (ActClass::Refused, _) if refused_by_authority => ActionClass::AskAndFileOnly,
        _ => ActionClass::Run,
    };
    let message = |result: &CmdResult| {
        result
            .error
            .as_ref()
            .map_or_else(|| result.cmd_id.clone(), |error| error.message.clone())
    };
    let outcome = match (class, outcome) {
        (ActionClass::Late, _) => ActionOutcome::Late {
            typed: String::new(),
            delivered: String::new(),
            explanation: entry.text.clone().unwrap_or_else(|| {
                "Nothing ran: this input reached corrald more than 15 min after it was typed, so it cannot instruct. Say it again to act.".into()
            }),
        },
        (ActionClass::AskAndFileOnly, Some(result)) => {
            ActionOutcome::AskAndFileOnly(vec![span(message(result))])
        }
        (_, None) => ActionOutcome::Pending("running · no receipt yet".into()),
        (_, Some(result)) => match result.state {
            CmdState::Done => ActionOutcome::Executed(entry.text.clone().unwrap_or_else(|| {
                format!("done · {}", result.cmd_id)
            })),
            CmdState::Failed | CmdState::Busy => ActionOutcome::Refused(message(result)),
            CmdState::Conflict => {
                ActionOutcome::Refused(format!("changed meanwhile: {}", message(result)))
            }
            CmdState::Running => ActionOutcome::Pending("running".into()),
            CmdState::Abandoned => ActionOutcome::Declined("not run · corrald never executed it".into()),
            CmdState::Unknown | CmdState::Other => {
                ActionOutcome::Unknown("result unknown · it may have run".into())
            }
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

/// The assistant's ask_user question with its options; choosing one answers it canonically.
pub(super) fn question_card(
    state: &State,
    feed: &Feed,
    entry: &ThreadEntry,
    key: &str,
    rev: u32,
    options: &[QuestionOption],
    answer: Option<&QuestionAnswer>,
) -> AskUserCard {
    let remembered = subject("answer", key);
    let sending = busy(state, feed, &remembered);
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
            // A cancelled or expired question is mirrored as Withdrawn and takes no answer.
            let open = answer.is_none() && !sending && entry.state.is_none();
            AskOption {
                label: option.label.clone(),
                consequence,
                selected: answer.and_then(|answer| answer.option.as_deref())
                    == Some(option.label.as_str()),
                enabled: open,
                action: open.then(|| Action::Choose {
                    key: key.to_owned(),
                    rev,
                    option: option.label.clone(),
                }),
            }
        })
        .collect();
    let answered = answer.map(|answer| {
        vec![
            span(format!("Answered by {} · ", answer.by)),
            span(answer.via.clone()).mono(),
            span(format!(" · {}", clock(&answer.at, &feed.date))),
        ]
    });
    let status = act_chip(state, feed, &remembered, "Your answer")
        .map(|chip| vec![span(chip.label).hue(chip.hue)]);
    AskUserCard {
        heading: format!(
            "assistant asks · ask_user · {}",
            clock(&entry.at, &feed.date)
        ),
        question: vec![span(entry.text.clone().unwrap_or_else(|| key.to_owned()))],
        options,
        answer: answered.or(status),
    }
}

/// A permission or confirmation prompt. Confirm/Decline prompts draw as an owner action
/// waiting on this device; grant prompts as the permission card.
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
    needs_presence: bool,
) -> Body {
    let remembered = subject("prompt", key);
    let status = act_chip(state, feed, &remembered, "Your answer");
    let settled = choices.is_empty() || entry.state.is_some();
    let open = !settled && !busy(state, feed, &remembered);
    let choice = |choice: PromptChoice, confirm: bool| {
        let mut control = button(
            choice_label(choice),
            Action::Answer {
                key: key.to_owned(),
                rev,
                choice,
                confirm,
            },
        );
        control.enabled = open;
        control
    };
    if choices.contains(&PromptChoice::Confirm) {
        let mut buttons = Vec::new();
        if open {
            let mut confirm = choice(PromptChoice::Confirm, true);
            confirm.primary = true;
            buttons.push(confirm);
            if choices.contains(&PromptChoice::Decline) {
                buttons.push(choice(PromptChoice::Decline, false));
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
                let standing = matches!(grant, PromptChoice::Hour | PromptChoice::Session);
                let requires_presence = needs_presence && standing;
                let mut control = choice(grant, requires_presence);
                control.primary = grant == PromptChoice::Once;
                control.danger = grant == PromptChoice::Deny;
                PermissionChoice {
                    button: control,
                    requires_presence,
                    presence_prompt: requires_presence
                        .then_some("a fresh confirmation on this device"),
                }
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
            clock(&entry.at, &feed.date)
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
        presence_note: needs_presence.then_some(
            "The two standing grants ask for a fresh confirmation on this device first; Allow once and Deny do not.",
        ),
        timer_note: (!settled).then(|| {
            "This prompt stays answerable here and in Inbox on every device; one answer wins.".into()
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

/// A filed Idea: the owner's words verbatim, the generated fields, and the real Edit (title
/// only), Merge into and confirmed Discard commands with their status.
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
    let merging = state.live.merging.as_deref() == Some(task);
    let title = super::idea_title(feed, task).unwrap_or_else(|| title.to_owned());
    let project = feed
        .board
        .task(task)
        .and_then(|row| row.project.clone())
        .unwrap_or_else(|| "unknown".into());
    let status = [
        ("discard", "Discard"),
        ("merge", "Merge"),
        ("edit", "Retitle"),
    ]
    .into_iter()
    .find_map(|(kind, what)| act_chip(state, feed, &subject(kind, task), what));
    let busy = ["discard", "merge", "edit"]
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
    } else if merging {
        Vec::new()
    } else {
        let mut discard = button("Discard", Action::DiscardIdea(task.to_owned()));
        discard.danger = true;
        let mut controls = vec![
            button("Edit", Action::EditIdea(Some(task.to_owned()))),
            button("Merge into T-…", Action::MergeIdea(Some(task.to_owned()))),
            discard,
        ];
        for control in &mut controls {
            control.enabled = !busy;
        }
        controls
    };
    let merge = merging.then(|| {
        let mut merge = button(
            "Merge",
            Action::MergeInto {
                task: task.to_owned(),
                into: String::new(),
            },
        );
        merge.primary = true;
        MergeInput {
            buttons: vec![merge, button("Cancel", Action::MergeIdea(None))],
        }
    });
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
        merge,
        related,
        buttons,
        discard_note: (!editing && !merging).then_some("Discard asks on this device"),
        status,
        attached: false,
    }
}

/// An assistant prompt as Inbox's detail pane shows it, from the thread that carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LivePrompt {
    /// The thread is not loaded yet.
    Loading,
    /// The thread is loaded and has no such prompt, or cannot be loaded.
    Missing(String),
    Pane(PromptPane),
}

/// Builds the Inbox pane of prompt `key` in thread `scope` with its real answer controls.
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
            needs_presence,
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
                *needs_presence,
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
        origin: format!("{place} · {}", clock(&entry.at, &feed.date)),
        title,
        card,
        note: "One answer wins: answering here also settles it in the thread on every device.",
        open: button(
            "Open the thread",
            Action::OpenLive(Live::Thread(Id::from(scope))),
        ),
    })
}
