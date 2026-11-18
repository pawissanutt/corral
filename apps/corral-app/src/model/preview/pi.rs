//! The sample's pi hosts, answering as corral-pi answers through corrald. A host keeps its
//! session's transcript, streams the running turn on the live session stream one beat per clock
//! tick, cancels and pauses it, holds automated input while paused and delivers it on Resume or
//! before the owner's next message, asks the owner and takes the answer, and lists its turn
//! checkpoints and rewinds to one after saving the work under a rescue ref. A session's
//! transcript detail is read from its host on request, as corrald reads the file, so a recorded
//! turn shows only once the app re-reads it.

use std::collections::VecDeque;

use corral_protocol::detail::{SessionItem, SessionMessage, SessionToolState};
use corral_protocol::session::{RewindCheckpoint, RewindReply, Rewound};
use corral_protocol::subscription::{
    SessionLiveEvent, SessionLiveKind, SessionLiveToolState, SessionLiveTurnState,
};
use corral_types::Origin;
use serde_json::{Value, json};

use super::pi_rows::{settle, thread};
use super::pi_script::{self, Beat, Question};
use super::sample::sha;
use super::stamp;
use crate::model::Id;
use crate::model::agents::QUESTION_CARD;
use crate::model::daemon::Feed;

/// Words streamed per clock tick.
const WORDS_PER_TICK: usize = 3;

/// Which session a host runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Role {
    #[default]
    Worker,
    Chat,
}

/// One sample pi host.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Host {
    pub(super) role: Role,
    /// The recorded transcript, each message with the thread entry it is for a chat.
    pub(super) transcript: Vec<Recorded>,
    /// Where each turn starts in `transcript`: turn n at `starts[n - 1]`.
    pub(super) starts: Vec<usize>,
    /// The newest turn's number.
    pub(super) turn: u64,
    open: Option<Turn>,
    pub(super) paused: bool,
    /// Automated input held while paused, in arrival order.
    held: Vec<String>,
    /// Owner input that arrived during a turn, read when it ends.
    queued: Vec<Input>,
    generation: u32,
    seq: u64,
    pub(super) checkpoints: Vec<RewindCheckpoint>,
    pub(super) rewinds: Vec<Rewound>,
    /// Questions waiting for the owner.
    pub(super) pending: Vec<Question>,
    /// Terminated or lost: no process answers.
    pub(super) gone: bool,
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
    /// The thread entry a chat message is recorded as.
    pub(super) entry: Option<String>,
}

/// The running turn.
#[derive(Debug, Clone, PartialEq)]
struct Turn {
    turn: u64,
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
        self.checkpoint(self.turn);
        for text in inputs {
            self.transcript.push(input(at, text, None));
        }
        self.transcript.push(Recorded {
            entry: None,
            message: assistant(at, said, None),
        });
    }

    fn checkpoint(&mut self, turn: u64) {
        let id = format!("cp-{}-{turn}", self.generation);
        self.checkpoints.push(RewindCheckpoint {
            commit: Some(sha(&id, 40)),
            head: Some(sha(&format!("{id}/head"), 40)),
            reference: Some(format!("refs/worktree/corral/checkpoints/{turn}-start")),
            leaf: None,
            error: None,
            phase: "start".into(),
            turn,
            id,
        });
    }

    fn event(&mut self, at: &str, kind: SessionLiveKind) -> SessionLiveEvent {
        self.seq += 1;
        SessionLiveEvent {
            seq: self.seq,
            generation: self.generation,
            at: at.into(),
            kind,
        }
    }
}

/// A recorded owner or automated input; corrald shows automated input as a host notice.
fn input(at: &str, text: &str, entry: Option<String>) -> Recorded {
    let item = match text.strip_prefix("[corral] ") {
        Some(notice) => SessionItem::Card {
            title: "Host notice".into(),
            text: notice.into(),
            data: None,
        },
        None => SessionItem::Text { text: text.into() },
    };
    Recorded {
        entry,
        message: SessionMessage {
            author: "user".into(),
            at: Some(at.into()),
            items: vec![item],
            model: None,
            stop_reason: None,
            usage: None,
        },
    }
}

fn assistant(at: &str, items: Vec<SessionItem>, stop: Option<&str>) -> SessionMessage {
    SessionMessage {
        author: "assistant".into(),
        at: Some(at.into()),
        items,
        model: Some("preview · sol".into()),
        stop_reason: stop.map(str::to_owned),
        usage: None,
    }
}

/// A question snapshot, as the transcript records each change of one.
pub(super) fn question_card(
    at: &str,
    question: &Question,
    status: &str,
    answer: Value,
) -> Recorded {
    let data = json!({
        "version": 1,
        "id": question.id,
        "questions": [{
            "question": question.text,
            "options": question.options,
            "freeText": true,
        }],
        "status": status,
        "answers": answer,
    });
    Recorded {
        entry: None,
        message: SessionMessage {
            author: "custom".into(),
            at: Some(at.into()),
            items: vec![SessionItem::Card {
                title: QUESTION_CARD.into(),
                text: question.text.clone(),
                data: Some(data.to_string()),
            }],
            model: None,
            stop_reason: None,
            usage: None,
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
fn scopes(feed: &Feed, id: &str) -> Vec<Id> {
    feed.topics
        .keys()
        .filter_map(|topic| topic.strip_prefix("session:"))
        .filter(|scope| resolve(feed, scope).is_some_and(|host| host == id))
        .map(Id::from)
        .collect()
}

/// Delivers `events` of host `id` to every watcher, as corrald's page for each scope does.
fn publish(feed: &mut Feed, id: &str, events: Vec<SessionLiveEvent>, gap: bool) {
    let Some(generation) = feed.hosts.get(id).map(|host| host.generation) else {
        return;
    };
    for scope in scopes(feed, id) {
        feed.live
            .entry(scope)
            .or_default()
            .apply_page(Some(generation), events.clone(), gap);
    }
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
    feed.live
        .entry(Id::from(scope))
        .or_default()
        .apply_page(Some(generation), events, false);
}

/// One clock tick: every running turn takes its next beat.
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
                // The calling message and its result are recorded once the tool ends; the
                // stream shows both until the app re-reads them.
                let mut items = words(&turn.text);
                items.push(tool_item(call, tool, &args, &lines, true));
                host.transcript.push(Recorded {
                    entry: None,
                    message: assistant(&at, items, Some("toolUse")),
                });
                turn.beats.pop_front();
                turn.shown = None;
                turn.message += 1;
                turn.text.clear();
                After::Recorded
            } else {
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
        After::Recorded => thread(feed, id),
        After::End(_) => {
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
fn finish(host: &mut Host, at: &str, turn: &Turn, question: Option<Question>, aborted: bool) {
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
        host.transcript
            .push(question_card(at, &question, "pending", Value::Null));
        host.pending.push(question);
    }
}

/// Starts the next turn on `inputs`, recorded in order, saying what the script answers.
fn start(feed: &mut Feed, id: &str, inputs: Vec<Input>) {
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
    host.checkpoint(host.turn);
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
        thread(feed, id);
        settle(feed, id);
        return "steered";
    }
    start(feed, id, inputs);
    thread(feed, id);
    "delivered"
}

/// A session control a pi host answers: Cancel, Pause, Resume and the rewinds. The reply is
/// corrald's; `Err` is its refusal.
pub(super) fn control(
    feed: &mut Feed,
    id: &str,
    control: crate::model::agents::Control,
) -> Result<Value, String> {
    use crate::model::agents::Control;
    let at = stamp(feed.now);
    let host = feed
        .hosts
        .get(id)
        .ok_or_else(|| "session has no running Pi host; relaunch it first".to_owned())?;
    if host.gone {
        return Err("session has ended; relaunch it before sending controls".into());
    }
    let list = RewindReply::List {
        checkpoints: host.checkpoints.clone(),
        rewinds: host.rewinds.clone(),
    };
    let reply = match control {
        Control::Cancel => {
            let turn = cancel(feed, id, &at);
            match turn {
                Some(turn) => json!({"cancelled": true, "turn": turn}),
                None => json!({"cancelled": false, "reason": "no turn is running"}),
            }
        }
        Control::Pause => {
            let turn = cancel(feed, id, &at);
            if let Some(host) = feed.hosts.get_mut(id) {
                host.paused = true;
            }
            json!({"paused": true, "cancelled": turn.is_some(), "turn": turn})
        }
        Control::Resume => {
            let held = feed
                .hosts
                .get_mut(id)
                .map(|host| {
                    host.paused = false;
                    std::mem::take(&mut host.held)
                })
                .unwrap_or_default();
            if !held.is_empty() {
                let inputs = held
                    .into_iter()
                    .map(|text| Input {
                        text,
                        origin: Origin::Auto,
                        entry: None,
                    })
                    .collect();
                start(feed, id, inputs);
            }
            json!({"paused": false})
        }
        Control::Rewinds => serde_json::to_value(list).map_err(|error| error.to_string())?,
        Control::Rewind(turn) => rewind(feed, id, turn, &at)?,
        Control::Terminate | Control::Relaunch => {
            return Err("terminate and relaunch go through corrald, not the host".into());
        }
    };
    settle(feed, id);
    thread(feed, id);
    Ok(reply)
}

/// Cancels the running turn, keeping what it said; returns its number.
fn cancel(feed: &mut Feed, id: &str, at: &str) -> Option<u64> {
    let host = feed.hosts.get_mut(id)?;
    let turn = host.open.take()?;
    finish(host, at, &turn, None, true);
    let ended = SessionLiveKind::LiveTurn {
        turn: turn.turn,
        state: SessionLiveTurnState::Ended {
            aborted: true,
            error: None,
        },
    };
    let event = host.event(at, ended);
    publish(feed, id, vec![event], false);
    Some(turn.turn)
}

/// Saves the work under a rescue ref, then cuts the transcript back to the start of `turn`. The
/// live stream starts a new generation, so every watcher re-reads the transcript.
fn rewind(feed: &mut Feed, id: &str, turn: u64, at: &str) -> Result<Value, String> {
    let point = feed
        .hosts
        .get(id)
        .and_then(|host| host.checkpoints.iter().find(|point| point.turn == turn))
        .cloned()
        .ok_or_else(|| format!("no checkpoint at the start of turn {turn}"))?;
    cancel(feed, id, at);
    let host = feed
        .hosts
        .get_mut(id)
        .ok_or_else(|| "session has no running Pi host".to_owned())?;
    let cut = host
        .starts
        .get(
            usize::try_from(turn)
                .unwrap_or(usize::MAX)
                .saturating_sub(1),
        )
        .copied()
        .unwrap_or(host.transcript.len());
    let number = host.rewinds.len() + 1;
    let rewound = Rewound {
        turn,
        checkpoint: point.id.clone(),
        rescue_ref: format!("refs/worktree/corral/rescue/{id}/{number}"),
        from_head: sha(&format!("{id}/rescue/{number}"), 40),
        head: point.head.clone().unwrap_or_default(),
        at: at.into(),
    };
    host.transcript.truncate(cut);
    host.starts.truncate(cut_turns(turn));
    host.checkpoints.retain(|point| point.turn < turn);
    host.turn = turn.saturating_sub(1);
    host.pending.clear();
    host.queued.clear();
    host.rewinds.push(rewound.clone());
    host.generation += 1;
    publish(feed, id, Vec::new(), true);
    serde_json::to_value(RewindReply::Turn(rewound)).map_err(|error| error.to_string())
}

fn cut_turns(turn: u64) -> usize {
    usize::try_from(turn.saturating_sub(1)).unwrap_or(usize::MAX)
}

/// Answers a question the host asked: the transcript records it answered, a pause lifts and
/// the next turn starts on the answer.
pub(super) fn answer(
    feed: &mut Feed,
    answer: &corral_protocol::session::SessionPromptAnswer,
) -> Result<Value, String> {
    let id = resolve(feed, &answer.session)
        .ok_or_else(|| format!("{} has no Pi host", answer.session))?;
    let at = stamp(feed.now);
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .ok_or_else(|| format!("{} has no Pi host", answer.session))?;
    let index = host
        .pending
        .iter()
        .position(|question| question.id == answer.question)
        .ok_or_else(|| format!("no pending question {}", answer.question))?;
    let question = host.pending.remove(index);
    let given = json!([{"option": answer.option, "text": answer.text}]);
    host.transcript
        .push(question_card(&at, &question, "answered", given));
    let words = match (&answer.option, &answer.text) {
        (Some(option), Some(text)) => format!("{option} — {text}"),
        (Some(option), None) => option.clone(),
        (None, Some(text)) => text.clone(),
        (None, None) => String::new(),
    };
    deliver(
        feed,
        id.as_str(),
        &format!("[ask_user answer {}] {words}", question.id),
        Origin::Owner,
        None,
    );
    Ok(json!({"status": "delivered"}))
}

/// The host of a run that ended stops; one whose run is replaced continues its transcript.
pub(super) fn end(feed: &mut Feed, id: &str) {
    let at = stamp(feed.now);
    cancel(feed, id, &at);
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

/// The thread entry id corrald acknowledges a chat message with.
pub(super) fn input_entry(session: &str, cmd_id: &str) -> String {
    format!("{session}/input/{cmd_id}")
}
