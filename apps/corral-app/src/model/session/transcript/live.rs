//! What corrald streams about one watched session: its open turn, its child agents, the host's
//! current state, each owner input's admission and which session parts changed; and transcript
//! items whose content arrives in continuation pages. Updates are pure and independent of
//! transport.
use corral_protocol::detail::SessionItem;
use corral_protocol::session_state::SessionState;
use corral_protocol::subscription::{
    SessionFleet, SessionLiveEvent, SessionLiveInputState, SessionLiveKind, SessionLiveToolState,
    SessionLiveTurnState,
};

/// One transcript item continuation, keyed in the feed by its token.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemFetch {
    Loading,
    /// The complete item the preview stood for.
    Ready(SessionItem),
    Failed(String),
}

/// The open turn of a watched session as its host reports it, laid over the recorded
/// transcript. An element stays only until the recorded transcript holds it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveSession {
    pub generation: Option<u32>,
    pub turn: Option<u64>,
    /// True between a turn's start and its end.
    pub open: bool,
    /// How the last turn ended, until the next one starts.
    pub ended: Option<TurnEnd>,
    /// Assistant messages of the turn, by ordinal.
    pub messages: Vec<LiveMessage>,
    /// Tool calls of the turn, in start order.
    pub tools: Vec<LiveTool>,
    /// Owner input the host holds for after the current step, and for after the turn.
    pub steer: u64,
    pub follow_up: u64,
    /// Counts the times the recorded transcript fell behind the host: a tool or turn ended,
    /// or the stream had a gap. The session page re-reads its transcript when it changes.
    pub reread: u64,
    /// Highest sequence applied since the last discontinuity.
    pub(crate) last_seq: Option<u64>,
    /// The session's child agents and the messages between them, as of the latest page; only
    /// a connection that asked for `session.fleet.v1` ever holds one.
    pub fleet: Option<SessionFleet>,
    /// The host's current state, as the latest page that carried one replaced it; only a
    /// connection that asked for `session.state.v1` ever holds one.
    pub state: Option<SessionState>,
    /// When the open (or last) turn started and ended, as the host stamped its events.
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    /// Each owner input's latest admission, by input id (the sender's command id), newest
    /// last; bounded to `MAX_INPUTS`.
    pub inputs: Vec<(String, SessionLiveInputState)>,
    /// How many times each named session part (`cards`, `children`, …) changed; a watcher
    /// re-reads a part's detail when its count moves.
    pub parts: std::collections::BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEnd {
    pub aborted: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveMessage {
    /// One-based within the turn.
    pub ordinal: u64,
    pub text: String,
    pub thinking: String,
    /// The `reread` count at which this message was complete; it is dropped once a
    /// transcript read requested at or after that count has arrived.
    pub settled_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTool {
    pub call: String,
    pub tool: String,
    /// Bounded JSON text of the arguments, when the host sent them.
    pub args: Option<String>,
    pub args_truncated: bool,
    /// The bounded tail of the tool's output so far.
    pub partial: Option<String>,
    pub state: LiveToolState,
    pub elapsed_ms: Option<u64>,
    /// As `LiveMessage::settled_at`.
    pub settled_at: Option<u64>,
    /// The ordinal of the turn's latest message when the call started (0 before any): the call
    /// stands after that message's words and before the next message's.
    pub after: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveToolState {
    Running,
    Done,
    Failed,
}

const MAX_MESSAGE_BYTES: usize = 256 * 1024;
const MAX_TOOLS: usize = 256;
const MAX_INPUTS: usize = 64;

impl LiveSession {
    /// Invalidate the overlay without removing the watched session's entry.
    pub fn reset(&mut self) {
        self.turn = None;
        self.open = false;
        self.ended = None;
        self.messages.clear();
        self.tools.clear();
        self.steer = 0;
        self.follow_up = 0;
        self.last_seq = None;
        self.reread = self.reread.saturating_add(1);
    }

    /// A page's fleet is the session's whole current fleet, and a page without one says the
    /// session has no children or messages: it replaces what was held, never adds to it. The
    /// turn overlay's resets leave it alone, so a stale fleet stays shown as stale.
    pub fn replace_fleet(&mut self, fleet: Option<SessionFleet>) {
        self.fleet = fleet;
    }

    /// A page's state replaces the held one; a page without one says it is unchanged, so the
    /// held state stays (shown as stale while the link is down).
    pub fn replace_state(&mut self, state: Option<Box<SessionState>>) {
        if let Some(state) = state {
            self.state = Some(*state);
        }
    }

    /// The latest admission the host announced for input `id`.
    pub fn input(&self, id: &str) -> Option<&SessionLiveInputState> {
        self.inputs
            .iter()
            .rev()
            .find(|(input, _)| input == id)
            .map(|(_, state)| state)
    }

    /// A transcript requested at N cannot settle an element completed after N.
    pub fn settle(&mut self, reread: u64) {
        self.messages
            .retain(|item| item.settled_at.is_none_or(|at| at > reread));
        self.tools
            .retain(|item| item.settled_at.is_none_or(|at| at > reread));
    }

    pub fn apply_page(
        &mut self,
        generation: Option<u32>,
        mut events: Vec<SessionLiveEvent>,
        gap: bool,
    ) {
        if self.generation != generation || gap {
            self.reset();
            self.generation = generation;
        }
        events.sort_unstable_by_key(|event| event.seq);
        for event in events {
            if Some(event.generation) != self.generation
                || self.last_seq.is_some_and(|seq| event.seq <= seq)
            {
                continue;
            }
            self.apply_event(event.kind, event.at);
            self.last_seq = Some(event.seq);
        }
    }

    fn apply_event(&mut self, event: SessionLiveKind, at: String) {
        // A bounded producer buffer can begin after the turn's Started event.
        if self.turn.is_none() {
            let turn = match &event {
                SessionLiveKind::LiveTurn { turn, .. }
                | SessionLiveKind::LiveText { turn, .. }
                | SessionLiveKind::LiveThinking { turn, .. }
                | SessionLiveKind::LiveTool { turn, .. } => Some(*turn),
                _ => None,
            };
            if let Some(turn) = turn {
                self.turn = Some(turn);
                self.open = true;
                self.ended = None;
                self.started_at = Some(at.clone());
                self.ended_at = None;
            }
        }
        match event {
            SessionLiveKind::LiveTurn {
                turn,
                state: SessionLiveTurnState::Started,
            } => {
                if self.turn != Some(turn) {
                    self.messages.clear();
                    self.tools.clear();
                    self.turn = Some(turn);
                }
                self.open = true;
                self.ended = None;
                self.started_at = Some(at);
                self.ended_at = None;
            }
            SessionLiveKind::LiveTurn {
                turn,
                state: SessionLiveTurnState::Ended { aborted, error },
            } if self.open && self.turn == Some(turn) => {
                self.open = false;
                self.ended = Some(TurnEnd { aborted, error });
                self.ended_at = Some(at);
                self.reread = self.reread.saturating_add(1);
                for message in &mut self.messages {
                    message.settled_at.get_or_insert(self.reread);
                }
                for tool in &mut self.tools {
                    tool.settled_at.get_or_insert(self.reread);
                }
            }
            SessionLiveKind::LiveText {
                turn,
                message,
                offset,
                text,
            } => {
                self.chunk(turn, message, offset, &text, false);
            }
            SessionLiveKind::LiveThinking {
                turn,
                message,
                offset,
                text,
            } => {
                self.chunk(turn, message, offset, &text, true);
            }
            SessionLiveKind::LiveTool {
                turn,
                call,
                tool,
                state,
            } if self.open && self.turn == Some(turn) => {
                if matches!(&state, SessionLiveToolState::Other) {
                    return;
                }
                let index = match self.tools.iter().position(|item| item.call == call) {
                    Some(index) => {
                        self.tools[index].tool = tool;
                        index
                    }
                    None => {
                        if self.tools.len() >= MAX_TOOLS {
                            self.reset();
                            return;
                        }
                        // A tool runs once the message that called it is complete, so its words
                        // go before the call; the transcript read after the tool ends holds both.
                        if let Some(message) = self.messages.last_mut()
                            && message.settled_at.is_none()
                        {
                            message.settled_at = Some(self.reread.saturating_add(1));
                        }
                        self.tools.push(LiveTool {
                            call,
                            tool,
                            args: None,
                            args_truncated: false,
                            partial: None,
                            state: LiveToolState::Running,
                            elapsed_ms: None,
                            settled_at: None,
                            after: self.messages.last().map_or(0, |message| message.ordinal),
                        });
                        self.tools.len() - 1
                    }
                };
                let item = &mut self.tools[index];
                match state {
                    SessionLiveToolState::Started {
                        args,
                        args_truncated,
                    } => {
                        item.args = args;
                        item.args_truncated = args_truncated.unwrap_or(false);
                        item.partial = None;
                        item.state = LiveToolState::Running;
                        item.elapsed_ms = None;
                        item.settled_at = None;
                    }
                    SessionLiveToolState::Updated { partial } => item.partial = partial,
                    SessionLiveToolState::Ended { failed, elapsed_ms } => {
                        self.reread = self.reread.saturating_add(1);
                        item.state = if failed {
                            LiveToolState::Failed
                        } else {
                            LiveToolState::Done
                        };
                        item.elapsed_ms = elapsed_ms;
                        item.settled_at = Some(self.reread);
                    }
                    _ => {}
                }
            }
            SessionLiveKind::LiveQueue { steer, follow_up } => {
                self.steer = steer;
                self.follow_up = follow_up;
            }
            SessionLiveKind::LiveInput { input, state } => {
                self.inputs.retain(|(known, _)| *known != input);
                if self.inputs.len() >= MAX_INPUTS {
                    self.inputs.remove(0);
                }
                self.inputs.push((input, state));
            }
            SessionLiveKind::LivePart { part } => {
                let count = self.parts.entry(part).or_default();
                *count = count.saturating_add(1);
            }
            // A mark, custom message, compaction or detached cell update was recorded: the
            // transcript is behind the host.
            SessionLiveKind::LiveEntry {} => self.reread = self.reread.saturating_add(1),
            _ => {}
        }
    }

    fn chunk(&mut self, turn: u64, ordinal: u64, offset: u64, chunk: &str, thinking: bool) {
        if !self.open || self.turn != Some(turn) {
            return;
        }
        let index = match self
            .messages
            .iter()
            .position(|message| message.ordinal == ordinal)
        {
            Some(index) => index,
            None => {
                if let Some(previous) = self.messages.last_mut()
                    && previous.settled_at.is_none()
                {
                    self.reread = self.reread.saturating_add(1);
                    previous.settled_at = Some(self.reread);
                }
                self.messages.push(LiveMessage {
                    ordinal,
                    ..LiveMessage::default()
                });
                self.messages.len() - 1
            }
        };
        let message = &mut self.messages[index];
        let text = if thinking {
            &mut message.thinking
        } else {
            &mut message.text
        };
        let Ok(offset) = usize::try_from(offset) else {
            self.reset();
            return;
        };
        let end = offset.saturating_add(chunk.len());
        if offset > text.len()
            || end > MAX_MESSAGE_BYTES
            || !text.is_char_boundary(offset)
            || (end < text.len() && !text.is_char_boundary(end))
        {
            self.reset();
            return;
        }
        let replace_end = end.min(text.len());
        if text.get(offset..replace_end) != Some(chunk) {
            text.replace_range(offset..replace_end, chunk);
        }
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
