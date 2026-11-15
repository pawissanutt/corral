//! Bounded readback of trusted, stored session references. Request IDs never become paths.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use corral_protocol::assistant::ToolLine;
use corral_protocol::detail::session::{SessionItem, SessionMessage, TerminalSnapshot};
use serde_json::Value;

use super::short;
use crate::Ctx;

const WINDOW: u64 = 128 * 1024;
const MESSAGES: usize = 32;
const TEXT_BYTES: usize = 32 * 1024;
const ARCHIVE_BYTES: u64 = 8 * 1024 * 1024;
/// Lines a wall tile shows for what a session did last.
const TAIL_LINES: usize = 4;
const TAIL_BYTES: usize = 160;
const LOG_TAIL_WINDOW: u64 = 4 * 1024;

pub(super) fn cursor(page: Option<&str>, kind: &str) -> Result<Option<u64>> {
    page.map(|page| {
        let (prefix, offset) = page
            .split_once(':')
            .context("invalid session page cursor")?;
        ensure!(
            prefix == kind && offset.len() <= 20,
            "invalid session page cursor"
        );
        offset.parse().context("invalid session page position")
    })
    .transpose()
}

fn open(path: &Path) -> Result<File> {
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

/// Reads backwards by bytes, never rescanning the complete transcript or allocating its history.
/// Incomplete JSONL writes remain invisible until their newline is durable.
pub(super) fn messages(
    path: &Path,
    page: Option<&str>,
) -> Result<(Vec<SessionMessage>, Option<String>)> {
    let mut file = open(path)?;
    let len = file.metadata().context("reading transcript size")?.len();
    let end = cursor(page, "transcript")?.unwrap_or(len);
    ensure!(end <= len, "transcript cursor is stale");
    let start = end.saturating_sub(WINDOW);
    file.seek(SeekFrom::Start(start))
        .context("seeking recorded transcript page")?;
    let mut bytes = Vec::new();
    file.take(end - start)
        .read_to_end(&mut bytes)
        .context("reading recorded transcript page")?;
    let first = if start == 0 {
        0
    } else {
        bytes
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(bytes.len())
    };
    let mut offsets = Vec::new();
    let mut position = first;
    for line in bytes[first..].split_inclusive(|b| *b == b'\n') {
        if line.ends_with(b"\n") {
            offsets.push((position, line));
        }
        position += line.len();
    }
    let mut messages = Vec::new();
    let mut size = 0;
    let mut older = start + first as u64;
    for (offset, line) in offsets.into_iter().rev() {
        let value: Value =
            serde_json::from_slice(line).context("decoding recorded transcript entry")?;
        if let Some(message) = message(&value) {
            let cost = crate::app_read::paging::json_len(&message)?;
            if !messages.is_empty() && (messages.len() >= MESSAGES || size + cost > TEXT_BYTES) {
                // Cursor includes the unconsumed record on the next page.
                older = start + offset as u64 + line.len() as u64;
                break;
            }
            size += cost;
            messages.push(message);
        }
        older = start + offset as u64;
    }
    messages.reverse();
    // A single record larger than the read window is unavailable, not silently replaced.
    if first == bytes.len() && !bytes.is_empty() && start > 0 {
        bail!("transcript_record_too_large: recorded entry exceeds bounded page window");
    }
    Ok((messages, (older > 0).then(|| format!("transcript:{older}"))))
}

fn excerpt(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    format!("{}… [excerpt]", short(value, maximum.saturating_sub(14)))
}

fn scalar(value: &Value, max: usize) -> String {
    match value.as_str() {
        Some(text) => excerpt(text, max),
        None => excerpt(&value.to_string(), max),
    }
}

fn message(value: &Value) -> Option<SessionMessage> {
    let body = value.get("message").unwrap_or(value);
    let role = body.get("role").and_then(Value::as_str).or_else(|| {
        match value.get("type").and_then(Value::as_str) {
            Some("user") => Some("user"),
            Some("assistant") => Some("assistant"),
            _ => None,
        }
    })?;
    let at = value
        .get("timestamp")
        .and_then(Value::as_str)
        .or_else(|| body.get("timestamp").and_then(Value::as_str))
        .map(|at| short(at, 80));
    let mut items = Vec::new();
    // Leave room for worst-case six-byte JSON escapes and tool metadata.
    let mut remaining = 4 * 1024;
    if let Some(text) = body.get("content").and_then(Value::as_str) {
        items.push(SessionItem::Text {
            text: excerpt(text, remaining),
        });
    } else if let Some(blocks) = body.get("content").and_then(Value::as_array) {
        for block in blocks.iter().take(16) {
            if remaining == 0 {
                break;
            }
            let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
            let item = match kind {
                "text" | "input_text" | "output_text" => {
                    let text = excerpt(
                        block.get("text").and_then(Value::as_str).unwrap_or(""),
                        remaining.min(8192),
                    );
                    remaining = remaining.saturating_sub(text.len());
                    SessionItem::Text { text }
                }
                "toolCall" | "tool_use" => {
                    let input = scalar(
                        block
                            .get("arguments")
                            .or_else(|| block.get("input"))
                            .unwrap_or(&Value::Null),
                        remaining.min(4096),
                    );
                    remaining = remaining.saturating_sub(input.len());
                    SessionItem::Tool {
                        tool: ToolLine {
                            tool: short(
                                block.get("name").and_then(Value::as_str).unwrap_or(""),
                                80,
                            ),
                            input,
                            output: None,
                            elapsed_ms: None,
                            failed: None,
                        },
                    }
                }
                "tool_result" => {
                    let output = scalar(
                        block.get("content").unwrap_or(&Value::Null),
                        remaining.min(8192),
                    );
                    remaining = remaining.saturating_sub(output.len());
                    SessionItem::Tool {
                        tool: ToolLine {
                            // A tool-use ID is not its name; missing recorded names stay empty.
                            tool: block
                                .get("name")
                                .and_then(Value::as_str)
                                .map(|name| short(name, 80))
                                .unwrap_or_default(),
                            input: String::new(),
                            output: Some(output),
                            elapsed_ms: None,
                            failed: block.get("is_error").and_then(Value::as_bool),
                        },
                    }
                }
                _ => continue,
            };
            items.push(item);
        }
    }
    if role == "toolResult" && !items.is_empty() {
        let output = items
            .into_iter()
            .filter_map(|item| match item {
                SessionItem::Text { text } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        items = vec![SessionItem::Tool {
            tool: ToolLine {
                tool: short(
                    body.get("toolName").and_then(Value::as_str).unwrap_or(""),
                    80,
                ),
                input: String::new(),
                output: Some(output),
                elapsed_ms: None,
                failed: body.get("isError").and_then(Value::as_bool),
            },
        }];
    }
    (!items.is_empty()).then(|| SessionMessage {
        author: short(role, 80),
        at,
        items,
    })
}

/// The complete lines in the last `window` bytes of a recorded file.
fn end_window(path: &Path, window: u64) -> Result<Vec<u8>> {
    let mut file = open(path)?;
    let len = file
        .metadata()
        .context("reading recorded output size")?
        .len();
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
    Ok(bytes)
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

/// What a session did last, oldest line first. One bounded window is read from the end and
/// only the entries the lines come from are decoded; an entry larger than the window, or one
/// still being written, contributes nothing.
pub(super) fn tail(path: &Path) -> Result<Vec<String>> {
    let bytes = end_window(path, WINDOW)?;
    let mut lines = Vec::new();
    for entry in bytes
        .split_inclusive(|b| *b == b'\n')
        .rev()
        .filter(|entry| entry.ends_with(b"\n"))
    {
        let Ok(value) = serde_json::from_slice::<Value>(entry) else {
            continue;
        };
        lines.extend(entry_lines(&value).into_iter().rev());
        if lines.len() >= TAIL_LINES {
            break;
        }
    }
    lines.truncate(TAIL_LINES);
    lines.reverse();
    Ok(lines)
}

/// The last lines a running gate wrote to its registered scratch log.
pub(super) fn gate_tail(ctx: &Ctx, task: &str, run: &str) -> Result<Vec<String>> {
    let path = crate::gate::gate_paths(ctx, task, run).log;
    let bytes = end_window(&path, LOG_TAIL_WINDOW)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text
        .lines()
        .rev()
        .filter_map(tail_line)
        .take(TAIL_LINES)
        .collect();
    lines.reverse();
    Ok(lines)
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

/// Preserve screen columns while bounding escaped rows below the CONTROL reply budget.
pub(super) fn screen(text: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut budget = 40 * 1024 - 2;
    for row in text.rsplit('\n') {
        let clean = if row.chars().any(char::is_control) {
            std::borrow::Cow::Owned(
                row.chars()
                    .filter(|ch| !ch.is_control())
                    .collect::<String>(),
            )
        } else {
            std::borrow::Cow::Borrowed(row)
        };
        let mut row = short(clean.trim_end(), 2048);
        row.truncate(row.trim_end().len());
        if rows.is_empty() && row.is_empty() {
            continue;
        }
        let cost = row
            .chars()
            .map(|ch| match ch {
                '"' | '\\' => 2,
                ch => ch.len_utf8(),
            })
            .sum::<usize>()
            + 3;
        if cost > budget {
            break;
        }
        budget -= cost;
        rows.push(row);
        if rows.len() == 200 {
            break;
        }
    }
    rows.reverse();
    rows
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (latest, next) = messages(&path, None)?;
        assert_eq!(latest.len(), 32);
        let (older, end) = messages(&path, next.as_deref())?;
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
        assert!(messages(&link, None).is_err());
        Ok(())
    }

    #[test]
    fn bounded_text_marks_the_actual_excerpt() {
        let text = excerpt(&"é".repeat(100), 32);
        assert!(text.ends_with("[excerpt]"));
        assert!(text.len() <= 32);
    }

    #[test]
    fn tail_is_the_latest_words_calls_and_failures_in_order() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("session.jsonl");
        let entries = [
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
        let mut text: String = entries.iter().map(|entry| format!("{entry}\n")).collect();
        // An entry still being written has no newline yet and is not shown.
        text.push_str("{\"message\":{\"role\":\"assistant\",\"content\":\"half");
        std::fs::write(&path, text)?;
        assert_eq!(
            tail(&path)?,
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
