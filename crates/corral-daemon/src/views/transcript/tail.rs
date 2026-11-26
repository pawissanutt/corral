//! Forward journal readback with a bounded window of projected messages.
use super::*;
use std::{collections::VecDeque, io::BufRead};

pub(crate) struct Row {
    pub(crate) before: Option<String>,
    calls: Vec<Call>,
    raw_bytes: usize,
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
        append_preview(id, &mut message.items, item)?;
    }
    Ok((
        message,
        Row {
            before,
            calls,
            raw_bytes,
        },
    ))
}

impl Row {
    fn cost(&self, message: &SessionMessage) -> Result<usize> {
        Ok(json_len(message)?.saturating_add(self.raw_bytes))
    }
}

pub(crate) type Message = (SessionMessage, Option<String>);
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
        let (page, rows, offset, leaf) = read_page(id, &mut file, None, true)?;
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
        let mut messages = Vec::new();
        for (message, row) in page.messages.into_iter().zip(rows) {
            messages.push((message.clone(), row.before.clone()));
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
        let mut branch = Branch::Latest;
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
            branch = Branch::Parent(branch_key(entry));
        }
        self.offset = end;
        let mut skip = Skipped::default();
        let assistant = value.pointer("/message/role").and_then(Value::as_str) == Some("assistant");
        if let (Some(mut selection), done) = select(id, value, start, end, &mut skip, &mut 2)? {
            let before = if done {
                parent.cursor(&self.stamp, start, Skipped::default())
            } else {
                branch.cursor(&self.stamp, end, skip)
            };
            pair_tools(std::slice::from_mut(&mut selection));
            for item in &mut selection.1 {
                self.fold(id, item, changes)?;
            }
            let (message, row) = render(id, selection, before, true)?;
            if !message.items.is_empty() || !message.attachments.is_empty() {
                changes.push((Some((message.clone(), row.before.clone())), assistant));
                self.remember(message, row)?;
                return Ok(true);
            }
        }
        if assistant {
            changes.push((None, true));
        }
        Ok(true)
    }

    fn fold(&mut self, id: &str, item: &mut SelectedItem, changes: &mut Vec<Change>) -> Result<()> {
        let Some(SessionItem::ToolCall {
            call_id,
            result: Some(_),
            ..
        }) = &item.value
        else {
            return Ok(());
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
            changes.push((Some((message.clone(), row.before.clone())), false));
            break;
        }
        self.trim();
        Ok(())
    }
}
