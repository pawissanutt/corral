//! Embedded, independently pinned fixture descriptors and the final provider body gate.
use anyhow::{Result, anyhow, ensure};
use serde_json::{Map, Value, json};
use std::{collections::HashSet, sync::OnceLock};

use super::wire::{self, PROMPT_SHA256, TOOL_POLICY_SHA256};

const PROMPT: &str =
    include_str!("../../../../docs/contracts/lite-analyst-v1/synthetic-prompt.txt");
const TOOL_POLICY: &[u8] =
    include_bytes!("../../../../docs/contracts/lite-analyst-v1/tool-policy.json");
pub(super) const MAX_BODY: usize = 262_144;
pub(super) const MODEL: &str = "fixture-v1";
const MODEL_NAMES: [&str; 4] = [
    "search_materials",
    "read_evidence",
    "calculate_metric",
    "finish_report",
];
const HOST_NAMES: [&str; 1] = ["read_result"];

fn load() -> Result<Value> {
    ensure!(
        wire::sha256(PROMPT.as_bytes()) == PROMPT_SHA256,
        "fixture prompt pin mismatch"
    );
    let registry: Value = wire::decode_signed(TOOL_POLICY)?;
    ensure!(
        wire::sha256(&wire::canonical_signed(&registry)?) == TOOL_POLICY_SHA256,
        "fixture tool policy pin mismatch"
    );
    for (kind, names) in [
        ("model_tools", MODEL_NAMES.as_slice()),
        ("host_only_tools", HOST_NAMES.as_slice()),
    ] {
        let entries = registry[kind]
            .as_array()
            .ok_or_else(|| anyhow!("invalid pinned registry"))?;
        ensure!(
            entries.len() == names.len(),
            "invalid pinned registry length"
        );
        for (entry, name) in entries.iter().zip(names) {
            closed(entry, &["name", "description", "inputSchema"])?;
            ensure!(
                entry["name"] == *name
                    && entry["description"].is_string()
                    && entry["inputSchema"].is_object(),
                "invalid pinned descriptor"
            );
        }
    }
    Ok(registry)
}

fn registry() -> Result<&'static Value> {
    static REGISTRY: OnceLock<Result<Value>> = OnceLock::new();
    REGISTRY
        .get_or_init(load)
        .as_ref()
        .map_err(|_| anyhow!("fixture policy unavailable"))
}

pub(super) fn prompt() -> Result<&'static str> {
    registry()?;
    Ok(PROMPT)
}

pub(super) fn model_descriptors() -> Result<Value> {
    Ok(registry()?["model_tools"].clone())
}

pub(super) fn descriptors(host_poll: bool) -> Result<Value> {
    let policy = registry()?;
    Ok(if host_poll {
        policy["host_only_tools"].clone()
    } else {
        let entries = policy["model_tools"]
            .as_array()
            .ok_or_else(|| anyhow!("invalid pinned registry"))?;
        Value::Array(entries[..3].to_vec())
    })
}

pub(super) fn validate_args(tool: &str, args: &Value) -> Result<()> {
    let registry = registry()?;
    let entry = if let Some(index) = MODEL_NAMES.iter().position(|name| *name == tool) {
        &registry["model_tools"][index]
    } else if tool == HOST_NAMES[0] {
        &registry["host_only_tools"][0]
    } else {
        anyhow::bail!("tool outside fixture policy");
    };
    ensure!(
        matches_schema(&entry["inputSchema"], args),
        "invalid fixture tool arguments"
    );
    Ok(())
}

pub(super) fn validate_reply(tool: &str, payload: &Value) -> Result<()> {
    let schema = registry()?["mcp_reply_schemas"]
        .get(tool)
        .ok_or_else(|| anyhow!("tool has no fixture reply schema"))?;
    ensure!(
        matches_schema(schema, payload),
        "invalid fixture tool reply"
    );
    Ok(())
}

pub(super) fn model_call_args(name: &str, arguments: &str) -> Result<Value> {
    ensure!(MODEL_NAMES.contains(&name), "tool is not model-visible");
    let value = wire::decode(arguments.as_bytes())?;
    validate_args(name, &value)?;
    Ok(value)
}

fn closed<'a>(value: &'a Value, fields: &[&str]) -> Result<&'a Map<String, Value>> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("invalid fixture object"))?;
    ensure!(
        object.len() == fields.len() && fields.iter().all(|name| object.contains_key(*name)),
        "invalid fixture object fields"
    );
    Ok(object)
}

// Only the vocabulary used by the exact pinned, hash-checked fixture. In particular JSON
// Schema string lengths are Unicode scalar counts, not byte counts (wire caps are separate).
fn matches_schema(schema: &Value, value: &Value) -> bool {
    if let Some(choices) = schema["oneOf"].as_array() {
        return choices
            .iter()
            .filter(|choice| matches_schema(choice, value))
            .count()
            == 1;
    }
    match schema["type"].as_str() {
        Some("object") => {
            let (Some(value), Some(properties), Some(required)) = (
                value.as_object(),
                schema["properties"].as_object(),
                schema["required"].as_array(),
            ) else {
                return false;
            };
            schema["additionalProperties"] == false
                && required
                    .iter()
                    .all(|r| r.as_str().is_some_and(|key| value.contains_key(key)))
                && value.iter().all(|(key, child)| {
                    properties
                        .get(key)
                        .is_some_and(|shape| matches_schema(shape, child))
                })
        }
        Some("array") => value.as_array().is_some_and(|entries| {
            length(schema, entries.len(), "minItems", "maxItems")
                && entries
                    .iter()
                    .all(|entry| matches_schema(&schema["items"], entry))
        }),
        Some("integer") => value.as_i64().is_some_and(|n| {
            schema["minimum"].as_i64().is_some_and(|min| n >= min)
                && schema["maximum"].as_i64().is_some_and(|max| n <= max)
        }),
        Some("string") => value.as_str().is_some_and(|s| {
            schema.get("const").is_none_or(|p| p == s)
                && schema
                    .get("enum")
                    .is_none_or(|p| p.as_array().is_some_and(|options| options.contains(value)))
                && schema
                    .get("pattern")
                    .is_none_or(|p| p == "^[A-Za-z0-9_-]{1,64}$" && wire::valid_id(s))
                && length(schema, s.chars().count(), "minLength", "maxLength")
        }),
        _ => false,
    }
}

fn length(schema: &Value, count: usize, min: &str, max: &str) -> bool {
    schema[min]
        .as_u64()
        .is_none_or(|bound| count as u64 >= bound)
        && schema[max]
            .as_u64()
            .is_none_or(|bound| count as u64 <= bound)
}

/// Return the only body that may be hashed for reservation and passed unchanged to send.
/// `expected_history` is built from supervisor-observed calls and validated broker results,
/// never from Pi's rendering or from previously accepted candidate messages.
pub(super) fn finalize(
    question: &str,
    candidate: &Value,
    expected_history: &[Value],
) -> Result<Vec<u8>> {
    closed(candidate, &["model", "stream", "messages", "tools"])?;
    ensure!(
        candidate["model"] == MODEL && candidate["stream"] == true,
        "invalid fixture route"
    );
    let messages = candidate["messages"]
        .as_array()
        .ok_or_else(|| anyhow!("invalid fixture messages"))?;
    let (first, history) = messages
        .split_at_checked(2)
        .ok_or_else(|| anyhow!("fixture prompt or question missing"))?;
    ensure!(
        first[0] == json!({"role":"system","content":prompt()?})
            && first[1] == json!({"role":"user","content":question})
            && history == expected_history,
        "fixture messages differ from approved context"
    );
    let tools = model_descriptors()?.as_array().ok_or_else(|| anyhow!("invalid pinned registry"))?.iter().map(|entry| {
        json!({"type":"function","function":{"name":entry["name"],"description":entry["description"],"parameters":entry["inputSchema"]}})
    }).collect::<Vec<_>>();
    ensure!(
        candidate["tools"] == Value::Array(tools),
        "fixture model registry mismatch"
    );
    check_history(history)?;
    let body = wire::canonical(candidate)?;
    ensure!(body.len() <= MAX_BODY, "fixture provider body too large");
    Ok(body)
}

fn check_history(history: &[Value]) -> Result<()> {
    let mut messages = history.iter();
    let mut seen = HashSet::new();
    while let Some(assistant) = messages.next() {
        closed(assistant, &["role", "content", "tool_calls"])?;
        ensure!(
            assistant["role"] == "assistant" && assistant["content"].is_null(),
            "invalid fixture assistant history"
        );
        let calls = assistant["tool_calls"]
            .as_array()
            .filter(|calls| !calls.is_empty())
            .ok_or_else(|| anyhow!("missing fixture history calls"))?;
        for call in calls {
            closed(call, &["id", "type", "function"])?;
            ensure!(
                call["type"] == "function",
                "invalid fixture history call type"
            );
            let id = call["id"]
                .as_str()
                .filter(|id| wire::valid_id(id) && seen.insert(*id))
                .ok_or_else(|| anyhow!("invalid fixture history call id"))?;
            let function = &call["function"];
            closed(function, &["name", "arguments"])?;
            let name = function["name"]
                .as_str()
                .ok_or_else(|| anyhow!("invalid fixture history tool name"))?;
            let arguments = function["arguments"]
                .as_str()
                .ok_or_else(|| anyhow!("invalid fixture history arguments"))?;
            model_call_args(name, arguments)?;
            let reply = messages
                .next()
                .ok_or_else(|| anyhow!("missing fixture history reply"))?;
            closed(reply, &["role", "tool_call_id", "content"])?;
            ensure!(
                reply["role"] == "tool" && reply["tool_call_id"] == id,
                "fixture history reply mismatch"
            );
            let content = reply["content"]
                .as_str()
                .ok_or_else(|| anyhow!("invalid fixture history reply content"))?;
            let payload = wire::decode_signed(content.as_bytes())?;
            validate_reply(name, &payload)?;
            ensure!(
                payload["status"] != "pending"
                    && wire::canonical_signed(&payload)? == content.as_bytes(),
                "invalid fixture history reply encoding or status"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
