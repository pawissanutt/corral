//! Bounded readback of the Pi transcript selected by the daemon's session resolver.

mod entries;
pub(crate) mod tail;
mod tokens;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use corral_protocol::detail::{SessionItem, SessionItemPage, SessionMessage};
use serde_json::Value;
use sha2::{Digest, Sha256};

use self::tokens::{Source, Token};
use super::{fingerprint, json_len};

const WINDOW: usize = 128 * 1024;
const RECORDS: usize = 32;
const PAGE_BYTES: usize = 28 * 1024;
const ITEM_BYTES: usize = 2048;
const CONTINUED_BYTES: usize = 4096;

/// A pairing key must survive even the smallest preview. Bound unusual journal IDs
/// independently of the item budget, reserving the digest prefix from raw IDs.
pub(crate) fn tool_call_id(id: &str) -> Cow<'_, str> {
    if id.len() <= 256 && !id.starts_with("sha256:") {
        Cow::Borrowed(id)
    } else {
        Cow::Owned(format!("sha256:{:x}", Sha256::digest(id.as_bytes())))
    }
}

struct Transcript {
    file: File,
    metadata: Metadata,
}

impl Transcript {
    fn open(path: &Path, scope: Option<&Path>) -> Result<Self> {
        if let Some(dir) = scope {
            return Self::from_file(crate::sessions::open_session_file(dir, path)?);
        }
        use rustix::fs::{Mode, OFlags};

        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("reading recorded transcript metadata {}", path.display()))?;
        ensure!(
            metadata.is_file(),
            "recorded transcript is not a regular file"
        );
        let file = File::from(
            rustix::fs::open(
                path,
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .with_context(|| format!("opening recorded transcript {}", path.display()))?,
        );
        Self::from_file(file)
    }

    fn from_file(file: File) -> Result<Self> {
        let metadata = file
            .metadata()
            .context("reading opened transcript metadata")?;
        ensure!(
            metadata.is_file(),
            "recorded transcript is not a regular file"
        );
        Ok(Self { file, metadata })
    }

    fn ends_line(&mut self, offset: u64) -> Result<bool> {
        self.file
            .seek(SeekFrom::Start(offset - 1))
            .context("seeking transcript newline")?;
        let mut byte = [0];
        self.file
            .read_exact(&mut byte)
            .context("reading transcript newline")?;
        Ok(byte[0] == b'\n')
    }

    fn record(&mut self, start: u64, end: u64) -> Result<Value> {
        ensure!(
            start < end && end <= self.metadata.len(),
            "transcript entry is stale"
        );
        self.file
            .seek(SeekFrom::Start(start))
            .context("seeking transcript entry")?;
        serde_json::from_reader(BufReader::new((&mut self.file).take(end - start)))
            .context("decoding recorded transcript entry")
    }

    fn load_item(&mut self, source: Source) -> Result<SessionItem> {
        ensure!(
            source.start < source.end && source.end <= self.metadata.len(),
            "transcript item is stale"
        );
        ensure!(
            (source.start == 0 || self.ends_line(source.start)?) && self.ends_line(source.end)?,
            "transcript item boundary changed; reload the session"
        );
        entries::message(self.record(source.start, source.end)?, None)?
            .and_then(|message| message.items.into_iter().nth(source.index))
            .context("transcript item no longer exists")
    }
}

/// Reuse the backwards-search window across adjacent records. A record may span any
/// number of windows; only its JSON decoder allocates in proportion to that record.
struct Boundaries {
    bytes: [u8; WINDOW],
    start: u64,
    end: u64,
}

impl Boundaries {
    fn before(&mut self, file: &mut File, mut end: u64) -> Result<u64> {
        while end > 0 {
            if end <= self.start || end > self.end {
                self.start = end.saturating_sub(WINDOW as u64);
                self.end = end;
                file.seek(SeekFrom::Start(self.start))
                    .context("seeking transcript boundary")?;
                file.read_exact(&mut self.bytes[..(end - self.start) as usize])
                    .context("reading transcript boundary")?;
            }
            if let Some(index) = self.bytes[..(end - self.start) as usize]
                .iter()
                .rposition(|byte| *byte == b'\n')
            {
                return Ok(self.start + index as u64 + 1);
            }
            end = self.start;
        }
        Ok(0)
    }
}

#[derive(Clone, Copy)]
enum Branch {
    Latest,
    Parent(u128),
    Root,
}

fn branch_key(id: &str) -> u128 {
    let digest = Sha256::digest(id.as_bytes());
    let mut key = [0; 16];
    key.copy_from_slice(&digest[..16]);
    u128::from_be_bytes(key)
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Skipped {
    items: usize,
    attachments: usize,
}

impl Branch {
    fn cursor(self, stamp: &str, end: u64, skip: Skipped) -> Option<String> {
        if end == 0 {
            return None;
        }
        let Skipped { items, attachments } = skip;
        let branch = match self {
            Self::Latest => "*".to_owned(),
            Self::Parent(key) => format!("{key:032x}"),
            Self::Root => return None,
        };
        Some(format!(
            "transcript:{stamp}:{end}:{items}:{branch}:{attachments}"
        ))
    }
}

#[derive(Debug)]
pub(crate) struct StaleTranscriptCursor;

impl std::fmt::Display for StaleTranscriptCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("transcript changed; reload the session")
    }
}

impl std::error::Error for StaleTranscriptCursor {}

fn position(page: &str, stamp: &str) -> Result<(u64, Skipped, Branch)> {
    let mut parts = page.split(':');
    ensure!(
        parts.next() == Some("transcript"),
        "invalid: expected a transcript cursor"
    );
    let tagged = parts
        .next()
        .filter(|tag| tag.len() == stamp.len() && tag.bytes().all(|b| b.is_ascii_hexdigit()))
        .context("invalid: malformed transcript stamp")?;
    let end = parts
        .next()
        .context("invalid: missing transcript position")?
        .parse()
        .context("invalid: malformed transcript position")?;
    let items = parts
        .next()
        .context("invalid: missing transcript item position")?
        .parse()
        .context("invalid: malformed transcript item position")?;
    let branch = match parts.next() {
        Some("*") => Branch::Latest,
        Some(key) if key.len() == 32 => Branch::Parent(
            u128::from_str_radix(key, 16).context("invalid: malformed transcript branch")?,
        ),
        _ => anyhow::bail!("invalid: malformed transcript branch"),
    };
    // Recognize the old shape so pre-upgrade cursors fail as stale, not malformed.
    let attachments = parts.next();
    ensure!(
        end > 0 && parts.next().is_none(),
        "invalid: malformed transcript cursor"
    );
    ensure!(tagged == stamp, StaleTranscriptCursor);
    let attachments = attachments
        .context("invalid: missing transcript attachment position")?
        .parse()
        .context("invalid: malformed transcript attachment position")?;
    Ok((end, Skipped { items, attachments }, branch))
}

struct SelectedItem {
    value: Option<SessionItem>,
    source: Source,
    pair: Option<Source>,
    size: Option<usize>,
}

impl SelectedItem {
    fn new(value: SessionItem, source: Source) -> Result<Self> {
        let size = if matches!(value, SessionItem::ToolCall { .. }) {
            // Its result can be folded in after selection.
            None
        } else {
            Some(json_len(&value)?)
        };
        Ok(Self {
            value: Some(value),
            source,
            pair: None,
            size,
        })
    }

    fn cost(&self) -> usize {
        self.size
            .filter(|&bytes| bytes <= ITEM_BYTES)
            .map_or(CONTINUED_BYTES, |bytes| bytes + 1)
    }
}

type Selection = (SessionMessage, Vec<SelectedItem>);

fn identity(id: &str, meta: &Metadata) -> Result<String> {
    fingerprint(&("transcript-cursor-v1", id, meta.dev(), meta.ino()))
}

pub(super) struct Page {
    pub(super) messages: Vec<SessionMessage>,
    pub(super) older: Option<String>,
    pub(super) stamp: String,
}

/// Return a newest-first slice of records in chronological display order. The cursor
/// skips already-returned item and attachment suffixes of a split record.
pub(super) fn page(
    id: &str,
    path: &Path,
    scope: Option<&Path>,
    page: Option<&str>,
) -> Result<Page> {
    let mut file = match Transcript::open(path, scope) {
        Err(error)
            if page.is_some()
                && error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|err| err.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Err(StaleTranscriptCursor.into());
        }
        result => result?,
    };
    read_page(id, &mut file, page, true).map(|(page, ..)| page)
}

struct Row {
    before: Option<String>,
    /// Only the record's last items, as a history page cut it: `before` resumes inside it.
    cut: bool,
}

/// Where the record a page cut starts, and the entry it follows.
type Cut = (u64, Option<String>);
type PageRead = (
    Page,
    Vec<Row>,
    u64,
    Option<String>,
    Option<Cut>,
    tail::Records,
);

fn read_page(
    id: &str,
    transcript: &mut Transcript,
    page: Option<&str>,
    pair_results: bool,
) -> Result<PageRead> {
    // The same open handle supplies the snapshot stamp, byte limit and records.
    let meta = &transcript.metadata;
    let len = meta.len();
    let identity = identity(id, meta)?;
    let stamp = fingerprint(&(
        "attachment-pages-v1",
        id,
        len,
        meta.dev(),
        meta.ino(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    ))?;
    let mut boundaries = Boundaries {
        bytes: [0; WINDOW],
        start: 0,
        end: 0,
    };
    let (mut end, mut skip, mut branch) = if let Some(page) = page {
        let (end, skip, branch) = position(page, &identity)?;
        ensure!(end <= len, StaleTranscriptCursor);
        ensure!(transcript.ends_line(end)?, StaleTranscriptCursor);
        (end, skip, branch)
    } else {
        let end = boundaries.before(&mut transcript.file, len)?;
        (end, Skipped::default(), Branch::Latest)
    };
    let offset = end;
    let mut leaf = None;
    let mut rows = Vec::new();
    let mut selected = Vec::new();
    let mut pin_pending = page.is_some() && matches!(branch, Branch::Parent(_));
    // Include both array brackets, and budget one separator per message and item.
    let mut size = 2;
    let mut records = 0;
    let mut cut = None;
    let mut journal = tail::Records {
        root: offset == 0,
        entries: Vec::new(),
    };
    while records < RECORDS || pin_pending {
        if end == 0 || matches!(branch, Branch::Root) {
            break;
        }
        if !pin_pending {
            records += 1;
        }
        let start = boundaries.before(&mut transcript.file, end - 1)?;
        let value = transcript.record(start, end)?;
        let entry = value.get("id").and_then(Value::as_str);
        if let Branch::Parent(expected) = branch
            && entry.is_none_or(|id| branch_key(id) != expected)
        {
            ensure!(skip == Skipped::default(), StaleTranscriptCursor);
            end = start;
            continue;
        }
        if pin_pending {
            records += 1;
            pin_pending = false;
        }
        if leaf.is_none() && value.get("parentId").is_some() {
            leaf = entry.map(str::to_owned);
        }
        let parent = if let Some(entry) = entry
            && value.get("parentId").is_some()
        {
            if matches!(branch, Branch::Latest) {
                // A split rooted record must still be pinned to its branch on resumption.
                branch = Branch::Parent(branch_key(entry));
            }
            value["parentId"]
                .as_str()
                .map_or(Branch::Root, |id| Branch::Parent(branch_key(id)))
        } else {
            branch
        };
        let follows = value
            .get("parentId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let record = (!pair_results).then(|| tail::Record::from(&value));
        let (selection, done) = select(id, value, start, end, &mut skip, &mut size, PAGE_BYTES)?;
        if let Some(record) = record.filter(|_| done || selection.is_some()) {
            journal.root = start == 0 || matches!(parent, Branch::Root);
            journal.entries.push(record);
        }
        if let Some(selection) = selection {
            let before = if done {
                parent.cursor(&identity, start, Skipped::default())
            } else {
                cut = Some((start, follows));
                branch.cursor(&identity, end, skip)
            };
            rows.push(before);
            selected.push(selection);
        }
        if !done {
            break;
        }
        branch = parent;
        end = start;
        skip = Skipped::default();
    }
    ensure!(!pin_pending, StaleTranscriptCursor);
    selected.reverse();
    rows.reverse();
    if pair_results {
        pair_tools(&mut selected);
    }
    let mut messages = Vec::with_capacity(selected.len());
    let mut retained = Vec::new();
    for (selection, before) in selected.into_iter().zip(rows) {
        let message = render(id, selection, ITEM_BYTES)?;
        if !message.items.is_empty() || !message.attachments.is_empty() {
            retained.push(Row { before, cut: false });
            messages.push(message);
        }
    }
    if cut.is_some()
        && let Some(first) = retained.first_mut()
    {
        first.cut = true;
    }
    ensure!(
        json_len(&messages)? <= PAGE_BYTES,
        "transcript preview exceeds page budget"
    );
    let older = branch.cursor(&identity, end, skip);
    journal.entries.reverse();
    Ok((
        Page {
            messages,
            older,
            stamp,
        },
        retained,
        offset,
        leaf,
        cut,
        journal,
    ))
}

fn select(
    id: &str,
    value: Value,
    start: u64,
    end: u64,
    skip: &mut Skipped,
    size: &mut usize,
    budget: usize,
) -> Result<(Option<Selection>, bool)> {
    if let Some(mut message) = entries::message(value, Some((id, start)))? {
        entries::bound_header(&mut message);
        let all_items = std::mem::take(&mut message.items);
        let all_facts = std::mem::take(&mut message.attachments);
        let (item_count, fact_count) = (all_items.len(), all_facts.len());
        ensure!(
            skip.items <= item_count && skip.attachments <= fact_count,
            StaleTranscriptCursor
        );
        let mut cost = json_len(&message)? + 1;
        let mut remaining = all_items
            .into_iter()
            .enumerate()
            .rev()
            .skip(skip.items)
            .map(|(index, value)| SelectedItem::new(value, Source { start, end, index }));
        let first = remaining.next().transpose()?;
        // Leave room for a body item as long as there is one left; later pages
        // may carry only metadata for this same message ID.
        let reserve = first.as_ref().map_or(0, SelectedItem::cost);
        if *size + cost + reserve > budget {
            ensure!(*size > 2, "transcript record exceeds page budget");
            return Ok((None, false));
        }
        for fact in all_facts.into_iter().rev().skip(skip.attachments) {
            let extra = json_len(&fact)?
                + if message.attachments.is_empty() {
                    r#","attachments":[]"#.len()
                } else {
                    1
                };
            if *size + cost + extra + reserve > budget {
                break;
            }
            cost += extra;
            skip.attachments += 1;
            message.attachments.push(fact);
        }
        message.attachments.reverse();
        let mut items = Vec::new();
        for item in first.into_iter().map(Ok).chain(remaining) {
            let item = item?;
            let extra = item.cost();
            if *size + cost + extra > budget {
                break;
            }
            cost += extra;
            skip.items += 1;
            items.push(item);
        }
        if !items.is_empty() || !message.attachments.is_empty() {
            *size += cost;
            items.reverse();
            return Ok((
                Some((message, items)),
                skip.items == item_count && skip.attachments == fact_count,
            ));
        }
    } else {
        ensure!(*skip == Skipped::default(), StaleTranscriptCursor);
    }
    Ok((None, true))
}

/// Pair only tools selected on this page. Images remain independent items.
fn pair_tools(messages: &mut [Selection]) {
    let mut calls = BTreeMap::new();
    let mut matches = Vec::new();
    for (row, (_, items)) in messages.iter().enumerate() {
        for (column, item) in items.iter().enumerate() {
            if let Some(SessionItem::ToolCall {
                call_id, result, ..
            }) = item.value.as_ref()
                && !call_id.is_empty()
            {
                if result.is_none() {
                    calls.insert(call_id.as_str(), (row, column));
                } else if let Some(call) = calls.remove(call_id.as_str()) {
                    matches.push((call, (row, column)));
                }
            }
        }
    }
    drop(calls);
    for ((call_row, call_column), (result_row, result_column)) in matches {
        let result = &mut messages[result_row].1[result_column];
        let source = result.source;
        if let Some(result) = result.value.take() {
            let call = &mut messages[call_row].1[call_column];
            if let Some(value) = &mut call.value {
                entries::merge(value, result);
                call.pair = Some(source);
            }
        }
    }
}

fn render(id: &str, (mut message, items): Selection, bound: usize) -> Result<SessionMessage> {
    for item in items {
        append_preview(id, &mut message.items, item, bound)?;
    }
    Ok(message)
}

/// The item whole when it is at most `bound` bytes, else a preview of it and where the rest
/// is read.
fn append_preview(
    id: &str,
    items: &mut Vec<SessionItem>,
    selected: SelectedItem,
    bound: usize,
) -> Result<()> {
    let Some(mut value) = selected.value else {
        return Ok(());
    };
    let size = match selected.size {
        Some(size) => size,
        None => json_len(&value)?,
    };
    if size <= bound {
        items.push(value);
    } else {
        let token = tokens::begin(id, selected.source, selected.pair, &value)?;
        for limit in [256, 128, 64, 16, 0] {
            entries::clip_item(&mut value, limit);
            if json_len(&value)? <= bound {
                break;
            }
        }
        items.push(value);
        items.push(SessionItem::Continuation {
            token,
            bytes: size as u64,
        });
    }
    Ok(())
}

pub(super) fn item(
    id: &str,
    path: &Path,
    scope: Option<&Path>,
    token: &str,
) -> Result<SessionItemPage> {
    let token = Token::decode(id, token)?;
    let mut transcript = Transcript::open(path, scope)?;
    let mut item = transcript.load_item(token.source)?;
    if let Some(pair) = token.pair {
        entries::merge(&mut item, transcript.load_item(pair)?);
    }
    token.fragment(id, &item)
}

#[cfg(test)]
mod tests;
