//! Forward journal readback in bounded steps.
use super::*;
use std::io::BufRead;

/// A record read forward is carried whole, where a history page would cut it: a reader
/// holds what came before it and could not reach the items a cut left out. Its message is
/// at most this, so that one operation fits a page of the session topic.
const PUT_BYTES: usize = 48 * 1024;
/// What the items of a record are sized for; the rest is room for previews that could not
/// be made as small as asked.
const PUT_TARGET: usize = 40 * 1024;
/// A continuation item: a signed token naming the item.
const CONTINUATION_BYTES: usize = 640;
/// Bounded steps of the tail read to reach the journal's end at once, before its last page
/// is read instead.
pub(crate) const STEPS: usize = 8;

/// The size an item of this record is carried whole at, so that all of them fit one
/// operation: a history page's size for a record of a few items, less for one of many.
fn put_bound((message, items): &Selection) -> Result<usize> {
    let room = PUT_TARGET.saturating_sub(json_len(message)?);
    Ok((room / items.len().max(1))
        .saturating_sub(CONTINUATION_BYTES)
        .min(ITEM_BYTES))
}

/// A message, the page before it, and whether it is only the last items of its record.
pub(crate) type Message = (SessionMessage, Option<String>, bool);
/// What a record appended to the journal changes.
pub(crate) struct Change {
    /// The record's message, none when it shows nothing.
    pub(crate) row: Option<Message>,
    pub(crate) record: Record,
}

/// The branch order includes invisible entries, which may anchor a turn or count as replies.
pub(crate) struct Record {
    pub(crate) entry_id: Option<String>,
    pub(crate) assistant: bool,
}

impl From<&Value> for Record {
    fn from(value: &Value) -> Self {
        Self {
            entry_id: value["id"]
                .as_str()
                .filter(|id| id.len() <= 128)
                .map(str::to_owned),
            assistant: value.pointer("/message/role").and_then(Value::as_str) == Some("assistant"),
        }
    }
}

pub(crate) struct Records {
    /// The journal root can be covered even when the oldest displayed row has a
    /// continuation cursor inside that record.
    pub(crate) root: bool,
    pub(crate) entries: Vec<Record>,
}

type Snapshot = (Tail, Option<String>, Vec<Message>, Records);

pub(crate) struct Tail {
    journal: (u64, u64),
    len: u64,
    offset: u64,
    leaf: Option<String>,
    stamp: String,
    pending: bool,
}

impl Tail {
    pub(crate) fn snapshot(id: &str, path: &Path, scope: Option<&Path>) -> Result<Snapshot> {
        let mut file = Transcript::open(path, scope)?;
        let (page, rows, offset, leaf, cut, journal) = read_page(id, &mut file, None, false)?;
        let mut tail = Self {
            journal: (file.metadata.dev(), file.metadata.ino()),
            len: file.metadata.len(),
            offset,
            leaf,
            stamp: identity(id, &file.metadata)?,
            pending: false,
        };
        drop(file);
        if let Some((start, follows)) = cut {
            // The page begins inside a record. Read forward from where that record starts
            // instead, so the tail begins with a whole message a reader can join at.
            let newest = (tail.offset, tail.leaf.take());
            (tail.offset, tail.leaf, tail.pending) = (start, follows, true);
            let mut messages = Vec::new();
            let mut records = Vec::new();
            // A step reads a bounded part of the journal. The messages are its tail only
            // once the steps reach its end, as the page read backwards does.
            let mut reached = false;
            for _ in 0..STEPS {
                let Some(changes) = tail.advance(id, path, scope)? else {
                    break;
                };
                for Change { row, record } in changes {
                    messages.extend(row);
                    records.push(record);
                }
                if !tail.pending {
                    reached = true;
                    break;
                }
            }
            if reached && let Some((_, before, _)) = messages.first() {
                return Ok((
                    tail,
                    before.clone(),
                    messages,
                    Records {
                        root: journal.root,
                        entries: records,
                    },
                ));
            }
            // The journal forks after it, or is longer than these steps read: the page as
            // it was cut.
            (tail.offset, tail.leaf) = newest;
            tail.pending = tail.offset < tail.len;
        }
        let messages = page
            .messages
            .into_iter()
            .zip(rows)
            .map(|(message, row)| (message, row.before, row.cut))
            .collect();
        Ok((tail, page.older, messages, journal))
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
        let record = Record::from(&value);
        // Every item of the record: a cut would leave out items no reader could reach.
        let selected = select(id, value, start, end, &mut skip, &mut 2, usize::MAX)?;
        if let (Some(selection), _) = selected {
            let mut before = parent.cursor(&self.stamp, start, Skipped::default());
            let bound = put_bound(&selection)?;
            let mut message = render(id, selection, bound)?;
            let cut = json_len(&message)? > PUT_BYTES;
            if cut {
                // More items than one operation holds at any preview size: its last items,
                // as a history page cuts it. A reader starts again here and pages back.
                let value = serde_json::from_slice(bytes).context("decoding a long record")?;
                let mut skip = Skipped::default();
                let last = select(id, value, start, end, &mut skip, &mut 2, PAGE_BYTES)?;
                let (Some(selection), _) = last else {
                    anyhow::bail!("transcript record exceeds page budget");
                };
                before = branch.cursor(&self.stamp, end, skip);
                message = render(id, selection, ITEM_BYTES)?;
            }
            changes.push(Change {
                row: Some((message, before, cut)),
                record,
            });
        } else if record.assistant || record.entry_id.is_some() {
            changes.push(Change { row: None, record });
        }
        Ok(true)
    }
}
