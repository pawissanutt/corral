//! Bounded readback of trusted, stored session references. Request IDs never become paths.
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result, ensure};
use corral_protocol::detail::session::{SessionItem, SessionMessage, TerminalSnapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{activity, replay, short};
use crate::Ctx;

const WINDOW: u64 = 128 * 1024;
const MESSAGES: usize = 32;
const ARCHIVE_BYTES: u64 = 8 * 1024 * 1024;
/// Lines a wall tile shows for what a session did last.
const TAIL_LINES: usize = 4;
const TAIL_BYTES: usize = 160;
const LOG_TAIL_WINDOW: u64 = 4 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) enum Source {
    Transcript { start: u64, end: u64, index: usize },
    Thread { before: Option<u64>, index: usize },
}
impl Source {
    pub(super) fn index(&self, index: usize) -> Self {
        match self {
            Self::Transcript { start, end, .. } => Self::Transcript {
                start: *start,
                end: *end,
                index,
            },
            Self::Thread { before, .. } => Self::Thread {
                before: *before,
                index,
            },
        }
    }
    pub(super) fn item(&self) -> usize {
        match self {
            Self::Transcript { index, .. } | Self::Thread { index, .. } => *index,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Token {
    pub source: Source,
    pub pair: Option<Source>,
    pub offset: usize,
    pub digest: String,
}

fn secret() -> Result<&'static [u8; 32]> {
    static SECRET: LazyLock<std::result::Result<[u8; 32], String>> = LazyLock::new(|| {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
        Ok(bytes)
    });
    SECRET
        .as_ref()
        .map_err(|error| anyhow::anyhow!("creating transcript token key: {error}"))
}
pub(super) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(DIGITS[(byte >> 4) as usize] as char);
        result.push(DIGITS[(byte & 15) as usize] as char);
    }
    result
}
fn unhex(text: &str) -> Result<Vec<u8>> {
    ensure!(text.len().is_multiple_of(2), "invalid replay token");
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| (byte as char).to_digit(16).context("invalid replay token");
            Ok((digit(pair[0])? * 16 + digit(pair[1])?) as u8)
        })
        .collect()
}
fn signature(id: &str, payload: &[u8]) -> Result<String> {
    // HMAC-SHA256: the session id is authenticated, not trusted token metadata.
    let mut inner = [0x36; 64];
    let mut outer = [0x5c; 64];
    for (index, byte) in secret()?.iter().enumerate() {
        inner[index] ^= byte;
        outer[index] ^= byte;
    }
    let mut hash = Sha256::new();
    hash.update(inner);
    hash.update((id.len() as u64).to_be_bytes());
    hash.update(id.as_bytes());
    hash.update(payload);
    let mut final_hash = Sha256::new();
    final_hash.update(outer);
    final_hash.update(hash.finalize());
    Ok(hex(&final_hash.finalize()))
}
pub(super) fn encode(id: &str, token: &Token) -> Result<String> {
    let bytes = serde_json::to_vec(token).context("encoding replay item token")?;
    Ok(format!("{}.{}", hex(&bytes), signature(id, &bytes)?))
}
pub(super) fn decode(id: &str, text: &str) -> Result<Token> {
    ensure!(text.len() <= 2048, "invalid replay token");
    let (payload, mac) = text.split_once('.').context("invalid replay token")?;
    let bytes = unhex(payload)?;
    let expected = signature(id, &bytes)?;
    ensure!(
        mac.len() == expected.len()
            && mac
                .bytes()
                .zip(expected.bytes())
                .fold(0, |diff, (a, b)| diff | (a ^ b))
                == 0,
        "invalid replay token"
    );
    serde_json::from_slice(&bytes).context("invalid replay token payload")
}
pub(super) fn digest(item: &SessionItem) -> Result<String> {
    Ok(hex(&Sha256::digest(
        serde_json::to_vec(item).context("encoding replay item")?,
    )))
}

pub(super) fn open(path: &Path) -> Result<File> {
    let meta = std::fs::symlink_metadata(path).context("reading recorded output metadata")?;
    ensure!(meta.is_file(), "recorded output is not a regular file");
    #[cfg(unix)]
    let file = File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .context("opening recorded output")?,
    );
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x0020_0000)
            .open(path)
            .context("opening recorded output")?
    };
    #[cfg(not(any(unix, windows)))]
    let file = File::open(path).context("opening recorded output")?;
    ensure!(
        file.metadata()
            .context("reading opened output metadata")?
            .is_file(),
        "recorded output is not a regular file"
    );
    Ok(file)
}

/// Finds one record boundary using fixed windows, including when a single JSONL record is huge.
fn boundary(file: &mut File, mut end: u64) -> Result<u64> {
    let mut bytes = [0; WINDOW as usize];
    while end > 0 {
        let start = end.saturating_sub(WINDOW);
        let count = (end - start) as usize;
        file.seek(SeekFrom::Start(start))
            .context("seeking transcript boundary")?;
        file.read_exact(&mut bytes[..count])
            .context("reading transcript boundary")?;
        if let Some(index) = bytes[..count].iter().rposition(|byte| *byte == b'\n') {
            return Ok(start + index as u64 + 1);
        }
        end = start;
    }
    Ok(0)
}

/// Deserialize only the selected record, through a bounded-buffer reader, never a whole-file read.
pub(super) fn record(path: &Path, start: u64, end: u64) -> Result<Value> {
    let mut file = open(path)?;
    let len = file.metadata().context("reading transcript size")?.len();
    ensure!(start < end && end <= len, "transcript item is stale");
    if start > 0 {
        file.seek(SeekFrom::Start(start - 1))
            .context("seeking transcript record start")?;
        let mut byte = [0];
        file.read_exact(&mut byte)
            .context("reading transcript record start")?;
        ensure!(byte[0] == b'\n', "invalid transcript record boundary");
    }
    file.seek(SeekFrom::Start(end - 1))
        .context("seeking transcript record end")?;
    let mut byte = [0];
    file.read_exact(&mut byte)
        .context("reading transcript record end")?;
    ensure!(byte[0] == b'\n', "transcript record is incomplete");
    file.seek(SeekFrom::Start(start))
        .context("seeking transcript record")?;
    serde_json::from_reader(BufReader::new(file.take(end - start)))
        .context("decoding recorded transcript entry")
}

/// The skip counts items already returned from the end of a multi-page record.
pub(super) fn position(page: Option<&str>) -> Result<(Option<u64>, usize, String)> {
    let Some(page) = page else {
        return Ok((None, 0, "*".into()));
    };
    let mut parts = page.split(':');
    ensure!(
        parts.next() == Some("transcript"),
        "invalid transcript cursor"
    );
    let end = parts
        .next()
        .context("missing transcript position")?
        .parse()
        .context("invalid transcript position")?;
    let skip = parts
        .next()
        .unwrap_or("0")
        .parse()
        .context("invalid transcript item position")?;
    let branch = parts.next().unwrap_or("*");
    ensure!(
        matches!(branch, "*" | "-")
            || (branch.len() == 32 && branch.bytes().all(|byte| byte.is_ascii_hexdigit())),
        "invalid transcript branch cursor"
    );
    ensure!(parts.next().is_none(), "invalid transcript cursor");
    Ok((Some(end), skip, branch.into()))
}

/// Older pages preserve every block, including blocks of a record larger than a reply.
pub(super) fn messages(
    path: &Path,
    id: &str,
    page: Option<&str>,
) -> Result<(Vec<SessionMessage>, Option<String>)> {
    messages_with_words(path, id, page, true, replay::PAGE_BYTES)
}

pub(super) fn messages_with_words(
    path: &Path,
    id: &str,
    page: Option<&str>,
    words: bool,
    budget: usize,
) -> Result<(Vec<SessionMessage>, Option<String>)> {
    let mut file = open(path)?;
    let len = file.metadata().context("reading transcript size")?.len();
    let (requested, mut skip, mut branch) = position(page)?;
    ensure!(
        requested.is_none_or(|end| end <= len),
        "transcript cursor is stale"
    );
    let mut end = boundary(&mut file, requested.unwrap_or(len))?;
    let mut selected = Vec::new();
    let mut size = 0;
    let mut older = None;
    for _ in 0..MESSAGES {
        if end == 0 || branch == "-" {
            break;
        }
        let start = boundary(&mut file, end - 1)?;
        let value = record(path, start, end)?;
        let key = |id: &str| hex(&Sha256::digest(id.as_bytes())[..16]);
        let entry = value.get("id").and_then(Value::as_str);
        if branch != "*" && entry.is_none_or(|id| key(id) != branch) {
            end = start;
            older = (end > 0).then(|| format!("transcript:{end}:0:{branch}"));
            continue;
        }
        let parent = if entry.is_some() && value.get("parentId").is_some() {
            if branch == "*" {
                branch = entry.map(key).unwrap_or_else(|| "*".into());
            }
            value
                .get("parentId")
                .and_then(Value::as_str)
                .map(key)
                .unwrap_or_else(|| "-".into())
        } else {
            branch.clone()
        };
        if let Some(mut message) = replay::message(&value) {
            let source = replay::Source::Transcript {
                start,
                end,
                index: 0,
            };
            let all = std::mem::take(&mut message.items);
            ensure!(skip <= all.len(), "transcript item cursor is stale");
            replay::header(&mut message);
            let mut cost = crate::app_read::paging::json_len(&message)?;
            let mut items = Vec::new();
            let count = all.len();
            let mut consumed = skip;
            for (index, value) in all.into_iter().enumerate().rev().skip(skip) {
                if !words
                    && matches!(&value, corral_protocol::detail::SessionItem::Text { .. })
                    && matches!(message.author.as_str(), "user" | "assistant")
                {
                    consumed += 1;
                    continue;
                }
                let item = replay::Item {
                    value,
                    source: source.index(index),
                    pair: None,
                    hidden: false,
                };
                let item_cost = if matches!(
                    &item.value,
                    corral_protocol::detail::SessionItem::ToolCall { .. }
                ) {
                    // Pairing may add a result after selection; reserve its largest preview and token.
                    4096
                } else {
                    crate::app_read::paging::json_len(&replay::bounded(
                        id,
                        replay::Item {
                            value: item.value.clone(),
                            source: item.source.clone(),
                            pair: None,
                            hidden: false,
                        },
                    )?)?
                };
                if size + cost + item_cost > budget && (!selected.is_empty() || !items.is_empty()) {
                    older = Some(format!("transcript:{end}:{consumed}:{branch}"));
                    break;
                }
                cost += item_cost;
                consumed += 1;
                items.push(item);
            }
            items.reverse();
            if !items.is_empty() {
                size += cost;
                selected.push((message, items));
            }
            if consumed < count {
                break;
            }
        }
        branch = parent;
        end = start;
        skip = 0;
        older = (end > 0 && branch != "-").then(|| format!("transcript:{end}:0:{branch}"));
    }
    selected.reverse();
    replay::pair(&mut selected);
    let mut messages = Vec::new();
    for (mut message, items) in selected {
        for item in items {
            message.items.extend(replay::bounded(id, item)?);
        }
        if !message.items.is_empty() {
            messages.push(message);
        }
    }
    ensure!(
        crate::app_read::paging::json_len(&messages)? <= budget + 4096,
        "transcript preview exceeds page budget"
    );
    Ok((messages, older))
}

/// The complete lines in the last `window` bytes of a recorded file.
fn end_window(path: &Path, window: u64) -> Result<(Vec<u8>, Option<jiff::Timestamp>)> {
    let mut file = open(path)?;
    let meta = file.metadata().context("reading recorded output size")?;
    let captured_at = meta
        .modified()
        .context("reading recorded output capture time")
        .ok()
        .and_then(|at| jiff::Timestamp::try_from(at).ok());
    let len = meta.len();
    let start = len.saturating_sub(window);
    file.seek(SeekFrom::Start(start))
        .context("seeking recorded output tail")?;
    let mut bytes = Vec::new();
    file.take(len - start)
        .read_to_end(&mut bytes)
        .context("reading recorded output tail")?;
    if start > 0 {
        let first = bytes
            .iter()
            .position(|b| *b == b'\n')
            .map_or(bytes.len(), |n| n + 1);
        bytes.drain(..first);
    }
    Ok((bytes, captured_at))
}

/// One display line: no terminal escape sequences or control characters, bounded, or nothing
/// when blank.
fn tail_line(text: &str) -> Option<String> {
    let mut clean = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            // A CSI sequence runs to its final byte; any other escape is one more character.
            if chars.next() == Some('[') {
                chars.by_ref().find(|ch| ('@'..='~').contains(ch));
            }
        } else if !ch.is_control() {
            clean.push(ch);
        }
    }
    let clean = clean.trim();
    (!clean.is_empty()).then(|| short(clean, TAIL_BYTES))
}

fn last_line(text: &str) -> Option<String> {
    text.lines().rev().find_map(tail_line)
}

/// What a tool was called with, as one phrase: its command or path when it has one.
fn tool_argument(block: &Value) -> Option<String> {
    let input = block.get("arguments").or_else(|| block.get("input"))?;
    let named = ["command", "path", "file_path", "pattern", "query", "url"]
        .iter()
        .find_map(|key| input.get(key).and_then(Value::as_str));
    match (named, input) {
        (Some(text), _) => text.lines().find_map(tail_line),
        (None, Value::String(text)) => text.lines().find_map(tail_line),
        (None, Value::Null) => None,
        (None, other) => tail_line(&other.to_string()),
    }
}

/// The text of a tool result block, which is a string or a list of text parts.
fn result_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => last_line(text),
        Value::Array(parts) => parts
            .iter()
            .rev()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .find_map(last_line),
        _ => None,
    }
}

/// One recorded entry as tail lines, oldest first: what was said and each tool call. A tool's
/// output is shown only when it failed; the call line already says what ran.
fn entry_lines(value: &Value) -> Vec<String> {
    let body = value.get("message").unwrap_or(value);
    let role = body.get("role").and_then(Value::as_str).or_else(|| {
        match value.get("type").and_then(Value::as_str) {
            Some("user") => Some("user"),
            Some("assistant") => Some("assistant"),
            _ => None,
        }
    });
    let Some(role) = role else {
        return Vec::new();
    };
    let failed = body.get("isError").and_then(Value::as_bool) == Some(true);
    // A prompt is read from its start; an answer or a failure from its end.
    let said = |text: &str| match role {
        "user" => text
            .lines()
            .find_map(tail_line)
            .map(|line| short(&format!("> {line}"), TAIL_BYTES)),
        "toolResult" if !failed => None,
        _ => last_line(text),
    };
    match body.get("content") {
        Some(Value::String(text)) => said(text).into_iter().collect(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(
                |block| match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" | "input_text" | "output_text" => {
                        said(block.get("text").and_then(Value::as_str)?)
                    }
                    "toolCall" | "tool_use" => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                        let call = match tool_argument(block) {
                            Some(argument) => format!("{name} {argument}"),
                            None => name.to_owned(),
                        };
                        tail_line(&call)
                    }
                    "tool_result"
                        if block.get("is_error").and_then(Value::as_bool) == Some(true) =>
                    {
                        result_text(block.get("content")?)
                    }
                    _ => None,
                },
            )
            .collect(),
        _ => Vec::new(),
    }
}

/// What a session did last, oldest line first, and timestamps from the same bounded read.
/// Entries larger than the window or still being written contribute nothing. Every complete
/// timestamped event in this window is counted, even after the four display lines are full;
/// older minutes can undercount in busy sessions (see `activity::Records`).
pub(super) fn tail(path: &Path, now: jiff::Timestamp) -> Result<activity::Output> {
    let (bytes, _) = end_window(path, WINDOW)?;
    let mut records = activity::Records::new(now);
    let mut lines = Vec::new();
    for entry in bytes
        .split_inclusive(|b| *b == b'\n')
        .rev()
        .filter(|entry| entry.ends_with(b"\n"))
    {
        let Ok(value) = serde_json::from_slice::<Value>(entry) else {
            continue;
        };
        if let Some(at) = event_at(&value) {
            records.observe(at);
        }
        if lines.len() < TAIL_LINES {
            lines.extend(entry_lines(&value).into_iter().rev());
        }
    }
    lines.truncate(TAIL_LINES);
    lines.reverse();
    Ok(records.finish(lines))
}

/// Match replay's top-level/string and nested/millisecond timestamp forms without
/// allocating the full session message (tool results can contain large item lists).
fn event_at(value: &Value) -> Option<jiff::Timestamp> {
    if matches!(
        value.get("type").and_then(Value::as_str),
        Some("session" | "context_edit" | "session_info")
    ) {
        return None;
    }
    let body = value.get("message").unwrap_or(value);
    let timestamp = value.get("timestamp").or_else(|| body.get("timestamp"))?;
    if let Some(text) = timestamp.as_str() {
        text.parse().ok()
    } else {
        timestamp
            .as_i64()
            .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
    }
}

/// The last lines a running gate wrote to its registered scratch log.
pub(super) fn gate_tail(ctx: &Ctx, task: &str, run: &str) -> Result<activity::Output> {
    let path = crate::gate::gate_paths(ctx, task, run).log;
    let (bytes, captured_at) = end_window(&path, LOG_TAIL_WINDOW)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text
        .lines()
        .rev()
        .filter_map(tail_line)
        .take(TAIL_LINES)
        .collect();
    lines.reverse();
    // Plain gate log lines have no event timestamps; an event rate is unknown.
    Ok(activity::Output {
        lines,
        last_output_at: captured_at.map(|at| at.to_string()),
        activity: None,
    })
}

fn lines(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = Vec::new();
    let mut budget: usize = 32 * 1024;
    for line in text.lines().rev().take(200) {
        let line = short(line, 160);
        // JSON escapes count toward the CONTROL budget, not only UTF-8 payload bytes.
        let cost = line
            .chars()
            .map(|ch| match ch {
                '"' | '\\' => 2,
                ch if ch.is_control() => 6,
                ch => ch.len_utf8(),
            })
            .sum::<usize>()
            + 4;
        if cost > budget {
            break;
        }
        budget -= cost;
        lines.push(line);
    }
    lines.reverse();
    lines
}

pub(super) fn gate_snapshot(ctx: &Ctx, task: &str, run: &str) -> Result<TerminalSnapshot> {
    let head = ctx.store.head(task)?;
    let run = head.run_head(run)?.context("gate run not found")?;
    let gate = run.gate.as_ref().context("gate not found")?;
    if let Some(identity) = &gate.identity
        && let Some(attempt) = ctx.store.gate_attempt(task, &run.id, &identity.attempt)?
        && let Some(blob) = attempt.log
    {
        let reader = ctx
            .store
            .blob_reader(blob.hash())?
            .context("recorded gate log unavailable")?;
        let mut bytes = Vec::new();
        reader
            .take(ARCHIVE_BYTES + 1)
            .read_to_end(&mut bytes)
            .context("reading recorded gate log")?;
        ensure!(
            bytes.len() as u64 <= ARCHIVE_BYTES,
            "recorded gate log exceeds bounded snapshot read"
        );
        return Ok(TerminalSnapshot {
            lines: lines(&bytes),
            captured_at: attempt.ended.unwrap_or(attempt.started).to_string(),
            live: false,
            rows: Vec::new(),
        });
    }
    // Canonical gate_paths chooses only this recorded attempt's registered scratch log.
    let path = crate::gate::gate_paths(ctx, task, &run.id).log;
    let mut file = open(&path)?;
    let meta = file
        .metadata()
        .context("reading recorded gate log metadata")?;
    let captured_at =
        jiff::Timestamp::try_from(meta.modified().context("reading log capture time")?)
            .context("converting log capture time")?
            .to_string();
    let start = meta.len().saturating_sub(WINDOW);
    file.seek(SeekFrom::Start(start))
        .context("seeking recorded gate output")?;
    let mut bytes = Vec::new();
    file.take(meta.len() - start)
        .read_to_end(&mut bytes)
        .context("reading recorded gate output")?;
    let first = if start == 0 {
        0
    } else {
        bytes
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(bytes.len())
    };
    Ok(TerminalSnapshot {
        lines: lines(&bytes[first..]),
        captured_at,
        live: false,
        rows: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::detail::SessionItem;

    #[test]
    fn transcript_continuation_keeps_unconsumed_records() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("session.jsonl");
        let mut text = String::new();
        for index in 0..40 {
            text.push_str(&serde_json::to_string(&serde_json::json!({
                "message": { "role": "user", "content": format!("message-{index}") }
            }))?);
            text.push('\n');
        }
        std::fs::write(&path, text)?;
        let (latest, next) = messages(&path, "session", None)?;
        assert_eq!(latest.len(), 32);
        let (older, end) = messages(&path, "session", next.as_deref())?;
        assert_eq!(older.len(), 8);
        assert!(end.is_none());
        let found: Vec<_> = older
            .into_iter()
            .chain(latest)
            .map(|message| {
                let SessionItem::Text { text } = &message.items[0] else {
                    panic!("wrong item");
                };
                text.clone()
            })
            .collect();
        assert_eq!(
            found,
            (0..40)
                .map(|index| format!("message-{index}"))
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn recorded_transcript_symlink_is_refused() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let target = directory.path().join("target");
        std::fs::write(&target, b"private host contents")?;
        let link = directory.path().join("session.jsonl");
        std::os::unix::fs::symlink(target, &link)?;
        assert!(messages(&link, "session", None).is_err());
        Ok(())
    }

    #[test]
    fn tail_keeps_latest_lines_and_counts_records_past_display_limit() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("session.jsonl");
        let mut entries = [
            serde_json::json!({"message": {"role": "user", "content": "an earlier prompt"}}),
            serde_json::json!({"message": {"role": "assistant", "content": [
                {"type": "text", "text": "Checking.\n\nRunning the tests now."},
                {"type": "toolCall", "name": "bash", "arguments": {"command": "cargo test -p x"}},
            ]}}),
            serde_json::json!({"message": {"role": "toolResult", "toolName": "bash", "content": [
                {"type": "text", "text": "running 3 tests\n\u{1b}[32mtest result: ok. 3 passed\n\n"},
            ]}}),
            serde_json::json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "name": "Read", "input": {"file_path": "src/lib.rs"}},
            ]}}),
            serde_json::json!({"message": {"role": "toolResult", "isError": true, "content": [
                {"type": "text", "text": "\u{1b}[31mENOENT: no such file\n"},
            ]}}),
        ];
        let now: jiff::Timestamp = "2026-11-16T12:20:45Z".parse()?;
        for entry in &mut entries {
            entry["timestamp"] = serde_json::json!("2026-11-16T12:20:00Z");
        }
        // This event lies before the four visible lines, but is the newest timestamp.
        entries[0]["timestamp"] = serde_json::json!("2026-11-16T12:20:40Z");
        entries[2].as_object_mut().unwrap().remove("timestamp");
        entries[2]["message"]["timestamp"] = serde_json::json!(now.as_millisecond() - 10_000);
        let mut text: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
        // An entry still being written has no newline yet and is not shown.
        text.push_str("{\"message\":{\"role\":\"assistant\",\"content\":\"half");
        std::fs::write(&path, text)?;
        let output = tail(&path, now)?;
        assert_eq!(output.last_output_at, Some("2026-11-16T12:20:40Z".into()));
        assert_eq!(output.activity.unwrap().per_minute[19], 5);
        assert_eq!(
            output.lines,
            [
                "Running the tests now.",
                "bash cargo test -p x",
                "Read src/lib.rs",
                "ENOENT: no such file",
            ]
        );
        Ok(())
    }
}
