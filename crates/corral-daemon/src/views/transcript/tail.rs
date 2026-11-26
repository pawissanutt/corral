//! Forward journal readback with a bounded window of projected messages.
use super::*;
use std::{collections::VecDeque, io::BufRead};

pub(crate) struct Row {
    pub(crate) before: Option<String>,
    /// Only the record's last items, as a history page cut it: `before` resumes inside it.
    pub(crate) cut: bool,
    calls: Vec<Call>,
    raw_bytes: usize,
    /// The size up to which an item of this message is carried whole.
    bound: usize,
}

/// A record read forward is carried whole, where a history page would cut it: a reader
/// holds what came before it and could not reach the items a cut left out. Its message is
/// at most this, so that one operation fits a page of the session topic.
const PUT_BYTES: usize = 48 * 1024;
/// What the items of a record are sized for; the rest is room for previews that could not
/// be made as small as asked.
const PUT_TARGET: usize = 40 * 1024;
/// A continuation item: a signed token naming the item and its pair.
const CONTINUATION_BYTES: usize = 640;

/// The size an item of this record is carried whole at, so that all of them fit one
/// operation with every call's result folded in: a history page's size for a record of a
/// few items, less for one of many.
fn put_bound((message, items): &Selection) -> Result<usize> {
    let room = PUT_TARGET.saturating_sub(json_len(message)?);
    Ok((room / items.len().max(1))
        .saturating_sub(CONTINUATION_BYTES)
        .min(ITEM_BYTES))
}

struct Call {
    id: String,
    index: usize,
    source: Source,
    encoded: Vec<u8>,
}

pub(super) fn render(
    id: &str,
    (mut message, items): Selection,
    before: Option<String>,
    keep_calls: bool,
    bound: usize,
) -> Result<(SessionMessage, Row)> {
    let mut calls = Vec::new();
    let mut raw_bytes = 0usize;
    for item in items {
        if keep_calls
            && let Some(SessionItem::ToolCall {
                call_id,
                result: None,
                ..
            }) = &item.value
            && !call_id.is_empty()
            && let Some(value) = &item.value
        {
            let cost = json_len(value)? + call_id.len();
            raw_bytes = raw_bytes.saturating_add(cost);
            if raw_bytes <= WINDOW {
                calls.push(Call {
                    id: call_id.clone(),
                    index: message.items.len(),
                    source: item.source,
                    encoded: serde_json::to_vec(value).context("retaining transcript call")?,
                });
            } else {
                calls.clear();
            }
        }
        append_preview(id, &mut message.items, item, bound)?;
    }
    Ok((
        message,
        Row {
            before,
            cut: false,
            calls,
            raw_bytes,
            bound,
        },
    ))
}

impl Row {
    fn cost(&self, message: &SessionMessage) -> Result<usize> {
        Ok(json_len(message)?.saturating_add(self.raw_bytes))
    }
}

/// A message, the page before it, and whether it is only the last items of its record.
pub(crate) type Message = (SessionMessage, Option<String>, bool);
pub(crate) type Change = (Option<Message>, bool);

pub(crate) struct Tail {
    journal: (u64, u64),
    len: u64,
    offset: u64,
    leaf: Option<String>,
    rows: VecDeque<(SessionMessage, Row, usize)>,
    bytes: usize,
    stamp: String,
    pending: bool,
}

impl Tail {
    pub(crate) fn snapshot(
        id: &str,
        path: &Path,
        scope: Option<&Path>,
    ) -> Result<(Self, Option<String>, Vec<Message>)> {
        let mut file = Transcript::open(path, scope)?;
        let (page, rows, offset, leaf, cut) = read_page(id, &mut file, None, true)?;
        let mut tail = Self {
            journal: (file.metadata.dev(), file.metadata.ino()),
            len: file.metadata.len(),
            offset,
            leaf,
            rows: VecDeque::new(),
            bytes: 0,
            stamp: identity(id, &file.metadata)?,
            pending: false,
        };
        drop(file);
        if let Some((start, follows)) = cut {
            // The page begins inside a record. Read forward from where that record starts
            // instead, so the tail begins with a whole message a reader can join at.
            let newest = (tail.offset, tail.leaf.take());
            (tail.offset, tail.leaf, tail.pending) = (start, follows, true);
            let mut messages: Vec<Message> = Vec::new();
            let changes = tail.advance(id, path, scope)?.into_iter().flatten();
            for message in changes.filter_map(|(row, _)| row) {
                // A call's message comes again with each result folded in.
                match messages.iter_mut().find(|held| held.0.id == message.0.id) {
                    Some(held) => *held = message,
                    None => messages.push(message),
                }
            }
            if let Some((_, before, _)) = messages.first() {
                return Ok((tail, before.clone(), messages));
            }
            // It cannot be carried whole, or the journal forks after it: its last items.
            (tail.offset, tail.leaf) = newest;
            (tail.pending, tail.bytes) = (false, 0);
            tail.rows.clear();
        }
        let mut messages = Vec::new();
        for (message, row) in page.messages.into_iter().zip(rows) {
            messages.push((message.clone(), row.before.clone(), row.cut));
            tail.remember(message, row)?;
        }
        Ok((tail, page.older, messages))
    }

    fn trim(&mut self) {
        while self.rows.len() > RECORDS || self.bytes > WINDOW {
            if let Some((_, _, cost)) = self.rows.pop_front() {
                self.bytes -= cost;
            }
        }
    }

    fn remember(&mut self, message: SessionMessage, row: Row) -> Result<()> {
        let cost = row.cost(&message)?;
        if cost > WINDOW {
            self.rows.clear();
            self.bytes = 0;
            return Ok(());
        }
        self.rows.push_back((message, row, cost));
        self.bytes += cost;
        self.trim();
        Ok(())
    }

    /// None indicates that the reader must re-snapshot. Incomplete records are retried on growth.
    pub(crate) fn advance(
        &mut self,
        id: &str,
        path: &Path,
        scope: Option<&Path>,
    ) -> Result<Option<Vec<Change>>> {
        let meta = std::fs::symlink_metadata(path).context("stating transcript tail")?;
        let continues =
            |meta: &Metadata| self.journal == (meta.dev(), meta.ino()) && meta.len() >= self.len;
        if !continues(&meta) {
            return Ok(None);
        }
        if meta.len() == self.len && !self.pending {
            return Ok(Some(Vec::new()));
        }
        let transcript = Transcript::open(path, scope)?;
        if !continues(&transcript.metadata) {
            return Ok(None);
        }
        self.len = transcript.metadata.len();
        let mut file = transcript.file;
        file.seek(SeekFrom::Start(self.offset))
            .context("seeking appended transcript")?;
        let mut reader = BufReader::new(file.take(self.len - self.offset));
        let mut changes = Vec::new();
        self.pending = false;
        let visit_start = self.offset;
        for _ in 0..RECORDS {
            if self.offset == self.len || self.offset - visit_start >= 4 * WINDOW as u64 {
                break;
            }
            let mut bytes = Vec::new();
            reader
                .read_until(b'\n', &mut bytes)
                .context("reading appended transcript")?;
            if bytes.last() != Some(&b'\n') {
                return Ok(Some(changes));
            }
            if !self.project(id, &bytes, &mut changes)? {
                return Ok(None);
            }
        }
        self.pending = self.offset < self.len;
        Ok(Some(changes))
    }

    pub(crate) fn has_pending_records(&self) -> bool {
        self.pending
    }

    fn project(&mut self, id: &str, bytes: &[u8], changes: &mut Vec<Change>) -> Result<bool> {
        let value: Value = serde_json::from_slice(bytes).context("decoding appended transcript")?;
        let start = self.offset;
        let end = start + bytes.len() as u64;
        let mut parent = Branch::Latest;
        if let Some(entry) = value.get("id").and_then(Value::as_str)
            && value.get("parentId").is_some()
        {
            if value["parentId"].as_str() != self.leaf.as_deref() {
                return Ok(false);
            }
            parent = self
                .leaf
                .as_deref()
                .map_or(Branch::Root, |id| Branch::Parent(branch_key(id)));
            self.leaf = Some(entry.to_owned());
        }
        self.offset = end;
        let mut skip = Skipped::default();
        let assistant = value.pointer("/message/role").and_then(Value::as_str) == Some("assistant");
        // Every item of the record: a cut would leave out items no reader could reach.
        let selected = select(id, value, start, end, &mut skip, &mut 2, usize::MAX)?;
        if let (Some(mut selection), _) = selected {
            let before = parent.cursor(&self.stamp, start, Skipped::default());
            pair_tools(std::slice::from_mut(&mut selection));
            for item in &mut selection.1 {
                if !self.fold(id, item, changes)? {
                    return Ok(false);
                }
            }
            let bound = put_bound(&selection)?;
            let (message, row) = render(id, selection, before, true, bound)?;
            if json_len(&message)? > PUT_BYTES {
                // More items than one operation holds at any size: a history page's cut.
                return Ok(false);
            }
            if !message.items.is_empty() || !message.attachments.is_empty() {
                changes.push((
                    Some((message.clone(), row.before.clone(), false)),
                    assistant,
                ));
                self.remember(message, row)?;
                return Ok(true);
            }
        }
        if assistant {
            changes.push((None, true));
        }
        Ok(true)
    }

    /// Whether the call's message still fits one operation with the result folded in.
    fn fold(
        &mut self,
        id: &str,
        item: &mut SelectedItem,
        changes: &mut Vec<Change>,
    ) -> Result<bool> {
        let Some(SessionItem::ToolCall {
            call_id,
            result: Some(_),
            ..
        }) = &item.value
        else {
            return Ok(true);
        };
        for (message, row, cost) in self.rows.iter_mut().rev() {
            let Some(index) = row.calls.iter().rposition(|call| &call.id == call_id) else {
                continue;
            };
            let Call {
                id: call_id,
                index,
                source,
                encoded,
            } = row.calls.remove(index);
            row.raw_bytes -= encoded.len() + call_id.len();
            let mut call = serde_json::from_slice(&encoded).context("decoding retained call")?;
            if let Some(result) = item.value.take() {
                entries::merge(&mut call, result);
            }
            let mut preview = Vec::new();
            append_preview(
                id,
                &mut preview,
                SelectedItem {
                    value: Some(call),
                    source,
                    pair: Some(item.source),
                    size: None,
                },
                row.bound,
            )?;
            let end = index
                + 1
                + usize::from(matches!(
                    message.items.get(index + 1),
                    Some(SessionItem::Continuation { .. })
                ));
            let shift = preview.len() as isize - (end - index) as isize;
            message.items.splice(index..end, preview);
            for other in &mut row.calls {
                if other.index > index {
                    other.index = other.index.saturating_add_signed(shift);
                }
            }
            self.bytes -= *cost;
            *cost = row.cost(message)?;
            self.bytes += *cost;
            if json_len(message)? > PUT_BYTES {
                return Ok(false);
            }
            changes.push((Some((message.clone(), row.before.clone(), row.cut)), false));
            break;
        }
        self.trim();
        Ok(true)
    }
}
