//! Closed, complete script-response events, not a general OpenAI or SSE client.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::HashSet;

use super::super::{policy, wire};

pub(super) const MAX_SSE: usize = 262_144;
const MAX_EVENTS: usize = 128;
const MAX_EVENT: usize = 32_768;

#[derive(Default)]
enum Payload {
    #[default]
    Empty,
    Text(String),
    Calls(Vec<Value>),
}

pub(super) fn assistant(mut bytes: &[u8]) -> Result<Value> {
    ensure!(bytes.len() <= MAX_SSE, "fixture SSE body too large");
    let mut response_id: Option<String> = None;
    let mut call_ids = HashSet::new();
    let mut payload = Payload::Empty;
    let mut terminal = false;
    let mut reporting = false;
    for index in 0..MAX_EVENTS {
        let end = bytes[..bytes.len().min(MAX_EVENT)]
            .windows(2)
            .position(|s| s == b"\n\n")
            .context("missing or oversized fixture SSE event")?;
        let data = bytes[..end]
            .strip_prefix(b"data: ")
            .context("invalid fixture SSE field")?;
        bytes = &bytes[end + 2..];
        ensure!(
            !data.iter().any(|b| matches!(b, b'\r' | b'\n')),
            "invalid multiline fixture SSE data"
        );
        if data == b"[DONE]" {
            ensure!(
                terminal && bytes.is_empty(),
                "invalid fixture SSE completion"
            );
            return match payload {
                Payload::Calls(calls) => {
                    Ok(json!({"role":"assistant","content":null,"tool_calls":calls}))
                }
                Payload::Text(content) => Ok(json!({"role":"assistant","content":content})),
                Payload::Empty => anyhow::bail!("empty fixture SSE completion"),
            };
        }
        ensure!(!terminal, "fixture SSE data after terminal event");
        let event: Value = wire::decode(data)?;
        closed(&event, &["id", "object", "created", "model", "choices"])?;
        let id = event["id"]
            .as_str()
            .filter(|id| wire::valid_id(id))
            .context("invalid fixture SSE response id")?;
        ensure!(
            response_id.as_deref().is_none_or(|old| old == id),
            "changed fixture SSE response id"
        );
        response_id.get_or_insert_with(|| id.to_owned());
        ensure!(
            event["object"] == "chat.completion.chunk"
                && event["model"] == policy::MODEL
                && event["created"].is_u64(),
            "invalid fixture SSE metadata"
        );
        let choices = event["choices"]
            .as_array()
            .filter(|choices| choices.len() == 1)
            .context("invalid fixture SSE choices")?;
        let choice = &choices[0];
        closed(choice, &["index", "delta", "finish_reason"])?;
        ensure!(
            choice["index"].as_u64() == Some(0),
            "invalid fixture SSE choice index"
        );
        let delta = &choice["delta"];
        let reason = &choice["finish_reason"];
        if index == 0 {
            ensure!(
                *delta == json!({"role":"assistant"}) && reason.is_null(),
                "missing initial fixture SSE role"
            );
            continue;
        }
        if !reason.is_null() {
            closed(delta, &[])?;
            ensure!(
                matches!(
                    (&payload, reason.as_str()),
                    (Payload::Calls(_), Some("tool_calls")) | (Payload::Text(_), Some("stop"))
                ),
                "invalid fixture SSE finish reason"
            );
            terminal = true;
            continue;
        }
        if let Some(text) = delta.get("content") {
            closed(delta, &["content"])?;
            let text = text.as_str().context("invalid fixture SSE content")?;
            match &mut payload {
                Payload::Empty => payload = Payload::Text(text.to_owned()),
                Payload::Text(current) => current.push_str(text),
                Payload::Calls(_) => anyhow::bail!("mixed fixture SSE calls and text"),
            }
        } else {
            closed(delta, &["tool_calls"])?;
            ensure!(!reporting, "unexpected fixture call after finish_report");
            let calls = delta["tool_calls"]
                .as_array()
                .filter(|calls| calls.len() == 1)
                .context("expected one complete fixture SSE call per event")?;
            if matches!(payload, Payload::Empty) {
                payload = Payload::Calls(Vec::new());
            }
            let Payload::Calls(collected) = &mut payload else {
                anyhow::bail!("mixed fixture SSE calls and text");
            };
            let call = &calls[0];
            closed(call, &["index", "id", "type", "function"])?;
            ensure!(
                call["index"].as_u64() == Some(collected.len() as u64)
                    && call["type"] == "function",
                "invalid fixture SSE call index or type"
            );
            let id = call["id"]
                .as_str()
                .filter(|id| wire::valid_id(id))
                .context("invalid fixture SSE call id")?;
            ensure!(
                call_ids.insert(id.to_owned()),
                "duplicate fixture SSE call id"
            );
            let function = &call["function"];
            closed(function, &["name", "arguments"])?;
            let name = function["name"]
                .as_str()
                .context("invalid fixture SSE tool name")?;
            let arguments = function["arguments"]
                .as_str()
                .context("invalid fixture SSE arguments")?;
            policy::model_call_args(name, arguments)?;
            reporting = name == "finish_report";
            collected.push(json!({"id":id,"type":"function","function":function}));
        }
    }
    anyhow::bail!("too many fixture SSE events")
}

fn closed(value: &Value, fields: &[&str]) -> Result<()> {
    ensure!(
        value
            .as_object()
            .is_some_and(|o| o.len() == fields.len() && fields.iter().all(|k| o.contains_key(*k))),
        "invalid fixture SSE object fields"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
