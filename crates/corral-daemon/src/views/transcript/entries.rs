//! Project durable Pi entries, not the compacted model context or transient terminal rows.

use anyhow::Result;
use corral_protocol::{
    detail::{InputFacts, SessionItem, SessionMessage, SessionToolState},
    session_state::InputOrigin,
};
use serde_json::{Value, json};

use super::{json_len, tool_call_id};
use crate::{assistant, sessions::valid_input_id, views::fingerprint};

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or_default()
}

fn pretty(value: &Value) -> String {
    format!("{value:#}")
}

fn text_blocks(value: &Value) -> impl Iterator<Item = &str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
}

fn content(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.into();
    }
    let mut parts = text_blocks(value);
    let mut joined = parts.next().unwrap_or_default().to_owned();
    for part in parts {
        joined.push('\n');
        joined.push_str(part);
    }
    joined
}

fn card(title: &str, value: &Value) -> SessionItem {
    SessionItem::Card {
        title: title.into(),
        text: content(&value["content"]),
        data: Some(pretty(value)),
    }
}

fn image(block: &Value) -> SessionItem {
    let source = block.get("source").unwrap_or(block);
    let data = field(source, "data");
    let media_type = source
        .get("mimeType")
        .or_else(|| source.get("media_type"))
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream");
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
        media_type: media_type.into(),
        size,
        encoding: "base64".into(),
        data: data.into(),
    }
}

fn images(content: &Value) -> impl Iterator<Item = SessionItem> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| field(block, "type") == "image")
        .map(image)
}

fn tool(block: &Value, result: bool, stop_reason: &str) -> SessionItem {
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
    .and_then(|summary| summary.lines().next())
    .unwrap_or_default();
    let state = if result {
        let failed = block
            .get("isError")
            .or_else(|| block.get("is_error"))
            .and_then(Value::as_bool)
            .unwrap_or_default();
        if failed {
            SessionToolState::Failed
        } else {
            SessionToolState::Done
        }
    } else {
        match stop_reason {
            "aborted" => SessionToolState::NotRun,
            "error" => SessionToolState::Failed,
            _ => SessionToolState::Running,
        }
    };
    SessionItem::ToolCall {
        call_id: tool_call_id(
            block
                .get("toolCallId")
                .or_else(|| block.get("tool_use_id"))
                .or_else(|| block.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .into_owned(),
        name: block
            .get("toolName")
            .or_else(|| block.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        arguments: if result { String::new() } else { pretty(args) },
        summary: summary.into(),
        state,
        paired: false,
        call: (!result).then(|| pretty(block)),
        output: result.then(|| content(&block["content"])),
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
        content: string(args, "content"),
    }
}

pub(super) fn merge(call: &mut SessionItem, result: SessionItem) {
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
        *state = result_state;
        *paired = true;
        *output = result_output;
        *raw = result_raw;
        *elapsed_ms = result_elapsed.or(*elapsed_ms);
        if result_diff.is_some() {
            *diff = result_diff;
        }
    }
}

fn compaction(value: &Value) -> SessionItem {
    SessionItem::Compaction {
        summary: field(value, "summary").into(),
        tokens_before: value["tokensBefore"].as_u64().unwrap_or_default(),
    }
}

fn branch_summary(value: &Value) -> SessionItem {
    SessionItem::BranchSummary {
        summary: field(value, "summary").into(),
        from_id: string(value, "fromId"),
    }
}

fn tool_result(items: &mut Vec<SessionItem>, value: &Value) {
    items.push(tool(value, true, ""));
    items.extend(images(&value["content"]));
}

fn host_notice(parts: &Value) -> Option<String> {
    let first = parts.as_str().or_else(|| text_blocks(parts).next())?;
    let general = [
        "[corral ",
        "[corral_",
        "[corral] ",
        "[command ",
        "NEW REVIEW ROUND ",
    ]
    .into_iter()
    .any(|prefix| first.starts_with(prefix));
    let relink = first.starts_with("You now work in ");
    let kickoff = first.starts_with("Read ");
    if !(general || relink || kickoff) {
        return None;
    }
    let text = content(parts);
    if text.ends_with(']') && text.contains("\n\n[owner input ") {
        return None;
    }
    (general
        || (relink && text.ends_with(". Continue the task from this conversation."))
        || (kickoff
            && text.contains(" in full and follow it: that file is your complete ")
            && (text.contains(" for corral task ") || text.ends_with("prompt for this session."))))
    .then_some(text)
}

fn conversation(items: &mut Vec<SessionItem>, body: &Value, role: &str) {
    let parts = &body["content"];
    let notice = if role == "user" {
        host_notice(parts)
    } else {
        None
    };
    let keep_text = notice.is_none();
    if let Some(text) = notice {
        items.push(SessionItem::Card {
            title: "Host notice".into(),
            text,
            data: None,
        });
    }
    if keep_text && let Some(text) = parts.as_str() {
        items.push(SessionItem::Text { text: text.into() });
    }
    let stop_reason = field(body, "stopReason");
    if let Some(blocks) = parts.as_array() {
        for block in blocks {
            match field(block, "type") {
                "text" | "input_text" | "output_text" if keep_text => {
                    items.push(SessionItem::Text {
                        text: field(block, "text").into(),
                    })
                }
                "thinking" => items.push(SessionItem::Thinking {
                    text: field(block, "thinking").into(),
                }),
                "image" => items.push(image(block)),
                "toolCall" | "tool_use" => items.push(tool(block, false, stop_reason)),
                "tool_result" => tool_result(items, block),
                _ => {}
            }
        }
    }
    let failure = |fallback: &str| {
        string(body, "errorMessage")
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| fallback.into())
    };
    let facts = || body.get("providerFailure").map(pretty);
    match stop_reason {
        "aborted" => items.push(SessionItem::Aborted {
            message: failure("Operation aborted"),
            facts: facts(),
        }),
        "error" => items.push(SessionItem::ProviderError {
            message: failure("Unknown error"),
            facts: facts(),
        }),
        "length" => items.push(SessionItem::Card {
            title: "Reply cut".into(),
            text: "Response was truncated before completion.".into(),
            data: None,
        }),
        _ => {}
    }
}

fn input_facts(body: &Value) -> Option<InputFacts> {
    let id = body["corralRequestId"]
        .as_str()
        .filter(|id| valid_input_id(id))
        .map(str::to_owned);
    let recorded = &body["corralInput"];
    let origin = match recorded["origin"].as_str() {
        Some("owner") => InputOrigin::Owner,
        Some("auto") => InputOrigin::Auto,
        Some("pane") => InputOrigin::Pane,
        Some(_) => InputOrigin::Other,
        None if id.is_some() => InputOrigin::Owner,
        None => return None,
    };
    let bounded = |name: &str, limit| {
        recorded[name]
            .as_str()
            .map(|value| value[..utf8_end(value, limit)].to_owned())
    };
    Some(InputFacts {
        id,
        origin,
        typed_at: bounded("typed_at", 64),
        device: bounded("device", 64),
        to: bounded("to", 128),
    })
}

pub(super) fn message(
    mut value: Value,
    identity: Option<(&str, u64)>,
) -> Result<Option<SessionMessage>> {
    // The follower reads this journal annotation separately. It is not reply content
    // and must not appear in a Turn footer (or cause one on its own).
    if let Some(body) = value.get_mut("message").and_then(Value::as_object_mut)
        && body.get("role").and_then(Value::as_str) == Some("assistant")
    {
        body.remove(super::tail::REPLY_MARKER);
    }
    let entry = field(&value, "type");
    let nested = value.get("message").is_some();
    let body = value.get("message").unwrap_or(&value);
    let role = body["role"].as_str().unwrap_or(match entry {
        "user" => "user",
        "assistant" => "assistant",
        "tool_result" | "toolResult" => "toolResult",
        _ => "system",
    });
    let mut items = Vec::new();
    match entry {
        "session" | "context_edit" | "session_info" => return Ok(None),
        "custom" if role == "system" => {
            let kind = field(&value, "customType");
            if !matches!(
                kind,
                "corral_mark"
                    | "corral_code_cell"
                    | "corral_code_call"
                    | "corral_owner_question"
                    | "corral_command"
                    | "corral_command_monitor"
                    | "corral_job_control"
            ) {
                return Ok(None);
            }
            items.push(card(kind, &value["data"]));
        }
        "compaction" => items.push(compaction(&value)),
        "branch_summary" => items.push(branch_summary(&value)),
        "model_change" => items.push(SessionItem::ModelChange {
            provider: field(&value, "provider").into(),
            model: field(&value, "modelId").into(),
        }),
        "thinking_level_change" => items.push(SessionItem::ThinkingLevelChange {
            level: field(&value, "thinkingLevel").into(),
        }),
        "label" => items.push(SessionItem::Label {
            target_id: field(&value, "targetId").into(),
            label: string(&value, "label"),
        }),
        "usage" if field(&value, "kind") == "cache_warm" => {
            items.push(card("Cache warming", &value))
        }
        "usage" => return Ok(None),
        _ => match role {
            "toolResult" | "tool_result" => tool_result(&mut items, body),
            "bashExecution" => items.push(SessionItem::BashExecution {
                command: field(body, "command").into(),
                output: field(body, "output").into(),
                exit_code: body["exitCode"].as_i64(),
                cancelled: flag(body, "cancelled"),
                truncated: flag(body, "truncated"),
                exclude_from_context: flag(body, "excludeFromContext"),
                full_output_path: string(body, "fullOutputPath"),
            }),
            "compactionSummary" => items.push(compaction(body)),
            "branchSummary" => items.push(branch_summary(body)),
            "custom" | "system" if entry == "custom_message" || role == "custom" => {
                if !flag(body, "display") {
                    return Ok(None);
                }
                items.push(card(field(body, "customType"), body));
                items.extend(images(&body["content"]));
            }
            "user" | "assistant" => conversation(&mut items, body, role),
            _ => return Ok(None),
        },
    }
    if matches!(entry, "compaction" | "branch_summary")
        && let Some(usage) = value.get("usage")
    {
        items.push(card("Summary cost", &json!({"usage": usage})));
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
    let model = string(body, "model");
    let stop_reason = string(body, "stopReason");
    let usage = match body.get("usage") {
        Some(usage) if json_len(usage)? <= 1024 => Some(usage.clone()),
        _ => None,
    };
    let footer = role == "assistant"
        && body.as_object().is_some_and(|body| {
            body.keys()
                .any(|key| !matches!(key.as_str(), "type" | "role" | "timestamp" | "content"))
        });
    let input = if identity.is_some() && role == "user" {
        input_facts(body)
    } else {
        None
    };
    let id = if let Some((session, start)) = identity {
        if let Some(id) = input.as_ref().and_then(|input| input.id.as_deref()) {
            assistant::input_entry_id(session, id)
        } else if let Some(id) = value["id"].as_str().filter(|id| valid_input_id(id)) {
            format!("{session}/entry/{id}")
        } else {
            // The offset distinguishes identical legacy entries without a history walk,
            // and is constant across page splits and transcript appends.
            let hash = fingerprint(&(entry, role, &at, &model, &stop_reason))?;
            format!("{session}/replay/{hash}/{start}")
        }
    } else {
        String::new()
    };
    let attachments = if identity.is_some()
        && role == "user"
        && input
            .as_ref()
            .is_none_or(|facts| facts.origin == InputOrigin::Owner)
    {
        items
            .iter()
            .filter_map(|item| match item {
                SessionItem::Text { text } => Some(assistant::attachments::facts(text)),
                _ => None,
            })
            .find(|facts| !facts.is_empty())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let author = role.into();
    if footer {
        // Reuse the decoded entry: its content is already projected into items.
        let body = if nested {
            &mut value["message"]
        } else {
            &mut value
        };
        if let Some(object) = body.as_object_mut() {
            object.remove("content");
        }
        items.push(card("Turn footer", body));
    }
    Ok((!items.is_empty()).then_some(SessionMessage {
        id,
        input,
        attachments,
        author,
        at,
        items,
        model,
        stop_reason,
        usage,
    }))
}

fn utf8_end(text: &str, limit: usize) -> usize {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn clip(text: &mut String, limit: usize) {
    if text.len() > limit {
        *text = text[..utf8_end(text, limit)].into();
    }
}

pub(super) fn bound_header(message: &mut SessionMessage) {
    clip(&mut message.author, 80);
    for (text, limit) in [
        (&mut message.at, 80),
        (&mut message.model, 128),
        (&mut message.stop_reason, 32),
    ] {
        if let Some(text) = text {
            clip(text, limit);
        }
    }
    for attachment in &mut message.attachments {
        clip(
            &mut attachment.name,
            corral_protocol::attachment::ATTACHMENT_NAME_MAX,
        );
        if let Some(media_type) = &mut attachment.media_type {
            clip(media_type, 128);
        }
        if let Some(url) = &mut attachment.url {
            clip(url, corral_protocol::attachment::ATTACHMENT_URL_MAX);
        }
    }
}

pub(super) fn clip_item(item: &mut SessionItem, limit: usize) {
    let clip = |text: &mut String| clip(text, limit);
    let clip_optional = |text: &mut Option<String>| {
        if let Some(text) = text {
            clip(text);
        }
    };
    match item {
        SessionItem::Text { text } | SessionItem::Thinking { text } => clip(text),
        SessionItem::ToolCall {
            name,
            arguments,
            summary,
            call,
            output,
            result,
            diff,
            content,
            ..
        } => {
            for field in [name, arguments, summary] {
                clip(field);
            }
            for field in [call, output, result, diff, content] {
                clip_optional(field);
            }
        }
        SessionItem::BashExecution {
            command,
            output,
            full_output_path,
            ..
        } => {
            clip(command);
            clip(output);
            clip_optional(full_output_path);
        }
        SessionItem::Image {
            media_type, data, ..
        } => {
            clip(media_type);
            clip(data);
        }
        SessionItem::Compaction { summary, .. } => clip(summary),
        SessionItem::BranchSummary { summary, from_id } => {
            clip(summary);
            clip_optional(from_id);
        }
        SessionItem::ModelChange { provider, model } => {
            clip(provider);
            clip(model);
        }
        SessionItem::ThinkingLevelChange { level } => clip(level),
        SessionItem::Label { target_id, label } => {
            clip(target_id);
            clip_optional(label);
        }
        SessionItem::Aborted { message, facts } | SessionItem::ProviderError { message, facts } => {
            clip(message);
            clip_optional(facts);
        }
        SessionItem::Card { title, text, data } => {
            clip(title);
            clip(text);
            clip_optional(data);
        }
        // Replay never emits these variants as source items.
        SessionItem::QuestionCard { .. }
        | SessionItem::Continuation { .. }
        | SessionItem::Other => {}
    }
}
