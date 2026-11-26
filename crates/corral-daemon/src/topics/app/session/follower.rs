use std::{collections::BTreeMap, path::Path};

use anyhow::Result;
use corral_protocol::{
    detail::SessionMessage,
    subscription::{
        SessionLiveKind, SessionLiveTurnState, TranscriptOp as Op, TranscriptPart as Part,
    },
};
use serde::Deserialize;

use super::reader::LivePage;
use crate::views::transcript::tail::Tail;

#[derive(Deserialize)]
pub(super) struct HostOpen {
    turn: u64,
    message: u64,
    ended: bool,
    text_offset: u64,
    text: String,
    thinking_offset: u64,
    thinking: String,
}

#[derive(Default)]
pub(super) struct Follower {
    tail: Option<Tail>,
    generation: Option<u32>,
    skip_through: u64,
    turn: Option<u64>,
    pending_end: Option<u64>,
    recorded: u64,
    open: BTreeMap<u64, Open>,
    from_start: bool,
}

struct Open {
    id: String,
    offsets: [u64; 2],
}

/// The words of one part of an open message that are carried. Past it the part stops
/// growing until the message's record arrives, for every subscriber alike, so the tail a
/// late subscriber is given and the appends that follow it count from the same start.
pub(super) const OPEN_PART_BYTES: u64 = 128 * 1024;

impl Follower {
    /// The next page was asked for from the start of the host's ring: what it replays is
    /// older than the journal this follower has read, so it joins as on a gap.
    pub(super) fn reads_from_start(&mut self) {
        self.from_start = true;
    }

    pub(super) fn visit(
        &mut self,
        id: &str,
        path: &Path,
        scope: Option<&Path>,
        page: Option<&LivePage>,
        seed: Option<HostOpen>,
    ) -> Vec<Op> {
        match self.read(id, path, scope, page, seed) {
            Ok(ops) => ops,
            Err(_) => {
                self.tail = None;
                Vec::new()
            }
        }
    }

    fn read(
        &mut self,
        id: &str,
        path: &Path,
        scope: Option<&Path>,
        page: Option<&LivePage>,
        seed: Option<HostOpen>,
    ) -> Result<Vec<Op>> {
        let generation = page.and_then(|p| p.generation);
        let from_start = std::mem::take(&mut self.from_start);
        let gap = self.generation != generation
            || page.is_some_and(|p| {
                p.generation.is_some() && (p.gap.is_some() || (from_start && p.host_tail > 0))
            });
        // Joining the host's stream, not only re-reading the journal on another branch.
        let join = gap || self.tail.is_none();
        if self.generation != generation {
            self.skip_through = 0;
        }
        self.generation = generation;
        let changes = if gap {
            None
        } else if let Some(tail) = &mut self.tail {
            tail.advance(id, path, scope)?
        } else {
            None
        };
        let reset = changes.is_none();
        let mut ops = Vec::new();
        let mut starts = page
            .into_iter()
            .flat_map(|page| &page.events)
            .filter_map(|(event, _)| match event.kind {
                SessionLiveKind::LiveTurn {
                    turn,
                    state: SessionLiveTurnState::Started,
                } if event.seq > self.skip_through && self.turn != Some(turn) => Some(Start {
                    turn,
                    at: event.at.parse().ok(),
                    records: 0,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        if let Some(changes) = changes {
            for (row, assistant) in changes {
                let replaces = if assistant {
                    let at = row
                        .as_ref()
                        .and_then(|(message, ..)| message.at.as_deref())
                        .and_then(|at| at.parse().ok());
                    if let Some(start) = locate_start(&mut starts, at, self.turn.is_some()) {
                        start.records += 1;
                        None
                    } else if self.turn.is_some() {
                        self.recorded += 1;
                        self.open.remove(&self.recorded).map(|open| open.id)
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some((message, before, cut)) = row {
                    ops.push(Op::Put {
                        message,
                        before,
                        replaces,
                        open: false,
                        cut,
                    });
                } else if let Some(id) = replaces {
                    ops.push(Op::Drop { id });
                }
            }
        } else {
            let (tail, older, messages) = Tail::snapshot(id, path, scope)?;
            self.tail = Some(tail);
            self.turn = None;
            self.pending_end = None;
            self.recorded = 0;
            self.open.clear();
            if join {
                // What the ring still holds cannot be paired by order with records this
                // snapshot already has: a turn's start may be pages away from its words,
                // and the snapshot is one page of its records. The host's `open` says
                // where the turn is, and only what it emits after this page is followed.
                self.skip_through = page.map_or(0, |page| page.host_tail);
                starts.clear();
            } else {
                for (message, ..) in &messages {
                    if message.author == "assistant"
                        && let Some(at) = message.at.as_deref().and_then(|at| at.parse().ok())
                        && let Some(start) = locate_start(&mut starts, Some(at), true)
                    {
                        start.records += 1;
                    }
                }
            }
            ops.push(Op::Reset { older });
            ops.extend(messages.into_iter().map(|(message, before, cut)| Op::Put {
                message,
                before,
                replaces: None,
                open: false,
                cut,
            }));
        }
        let (Some(generation), Some(page)) = (generation, page) else {
            return Ok(ops);
        };
        if reset && let Some(seed) = seed {
            self.turn = Some(seed.turn);
            self.recorded = if seed.ended {
                seed.message
            } else {
                seed.message.saturating_sub(1)
            };
            if !seed.ended && seed.message != 0 {
                let at = page
                    .events
                    .iter()
                    .rev()
                    .find_map(|(event, _)| match event.kind {
                        SessionLiveKind::LiveText { turn, message, .. }
                        | SessionLiveKind::LiveThinking { turn, message, .. }
                            if turn == seed.turn && message == seed.message =>
                        {
                            Some(event.at.clone())
                        }
                        _ => None,
                    });
                let open = start(id, generation, seed.turn, seed.message, at, &mut ops);
                for (part, text) in [(Part::Thinking, seed.thinking), (Part::Text, seed.text)] {
                    chunks(&mut ops, &open.id, part, 0, &text);
                }
                self.open.insert(
                    seed.message,
                    Open {
                        offsets: [seed.text_offset, seed.thinking_offset],
                        ..open
                    },
                );
            }
            // A seed describes the host's current turn, not the oldest surviving event.
            return Ok(ops);
        }
        let journal_pending = self.tail.as_ref().is_some_and(Tail::has_pending_records);
        if !journal_pending
            && let Some(turn) = self.pending_end.take()
            && self.turn == Some(turn)
        {
            self.drop_open(&mut ops);
            self.turn = None;
        }
        for (event, _) in &page.events {
            if event.seq <= self.skip_through {
                continue;
            }
            match &event.kind {
                SessionLiveKind::LiveTurn {
                    turn,
                    state: SessionLiveTurnState::Started,
                } if self.turn != Some(*turn) => {
                    self.drop_open(&mut ops);
                    self.turn = Some(*turn);
                    self.recorded = starts
                        .iter()
                        .find(|start| start.turn == *turn)
                        .map_or(0, |start| start.records);
                }
                SessionLiveKind::LiveTurn {
                    turn,
                    state: SessionLiveTurnState::Ended { .. },
                } if self.turn == Some(*turn) => {
                    if journal_pending {
                        self.pending_end = Some(*turn);
                    } else {
                        self.drop_open(&mut ops);
                        self.turn = None;
                    }
                }
                SessionLiveKind::LiveText {
                    turn,
                    message,
                    offset,
                    text,
                }
                | SessionLiveKind::LiveThinking {
                    turn,
                    message,
                    offset,
                    text,
                } if self.turn == Some(*turn) && *message > self.recorded => {
                    let part = if matches!(event.kind, SessionLiveKind::LiveText { .. }) {
                        Part::Text
                    } else {
                        Part::Thinking
                    };
                    // Earlier ordinals remain until journal pairing, but bound unread ephemera.
                    if !self.open.contains_key(message) && self.open.len() >= 32 {
                        continue;
                    }
                    let open = self.open.entry(*message).or_insert_with(|| {
                        start(
                            id,
                            generation,
                            *turn,
                            *message,
                            Some(event.at.clone()),
                            &mut ops,
                        )
                    });
                    let base = open.offsets[usize::from(part == Part::Thinking)];
                    let overlap =
                        usize::try_from(base.saturating_sub(*offset)).unwrap_or(usize::MAX);
                    let at = offset.saturating_sub(base);
                    let room =
                        usize::try_from(OPEN_PART_BYTES.saturating_sub(at)).unwrap_or(usize::MAX);
                    if let Some(text) = text.get(overlap..) {
                        let text = &text[..text.floor_char_boundary(room)];
                        chunks(&mut ops, &open.id, part, at, text);
                    }
                }
                _ => {}
            }
        }
        Ok(ops)
    }

    fn drop_open(&mut self, ops: &mut Vec<Op>) {
        ops.extend(
            std::mem::take(&mut self.open)
                .into_values()
                .map(|open| Op::Drop { id: open.id }),
        );
    }
}

struct Start {
    turn: u64,
    at: Option<jiff::Timestamp>,
    records: u64,
}

// Most visits have no new turn. If the host publishes two turns in one page, the
// journal timestamp distinguishes records across the start boundary. When there
// is no timestamp (foreign journals), fall back to the turn already being watched.
fn locate_start(
    starts: &mut [Start],
    at: Option<jiff::Timestamp>,
    has_current: bool,
) -> Option<&mut Start> {
    let position = match at {
        Some(at) => starts
            .iter()
            .rposition(|start| start.at.is_some_and(|start| start <= at)),
        None if !has_current && !starts.is_empty() => Some(0),
        None => None,
    }?;
    starts.get_mut(position)
}

fn start(
    id: &str,
    generation: u32,
    turn: u64,
    ordinal: u64,
    at: Option<String>,
    ops: &mut Vec<Op>,
) -> Open {
    let id = format!("{id}/open/{generation}.{turn}.{ordinal}");
    ops.push(Op::Put {
        message: SessionMessage {
            id: id.clone(),
            author: "assistant".into(),
            at,
            items: Vec::new(),
            model: None,
            stop_reason: None,
            usage: None,
            input: None,
            attachments: Vec::new(),
        },
        before: None,
        replaces: None,
        open: true,
        cut: false,
    });
    Open {
        id,
        offsets: [0; 2],
    }
}

pub(super) fn chunks(ops: &mut Vec<Op>, id: &str, part: Part, mut offset: u64, mut text: &str) {
    while !text.is_empty() {
        let (chunk, rest) = text.split_at(text.floor_char_boundary(4096));
        ops.push(Op::Append {
            id: id.into(),
            part,
            offset,
            text: chunk.into(),
        });
        offset += chunk.len() as u64;
        text = rest;
    }
}
