//! Protocol v1 envelope, methods and `initialize` negotiation types.

use super::error::{ErrorCode, ExtError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub corral_ext: u32,
    pub id: String,
    #[serde(flatten)]
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    Request {
        method: Method,
        #[serde(default)]
        params: Value,
    },
    Response {
        result: Value,
    },
    Error {
        error: ExtError,
    },
    Progress {
        progress: Progress,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvelopeKind {
    Request,
    Response,
    Error,
    Progress,
}

impl EnvelopeKind {
    pub const ALL: [EnvelopeKind; 4] = [Self::Request, Self::Response, Self::Error, Self::Progress];
}

impl Body {
    pub fn kind(&self) -> EnvelopeKind {
        match self {
            Self::Request { .. } => EnvelopeKind::Request,
            Self::Response { .. } => EnvelopeKind::Response,
            Self::Error { .. } => EnvelopeKind::Error,
            Self::Progress { .. } => EnvelopeKind::Progress,
        }
    }
}

impl Envelope {
    pub fn new(id: impl Into<String>, body: Body) -> Self {
        Self {
            corral_ext: PROTOCOL_VERSION,
            id: id.into(),
            body,
        }
    }

    /// Decodes one frame body. Malformed JSON is `protocol_violation`; another envelope
    /// version is `unsupported_protocol`.
    pub fn decode(body: &str) -> Result<Self, ExtError> {
        let envelope: Self = serde_json::from_str(body).map_err(|e| {
            ExtError::protocol_violation("invalid_envelope", format!("invalid envelope: {e}"))
        })?;
        if envelope.corral_ext != PROTOCOL_VERSION {
            return Err(ExtError::new(
                ErrorCode::UnsupportedProtocol,
                format!(
                    "envelope version {} is not {PROTOCOL_VERSION}",
                    envelope.corral_ext
                ),
            ));
        }
        Ok(envelope)
    }

    pub fn encode(&self) -> Result<String, ExtError> {
        serde_json::to_string(self)
            .map_err(|e| ExtError::new(ErrorCode::Internal, format!("encoding envelope: {e}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Method {
    #[serde(rename = "initialize")]
    Initialize,
    #[serde(rename = "provider.invoke")]
    ProviderInvoke,
    #[serde(rename = "events.deliver")]
    EventsDeliver,
    #[serde(rename = "request.cancel")]
    RequestCancel,
    #[serde(rename = "command.invoke")]
    CommandInvoke,
    #[serde(rename = "evidence.read")]
    EvidenceRead,
    #[serde(rename = "health")]
    Health,
    #[serde(rename = "shutdown")]
    Shutdown,
}

impl Method {
    pub const ALL: [Method; 8] = [
        Self::Initialize,
        Self::ProviderInvoke,
        Self::EventsDeliver,
        Self::RequestCancel,
        Self::CommandInvoke,
        Self::EvidenceRead,
        Self::Health,
        Self::Shutdown,
    ];

    /// Whether the stage-1 pilot host sends this method; the rest are reserved names.
    pub fn in_pilot(self) -> bool {
        matches!(self, Self::Initialize | Self::ProviderInvoke)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub stage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

/// `<name>@<version>`, the capability identity form (for example `task.context@1`).
pub fn capability_id(name: &str, version: u32) -> String {
    format!("{name}@{version}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityOffer {
    pub name: String,
    pub versions: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    pub corral_version: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeParams {
    pub protocol_versions: Vec<u32>,
    pub capabilities: Vec<CapabilityOffer>,
    pub host: HostInfo,
    #[serde(default)]
    pub limits: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityChoice {
    pub name: String,
    pub version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitializeResult {
    pub protocol_version: u32,
    pub capabilities: Vec<CapabilityChoice>,
    #[serde(default)]
    pub unsupported: Vec<String>,
}
