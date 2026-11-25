//! Bounded readback of the Pi transcript selected by the daemon's session resolver.

mod entries;
mod tokens;

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

impl Branch {
    fn cursor(self, stamp: &str, end: u64, skip: usize) -> Option<String> {
        if end == 0 {
            return None;
        }
        match self {
            Self::Latest => Some(format!("transcript:{stamp}:{end}:{skip}:*")),
            Self::Parent(key) => Some(format!("transcript:{stamp}:{end}:{skip}:{key:032x}")),
            Self::Root => None,
        }
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

fn position(page: &str, stamp: &str) -> Result<(u64, usize, Branch)> {
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
    let skip = parts
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
    ensure!(
        end > 0 && parts.next().is_none(),
        "invalid: malformed transcript cursor"
    );
    ensure!(tagged == stamp, StaleTranscriptCursor);
    Ok((end, skip, branch))
}

struct SelectedItem {
    value: Option<SessionItem>,
    source: Source,
    pair: Option<Source>,
    size: Option<usize>,
}

type Selection = (SessionMessage, Vec<SelectedItem>);

fn header_cost(message: &mut SessionMessage) -> Result<usize> {
    entries::bound_header(message);
    let attachments = std::mem::take(&mut message.attachments);
    let mut cost = json_len(message)? + 1;
    for fact in attachments {
        let extra = json_len(&fact)?
            + if message.attachments.is_empty() {
                r#",\"attachments\":[]"#.len()
            } else {
                1
            };
        // Keep room for at least one item and the page's array brackets. Any facts
        // not shown in the structured preview remain in the recorded text item.
        if cost + extra + CONTINUED_BYTES + 2 > PAGE_BYTES {
            break;
        }
        message.attachments.push(fact);
        cost += extra;
    }
    Ok(cost)
}

pub(super) struct Page {
    pub(super) messages: Vec<SessionMessage>,
    pub(super) older: Option<String>,
    pub(super) stamp: String,
}

/// Return a newest-first slice of records in chronological display order. The cursor
/// skips already-returned items at the newer end of a split record.
pub(super) fn page(
    id: &str,
    path: &Path,
    scope: Option<&Path>,
    page: Option<&str>,
) -> Result<Page> {
    let mut transcript = Transcript::open(path, scope)?;
    // The same open handle supplies the snapshot stamp, byte limit and records.
    let meta = &transcript.metadata;
    let len = meta.len();
    let stamp = fingerprint(&(
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
        let (end, skip, branch) = position(page, &stamp)?;
        ensure!(
            end <= len,
            "invalid: transcript cursor is beyond the snapshot"
        );
        ensure!(
            transcript.ends_line(end)?,
            "invalid: transcript cursor is not at a record boundary"
        );
        (end, skip, branch)
    } else {
        let end = boundaries.before(&mut transcript.file, len)?;
        (end, 0, Branch::Latest)
    };
    let mut selected = Vec::new();
    // Include both array brackets, and budget one separator per message and item.
    let mut size = 2;
    for _ in 0..RECORDS {
        if end == 0 || matches!(branch, Branch::Root) {
            break;
        }
        let start = boundaries.before(&mut transcript.file, end - 1)?;
        let value = transcript.record(start, end)?;
        let entry = value.get("id").and_then(Value::as_str);
        if let Branch::Parent(expected) = branch
            && entry.is_none_or(|id| branch_key(id) != expected)
        {
            ensure!(
                skip == 0,
                "invalid: transcript item cursor does not match the record"
            );
            end = start;
            continue;
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
        if let Some(mut message) = entries::message(value, Some((id, start)))? {
            let all = std::mem::take(&mut message.items);
            ensure!(
                skip <= all.len(),
                "invalid: transcript item cursor is beyond the record"
            );
            let mut cost = header_cost(&mut message)?;
            let mut items = Vec::new();
            let count = all.len();
            for (index, value) in all.into_iter().enumerate().rev().skip(skip) {
                let bytes = if matches!(value, SessionItem::ToolCall { .. }) {
                    // Its result can be folded in after selection.
                    None
                } else {
                    Some(json_len(&value)?)
                };
                let item_cost = bytes
                    .filter(|&bytes| bytes <= ITEM_BYTES)
                    .map_or(CONTINUED_BYTES, |bytes| bytes + 1);
                if size + cost + item_cost > PAGE_BYTES
                    && (!selected.is_empty() || !items.is_empty())
                {
                    break;
                }
                cost += item_cost;
                skip += 1;
                items.push(SelectedItem {
                    value: Some(value),
                    source: Source { start, end, index },
                    pair: None,
                    size: bytes,
                });
            }
            if !items.is_empty() {
                size += cost;
                items.reverse();
                selected.push((message, items));
            }
            if skip < count {
                break;
            }
        } else {
            ensure!(
                skip == 0,
                "invalid: transcript item cursor does not match the record"
            );
        }
        branch = parent;
        end = start;
        skip = 0;
    }
    selected.reverse();
    pair_tools(&mut selected);
    let mut messages = Vec::with_capacity(selected.len());
    for (mut message, items) in selected {
        for item in items {
            append_preview(id, &mut message.items, item)?;
        }
        if !message.items.is_empty() {
            messages.push(message);
        }
    }
    ensure!(
        json_len(&messages)? <= PAGE_BYTES,
        "transcript preview exceeds page budget"
    );
    let older = branch.cursor(&stamp, end, skip);
    Ok(Page {
        messages,
        older,
        stamp,
    })
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

fn append_preview(id: &str, items: &mut Vec<SessionItem>, selected: SelectedItem) -> Result<()> {
    let Some(mut value) = selected.value else {
        return Ok(());
    };
    let size = match selected.size {
        Some(size) => size,
        None => json_len(&value)?,
    };
    if size <= ITEM_BYTES {
        items.push(value);
    } else {
        let token = tokens::begin(id, selected.source, selected.pair, &value)?;
        for limit in [256, 128, 64, 16, 0] {
            entries::clip_item(&mut value, limit);
            if json_len(&value)? <= ITEM_BYTES {
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
