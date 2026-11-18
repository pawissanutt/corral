//! A served thread as transcript items: durable entries in order, the streamed answer from the
//! running turn's text, and this device's own sends still in its outbox. Loading, empty, stale
//! and error states are said plainly.

use std::collections::BTreeMap;

use corral_protocol::assistant::{
    AssistantSend, ContextRef, EntryKind, MessageState as WireState, ThreadEntry, ToolLine,
    TurnCompletion,
};
use corral_protocol::op::OpReply;

use super::cards::{WAIT_LIFETIME, WAIT_NOTE};
use super::views::button;
use super::{
    Action, AssistantMessage, Body, Chip, OwnerMessage, QUICK, QueueNote, RollDivider, SourceLink,
    State, ThreadItem, ToolRow, WaitingBlock, clock, entry_session, fact_chips, question_card,
    quick_id, record_link, row, send_chips, wait_explanation, wait_label,
};
use crate::model::act::{self, ActStatus};
use crate::model::daemon::{CommandStatus, Feed, Link};
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::live::Live;
use crate::model::session_feed::LiveSession;
/// An owner message's outlined tag from its served state.
fn tag(state: Option<&WireState>) -> Chip {
    let (label, hue) = match state {
        None | Some(WireState::Sent) => ("sent".to_owned(), Hue::Neutral),
        Some(WireState::Waiting { reason }) => (
            format!("waiting for assistant · {}", wait_label(reason)),
            Hue::Held,
        ),
        Some(WireState::Delivered | WireState::PmDelivered) => ("delivered".into(), Hue::Neutral),
        Some(WireState::Late) => ("late · cannot instruct".into(), Hue::Held),
        Some(WireState::QueuedForPm | WireState::PmReplied) => {
            ("state unknown".into(), Hue::Neutral)
        }
        Some(WireState::Saved { task }) => (format!("saved · {task}"), Hue::Neutral),
        Some(WireState::Withdrawn) => ("withdrawn".into(), Hue::Neutral),
        Some(WireState::Other) => ("state unknown".into(), Hue::Neutral),
    };
    Chip::new(label, hue)
}

/// The bounded context a message carried, as its attachment chip.
fn attachment(links: &[ContextRef]) -> Option<String> {
    let first = links.first()?;
    Some(match links.len() {
        1 => format!("{} {}", first.kind, first.id),
        n => format!("{} {} +{}", first.kind, first.id, n - 1),
    })
}

fn owner(state: &State, feed: &Feed, entry: &ThreadEntry) -> OwnerMessage {
    let (files, opened) = fact_chips(state, feed, &entry.id, &entry.attachments);
    let (file, filed) = filing(state, feed, entry);
    OwnerMessage {
        author: "you",
        provenance: entry
            .via
            .clone()
            .or_else(|| entry.device.as_ref().map(|device| format!("app:{device}")))
            .unwrap_or_else(|| "origin unknown".into()),
        // The time the owner typed it, as the queued copy promised; corrald's receipt (which
        // orders the thread) follows when it differs, and stands alone for older records.
        at: clock(entry.typed_at.as_deref().unwrap_or(&entry.at), feed),
        text: entry.text.clone().unwrap_or_default(),
        tag: tag(entry.state.as_ref()),
        extra: entry
            .typed_at
            .as_deref()
            .map(|typed| (clock(typed, feed), clock(&entry.at, feed)))
            .filter(|(typed, received)| typed != received)
            .map(|(_, received)| format!("received {received}")),
        attachment: attachment(&entry.links),
        files,
        opened,
        file,
        filed,
    }
}

/// File as idea under an owner message, and how a filing sent from here went: the new task's
/// id, or what stopped it. The button stays while nothing is filed, so a refused filing can be
/// sent again; a message corrald already saved or withdrew has none.
fn filing(
    state: &State,
    feed: &Feed,
    entry: &ThreadEntry,
) -> (Option<super::Button>, Option<Chip>) {
    let key = state.live.filed.get(entry.id.as_str());
    let status = key.and_then(|key| act::status(feed, key.as_str()));
    let chip = status.as_ref().map(|status| match status {
        ActStatus::Pending | ActStatus::Running => Chip::new("filing as idea…", Hue::Corral),
        ActStatus::Done => {
            let created = key
                .and_then(|key| feed.operations.get(key.as_str()))
                .and_then(|operation| operation.decode::<OpReply>());
            match created {
                Some(OpReply::Done {
                    task: Some(task), ..
                }) => Chip::new(format!("filed as idea · {task}"), Hue::Ok),
                _ => Chip::new("filed as idea", Hue::Ok),
            }
        }
        ActStatus::Refused(refusal) => {
            Chip::new(format!("not filed · {}", refusal.reason), Hue::Fail)
        }
        ActStatus::Failed(why) => Chip::new(format!("not filed · {why}"), Hue::Fail),
    });
    let settled = matches!(
        entry.state,
        Some(WireState::Saved { .. } | WireState::Withdrawn)
    );
    let open = status
        .as_ref()
        .is_none_or(|status| matches!(status, ActStatus::Failed(_) | ActStatus::Refused(_)));
    let file = (open
        && !settled
        && entry
            .text
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty()))
    .then(|| button("File as idea", Action::FileEntry(entry.id.clone())));
    (file, chip)
}

fn tools(lines: &[ToolLine]) -> (Option<String>, Vec<ToolRow>) {
    if lines.is_empty() {
        return (None, Vec::new());
    }
    let rows: Vec<ToolRow> = lines
        .iter()
        .map(|line| ToolRow {
            kind: line.tool.clone(),
            command: line.input.clone(),
            elapsed: line
                .elapsed_ms
                .map_or_else(String::new, |ms| format!("{:.1} s", ms as f64 / 1000.0)),
            failed: line.failed == Some(true),
        })
        .collect();
    let failed = rows.iter().filter(|row| row.failed).count();
    let total: u64 = lines.iter().filter_map(|line| line.elapsed_ms).sum();
    let mut summary = format!(
        "{} tool call{}",
        rows.len(),
        if rows.len() == 1 { "" } else { "s" }
    );
    if total > 0 {
        summary.push_str(&format!(" · {:.1} s", total as f64 / 1000.0));
    }
    if failed > 0 {
        summary.push_str(&format!(" · {failed} failed"));
    }
    summary.push_str(" · tool activity");
    (Some(summary), rows)
}

/// The assistant's words as one Markdown body; nothing while they are blank.
fn markdown(text: &str) -> Vec<Body> {
    if text.trim().is_empty() {
        Vec::new()
    } else {
        vec![Body::Markdown(text.trim_end().to_owned())]
    }
}

/// What every assistant message of this thread shares.
struct Thread<'a> {
    feed: &'a Feed,
    author: String,
    session: Option<String>,
    scope: &'a str,
}

impl Thread<'_> {
    fn message(&self, entry: &ThreadEntry, body: Vec<Body>) -> AssistantMessage {
        let (tool_summary, tools) = tools(&entry.tools);
        AssistantMessage {
            key: entry.id.clone(),
            // Each reply names the session that wrote it, never whichever is current now.
            id: entry_session(&entry.id).map_or_else(|| self.author.clone(), str::to_owned),
            at: clock(&entry.at, self.feed),
            model: entry
                .model
                .clone()
                .unwrap_or_else(|| "model unknown".into()),
            state: None,
            body,
            tool_summary,
            tools,
            streaming: false,
            stop: None,
            // One semantic answer's own amount, reported or estimated as corrald attributed
            // it; without an attribution it is unknown, never a share of the session's total.
            cost: (entry.kind == EntryKind::Answer).then(|| match entry.cost_usd {
                Some(usd) => format!("this ask {}", super::amount(usd, entry.cost_basis)),
                None => "this ask · cost unknown".into(),
            }),
            sources: entry
                .links
                .iter()
                .filter_map(record_link)
                .map(|(label, live)| SourceLink {
                    label,
                    action: Action::OpenLive(live),
                })
                .collect(),
            stopped_note: None,
            illustrative: None,
        }
    }

    /// An assistant reply or card entry, as recorded.
    fn reply(&self, entry: &ThreadEntry) -> AssistantMessage {
        let text = if entry.kind == EntryKind::Answer || entry.card.is_none() {
            entry.text.as_deref().unwrap_or("")
        } else {
            ""
        };
        let body = markdown(text);
        let mut message = self.message(entry, body);
        match entry.completion {
            Some(TurnCompletion::Aborted) => {
                message.state = Some(Chip::new("cancelled", Hue::Neutral));
                message.stopped_note = Some("This turn was cancelled · partial answer kept.");
            }
            Some(TurnCompletion::Failed) => {
                message.state = Some(Chip::new("turn failed", Hue::Fail));
                message.stopped_note = Some("This turn failed · what arrived is kept.");
            }
            Some(TurnCompletion::Other) => {
                message.state = Some(Chip::new("turn ended · outcome unknown", Hue::Neutral));
            }
            Some(TurnCompletion::Completed) | None => {}
        }
        message
    }

    /// The answer still streaming in from the running turn, with its caret and Cancel; it
    /// stays until the thread records the turn's answer.
    fn streaming(&self, live: &LiveSession) -> Option<AssistantMessage> {
        let text = live
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        if text.is_empty() && !live.open {
            return None;
        }
        let session = self.session.clone()?;
        let mut message = self.message(
            &self.placeholder(format!("{session}/live")),
            markdown(&text),
        );
        message.at = "now".into();
        // A turn still answering has no amount of its own yet, and cites its records once done.
        message.cost = None;
        message.sources.clear();
        if live.open {
            message.streaming = true;
            message.state = Some(Chip::new("answering", Hue::Corral));
            message.stop = Some(button(
                "Cancel",
                Action::Control {
                    session,
                    control: crate::model::agents::Control::Cancel,
                },
            ));
        } else if live.ended.as_ref().is_some_and(|end| end.aborted) {
            message.state = Some(Chip::new("cancelled", Hue::Neutral));
            message.stopped_note = Some("This turn was cancelled · partial answer kept.");
        }
        Some(message)
    }
    /// An entry standing for content the thread has not recorded: the running turn's answer,
    /// a question waiting for the owner.
    fn placeholder(&self, id: String) -> ThreadEntry {
        ThreadEntry {
            id,
            rev: 0,
            at: String::new(),
            kind: EntryKind::Answer,
            device: None,
            via: None,
            text: None,
            state: None,
            model: row(self.feed, self.scope).and_then(|row| row.profile.clone()),
            cost_usd: None,
            tools: Vec::new(),
            links: Vec::new(),
            card: None,
            attachments: Vec::new(),
            typed_at: None,
            part: None,
            completion: None,
            cost_basis: None,
        }
    }

    /// The waiting block under a stored message.
    fn waiting(&self, reason: &corral_protocol::rows::WaitReason, full: bool) -> WaitingBlock {
        WaitingBlock {
            state: Chip::new(
                format!("waiting for assistant · {}", wait_label(reason)),
                Hue::Held,
            ),
            explanation: full.then(|| wait_explanation(reason, self.feed)),
            note: if full { WAIT_NOTE } else { "" },
            lifetime: WAIT_LIFETIME,
        }
    }
}

/// The served thread `scope` ("quick" or A-n) as transcript items.
pub fn live_items(state: &State, feed: &Feed, scope: &str) -> Vec<ThreadItem> {
    let topic = feed.topics.get(&format!("thread:{scope}"));
    let Some(snapshot) = feed.threads.get(scope) else {
        return vec![ThreadItem::Note(
            match topic.and_then(|topic| topic.error.as_ref()) {
                Some(error) => format!("This thread is unavailable: {}", error.message),
                None if feed.link != Link::Up => {
                    "Not loaded yet; it loads when corrald is reachable.".into()
                }
                None => "Loading the thread…".into(),
            },
        )];
    };
    // The thread's current session: the quick thread's newest generation by rows or entries,
    // whichever arrived first; a named thread is its own session.
    let current = if scope == QUICK {
        quick_id(feed)
    } else {
        Some(scope.to_owned())
    };
    let thread = Thread {
        feed,
        author: current.clone().unwrap_or_else(|| scope.to_owned()),
        session: current.clone(),
        scope,
    };
    let entries = super::live_parts::assemble(&snapshot.entries);
    // The waiting block under a message already shows the failure that left it waiting; the
    // notice recording that same failure right after it is the same event, shown once. Any
    // other notice, including a later repeat of the same failure, is its own event.
    let mut explained: Option<String> = None;
    let last_waiting = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.state, Some(WireState::Waiting { .. })))
        .map(|entry| entry.id.clone());
    let mut items = Vec::with_capacity(entries.len() + 2);
    for entry in &entries {
        if entry.kind != EntryKind::Notice {
            explained = None;
        }
        match entry.kind {
            EntryKind::Message => {
                items.push(ThreadItem::Owner(owner(state, feed, entry)));
                match &entry.state {
                    Some(WireState::Waiting { reason }) => {
                        // Only the full block (the latest waiting message) shows the reason.
                        let full = last_waiting.as_ref() == Some(&entry.id);
                        explained = full.then(|| reason.text.trim().to_owned());
                        items.push(ThreadItem::Waiting(thread.waiting(reason, full)));
                    }
                    Some(WireState::Withdrawn) => items.push(ThreadItem::Note(
                        "withdrawn by you · nothing was filed or run · the text stays here for reference".into(),
                    )),
                    _ => {}
                }
            }
            EntryKind::Roll => items.push(ThreadItem::Roll(RollDivider {
                text: entry
                    .text
                    .clone()
                    .unwrap_or_else(|| "earlier conversation summarized".into()),
                transcript: entry
                    .links
                    .iter()
                    .find(|link| link.kind == "thread")
                    .map(|link| {
                        button(
                            "Earlier transcript",
                            Action::OpenLive(Live::Thread(Id::from(link.id.as_str()))),
                        )
                    }),
            })),
            EntryKind::Notice => {
                let text = entry.text.clone().unwrap_or_default();
                if explained.as_deref() == Some(text.trim()) {
                    explained = None;
                } else {
                    items.push(ThreadItem::Note(text));
                }
            }
            EntryKind::Answer | EntryKind::Question | EntryKind::Prompt => {
                items.push(ThreadItem::Assistant(thread.reply(entry)))
            }
            EntryKind::Idea
            | EntryKind::Action
            | EntryKind::PmMessage
            | EntryKind::PmReply
            | EntryKind::Other => items.push(ThreadItem::Note(
                "An entry this version of the app cannot show.".into(),
            )),
        }
    }
    // The running turn's answer as it streams in, then what the agent asks the owner.
    if let Some(message) = feed.live.get(scope).and_then(|live| thread.streaming(live)) {
        items.push(ThreadItem::Assistant(message));
    }
    if let Some(prompt) = crate::model::agents::prompt(feed, scope) {
        let mut message = thread.message(
            &thread.placeholder(format!("{scope}/ask/{}", prompt.question)),
            vec![Body::AskUser(question_card(&prompt))],
        );
        message.at = "now".into();
        message.cost = None;
        message.state = Some(Chip::new("waiting on you", Hue::You));
        items.push(ThreadItem::Assistant(message));
    }
    outbox(&mut items, feed, scope, &snapshot.entries);
    if items.is_empty() {
        items.push(ThreadItem::Note(if snapshot.loading {
            "Loading the thread…".into()
        } else if scope == QUICK {
            "No messages yet. Ask a question, drop an idea, or give an instruction.".into()
        } else {
            "No messages in this session yet.".into()
        }));
    }
    // Stale while the link is down is a reconnect; stale with the link up is corrald reloading
    // this thread (a reset reopens it), or failing to, which its error says.
    let error = topic.and_then(|topic| topic.error.as_ref());
    if snapshot.stale && feed.link != Link::Up {
        items.push(ThreadItem::Note(match &feed.synced {
            Some(synced) => format!(
                "Shown as of {synced}; reconnecting. New messages appear when corrald is back."
            ),
            None => "Shown as last received; reconnecting.".into(),
        }));
    } else if let Some(error) = error {
        items.push(ThreadItem::Queued(QueueNote {
            text: format!("Updates stopped: {}", error.message),
            buttons: vec![button(
                "Retry updates",
                Action::RetryThread(scope.to_owned()),
            )],
        }));
    } else if snapshot.stale {
        items.push(ThreadItem::Note(
            "Shown as last received; corrald is reloading this thread.".into(),
        ));
    }
    items
}

/// This device's own sends to `scope` that the thread does not show yet: queued offline,
/// in flight, refused or of unknown outcome. Delivered ones appear as their entries. Each one
/// not yet acknowledged can be withdrawn under its own command id: the client drops a send
/// that never left (or that corrald refused) and asks corrald to cancel the rest; it shows
/// "withdrawn" only once one of those is certain, never on a guess.
fn outbox(
    items: &mut Vec<ThreadItem>,
    feed: &Feed,
    scope: &str,
    entries: &BTreeMap<Id, ThreadEntry>,
) {
    let mut pending: Vec<(Id, AssistantSend, CommandStatus, Option<String>)> = feed
        .commands
        .iter()
        .filter(|(_, command)| command.method == corral_protocol::assistant::ASSISTANT_SEND)
        .filter(|(_, command)| {
            command
                .entry
                .as_ref()
                .is_none_or(|entry| !entries.contains_key(entry))
        })
        .filter_map(|(key, command)| {
            let send: AssistantSend = serde_json::from_value(command.request.clone()).ok()?;
            let code = command.error.as_ref().map(|error| error.code.clone());
            (send.thread == scope).then(|| (key.clone(), send, command.status, code))
        })
        .collect();
    pending.sort_by(|a, b| a.1.typed_at.cmp(&b.1.typed_at));
    for (key, send, status, code) in pending {
        let withdraw = || vec![button("Withdraw", Action::CancelSend(key.to_string()))];
        let code = code.unwrap_or_else(|| "no reason given".into());
        let (label, hue, note, buttons): (&str, Hue, Option<String>, Vec<_>) = if feed
            .cancelling
            .contains(&key)
        {
            let label = if feed.link == Link::Up {
                "withdrawing…"
            } else {
                "Withdrawing… confirms when reconnected"
            };
            (label, Hue::Held, None, Vec::new())
        } else {
            match status {
                    CommandStatus::Queued => (
                        "queued on this device",
                        Hue::Held,
                        Some("Sends once when corrald is reachable, with the time you typed it; it never expires. If that is more than 15 min from now it can be answered and filed, not instruct.".into()),
                        withdraw(),
                    ),
                    CommandStatus::Pending | CommandStatus::Running => (
                        "sending",
                        Hue::Neutral,
                        Some("Withdraw asks corrald to drop it under the same identity if it has not reached the assistant.".into()),
                        withdraw(),
                    ),
                    CommandStatus::Done => (
                        "sent",
                        Hue::Neutral,
                        feed.not_withdrawn.contains(&key).then(|| {
                            "Already delivered — not withdrawn. Once it shows here, withdraw it from the thread while it still waits.".into()
                        }),
                        Vec::new(),
                    ),
                    CommandStatus::Unknown => (
                        "delivery unknown",
                        Hue::Held,
                        Some("It may have arrived; it is resent under the same identity on reconnect, never duplicated. Withdraw asks corrald to drop it instead.".into()),
                        withdraw(),
                    ),
                    CommandStatus::Refused | CommandStatus::Conflict => {
                        let mut discard = button("Discard", Action::CancelSend(key.to_string()));
                        discard.danger = true;
                        (
                            "not sent · refused",
                            Hue::Fail,
                            Some(format!("corrald refused it ({code}); it was not delivered.")),
                            vec![discard],
                        )
                    }
                    CommandStatus::Failed => (
                        "not sent · failed",
                        Hue::Fail,
                        Some(format!(
                            "It failed ({code}) and may not have arrived. Withdraw asks corrald under the same identity."
                        )),
                        withdraw(),
                    ),
                    CommandStatus::Abandoned => {
                        ("withdrawn · never delivered", Hue::Neutral, None, Vec::new())
                    }
                }
        };
        items.push(ThreadItem::Owner(OwnerMessage {
            author: "you",
            provenance: "this device".into(),
            at: clock(&send.typed_at, feed),
            text: send.text.clone(),
            tag: Chip::new(label, hue),
            extra: None,
            attachment: attachment(&send.context),
            files: send_chips(feed, &send.attachments),
            opened: None,
            file: None,
            filed: None,
        }));
        if note.is_some() || !buttons.is_empty() {
            items.push(ThreadItem::Queued(QueueNote {
                text: note.unwrap_or_default(),
                buttons,
            }));
        }
    }
}
