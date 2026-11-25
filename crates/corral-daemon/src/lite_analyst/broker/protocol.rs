//! Exact messages for the pinned fixture MCP subset; nothing here grants access.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::{bootstrap::Enrollment, policy, wire};

const VERSION: &str = "2025-06-18";
pub(super) const SHIM_SERVER: &str = "corral-lite-mcp-shim";
pub(super) const LITE_SERVER: &str = "mitimind-lite-analyst";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum BindingKind {
    BrokerAttach,
    BrokerAttached,
    ShimBind,
    ShimBound,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BindingRecord {
    v: u64,
    #[serde(rename = "type")]
    pub(super) kind: BindingKind,
    link_id: String,
    channel_epoch: String,
    request_id: String,
    request_digest: String,
    run_id: String,
    generation: u64,
    grant_id: String,
    grant_generation: u64,
    policy_ref: String,
}

impl BindingRecord {
    pub(super) fn from_bound(enrollment: &Enrollment, binding: &wire::Binding) -> Result<Self> {
        let record = Self {
            v: 1,
            kind: BindingKind::BrokerAttach,
            link_id: enrollment.link_id.clone(),
            channel_epoch: enrollment.channel_epoch.clone(),
            request_id: binding.request_id.clone(),
            request_digest: binding.request_digest.clone(),
            run_id: binding.run_id.clone(),
            generation: binding.generation,
            grant_id: binding.grant_id.clone(),
            grant_generation: binding.grant_generation,
            policy_ref: binding.policy_ref.clone(),
        };
        record.check(BindingKind::BrokerAttach)?;
        Ok(record)
    }

    pub(super) fn parse(value: Value, kind: BindingKind) -> Result<Self> {
        let record: Self = typed(value)?;
        record.check(kind)?;
        Ok(record)
    }

    fn check(&self, kind: BindingKind) -> Result<()> {
        ensure!(
            self.v == 1
                && self.kind == kind
                && [
                    &self.link_id,
                    &self.channel_epoch,
                    &self.request_id,
                    &self.run_id,
                    &self.grant_id
                ]
                .into_iter()
                .all(|id| wire::valid_id(id))
                && self.link_id != self.channel_epoch
                && wire::valid_digest(&self.request_digest)
                && self.policy_ref == wire::POLICY_REF
                && (1..=wire::MAX_SAFE).contains(&self.generation)
                && (1..=wire::MAX_SAFE).contains(&self.grant_generation),
            "invalid fixture broker binding"
        );
        Ok(())
    }

    pub(super) fn record(&self, kind: BindingKind) -> Result<Value> {
        let mut value = serde_json::to_value(self)?;
        value["type"] = serde_json::to_value(kind)?;
        Ok(value)
    }
}

pub(super) fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":VERSION,"capabilities":{},"clientInfo":{"name":"corral-synthetic","version":"1"}
    }})
}

pub(super) fn initialized() -> Value {
    json!({"jsonrpc":"2.0","method":"notifications/initialized"})
}

pub(super) fn list() -> Value {
    json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
}

pub(super) fn server_info(name: &str) -> Value {
    json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":VERSION,"capabilities":{"tools":{}},"serverInfo":{"name":name,"version":"1"}}})
}

pub(super) fn listing(host_poll: bool) -> Result<Value> {
    Ok(json!({"jsonrpc":"2.0","id":2,"result":{"tools":policy::descriptors(host_poll)?}}))
}

pub(super) fn check_listing(mut actual: Value, host_poll: bool) -> Result<()> {
    fn sort_tools(record: &mut Value) -> Result<()> {
        let tools = record
            .pointer_mut("/result/tools")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| anyhow::anyhow!("missing fixture MCP descriptors"))?;
        tools.sort_unstable_by(|a, b| {
            a.get("name")
                .and_then(Value::as_str)
                .cmp(&b.get("name").and_then(Value::as_str))
        });
        Ok(())
    }
    let mut expected = listing(host_poll)?;
    sort_tools(&mut actual)?;
    sort_tools(&mut expected)?;
    ensure!(actual == expected, "fixture MCP descriptor mismatch");
    Ok(())
}

fn typed<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|_| anyhow::anyhow!("invalid fixture MCP record"))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Call {
    jsonrpc: String,
    pub(super) id: u64,
    method: String,
    pub(super) params: Params,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Params {
    pub(super) name: String,
    pub(super) arguments: Value,
}

impl Call {
    pub(super) fn new(id: u64, tool: &str, args: &Value, host_poll: bool) -> Result<Self> {
        let call = Self {
            jsonrpc: "2.0".into(),
            id,
            method: "tools/call".into(),
            params: Params {
                name: tool.to_owned(),
                arguments: args.clone(),
            },
        };
        call.validate(host_poll)?;
        Ok(call)
    }

    pub(super) fn parse(value: Value, host_poll: bool) -> Result<Self> {
        let call: Self = typed(value)?;
        call.validate(host_poll)?;
        Ok(call)
    }

    fn validate(&self, host_poll: bool) -> Result<()> {
        ensure!(
            self.jsonrpc == "2.0"
                && self.method == "tools/call"
                && (3..=wire::MAX_SAFE).contains(&self.id),
            "invalid fixture MCP call envelope"
        );
        let authorized = if host_poll {
            self.params.name == "read_result"
        } else {
            is_evidence(&self.params.name)
        };
        ensure!(authorized, "fixture MCP tool not authorized on channel");
        policy::validate_args(&self.params.name, &self.params.arguments)
    }

    fn args_digest(&self) -> Result<String> {
        wire::digest(
            &json!({"v":1,"kind":"mcp_args","tool":self.params.name,"args":self.params.arguments}),
        )
    }
}

fn is_evidence(tool: &str) -> bool {
    matches!(
        tool,
        "search_materials" | "read_evidence" | "calculate_metric"
    )
}

/// An idle private control round trip, never an MCP request or authorization.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Probe {
    v: u64,
    #[serde(rename = "type")]
    kind: String,
    nonce: String,
}

impl Probe {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            v: 1,
            kind: "shim_probe".into(),
            nonce: wire::fresh_id()?,
        })
    }

    pub(super) fn parse(value: Value) -> Result<Self> {
        let probe: Self = typed(value)?;
        ensure!(
            probe.v == 1 && probe.kind == "shim_probe" && wire::valid_id(&probe.nonce),
            "invalid fixture shim observation request"
        );
        Ok(probe)
    }

    pub(super) fn ack(&self) -> Result<Value> {
        let mut value = serde_json::to_value(self)?;
        value["type"] = "shim_probed".into();
        Ok(value)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Arm {
    v: u64,
    #[serde(rename = "type")]
    kind: String,
    pub(super) id: u64,
    pub(super) tool: String,
    pub(super) invocation_id: String,
    pub(super) args_digest: String,
}

impl Arm {
    pub(super) fn new(call: &Call) -> Result<Self> {
        Ok(Self {
            v: 1,
            kind: "shim_arm".into(),
            id: call.id,
            tool: call.params.name.clone(),
            invocation_id: wire::fresh_id()?,
            args_digest: call.args_digest()?,
        })
    }

    pub(super) fn parse(value: Value, last_id: u64) -> Result<Self> {
        let arm: Self = typed(value)?;
        ensure!(
            arm.v == 1
                && arm.kind == "shim_arm"
                && (3..=wire::MAX_SAFE).contains(&arm.id)
                && arm.id > last_id
                && is_evidence(&arm.tool)
                && wire::valid_id(&arm.invocation_id)
                && wire::valid_digest(&arm.args_digest),
            "invalid or repeated fixture shim arm"
        );
        Ok(arm)
    }

    pub(super) fn ack(&self) -> Result<Value> {
        let mut value = serde_json::to_value(self)?;
        value["type"] = "shim_armed".into();
        Ok(value)
    }

    pub(super) fn request(&self, bound: &BindingRecord, call: &Call) -> Result<Value> {
        ensure!(
            self.id == call.id
                && self.tool == call.params.name
                && self.args_digest == call.args_digest()?,
            "fixture shim arm does not match call"
        );
        Ok(
            json!({"run_id":bound.run_id,"generation":bound.generation,"invocation_id":self.invocation_id,"args_digest":self.args_digest,"request":call}),
        )
    }

    pub(super) fn reply(
        &self,
        bound: &BindingRecord,
        call: &Call,
        reply: Value,
    ) -> Result<(Value, Value)> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wrapped {
            run_id: String,
            generation: u64,
            invocation_id: String,
            response: Value,
        }
        let wrapped: Wrapped = typed(reply)?;
        ensure!(
            wrapped.run_id == bound.run_id
                && wrapped.generation == bound.generation
                && wrapped.invocation_id == self.invocation_id,
            "foreign fixture broker reply identity"
        );
        let payload = structured_reply(call, wrapped.response.clone())?;
        Ok((wrapped.response, payload))
    }
}

pub(super) fn structured_reply(call: &Call, response: Value) -> Result<Value> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RpcReply {
        jsonrpc: String,
        id: u64,
        result: ToolResult,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields, rename_all = "camelCase")]
    struct ToolResult {
        content: Vec<Value>,
        structured_content: Value,
        is_error: bool,
    }
    let reply: RpcReply = typed(response)?;
    let payload = reply.result.structured_content;
    ensure!(
        reply.jsonrpc == "2.0" && reply.id == call.id && reply.result.content.is_empty(),
        "invalid fixture MCP result envelope"
    );
    policy::validate_reply(&call.params.name, &payload)?;
    let status = payload["status"].as_str();
    ensure!(
        reply.result.is_error == (status == Some("error")),
        "inconsistent fixture MCP isError flag"
    );
    match (call.params.name.as_str(), status) {
        ("search_materials", Some("ok")) => {
            ensure!(
                payload["hits"]
                    .as_array()
                    .zip(call.params.arguments["limit"].as_u64())
                    .is_some_and(|(hits, max)| hits.len() as u64 <= max),
                "fixture search exceeds requested count"
            );
        }
        ("read_evidence", Some("ok")) => {
            ensure!(
                payload["source"] == call.params.arguments["source"]
                    && payload["start_byte"] == call.params.arguments["start_byte"]
                    && payload["text"]
                        .as_str()
                        .zip(call.params.arguments["max_bytes"].as_u64())
                        .is_some_and(|(text, max)| text.len() as u64 <= max),
                "fixture evidence does not match requested slice"
            );
        }
        ("read_result", Some("ok" | "pending")) => {
            ensure!(
                payload["result"] == call.params.arguments["result"],
                "foreign fixture poll result handle"
            );
        }
        _ => {}
    }
    Ok(payload)
}
