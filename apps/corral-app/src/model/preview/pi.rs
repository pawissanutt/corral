//! The sample's pi hosts, answering as corral-pi answers through corrald. A host keeps its
//! session's transcript, streams the running turn on the live session stream one beat per clock
//! tick, holds automated input while paused and delivers it on Resume or before the owner's
//! next message, and records its turn checkpoints. Its controls (cancel, pause, resume, the
//! rewinds) are in `pi_control.rs`; the cards it waits on, its queue and a build's exit are
//! answered in `build_ops.rs`. The fixture publishes recorded changes and live words through
//! the same transcript mirror as a connected session.

use std::collections::VecDeque;

use corral_protocol::detail::{
    InputFacts, SessionCard, SessionItem, SessionMessage, SessionToolState,
};
use corral_protocol::session::{RewindCheckpoint, Rewound};
use corral_protocol::session_state::{ExitBrief, InputOrigin};
use corral_protocol::subscription::{
    SessionLiveEvent, SessionLiveKind, SessionLiveToolState, SessionLiveTurnState,
};
use corral_types::Origin;
use serde_json::{Value, json};

use super::pi_rows::settle;
use super::pi_script::{self, Beat, Question};
use super::sample::sha;
use super::stamp;
use crate::model::Id;
use crate::model::daemon::Feed;

/// Words streamed per clock tick.
const WORDS_PER_TICK: usize = 3;

/// The card title the transcript gives each snapshot of an owner question.
const QUESTION_CARD: &str = "corral_owner_question";

/// Which session a host runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Role {
    #[default]
    Worker,
    Chat,
    Build,
}

/// One sample pi host.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Host {
    pub(super) role: Role,
    /// The recorded transcript, each message with the input id it was sent under, if any.
    pub(super) transcript: Vec<Recorded>,
    /// Where each turn starts in `transcript`: turn n at `starts[n - 1]`.
    pub(super) starts: Vec<usize>,
    /// The newest turn's number.
    pub(super) turn: u64,
    pub(super) open: Option<Turn>,
    pub(super) paused: bool,
    /// Automated input held while paused, in arrival order.
    pub(super) held: Vec<String>,
    /// Owner input that arrived during a turn, read when it ends.
    pub(super) queued: Vec<Input>,
    pub(super) generation: u32,
    seq: u64,
    pub(super) checkpoints: Vec<RewindCheckpoint>,
    pub(super) rewinds: Vec<Rewound>,
    /// Cards waiting for the owner, oldest first.
    pub(super) pending: Vec<SessionCard>,
    /// A build's exit, once one was asked for.
    pub(super) exit: Option<ExitBrief>,
    /// Terminated or lost: no process answers.
    pub(super) gone: bool,
    /// What a build's host serves besides its transcript and cards.
    pub(super) parts: Option<Box<super::parts::Parts>>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Recorded {
    pub(super) entry: Option<String>,
    pub(super) message: SessionMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Input {
    pub(super) text: String,
    pub(super) origin: Origin,
    /// `<session>/input/<id>` for a message sent through the socket.
    pub(super) entry: Option<String>,
}

impl Input {
    /// The queue key of an input sent through the socket: its input id.
    pub(super) fn key(&self) -> Option<&str> {
        self.entry
            .as_deref()
            .and_then(|entry| entry.split_once("/input/"))
            .map(|(_, key)| key)
    }
}

/// The running turn.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Turn {
    pub(super) turn: u64,
    beats: VecDeque<Beat>,
    /// Bytes of the front `Say` streamed so far.
    said: usize,
    /// Output lines of the front tool shown so far; none before it started.
    shown: Option<usize>,
    /// The streaming assistant message's ordinal and words.
    message: u64,
    text: String,
}

impl Host {
    pub(super) fn new(role: Role) -> Self {
        Self {
            role,
            generation: 1,
            ..Self::default()
        }
    }

    /// Whether a turn is running.
    pub(super) fn busy(&self) -> bool {
        self.open.is_some()
    }

    /// A new host on this session's transcript, as a relaunched one opens it: nothing running,
    /// paused or held.
    pub(super) fn continued(&self) -> Self {
        Self {
            role: self.role,
            transcript: self.transcript.clone(),
            starts: self.starts.clone(),
            turn: self.turn,
            checkpoints: self.checkpoints.clone(),
            rewinds: self.rewinds.clone(),
            ..Self::new(self.role)
        }
    }

    /// Records a finished turn, as a seeded transcript holds it.
    pub(super) fn record_turn(&mut self, at: &str, inputs: &[&str], said: Vec<SessionItem>) {
        self.turn += 1;
        self.starts.push(self.transcript.len());
        self.checkpoint(self.turn, at);
        for text in inputs {
            self.transcript.push(input(at, text, None));
        }
        self.transcript.push(Recorded {
            entry: None,
            message: assistant(at, said, None),
        });
    }

    fn checkpoint(&mut self, turn: u64, at: &str) {
        let id = format!("cp-{}-{turn}", self.generation);
        self.checkpoints.push(RewindCheckpoint {
            commit: Some(sha(&id, 40)),
            head: Some(sha(&format!("{id}/head"), 40)),
            reference: Some(format!("refs/worktree/corral/checkpoints/{turn}-start")),
            phase: "start".into(),
            turn,
            id,
            at: Some(at.into()),
            ..RewindCheckpoint::default()
        });
    }

    pub(super) fn event(&mut self, at: &str, kind: SessionLiveKind) -> SessionLiveEvent {
        self.seq += 1;
        SessionLiveEvent {
            seq: self.seq,
            generation: self.generation,
            at: at.into(),
            kind,
        }
    }
}

/// A recorded owner or automated input; corrald shows automated input as a host notice. An
/// input sent through the socket is served under its input id (`<session>/input/<id>`).
fn input(at: &str, text: &str, entry: Option<String>) -> Recorded {
    let (item, origin) = match text.strip_prefix("[corral] ") {
        Some(notice) => (
            SessionItem::Card {
                title: "Host notice".into(),
                text: notice.into(),
                data: None,
            },
            InputOrigin::Auto,
        ),
        None => (SessionItem::Text { text: text.into() }, InputOrigin::Owner),
    };
    let input = entry
        .as_deref()
        .and_then(|entry| entry.split_once("/input/"))
        .map(|(_, id)| id.to_owned());
    Recorded {
        message: SessionMessage {
            id: entry.clone().unwrap_or_default(),
            author: "user".into(),
            at: Some(at.into()),
            items: vec![item],
            model: None,
            stop_reason: None,
            usage: None,
            input: Some(InputFacts {
                id: input,
                origin,
                typed_at: None,
                device: None,
                to: None,
            }),
            attachments: Vec::new(),
        },
        entry,
    }
}

fn assistant(at: &str, items: Vec<SessionItem>, stop: Option<&str>) -> SessionMessage {
    SessionMessage {
        id: String::new(),
        author: "assistant".into(),
        at: Some(at.into()),
        items,
        model: Some("preview · sol".into()),
        stop_reason: stop.map(str::to_owned),
        usage: None,
        input: None,
        attachments: Vec::new(),
    }
}

/// A question snapshot, as the transcript records each change of one.
pub(super) fn question_card(at: &str, card: &SessionCard, status: &str, answer: Value) -> Recorded {
    let questions: Vec<Value> = card
        .questions
        .iter()
        .map(|question| {
            json!({
                "question": question.text,
                "options": question.options.iter().map(|option| &option.label).collect::<Vec<_>>(),
                "freeText": question.free_text,
            })
        })
        .collect();
    let data = json!({
        "version": 1,
        "id": card.id,
        "questions": questions,
        "status": status,
        "answers": answer,
    });
    Recorded {
        entry: None,
        message: SessionMessage {
            id: String::new(),
            author: "custom".into(),
            at: Some(at.into()),
            items: vec![SessionItem::Card {
                title: QUESTION_CARD.into(),
                text: card.title.clone(),
                data: Some(data.to_string()),
            }],
            model: None,
            stop_reason: None,
            usage: None,
            input: None,
            attachments: Vec::new(),
        },
    }
}

/// The host a scope addresses: the quick thread's current session, else the scope itself.
pub(super) fn resolve(feed: &Feed, scope: &str) -> Option<Id> {
    let id = if scope == crate::model::assistant::QUICK {
        crate::model::assistant::quick_id(feed)?
    } else {
        scope.to_owned()
    };
    feed.hosts.contains_key(id.as_str()).then(|| Id::from(id))
}

/// The watched scopes that show host `id`.
pub(super) fn scopes(feed: &Feed, id: &str) -> Vec<Id> {
    feed.topics
        .keys()
        .filter_map(|topic| topic.strip_prefix("session:"))
        .filter(|scope| resolve(feed, scope).is_some_and(|host| host == id))
        .map(Id::from)
        .collect()
}

/// Delivers `events` of host `id` to every watcher, as corrald's page for each scope does.
pub(super) fn publish(feed: &mut Feed, id: &str, events: Vec<SessionLiveEvent>, gap: bool) {
    let Some(generation) = feed.hosts.get(id).map(|host| host.generation) else {
        return;
    };
    for scope in scopes(feed, id) {
        if gap
            || events.iter().any(|event| {
                matches!(
                    &event.kind,
                    SessionLiveKind::LiveEntry {}
                        | SessionLiveKind::LiveTurn { .. }
                        | SessionLiveKind::LiveTool {
                            state: SessionLiveToolState::Started { .. },
                            ..
                        }
                )
            })
        {
            super::pi_rows::page(feed, &scope);
        }
        apply_words(feed, &scope, generation, &events);
        feed.live
            .entry(scope)
            .or_default()
            .apply_page(Some(generation), events.clone(), gap);
    }
}

fn apply_words(feed: &mut Feed, scope: &str, generation: u32, events: &[SessionLiveEvent]) {
    use corral_protocol::detail::DetailRef;
    use corral_protocol::subscription::{TranscriptOp, TranscriptPart};
    let target = DetailRef::Session {
        id: scope.to_owned(),
    };
    let Some(cache) = feed
        .details
        .get_mut(&crate::model::daemon::detail_key(&target))
    else {
        return;
    };
    for event in events {
        let SessionLiveKind::LiveText {
            turn,
            message,
            offset,
            text,
        } = &event.kind
        else {
            continue;
        };
        let id = format!("{scope}/open/{generation}.{turn}.{message}");
        let mut ops = Vec::new();
        if cache.transcript_seams.mirror.open.as_ref() != Some(&id) {
            let mut message = assistant(&event.at, Vec::new(), None);
            message.id = id.clone();
            ops.push(TranscriptOp::Put {
                message,
                before: None,
                replaces: None,
                open: true,
                cut: false,
                again: false,
            });
        }
        ops.push(TranscriptOp::Append {
            id,
            part: TranscriptPart::Text,
            offset: *offset,
            text: text.clone(),
        });
        crate::model::session::transcript::mirror::apply(cache, ops, false);
    }
}

/// Publish the host's changed records to every watcher.
pub(super) fn recorded(feed: &mut Feed, id: &str) {
    let at = stamp(feed.now);
    let Some(host) = feed.hosts.get_mut(id) else {
        return;
    };
    let event = host.event(&at, SessionLiveKind::LiveEntry {});
    publish(feed, id, vec![event], false);
}

/// A new watcher gets the running turn so far, as a page from the present does.
pub(super) fn watch(feed: &mut Feed, scope: &str) {
    let Some(id) = resolve(feed, scope) else {
        return;
    };
    let at = stamp(feed.now);
    let Some(host) = feed.hosts.get_mut(id.as_str()) else {
        return;
    };
    let mut events = Vec::new();
    if let Some(turn) = host.open.clone() {
        let started = SessionLiveKind::LiveTurn {
            turn: turn.turn,
            state: SessionLiveTurnState::Started,
        };
        events.push(host.event(&at, started));
        if !turn.text.is_empty() {
            let text = SessionLiveKind::LiveText {
                turn: turn.turn,
                message: turn.message,
                offset: 0,
                text: turn.text,
            };
            events.push(host.event(&at, text));
        }
    }
    let generation = host.generation;
    super::pi_rows::page(feed, scope);
    apply_words(feed, scope, generation, &events);
    feed.live
        .entry(Id::from(scope))
        .or_default()
        .apply_page(Some(generation), events, false);
}

/// One clock tick: every running turn takes its next beat, and a landing build's exit its next
/// phase.
pub fn tick(feed: &mut Feed) {
    let running: Vec<Id> = feed
        .hosts
        .iter()
        .filter(|(_, host)| host.open.is_some())
        .map(|(id, _)| id.clone())
        .collect();
    for id in running {
        step(feed, &id);
    }
    super::build_ops::tick(feed);
    super::host::tick(feed);
}

/// What a beat left to do once its events are out.
enum After {
    Nothing,
    /// A tool ended: its message is recorded.
    Recorded,
    /// The turn ended, asking this question when one is given.
    End(Option<Question>),
}

fn step(feed: &mut Feed, id: &str) {
    let at = stamp(feed.now);
    let Some(host) = feed.hosts.get_mut(id) else {
        return;
    };
    let Some(mut turn) = host.open.take() else {
        return;
    };
    let mut kinds = Vec::new();
    let after = match turn.beats.front().cloned() {
        None => After::End(None),
        Some(Beat::Ask(question)) => After::End(Some(question)),
        Some(Beat::Say(words)) => {
            let chunk = next_words(&words[turn.said..]);
            kinds.push(SessionLiveKind::LiveText {
                turn: turn.turn,
                message: turn.message,
                offset: turn.text.len() as u64,
                text: chunk.to_owned(),
            });
            turn.text.push_str(chunk);
            turn.said += chunk.len();
            if turn.said >= words.len() {
                turn.beats.pop_front();
                turn.said = 0;
            }
            After::Nothing
        }
        Some(Beat::Tool { tool, args, lines }) => {
            let call = format!("call-{}-{}", turn.turn, turn.message);
            let state = match turn.shown {
                None => SessionLiveToolState::Started {
                    args: Some(args.clone()),
                    args_truncated: None,
                },
                Some(shown) if shown < lines.len() => SessionLiveToolState::Updated {
                    partial: Some(lines[..=shown].join("\n")),
                },
                Some(_) => SessionLiveToolState::Ended {
                    failed: false,
                    elapsed_ms: Some(1000 * (lines.len() as u64 + 1)),
                },
            };
            let ended = matches!(state, SessionLiveToolState::Ended { .. });
            kinds.push(SessionLiveKind::LiveTool {
                turn: turn.turn,
                call: call.clone(),
                tool: tool.into(),
                state,
            });
            if ended {
                // Settle the existing call record rather than inserting a second row.
                if let Some(item) = host.transcript.iter_mut().rev().flat_map(|entry| entry.message.items.iter_mut()).find(|item| {
                    matches!(item, SessionItem::ToolCall { call_id, .. } if *call_id == call)
                }) {
                    *item = tool_item(call, tool, &args, &lines, true);
                }
                turn.beats.pop_front();
                turn.shown = None;
                turn.message += 1;
                turn.text.clear();
                After::Recorded
            } else {
                if turn.shown.is_none() {
                    let mut items = words(&turn.text);
                    let mut item = tool_item(call, tool, &args, &[], false);
                    if let SessionItem::ToolCall { state, .. } = &mut item {
                        *state = SessionToolState::Running;
                    }
                    items.push(item);
                    host.transcript.push(Recorded {
                        entry: None,
                        message: assistant(&at, items, Some("toolUse")),
                    });
                    turn.text.clear();
                }
                turn.shown = Some(turn.shown.map_or(0, |shown| shown + 1));
                After::Nothing
            }
        }
    };
    if let After::End(question) = &after {
        finish(host, &at, &turn, question.clone(), false);
        kinds.push(SessionLiveKind::LiveTurn {
            turn: turn.turn,
            state: SessionLiveTurnState::Ended {
                aborted: false,
                error: None,
            },
        });
    } else {
        host.open = Some(turn);
    }
    let events = kinds
        .into_iter()
        .map(|kind| host.event(&at, kind))
        .collect();
    publish(feed, id, events, false);
    match after {
        After::Nothing => {}
        After::Recorded => recorded(feed, id),
        After::End(_) => {
            // The turn's last words are recorded now.
            recorded(feed, id);
            let queued = feed
                .hosts
                .get_mut(id)
                .map(|host| std::mem::take(&mut host.queued))
                .unwrap_or_default();
            if !queued.is_empty() {
                start(feed, id, queued);
            }
            settle(feed, id);
        }
    }
}

/// The next few words of `rest`, with the space after them.
fn next_words(rest: &str) -> &str {
    let mut end = rest.len();
    let mut seen = 0;
    for (index, ch) in rest.char_indices() {
        if ch == ' ' {
            seen += 1;
            if seen == WORDS_PER_TICK {
                end = index + 1;
                break;
            }
        }
    }
    &rest[..end]
}

fn words(text: &str) -> Vec<SessionItem> {
    if text.trim().is_empty() {
        Vec::new()
    } else {
        vec![SessionItem::Text {
            text: text.trim_end().to_owned(),
        }]
    }
}

pub(super) fn tool_item(
    call: String,
    tool: &str,
    args: &str,
    lines: &[String],
    done: bool,
) -> SessionItem {
    SessionItem::ToolCall {
        call_id: call,
        name: tool.into(),
        arguments: args.into(),
        summary: args.lines().next().unwrap_or_default().into(),
        state: if done {
            SessionToolState::Done
        } else {
            SessionToolState::NotRun
        },
        paired: done,
        call: None,
        output: done.then(|| lines.join("\n")),
        result: None,
        elapsed_ms: done.then(|| 1000 * (lines.len() as u64 + 1)),
        diff: None,
        content: None,
    }
}

/// Records the end of `turn`: its last words, a call it cancelled before its result, and how
/// it stopped.
pub(super) fn finish(
    host: &mut Host,
    at: &str,
    turn: &Turn,
    question: Option<Question>,
    aborted: bool,
) {
    let mut items = words(&turn.text);
    if aborted {
        if let (Some(_), Some(Beat::Tool { tool, args, .. })) = (turn.shown, turn.beats.front()) {
            let call = format!("call-{}-{}", turn.turn, turn.message);
            items.push(tool_item(call, tool, args, &[], false));
        }
        items.push(SessionItem::Aborted {
            message: "Cancelled by the owner".into(),
            facts: None,
        });
    }
    if !items.is_empty() {
        let stop = if aborted { "aborted" } else { "stop" };
        host.transcript.push(Recorded {
            entry: None,
            message: assistant(at, items, Some(stop)),
        });
    }
    if let Some(question) = question {
        let card = question.card(at);
        host.transcript
            .push(question_card(at, &card, "pending", Value::Null));
        host.pending.push(card);
    }
}

/// Starts the next turn on `inputs`, recorded in order, saying what the script answers.
pub(super) fn start(feed: &mut Feed, id: &str, inputs: Vec<Input>) {
    let reply = pi_script::reply(feed, id, &inputs);
    start_with(feed, id, inputs, reply);
}

/// Starts the next turn on `inputs`, saying `reply`.
pub(super) fn start_with(feed: &mut Feed, id: &str, inputs: Vec<Input>, reply: Vec<Beat>) {
    let at = stamp(feed.now);
    let Some(host) = feed.hosts.get_mut(id) else {
        return;
    };
    host.turn += 1;
    host.starts.push(host.transcript.len());
    host.checkpoint(host.turn, &at);
    for input_ in &inputs {
        host.transcript
            .push(input(&at, &input_.text, input_.entry.clone()));
    }
    host.open = Some(Turn {
        turn: host.turn,
        beats: reply.into(),
        said: 0,
        shown: None,
        message: 1,
        text: String::new(),
    });
    let started = SessionLiveKind::LiveTurn {
        turn: host.turn,
        state: SessionLiveTurnState::Started,
    };
    let event = host.event(&at, started);
    publish(feed, id, vec![event], false);
    settle(feed, id);
}

/// How the host answers input: `queued` while paused and automated, else delivered. An owner's
/// input lifts a pause and goes after the input held meanwhile.
pub(super) fn deliver(
    feed: &mut Feed,
    id: &str,
    text: &str,
    origin: Origin,
    entry: Option<String>,
) -> &'static str {
    let Some(host) = feed.hosts.get_mut(id) else {
        return "unknown";
    };
    if host.gone {
        return "gone";
    }
    if host.paused && origin == Origin::Auto {
        host.held.push(text.to_owned());
        settle(feed, id);
        return "queued";
    }
    let mut inputs: Vec<Input> = Vec::new();
    if origin == Origin::Owner {
        host.paused = false;
        inputs.extend(host.held.drain(..).map(|text| Input {
            text,
            origin: Origin::Auto,
            entry: None,
        }));
    }
    inputs.push(Input {
        text: text.to_owned(),
        origin,
        entry,
    });
    if host.open.is_some() {
        host.queued.extend(inputs);
        // A steer waits in the host's queue; its acknowledgement says so until it starts.
        settle(feed, id);
        return "steered";
    }
    start(feed, id, inputs);
    recorded(feed, id);
    "delivered"
}

/// The host of a run that ended stops; one whose run is replaced continues its transcript.
pub(super) fn end(feed: &mut Feed, id: &str) {
    let at = stamp(feed.now);
    super::pi_control::cancel(feed, id, &at);
    if let Some(host) = feed.hosts.get_mut(id) {
        host.gone = true;
        host.paused = false;
        host.held.clear();
        host.generation += 1;
    }
    publish(feed, id, Vec::new(), true);
    settle(feed, id);
}

/// A chat host that was terminated or lost starts again on its transcript.
pub(super) fn revive(feed: &mut Feed, id: &str) {
    if let Some(host) = feed.hosts.get_mut(id) {
        host.gone = false;
    }
    settle(feed, id);
}

/// Every host's rows, after the engine changed the records: a host whose run ended stops.
pub(super) fn sync(feed: &mut Feed) {
    let ended: Vec<Id> = feed
        .hosts
        .iter()
        .filter(|(id, host)| {
            host.role == Role::Worker && !host.gone && !feed.agents.contains_key(id.as_str())
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in ended {
        end(feed, &id);
    }
    super::pi_rows::settle_all(feed);
}

/// The id corrald acknowledges a message with and serves it under.
pub(super) fn input_entry(session: &str, cmd_id: &str) -> String {
    format!("{session}/input/{cmd_id}")
}
