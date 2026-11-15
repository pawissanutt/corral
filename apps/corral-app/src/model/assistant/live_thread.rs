//! A served thread as transcript items: durable entries in order, the streamed answer from the
//! running turn's text, the PM's replies folded under the items they answer, and this device's
//! own sends still in its outbox. Loading, empty, stale and error states are said plainly.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::assistant::{
    AssistantSend, ContextRef, EntryCard, EntryKind, MessageState as WireState, ThreadEntry,
    ToolLine, TurnCompletion,
};

use super::cards::{WAIT_LIFETIME, WAIT_NOTE};
use super::live_cards::{IdeaEntry, act_chip, action_card, idea_card, prompt_body, question_card};
use super::views::button;
use super::{
    Action, AssistantMessage, Body, Chip, OwnerMessage, PmMessage, PmReply, QUICK, QueueNote,
    RollDivider, SourceLink, State, ThreadItem, ToolRow, WaitingBlock, clock, entry_session,
    fact_chips, quick_id, record_link, row, saved_as_written, send_chips, subject,
    wait_explanation, wait_label,
};
use crate::model::daemon::{CommandStatus, Feed, Link};
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::live::Live;
use crate::model::text::span;

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
        Some(WireState::QueuedForPm) => ("queued for PM".into(), Hue::Held),
        Some(WireState::PmReplied) => ("PM replied".into(), Hue::Ok),
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
    OwnerMessage {
        author: "you",
        provenance: entry
            .via
            .clone()
            .or_else(|| entry.device.as_ref().map(|device| format!("app:{device}")))
            .unwrap_or_else(|| "origin unknown".into()),
        // The time the owner typed it, as the queued copy promised; corrald's receipt (which
        // orders the thread) follows when it differs, and stands alone for older records.
        at: clock(entry.typed_at.as_deref().unwrap_or(&entry.at), &feed.date),
        text: entry.text.clone().unwrap_or_default(),
        tag: tag(entry.state.as_ref()),
        extra: entry
            .typed_at
            .as_deref()
            .map(|typed| (clock(typed, &feed.date), clock(&entry.at, &feed.date)))
            .filter(|(typed, received)| typed != received)
            .map(|(_, received)| format!("received {received}")),
        attachment: attachment(&entry.links),
        files,
        opened,
    }
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

fn paragraphs(text: &str) -> Vec<Body> {
    text.split("\n\n")
        .map(str::trim_end)
        .filter(|block| !block.trim().is_empty())
        .map(|block| Body::Paragraph(vec![span(block.to_owned())]))
        .collect()
}

/// What every assistant message of this thread shares.
struct Thread<'a> {
    state: &'a State,
    feed: &'a Feed,
    author: String,
    session: Option<String>,
    can_send: bool,
    /// Tasks the owner's own message was filed as, as written, with no model.
    as_written: BTreeSet<String>,
    scope: &'a str,
}

impl Thread<'_> {
    fn message(&self, entry: &ThreadEntry, body: Vec<Body>) -> AssistantMessage {
        let (tool_summary, tools) = tools(&entry.tools);
        AssistantMessage {
            key: entry.id.clone(),
            anchored: self.state.anchor.as_ref().is_some_and(|(scope, key)| {
                let matches_scope = match scope {
                    super::ThreadRef::Live(scope) => scope.as_str() == self.scope,
                    super::ThreadRef::Fixture(_) => false,
                };
                matches_scope
                    && entry.card.as_ref().is_some_and(|card| match card {
                        EntryCard::Prompt { key: entry_key, .. }
                        | EntryCard::Question { key: entry_key, .. } => entry_key == key,
                        _ => false,
                    })
            }),
            // Each reply names the session that wrote it, never whichever is current now.
            id: entry_session(&entry.id).map_or_else(|| self.author.clone(), str::to_owned),
            at: clock(&entry.at, &self.feed.date),
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
            cost: entry
                .cost_usd
                .map(|usd| format!("this ask {}", super::cost(Some(usd), entry.cost_basis))),
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

    fn card(&self, entry: &ThreadEntry, card: &EntryCard) -> Option<Body> {
        Some(match card {
            EntryCard::Idea {
                task,
                title,
                labels,
                criteria,
                duplicates,
                words,
            } => Body::Filed(idea_card(
                self.state,
                self.feed,
                IdeaEntry {
                    entry,
                    task,
                    title,
                    labels,
                    criteria,
                    duplicates,
                    words,
                    author: entry_session(&entry.id).unwrap_or(&self.author),
                    as_written: self.as_written.contains(task.as_str()),
                },
            )),
            EntryCard::Action {
                command,
                class,
                outcome,
                input,
            } => Body::Action(action_card(
                entry,
                command,
                *class,
                outcome.as_ref(),
                input.as_ref(),
            )),
            EntryCard::Question {
                key,
                rev,
                options,
                answer,
                ..
            } => Body::AskUser(question_card(
                self.state,
                self.feed,
                entry,
                key,
                *rev,
                options,
                answer.as_ref(),
            )),
            EntryCard::Prompt {
                key,
                rev,
                tool,
                path,
                preview,
                choices,
                needs_presence,
            } => prompt_body(
                self.state,
                self.feed,
                entry,
                key,
                *rev,
                tool.as_ref(),
                path.as_ref(),
                preview,
                choices,
                *needs_presence,
            ),
            EntryCard::PmItem { .. } | EntryCard::Other => return None,
        })
    }

    /// An assistant reply or card entry, with the streamed text while its turn runs.
    fn reply(&self, entry: &ThreadEntry, live: Option<(&str, u64, bool)>) -> AssistantMessage {
        let streaming = live.is_some_and(|(_, _, done)| !done);
        let text = match live {
            Some((text, _, _)) => text,
            None if entry.kind == EntryKind::Answer => entry.text.as_deref().unwrap_or(""),
            None if entry.card.is_none() => entry.text.as_deref().unwrap_or(""),
            None => "",
        };
        let mut body = paragraphs(text);
        let card = entry.card.as_ref().and_then(|card| self.card(entry, card));
        let filed = matches!(card, Some(Body::Filed(_)));
        let asks = match &entry.card {
            Some(EntryCard::Question { answer, .. }) => answer.is_none() && entry.state.is_none(),
            Some(EntryCard::Prompt { choices, .. }) => !choices.is_empty() && entry.state.is_none(),
            _ => false,
        };
        body.extend(card);
        let mut message = self.message(entry, body);
        self.turn(&mut message, live);
        match entry.completion {
            Some(TurnCompletion::Aborted) => {
                message.streaming = false;
                message.stop = None;
                message.state = Some(Chip::new("stopped", Hue::Neutral));
                message.stopped_note = Some("This turn was stopped · partial answer kept.");
            }
            Some(TurnCompletion::Failed) => {
                message.streaming = false;
                message.stop = None;
                message.state = Some(Chip::new("turn failed", Hue::Fail));
            }
            _ => {}
        }
        if message.state.is_none() {
            message.state = if asks {
                Some(Chip::new("waiting on you", Hue::You))
            } else if filed && entry.kind == EntryKind::Answer {
                Some(Chip::new("answered and filed", Hue::Neutral))
            } else {
                None
            };
        }
        // Records an answer rests on are cited once the turn completes.
        if streaming {
            message.sources.clear();
        }
        message
    }

    /// The streaming caret, Stop and the stopped note of a running or stopped turn.
    fn turn(&self, message: &mut AssistantMessage, live: Option<(&str, u64, bool)>) {
        let Some((_, turn, done)) = live else {
            return;
        };
        let stopping = self
            .session
            .as_ref()
            .and_then(|session| self.state.live.stopping.get(session))
            == Some(&turn);
        if !done {
            message.streaming = true;
            message.state = Some(if stopping {
                Chip::new("stopping", Hue::Neutral)
            } else {
                Chip::new("answering", Hue::Corral)
            });
            if let Some(session) = &self.session {
                let mut stop = button(
                    "Stop",
                    Action::Stop {
                        session: session.clone(),
                        turn,
                    },
                );
                stop.enabled = self.can_send && !stopping;
                message.stop = Some(stop);
            }
        }
    }

    /// The waiting block under a stored message, with File as idea now and Withdraw.
    fn waiting(
        &self,
        entry: &ThreadEntry,
        reason: &corral_protocol::rows::WaitReason,
        full: bool,
    ) -> WaitingBlock {
        let mut buttons = Vec::new();
        let filing = subject("file", &entry.id);
        let withdrawing = subject("withdraw", &entry.id);
        let status = act_chip(self.state, self.feed, &filing, "File as idea")
            .or_else(|| act_chip(self.state, self.feed, &withdrawing, "Withdraw"));
        for (label, action) in [
            ("File as idea now", Action::FileEntry(entry.id.clone())),
            ("Withdraw", Action::WithdrawEntry(entry.id.clone())),
        ] {
            let mut control = button(label, action);
            control.enabled = status.as_ref().is_none_or(|chip| chip.hue == Hue::Fail);
            buttons.push(control);
        }
        WaitingBlock {
            state: Chip::new(
                format!("waiting for assistant · {}", wait_label(reason)),
                Hue::Held,
            ),
            explanation: full.then(|| wait_explanation(reason, &self.feed.date)),
            note: if full { WAIT_NOTE } else { "" },
            buttons,
            status,
            lifetime: WAIT_LIFETIME,
        }
    }
}

/// The served thread `scope` ("quick" or A-n) as transcript items.
pub fn live_items(state: &State, feed: &Feed, scope: &str, can_send: bool) -> Vec<ThreadItem> {
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
    let row = row(feed, scope);
    // The thread's current session: the quick thread's newest generation by rows or entries,
    // whichever arrived first; a named thread is its own session.
    let current = if scope == QUICK {
        quick_id(feed)
    } else {
        Some(scope.to_owned())
    };
    let mut thread = Thread {
        state,
        feed,
        author: current.clone().unwrap_or_else(|| scope.to_owned()),
        session: current.clone(),
        can_send,
        as_written: BTreeSet::new(),
        scope,
    };
    let entries = super::live_parts::assemble(&snapshot.entries);
    let pm_items: BTreeSet<&str> = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::PmMessage)
        .filter_map(|entry| match &entry.card {
            Some(EntryCard::PmItem { id, .. }) => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let replies: BTreeMap<&str, PmReply> = entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::PmReply)
        .filter_map(|entry| match &entry.card {
            Some(EntryCard::PmItem { id, .. }) => Some((
                id.as_str(),
                PmReply {
                    at: clock(&entry.at, &feed.date),
                    text: entry.text.clone().unwrap_or_default(),
                },
            )),
            _ => None,
        })
        .collect();
    // An Idea card and the owner's "saved as written" state name the same filing once; the
    // Idea card (with its commands) stands for it.
    let ideas: BTreeSet<&str> = entries
        .iter()
        .filter_map(|entry| match &entry.card {
            Some(EntryCard::Idea { task, .. }) => Some(task.as_str()),
            _ => None,
        })
        .collect();
    thread.as_written = entries
        .iter()
        .filter_map(|entry| match &entry.state {
            Some(WireState::Saved { task }) => Some(task.clone()),
            _ => None,
        })
        .collect();
    // The waiting block under a message already shows the failure that left it waiting; the
    // notice recording that same failure right after it is the same event, shown once. Any
    // other notice, including a later repeat of the same failure, is its own event.
    let mut explained: Option<String> = None;
    let live = snapshot.live.as_ref();
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
                        items.push(ThreadItem::Waiting(thread.waiting(entry, reason, full)));
                    }
                    Some(WireState::Saved { task }) if !ideas.contains(task.as_str()) => {
                        let mut card = saved_as_written(task, entry.text.as_deref().unwrap_or(""));
                        card.attribution = "Idea · filed from your words, as written · no AI used to file";
                        items.push(ThreadItem::Saved(card));
                    }
                    Some(WireState::Withdrawn) => items.push(ThreadItem::Note(
                        "withdrawn by you · nothing was filed or run · the text stays here for reference".into(),
                    )),
                    _ => {}
                }
            }
            EntryKind::PmMessage => {
                items.push(ThreadItem::Pm(pm_item(state, feed, entry, &replies)))
            }
            EntryKind::PmReply => match &entry.card {
                Some(EntryCard::PmItem { id, .. }) if pm_items.contains(id.as_str()) => {}
                Some(EntryCard::PmItem { id, .. }) => items.push(ThreadItem::PmReply {
                    id: id.clone(),
                    reply: PmReply {
                        at: clock(&entry.at, &feed.date),
                        text: entry.text.clone().unwrap_or_default(),
                    },
                }),
                _ => items.push(ThreadItem::Note(entry.text.clone().unwrap_or_default())),
            },
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
            EntryKind::Answer
            | EntryKind::Idea
            | EntryKind::Action
            | EntryKind::Question
            | EntryKind::Prompt => {
                let streamed = live
                    .filter(|live| live.entry == entry.id)
                    .filter(|live| !super::live_parts::covers(entry, &live.text))
                    .map(|live| (live.text.as_str(), live.turn, live.done));
                items.push(ThreadItem::Assistant(thread.reply(entry, streamed)));
            }
            EntryKind::Other => items.push(ThreadItem::Note(
                "An entry this version of the app cannot show.".into(),
            )),
        }
    }
    // A turn still streaming with no entry yet, of this thread's current session only: a quick
    // thread that rolled to a new generation never shows the previous one's reply as its own,
    // and a finished turn waits for its entry.
    if let Some(live) = live.filter(|live| {
        !live.done
            && !snapshot.entries.contains_key(live.entry.as_str())
            && current
                .as_deref()
                .is_some_and(|session| entry_session(&live.entry) == Some(session))
    }) {
        let mut message = thread.message(
            &ThreadEntry {
                id: live.entry.clone(),
                rev: 0,
                at: String::new(),
                kind: EntryKind::Answer,
                device: None,
                via: None,
                text: None,
                state: None,
                model: row.and_then(|row| row.profile.clone()),
                cost_usd: None,
                tools: Vec::new(),
                links: Vec::new(),
                card: None,
                attachments: Vec::new(),
                typed_at: None,
                part: None,
                completion: None,
                cost_basis: None,
            },
            paragraphs(&live.text),
        );
        message.at = "now".into();
        thread.turn(
            &mut message,
            Some((live.text.as_str(), live.turn, live.done)),
        );
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

/// A PM item, its folded reply, and Withdraw while the PM has not replied.
fn pm_item(
    state: &State,
    feed: &Feed,
    entry: &ThreadEntry,
    replies: &BTreeMap<&str, PmReply>,
) -> PmMessage {
    let (id, revision) = match &entry.card {
        Some(EntryCard::PmItem { id, revision }) => (Some(id.clone()), *revision),
        _ => (None, 0),
    };
    let open = matches!(
        entry.state,
        Some(WireState::QueuedForPm | WireState::PmDelivered | WireState::Delivered)
    );
    let remembered = id.as_deref().map(|id| subject("withdraw", id));
    let status = remembered
        .as_deref()
        .and_then(|remembered| act_chip(state, feed, remembered, "Withdraw"));
    let buttons = match &id {
        Some(id) if open && revision > 0 => {
            let mut withdraw = button(
                "Withdraw",
                Action::WithdrawPm {
                    id: id.clone(),
                    revision,
                },
            );
            withdraw.enabled = status.as_ref().is_none_or(|chip| chip.hue == Hue::Fail);
            vec![withdraw]
        }
        _ => Vec::new(),
    };
    let mut message = owner(state, feed, entry);
    message.tag.hue = match entry.state {
        Some(WireState::PmReplied) => Hue::Ok,
        Some(WireState::Delivered | WireState::PmDelivered) => Hue::Neutral,
        _ => Hue::Held,
    };
    PmMessage {
        reply: id.as_deref().and_then(|id| replies.get(id).cloned()),
        id: id.unwrap_or_else(|| "PMI on arrival".into()),
        note: matches!(entry.state, Some(WireState::QueuedForPm))
            .then_some("Delivered when the PM next takes input; supervision is not interrupted."),
        owner: message,
        buttons,
        status,
    }
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
        let message = OwnerMessage {
            author: "you",
            provenance: "this device".into(),
            at: clock(&send.typed_at, &feed.date),
            text: send.text.clone(),
            tag: Chip::new(label, hue),
            extra: None,
            attachment: attachment(&send.context),
            files: send_chips(feed, &send.attachments),
            opened: None,
        };
        if send.target == corral_protocol::assistant::AskTarget::Pm {
            items.push(ThreadItem::Pm(PmMessage {
                owner: message,
                id: "PMI on arrival".into(),
                reply: None,
                note: None,
                buttons: Vec::new(),
                status: None,
            }));
        } else {
            items.push(ThreadItem::Owner(message));
        }
        if note.is_some() || !buttons.is_empty() {
            items.push(ThreadItem::Queued(QueueNote {
                text: note.unwrap_or_default(),
                buttons,
            }));
        }
    }
}
