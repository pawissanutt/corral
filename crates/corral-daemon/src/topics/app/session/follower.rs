use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
};

use anyhow::Result;
use corral_protocol::{
    detail::SessionMessage,
    subscription::{
        SessionLiveKind, SessionLiveTurnState, TranscriptOp as Op, TranscriptPart as Part,
    },
};
use serde::Deserialize;

use super::reader::{JournalAnchor, LivePage};
use crate::views::transcript::tail::{Change, Message, Record, Records, STEPS, Tail};

mod journal_order;
use journal_order::{Exact, Order};

#[derive(Deserialize)]
pub(super) struct HostOpen {
    turn: u64,
    message: u64,
    ended: bool,
    /// When `message` started, on the clock that stamps journal entries.
    #[serde(default)]
    message_at: Option<String>,
    journal: Option<JournalAnchor>,
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
    mark: Option<Mark>,
    exact: Option<Exact>,
    order: Order,
    /// When the turn began, or the message it was joined at: an assistant record written
    /// before it is not one of the messages being followed.
    began: Option<jiff::Timestamp>,
    /// The latest assistant records this follower read, in the journal's order: when each
    /// was written, and its message.
    seen: VecDeque<(jiff::Timestamp, String)>,
    /// When the first message of the tail last read again was written, unless that tail
    /// was the whole journal: records before it were not read.
    held: Option<jiff::Timestamp>,
}

/// Where the journal's clock parts a turn's records: the assistant records stamped at
/// `since` or later are the turn's messages after `base`. The host stamps its events and
/// the journal's entries with one clock.
#[derive(Clone)]
struct Mark {
    base: u64,
    since: jiff::Timestamp,
    /// The record the `base`th message was paired with, written at `since`. The records
    /// after it in the journal are the later ones, whatever their stamps.
    after: Option<String>,
}

impl Mark {
    /// The turn's start, or the start of the message after its first `base`.
    fn from(base: u64, since: jiff::Timestamp) -> Self {
        Self {
            base,
            since,
            after: None,
        }
    }
}

enum Journal {
    Changes(Vec<Change>),
    Snapshot(Option<String>, Vec<Message>, Records),
}

const SEEN: usize = 64;

struct Open {
    id: String,
    at: Option<String>,
    /// Where the words a subscriber was given start in the host's text and thinking.
    offsets: [u64; 2],
    /// Where the host's words so far end.
    ends: [u64; 2],
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
        let generation = page.and_then(|p| p.generation);
        let from_start = std::mem::take(&mut self.from_start);
        // Joining the host's stream. Reading the journal again alone (another branch, or a
        // read that failed) leaves the turn the host is in as it was followed.
        let join = self.generation != generation
            || page.is_some_and(|p| {
                p.generation.is_some() && (p.gap.is_some() || (from_start && p.host_tail > 0))
            });
        if self.generation != generation {
            self.skip_through = 0;
        }
        self.generation = generation;
        // The host's page is followed whether or not the journal can be read: the reader
        // does not ask for it again.
        let journal = self.journal(id, path, scope, join);
        let read_failed = journal.is_err();
        if read_failed {
            self.tail = None;
            self.held = None;
        }
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
                    exact: None,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        if join {
            // What the ring still holds cannot be paired by order with records the journal
            // already has: a turn's start may be pages away from its words. The host's
            // `open` says where the turn is, and only what it emits after this page is
            // followed.
            if read_failed {
                // No reset takes the messages that were being written from a reader.
                self.drop_open(&mut ops);
            }
            self.turn = None;
            self.pending_end = None;
            self.recorded = 0;
            self.mark = None;
            self.exact = None;
            self.began = None;
            self.open.clear();
            self.skip_through = page.map_or(0, |page| page.host_tail);
            starts.clear();
        }
        // Index the actual journal order before consuming the page. An append may already
        // be in the journal even when its notification is on a later host page.
        let first_rank = match &journal {
            Ok(Journal::Changes(changes)) => self.order.append(changes.iter().map(|c| &c.record)),
            Ok(Journal::Snapshot(_, _, records)) => {
                self.order = Order::default();
                if let Some(exact) = &mut self.exact {
                    exact.invalidate();
                }
                self.order.snapshot(records.root, &records.entries);
                0
            }
            Err(_) => 0,
        };
        for (seq, turn, anchor) in page.into_iter().flat_map(|page| &page.journal) {
            if *seq <= self.skip_through {
                continue;
            }
            if self.turn == Some(*turn) {
                Exact::update(&mut self.exact, anchor);
            } else if let Some(start) = starts.iter_mut().find(|start| start.turn == *turn) {
                Exact::update(&mut start.exact, anchor);
            }
        }
        if !read_failed {
            for exact in self
                .exact
                .iter_mut()
                .chain(starts.iter_mut().filter_map(|s| s.exact.as_mut()))
            {
                exact.resolve(&self.order);
            }
        }
        if !read_failed {
            let mut through = self.order.rank;
            for start in starts.iter_mut().rev() {
                if let Some(exact) = &start.exact {
                    start.records = exact.ordinal(through).unwrap_or(0);
                    if let Some(before) = exact.before_turn() {
                        through = through.min(before);
                    }
                }
            }
            if let Some(exact) = &self.exact
                && let Some(recorded) = exact.ordinal(through)
            {
                self.recorded = recorded;
            }
        }
        match journal {
            Ok(Journal::Changes(changes)) => {
                if self.exact_missing() {
                    self.drop_open(&mut ops);
                }
                self.pair(
                    changes,
                    first_rank,
                    &mut starts,
                    page.map_or(&[], |page| &page.journal),
                    &mut ops,
                );
            }
            Ok(Journal::Snapshot(older, messages, _)) => {
                self.held = older.as_ref().and_then(|_| {
                    let mut stamps = messages.iter().map(|(message, ..)| message.at.as_deref());
                    stamps.find_map(|at| at?.parse().ok())
                });
                self.seen.clear();
                for (message, ..) in &messages {
                    if message.author != "assistant" {
                        continue;
                    }
                    let Some(at) = message.at.as_deref().and_then(|at| at.parse().ok()) else {
                        continue;
                    };
                    self.see(at, &message.id);
                    if !join && let Some(start) = locate_start(&mut starts, Some(at), true) {
                        start.records += 1;
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
                if !join {
                    // Only the journal was read again: the turn the host is in goes on. A
                    // reset takes the messages being written from a reader, so those the
                    // journal does not hold yet are put again, from the words the host
                    // writes next.
                    if self.exact_missing() {
                        self.open.clear();
                    } else if !self.legacy_covered() {
                        self.lose_turn();
                    }
                    self.count();
                    let recorded = self.recorded;
                    self.open.retain(|ordinal, _| *ordinal > recorded);
                    for open in self.open.values_mut() {
                        open.offsets = open.ends;
                        ops.push(put_open(&open.id, open.at.clone()));
                    }
                }
            }
            Err(_) => {}
        }
        let (Some(generation), Some(page)) = (generation, page) else {
            self.order.trim();
            return ops;
        };
        if join && let Some(seed) = seed {
            self.turn = Some(seed.turn);
            if let Some(anchor) = seed
                .journal
                .filter(|anchor| anchor.valid() && anchor.message <= seed.message)
            {
                let mut exact = Exact::new(anchor);
                if !read_failed {
                    exact.resolve(&self.order);
                }
                self.recorded = exact.ordinal(self.order.rank).unwrap_or(0);
                self.exact = Some(exact);
            } else {
                // The journal was read after the host answered. The latest message's record may
                // have reached it in between, and a later message's too, or not yet though the
                // message has ended: the records of that message and of those after it are the
                // assistant entries stamped at or after the message started (the turn, before
                // its first message).
                let since: Option<jiff::Timestamp> =
                    seed.message_at.as_deref().and_then(|at| at.parse().ok());
                if since.is_some_and(|since| !reaches(self.held, since)) {
                    self.lose_turn();
                    return ops;
                }
                let before = seed.message.saturating_sub(1);
                self.mark = since.map(|since| Mark::from(before, since));
                self.began = since;
                self.recorded = match since {
                    Some(_) => before,
                    None if seed.message == 0 => 0,
                    None => before + u64::from(seed.ended),
                };
                self.count();
            }
            if self.recorded < seed.message && !seed.ended && !self.exact_missing() {
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
                let offsets = [seed.text_offset, seed.thinking_offset];
                let ends = [
                    seed.text_offset + seed.text.len() as u64,
                    seed.thinking_offset + seed.thinking.len() as u64,
                ];
                for (part, text) in [(Part::Thinking, seed.thinking), (Part::Text, seed.text)] {
                    chunks(&mut ops, &open.id, part, 0, &text);
                }
                self.open.insert(
                    seed.message,
                    Open {
                        offsets,
                        ends,
                        ..open
                    },
                );
            }
            // A seed describes the host's current turn, not the oldest surviving event.
            self.order.trim();
            return ops;
        }
        self.order.trim();
        let journal_pending = self.tail.as_ref().is_some_and(Tail::has_pending_records)
            || (read_failed && self.exact.is_some());
        if !journal_pending
            && let Some(turn) = self.pending_end.take()
            && self.turn == Some(turn)
        {
            self.drop_open(&mut ops);
            self.turn = None;
            self.exact = None;
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
                    let start = starts.iter_mut().find(|start| start.turn == *turn);
                    self.recorded = start.as_ref().map_or(0, |start| start.records);
                    self.exact = start.and_then(|start| start.exact.take());
                    self.began = None;
                    self.mark = None;
                    if self.exact.is_some() {
                        continue;
                    }
                    // Its records may have been read on an earlier visit than this page.
                    self.began = event.at.parse().ok();
                    self.mark = self.began.map(|since| Mark::from(0, since));
                    self.count();
                    // Or written before the tail that was read again begins.
                    if self.began.is_some_and(|began| !reaches(self.held, began)) {
                        self.lose_turn();
                    }
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
                        self.exact = None;
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
                } if self.turn == Some(*turn)
                    && *message > self.recorded
                    && !self.exact_missing() =>
                {
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
                    let which = usize::from(part == Part::Thinking);
                    // If we could not place the turn earlier, this may be the first
                    // chunk we can carry even though the host already sent its prefix.
                    if open.ends[which] == 0 {
                        open.offsets[which] = *offset;
                    }
                    let base = open.offsets[which];
                    open.ends[which] = open.ends[which].max(*offset + text.len() as u64);
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
        ops
    }

    /// What the journal holds now: what was appended to the tail this follower reads, or
    /// the tail read again.
    fn journal(
        &mut self,
        id: &str,
        path: &Path,
        scope: Option<&Path>,
        join: bool,
    ) -> Result<Journal> {
        if !join && let Some(tail) = &mut self.tail {
            // The tail reads a bounded step at a time. The host's page is followed only
            // once the journal is read to its end, so a record is never read after an
            // event the host emitted later: an earlier turn's last record after the next
            // turn's start, or a prompt after the words that answer it.
            let mut read = Vec::new();
            for _ in 0..STEPS {
                let Some(changes) = tail.advance(id, path, scope)? else {
                    break;
                };
                read.extend(changes);
                if !tail.has_pending_records() {
                    return Ok(Journal::Changes(read));
                }
            }
            // Another branch, or more appended than these steps read: the tail again.
        }
        let (tail, older, messages, records) = Tail::snapshot(id, path, scope)?;
        self.tail = Some(tail);
        Ok(Journal::Snapshot(older, messages, records))
    }

    /// Pairs records appended to the journal with the messages being written.
    fn pair(
        &mut self,
        changes: Vec<Change>,
        mut rank: u64,
        starts: &mut [Start],
        anchors: &[(u64, u64, JournalAnchor)],
        ops: &mut Vec<Op>,
    ) {
        let skip_through = self.skip_through;
        let anchors = anchors
            .iter()
            .filter(|(seq, _, anchor)| *seq > skip_through && anchor.message != 0);
        for Change { row, record } in changes {
            rank += u64::from(record.assistant);
            let replaces = if record.assistant {
                // When the record was written, and its message.
                let written: Option<(jiff::Timestamp, String)> =
                    row.as_ref().and_then(|(message, ..)| {
                        Some((message.at.as_deref()?.parse().ok()?, message.id.clone()))
                    });
                if let Some((at, id)) = &written {
                    self.see(*at, id);
                }
                let at = written.as_ref().map(|(at, _)| *at);
                // A record written before the turn began is an earlier turn's.
                let earlier = self.began.zip(at).is_some_and(|(began, at)| at < began);
                let named = record.entry_id.as_deref().and_then(|id| {
                    anchors
                        .clone()
                        .find(|(_, _, anchor)| anchor.entry_id.as_deref() == Some(id))
                });
                if let Some((_, turn, anchor)) = named {
                    // A name on this page binds the record directly, even if records
                    // for multiple turns have the same stamp.
                    if self.turn == Some(*turn) {
                        self.open.remove(&anchor.message).map(|open| open.id)
                    } else {
                        None
                    }
                } else if starts.iter().any(|start| {
                    start
                        .exact
                        .as_ref()
                        .is_some_and(|exact| exact.includes(rank))
                }) {
                    None
                } else if let Some(start) = locate_start(starts, at, self.turn.is_some()) {
                    start.records += 1;
                    None
                } else if let Some(exact) = &self.exact {
                    exact
                        .ordinal(rank)
                        .filter(|ordinal| {
                            *ordinal != 0
                                && !anchors.clone().any(|(_, turn, anchor)| {
                                    self.turn == Some(*turn) && anchor.message == *ordinal
                                })
                        })
                        .and_then(|ordinal| self.open.remove(&ordinal).map(|open| open.id))
                } else if self.turn.is_some() && !earlier {
                    self.recorded += 1;
                    if let Some((since, id)) = written {
                        self.mark = Some(Mark {
                            base: self.recorded,
                            since,
                            after: Some(id),
                        });
                    }
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
    }

    fn see(&mut self, at: jiff::Timestamp, id: &str) {
        if self.seen.len() == SEEN {
            self.seen.pop_front();
        }
        self.seen.push_back((at, id.to_owned()));
    }

    /// Counts the turn's records by the journal's clock. The journal and the host are
    /// read at different moments, so a record may have been read before the event that
    /// says which message it is of, or while the journal could not be followed.
    fn count(&mut self) {
        if self.exact.is_some() {
            return;
        }
        let Some(mark) = self.mark.as_ref().filter(|_| self.turn.is_some()) else {
            return;
        };
        let paired = mark.after.as_deref();
        let paired = paired.and_then(|id| self.seen.iter().position(|(_, seen)| seen == id));
        let records = match (paired, &mark.after) {
            (Some(at), _) => self.seen.len() - at - 1,
            // The record it was paired with is no longer among the latest: later stamps.
            (None, Some(_)) => self.seen.iter().filter(|(at, _)| *at > mark.since).count(),
            (None, None) => self.seen.iter().filter(|(at, _)| *at >= mark.since).count(),
        };
        if let Some((since, id)) = self.seen.back().filter(|_| records != 0) {
            self.recorded = self.recorded.max(mark.base + records as u64);
            self.mark = Some(Mark {
                base: self.recorded,
                since: *since,
                after: Some(id.clone()),
            });
        }
    }

    /// Whether the tail that was read again holds every record after the turn's mark. The
    /// record the mark was paired with says so by being among them. Its stamp alone does
    /// not: the tail may begin at a later record stamped the same instant.
    fn legacy_covered(&self) -> bool {
        if self.exact.is_some() {
            return true;
        }
        let Some(mark) = &self.mark else {
            return true;
        };
        match &mark.after {
            _ if self.held.is_none() => true,
            Some(id) => self.seen.iter().any(|(_, seen)| seen == id),
            None => reaches(self.held, mark.since),
        }
    }

    fn exact_missing(&self) -> bool {
        self.exact.as_ref().is_some_and(Exact::missing)
    }

    /// Stops following the turn's words: which of its messages the journal holds is not
    /// known. Its records still arrive, and the next turn is followed from its start.
    fn lose_turn(&mut self) {
        self.turn = None;
        self.pending_end = None;
        self.mark = None;
        self.exact = None;
        self.began = None;
        self.open.clear();
    }

    fn drop_open(&mut self, ops: &mut Vec<Op>) {
        ops.extend(
            std::mem::take(&mut self.open)
                .into_values()
                .map(|open| Op::Drop { id: open.id }),
        );
    }
}

/// Whether a tail that was read again holds every record written from `since` on. It is
/// one page of the journal's latest messages: records before its first are not counted.
fn reaches(held: Option<jiff::Timestamp>, since: jiff::Timestamp) -> bool {
    held.is_none_or(|first| first <= since)
}

struct Start {
    turn: u64,
    at: Option<jiff::Timestamp>,
    records: u64,
    exact: Option<Exact>,
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
            .rposition(|start| start.exact.is_none() && start.at.is_some_and(|start| start <= at)),
        None if !has_current => starts.iter().position(|start| start.exact.is_none()),
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
    ops.push(put_open(&id, at.clone()));
    Open {
        id,
        at,
        offsets: [0; 2],
        ends: [0; 2],
    }
}

fn put_open(id: &str, at: Option<String>) -> Op {
    Op::Put {
        message: SessionMessage {
            id: id.into(),
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
