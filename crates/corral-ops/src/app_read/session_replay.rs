//! Pi/foreign transcript semantics, paired tool rows and authenticated item continuations.

use anyhow::{Context, Result, ensure};
use corral_protocol::assistant::{EntryCard, EntryKind};
use corral_protocol::detail::{SessionItem, SessionItemPage, SessionMessage, SessionToolState};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::short;
pub(super) use super::transcript::{Source, decode};
use super::transcript::{Token, digest, encode, hex};

pub(super) const PAGE_BYTES: usize = 28 * 1024;
const ITEM_BYTES: usize = 2048;
const FRAGMENT_BYTES: usize = 4096;

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}
fn pretty(value: &Value) -> String {
    format!("{value:#}")
}
fn content(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    value
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
fn card(title: &str, value: &Value) -> SessionItem {
    SessionItem::Card {
        title: title.into(),
        text: content(value.get("content").unwrap_or(&Value::Null)),
        data: Some(pretty(value)),
    }
}
fn image(block: &Value) -> SessionItem {
    let source = block.get("source").unwrap_or(block);
    let data = text(source, "data");
    let media_type = source
        .get("mimeType")
        .or_else(|| source.get("media_type"))
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream")
        .into();
    let encoded = data
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .count();
    let padding = data
        .bytes()
        .rev()
        .filter(|byte| !byte.is_ascii_whitespace())
        .take_while(|byte| *byte == b'=')
        .count();
    let size = (encoded / 4 * 3 + encoded % 4 * 3 / 4).saturating_sub(padding) as u64;
    SessionItem::Image {
        media_type,
        size,
        encoding: "base64".into(),
        data,
    }
}
fn tool(block: &Value, result: bool, stopped: bool) -> SessionItem {
    let args = block
        .get("arguments")
        .or_else(|| block.get("input"))
        .unwrap_or(&Value::Null);
    let summary = [
        "command",
        "path",
        "file_path",
        "pattern",
        "query",
        "url",
        "prompt",
        "question",
        "job",
        "content_id",
        "document",
        "server",
        "uri",
        "fact",
        "id",
        "text",
        "title",
        "summary",
    ]
    .into_iter()
    .find_map(|key| args.get(key).and_then(Value::as_str))
    .unwrap_or("")
    .lines()
    .next()
    .unwrap_or("")
    .to_owned();
    let failed = block
        .get("isError")
        .or_else(|| block.get("is_error"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    SessionItem::ToolCall {
        call_id: block
            .get("toolCallId")
            .or_else(|| block.get("tool_use_id"))
            .or_else(|| block.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        name: block
            .get("toolName")
            .or_else(|| block.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
        arguments: if result { String::new() } else { pretty(args) },
        summary,
        state: if result {
            if failed {
                SessionToolState::Failed
            } else {
                SessionToolState::Done
            }
        } else if stopped {
            SessionToolState::NotRun
        } else {
            SessionToolState::Running
        },
        paired: false,
        call: (!result).then(|| pretty(block)),
        output: result.then(|| content(block.get("content").unwrap_or(&Value::Null))),
        result: result.then(|| pretty(block)),
        elapsed_ms: block
            .get("durationMs")
            .or_else(|| block.get("elapsed_ms"))
            .and_then(Value::as_u64),
        diff: block
            .pointer("/details/diff")
            .or_else(|| block.pointer("/details/patch"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        content: args
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
}

pub(super) fn message(value: &Value) -> Option<SessionMessage> {
    let body = value.get("message").unwrap_or(value);
    let entry_type = text(value, "type");
    let role = body
        .get("role")
        .and_then(Value::as_str)
        .or(match entry_type.as_str() {
            "user" => Some("user"),
            "assistant" => Some("assistant"),
            _ => None,
        })
        .unwrap_or("system");
    let mut items = Vec::new();
    match entry_type.as_str() {
        "session" | "context_edit" | "session_info" => return None,
        "custom" if role == "system" => {
            let kind = text(value, "customType");
            if !matches!(
                kind.as_str(),
                "corral_mark"
                    | "corral_code_cell"
                    | "corral_code_call"
                    | "corral_owner_question"
                    | "corral_command"
                    | "corral_command_monitor"
                    | "corral_job_control"
            ) {
                return None;
            }
            items.push(card(&kind, value.get("data").unwrap_or(&Value::Null)));
        }
        "compaction" => items.push(SessionItem::Compaction {
            summary: text(value, "summary"),
            tokens_before: value
                .get("tokensBefore")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        }),
        "branch_summary" => items.push(SessionItem::BranchSummary {
            summary: text(value, "summary"),
            from_id: value
                .get("fromId")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "model_change" => items.push(SessionItem::ModelChange {
            provider: text(value, "provider"),
            model: text(value, "modelId"),
        }),
        "thinking_level_change" => items.push(SessionItem::ThinkingLevelChange {
            level: text(value, "thinkingLevel"),
        }),
        "label" => items.push(SessionItem::Label {
            target_id: text(value, "targetId"),
            label: value
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "usage" => {
            if text(value, "kind") != "cache_warm" {
                return None;
            }
            items.push(card("Cache warming", value));
        }
        _ => match role {
            "toolResult" => {
                items.push(tool(body, true, false));
                if let Some(parts) = body.get("content").and_then(Value::as_array) {
                    items.extend(
                        parts
                            .iter()
                            .filter(|part| text(part, "type") == "image")
                            .map(image),
                    );
                }
            }
            "bashExecution" => items.push(SessionItem::BashExecution {
                command: text(body, "command"),
                output: text(body, "output"),
                exit_code: body.get("exitCode").and_then(Value::as_i64),
                cancelled: body
                    .get("cancelled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                truncated: body
                    .get("truncated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                exclude_from_context: body
                    .get("excludeFromContext")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                full_output_path: body
                    .get("fullOutputPath")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }),
            "compactionSummary" => items.push(SessionItem::Compaction {
                summary: text(body, "summary"),
                tokens_before: body
                    .get("tokensBefore")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            }),
            "branchSummary" => items.push(SessionItem::BranchSummary {
                summary: text(body, "summary"),
                from_id: body
                    .get("fromId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }),
            "custom" | "system" if entry_type == "custom_message" || role == "custom" => {
                if body.get("display").and_then(Value::as_bool) != Some(true) {
                    return None;
                }
                items.push(card(&text(body, "customType"), body));
                if let Some(parts) = body.get("content").and_then(Value::as_array) {
                    items.extend(
                        parts
                            .iter()
                            .filter(|part| text(part, "type") == "image")
                            .map(image),
                    );
                }
            }
            "user" | "assistant" => {
                let stopped = text(body, "stopReason") == "aborted";
                if let Some(string) = body.get("content").and_then(Value::as_str) {
                    items.push(SessionItem::Text {
                        text: string.into(),
                    });
                }
                if let Some(parts) = body.get("content").and_then(Value::as_array) {
                    for block in parts {
                        match text(block, "type").as_str() {
                            "text" | "input_text" | "output_text" => {
                                items.push(SessionItem::Text {
                                    text: text(block, "text"),
                                })
                            }
                            "thinking" => items.push(SessionItem::Thinking {
                                text: text(block, "thinking"),
                            }),
                            "image" => items.push(image(block)),
                            "toolCall" | "tool_use" => {
                                let mut call = tool(block, false, stopped);
                                if text(body, "stopReason") == "error"
                                    && let SessionItem::ToolCall { state, .. } = &mut call
                                {
                                    *state = SessionToolState::Failed;
                                }
                                items.push(call);
                            }
                            "tool_result" => {
                                items.push(tool(block, true, false));
                                if let Some(parts) = block.get("content").and_then(Value::as_array)
                                {
                                    items.extend(
                                        parts
                                            .iter()
                                            .filter(|part| text(part, "type") == "image")
                                            .map(image),
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                }
                if role == "user" {
                    let notice = content(body.get("content").unwrap_or(&Value::Null));
                    if host_input(&notice) {
                        items.retain(|item| !matches!(item, SessionItem::Text { .. }));
                        items.insert(
                            0,
                            SessionItem::Card {
                                title: "Host notice".into(),
                                text: notice,
                                data: None,
                            },
                        );
                    }
                }
                let failure = text(body, "errorMessage");
                let facts = body.get("providerFailure").map(pretty);
                match text(body, "stopReason").as_str() {
                    "aborted" => items.push(SessionItem::Aborted {
                        message: if failure.is_empty() {
                            "Operation aborted".into()
                        } else {
                            failure
                        },
                        facts,
                    }),
                    "error" => items.push(SessionItem::ProviderError {
                        message: if failure.is_empty() {
                            "Unknown error".into()
                        } else {
                            failure
                        },
                        facts,
                    }),
                    "length" => items.push(SessionItem::Card {
                        title: "Reply cut".into(),
                        text: "Response was truncated before completion.".into(),
                        data: None,
                    }),
                    _ => {}
                }
            }
            _ => return None,
        },
    }
    // Compaction's cost is a distinct TUI row. Keep all provider usage fields as recorded.
    if matches!(entry_type.as_str(), "compaction" | "branch_summary")
        && value.get("usage").is_some()
    {
        items.push(card("Summary cost", &json!({"usage": value["usage"]})));
    }
    let model = body.get("model").and_then(Value::as_str).map(str::to_owned);
    let stop_reason = body
        .get("stopReason")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let usage = body.get("usage").cloned();
    if role == "assistant"
        && (model.is_some()
            || stop_reason.is_some()
            || usage.is_some()
            || body.get("diagnostics").is_some())
    {
        let mut metadata = body.clone();
        if let Some(object) = metadata.as_object_mut() {
            object.remove("content");
        }
        items.push(card("Turn footer", &metadata));
    }
    let timestamp = value.get("timestamp").or_else(|| body.get("timestamp"));
    let at = timestamp
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            timestamp
                .and_then(Value::as_i64)
                .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
                .map(|time| time.to_string())
        });
    (!items.is_empty()).then(|| SessionMessage {
        author: role.into(),
        at,
        items,
        model,
        stop_reason,
        usage,
    })
}

/// A selected item keeps its original index even when its result is folded into an earlier call.
pub(super) struct Item {
    pub value: SessionItem,
    pub source: Source,
    pub pair: Option<Source>,
    pub hidden: bool,
}
pub(super) fn merge(call: &mut SessionItem, result: &SessionItem) {
    if let (
        SessionItem::ToolCall {
            state,
            paired,
            output,
            result: raw,
            elapsed_ms,
            diff,
            ..
        },
        SessionItem::ToolCall {
            state: result_state,
            output: result_output,
            result: result_raw,
            elapsed_ms: result_elapsed,
            diff: result_diff,
            ..
        },
    ) = (call, result)
    {
        *state = *result_state;
        *paired = true;
        output.clone_from(result_output);
        raw.clone_from(result_raw);
        *elapsed_ms = *result_elapsed;
        diff.clone_from(result_diff);
    }
}
pub(super) fn pair(messages: &mut [(SessionMessage, Vec<Item>)]) {
    let mut calls = std::collections::BTreeMap::new();
    let mut matches = Vec::new();
    for (message_index, (_, items)) in messages.iter().enumerate() {
        for (index, item) in items.iter().enumerate() {
            if let SessionItem::ToolCall {
                call_id, result, ..
            } = &item.value
            {
                if call_id.is_empty() {
                    continue;
                }
                if result.is_none() {
                    calls.insert(call_id.clone(), (message_index, index));
                } else if let Some(call) = calls.remove(call_id) {
                    matches.push((call, (message_index, index)));
                }
            }
        }
    }
    for ((message, item), (result_message, result_item)) in matches {
        let result = messages[result_message].1[result_item].value.clone();
        let source = messages[result_message].1[result_item].source.clone();
        merge(&mut messages[message].1[item].value, &result);
        messages[message].1[item].pair = Some(source);
        messages[result_message].1[result_item].hidden = true;
    }
}
fn clip(value: &mut Value, maximum: usize) {
    match value {
        Value::String(text) => *text = short(text, maximum),
        Value::Array(items) => {
            items.truncate(1);
            for item in items {
                clip(item, maximum);
            }
        }
        Value::Object(object) => {
            for (key, value) in object {
                if !matches!(key.as_str(), "kind" | "state" | "encoding") {
                    clip(value, maximum);
                }
            }
        }
        _ => {}
    }
}
pub(super) fn bounded(id: &str, item: Item) -> Result<Vec<SessionItem>> {
    if item.hidden {
        return Ok(Vec::new());
    }
    let bytes = serde_json::to_vec(&item.value).context("encoding replay item")?;
    if bytes.len() <= ITEM_BYTES {
        return Ok(vec![item.value]);
    }
    let mut preview = serde_json::to_value(&item.value).context("encoding replay preview")?;
    // Every new replay variant has a fixed number of scalar fields. Shrink against actual escaped JSON bytes.
    for maximum in [256, 128, 64, 16, 0] {
        clip(&mut preview, maximum);
        if serde_json::to_vec(&preview)?.len() <= ITEM_BYTES {
            break;
        }
    }
    let preview = serde_json::from_value(preview).context("decoding replay preview")?;
    let token = Token {
        source: item.source,
        pair: item.pair,
        offset: 0,
        digest: hex(&Sha256::digest(&bytes)),
    };
    Ok(vec![
        preview,
        SessionItem::Continuation {
            token: encode(id, &token)?,
            bytes: bytes.len() as u64,
        },
    ])
}
pub(super) fn header(message: &mut SessionMessage) {
    message.author = short(&message.author, 80);
    message.at = message.at.as_ref().map(|at| short(at, 80));
    message.model = message.model.as_ref().map(|model| short(model, 128));
    message.stop_reason = message.stop_reason.as_ref().map(|reason| short(reason, 32));
    if message
        .usage
        .as_ref()
        .is_some_and(|usage| usage.to_string().len() > 1024)
    {
        message.usage = None;
    }
}
pub(super) fn fragment(id: &str, mut token: Token, item: SessionItem) -> Result<SessionItemPage> {
    ensure!(
        digest(&item)? == token.digest,
        "transcript item changed; reload the session"
    );
    let json = serde_json::to_string(&item).context("encoding complete replay item")?;
    ensure!(
        token.offset < json.len() && json.is_char_boundary(token.offset),
        "invalid replay continuation offset"
    );
    let start = token.offset;
    let mut end = (start + FRAGMENT_BYTES).min(json.len());
    while !json.is_char_boundary(end) {
        end -= 1;
    }
    token.offset = end;
    Ok(SessionItemPage {
        json: json[start..end].into(),
        offset: start as u64,
        bytes: json.len() as u64,
        next: if end < json.len() {
            Some(encode(id, &token)?)
        } else {
            None
        },
    })
}

pub(super) fn assistant(
    ctx: &crate::Ctx,
    id: &str,
    path: Option<&std::path::Path>,
    page: Option<&str>,
    map: fn(corral_protocol::assistant::ThreadEntry) -> SessionMessage,
) -> Result<(Vec<SessionMessage>, Option<String>)> {
    let (before, skip, pi, pi_skip, branch) = if let Some(page) = page {
        let parts: Vec<_> = page.split(':').collect();
        ensure!(
            parts.len() == 6 && parts[0] == "assistant",
            "invalid assistant replay cursor"
        );
        let number = |value: &str| -> Result<Option<u64>> {
            if value == "-" {
                Ok(None)
            } else {
                Ok(Some(
                    value.parse().context("invalid assistant replay position")?,
                ))
            }
        };
        (
            number(parts[1])?,
            parts[2]
                .parse::<usize>()
                .context("invalid assistant item position")?,
            number(parts[3])?,
            parts[4]
                .parse::<usize>()
                .context("invalid Pi item position")?,
            parts[5].to_owned(),
        )
    } else {
        let head = ctx.store.app_thread_head(id)?;
        let end = path
            .map(|path| {
                super::transcript::open(path)?
                    .metadata()
                    .context("reading assistant transcript size")
                    .map(|meta| meta.len())
            })
            .transpose()?;
        (Some(head.next.saturating_mul(1024)), 0, end, 0, "*".into())
    };
    let mut messages = Vec::new();
    let mut thread_next = None;
    let mut thread_skip = 0;
    if let Some(before) = before {
        let (entries, next, _) = crate::agent::app_runtime::thread(ctx, id, Some(before), 1)?;
        if let Some(entry) = entries.into_iter().next() {
            let mut message = map(entry);
            let items = std::mem::take(&mut message.items);
            ensure!(skip <= items.len(), "assistant item cursor is stale");
            header(&mut message);
            let mut size = super::super::paging::json_len(&message)?;
            let count = items.len();
            let mut consumed = skip;
            for (index, value) in items.into_iter().enumerate().skip(skip) {
                if path.is_some() && matches!(&value, SessionItem::ToolCall { .. }) {
                    consumed += 1;
                    continue;
                }
                let bounded = bounded(
                    id,
                    Item {
                        value,
                        source: Source::Thread {
                            before: Some(before),
                            index,
                        },
                        pair: None,
                        hidden: false,
                    },
                )?;
                let cost = super::super::paging::json_len(&bounded)?;
                if size + cost > 12 * 1024 && !message.items.is_empty() {
                    break;
                }
                size += cost;
                message.items.extend(bounded);
                consumed += 1;
            }
            if consumed < count {
                thread_next = Some(before);
                thread_skip = consumed;
            } else {
                thread_next = next;
            }
            if !message.items.is_empty() {
                messages.push(message);
            }
        }
    }
    let mut pi_next = None;
    let mut pi_next_skip = 0;
    let mut pi_branch = branch;
    if let (Some(path), Some(end)) = (path, pi) {
        let cursor = format!("transcript:{end}:{pi_skip}:{pi_branch}");
        let (supplement, next) =
            super::transcript::messages_with_words(path, id, Some(&cursor), false, 12 * 1024)?;
        messages.extend(supplement);
        if let Some(next) = next {
            (pi_next, pi_next_skip, pi_branch) = super::transcript::position(Some(&next))?;
        }
    }
    messages.sort_by(|left, right| left.at.cmp(&right.at));
    let older = if thread_next.is_some() || pi_next.is_some() {
        Some(format!(
            "assistant:{}:{thread_skip}:{}:{pi_next_skip}:{pi_branch}",
            thread_next
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            pi_next
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into())
        ))
    } else {
        None
    };
    Ok((messages, older))
}

#[cfg(test)]
#[path = "session_replay_tests.rs"]
mod tests;
pub(super) fn entry_message(entry: corral_protocol::assistant::ThreadEntry) -> SessionMessage {
    let author = match entry.kind {
        EntryKind::Message | EntryKind::PmMessage | EntryKind::Idea => "owner",
        EntryKind::PmReply => "pm",
        EntryKind::Answer => "assistant",
        _ => "system",
    }
    .into();
    let mut items = Vec::new();
    if let Some(text) = entry.text {
        items.push(SessionItem::Text { text });
    }
    for (index, tool) in entry.tools.into_iter().enumerate() {
        let state = match (tool.failed, tool.output.is_some()) {
            (Some(true), _) => corral_protocol::detail::SessionToolState::Failed,
            (_, true) => corral_protocol::detail::SessionToolState::Done,
            _ => corral_protocol::detail::SessionToolState::Running,
        };
        let arguments = serde_json::from_str::<serde_json::Value>(&tool.input)
            .map(|value| format!("{value:#}"))
            .unwrap_or(tool.input);
        items.push(SessionItem::ToolCall {
            call_id: format!("{}/tool/{index}", entry.id),
            name: tool.tool,
            summary: arguments.lines().next().unwrap_or("").into(),
            arguments,
            state,
            paired: tool.output.is_some(),
            call: None,
            output: tool.output,
            result: None,
            elapsed_ms: tool.elapsed_ms,
            diff: None,
            content: None,
        });
    }
    if let Some(card) = entry.card {
        match card {
            EntryCard::Question {
                key,
                rev,
                options,
                answer,
                ..
            } => items.push(SessionItem::QuestionCard {
                key,
                rev,
                text: String::new(),
                options,
                answer,
            }),
            other => items.push(SessionItem::Card {
                title: "Owner card".into(),
                text: String::new(),
                data: Some(format!("{}", serde_json::json!(other))),
            }),
        }
    }
    if !entry.attachments.is_empty() || !entry.links.is_empty() {
        items.push(SessionItem::Card {
            title: "Attachments".into(),
            text: String::new(),
            data: Some(format!(
                "{}",
                serde_json::json!({ "attachments": entry.attachments, "links": entry.links })
            )),
        });
    }
    if entry.model.is_some() || entry.cost_usd.is_some() {
        items.push(SessionItem::Card {
            title: "Turn footer".into(),
            text: String::new(),
            data: Some(format!(
                "{}",
                serde_json::json!({ "model": entry.model, "cost_usd": entry.cost_usd })
            )),
        });
    }
    SessionMessage {
        author,
        at: Some(entry.at),
        items,
        model: entry.model,
        stop_reason: None,
        usage: entry
            .cost_usd
            .map(|cost| serde_json::json!({ "cost": { "total": cost } })),
    }
}

fn host_input(text: &str) -> bool {
    if text.ends_with(']') && text.contains("\n\n[owner input ") {
        return false;
    }
    text.starts_with("[corral ")
        || text.starts_with("[corral_")
        || text.starts_with("[corral] ")
        || text.starts_with("[command ")
        || text.starts_with("NEW REVIEW ROUND ")
        || (text.starts_with("You now work in ")
            && text.ends_with(". Continue the task from this conversation."))
        || (text.starts_with("Read ")
            && text.contains(" in full and follow it: that file is your complete ")
            && (text.contains(" for corral task ") || text.ends_with("prompt for this session.")))
}
