//! Protocol v1 error taxonomy (docs/extensions-protocol.md, "Errors").

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::fmt;

/// Every error code protocol v1 defines. The wire form is the snake_case name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    Conflict,
    ProtocolViolation,
    UnsupportedProtocol,
    UnsupportedCapability,
    ManifestMismatch,
    RegistrationStale,
    InvalidRequest,
    InputMismatch,
    Superseded,
    Cancelled,
    DeadlineExceeded,
    ResourceExhausted,
    LimitExceeded,
    SourceUnavailable,
    EvidenceUnavailable,
    NotAvailable,
    AccessRevoked,
    CursorExpired,
    LineageChanged,
    RecoveryRequired,
    ExtInvocationRefused,
    Internal,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 22] = [
        Self::Conflict,
        Self::ProtocolViolation,
        Self::UnsupportedProtocol,
        Self::UnsupportedCapability,
        Self::ManifestMismatch,
        Self::RegistrationStale,
        Self::InvalidRequest,
        Self::InputMismatch,
        Self::Superseded,
        Self::Cancelled,
        Self::DeadlineExceeded,
        Self::ResourceExhausted,
        Self::LimitExceeded,
        Self::SourceUnavailable,
        Self::EvidenceUnavailable,
        Self::NotAvailable,
        Self::AccessRevoked,
        Self::CursorExpired,
        Self::LineageChanged,
        Self::RecoveryRequired,
        Self::ExtInvocationRefused,
        Self::Internal,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Conflict => "conflict",
            Self::ProtocolViolation => "protocol_violation",
            Self::UnsupportedProtocol => "unsupported_protocol",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::ManifestMismatch => "manifest_mismatch",
            Self::RegistrationStale => "registration_stale",
            Self::InvalidRequest => "invalid_request",
            Self::InputMismatch => "input_mismatch",
            Self::Superseded => "superseded",
            Self::Cancelled => "cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::ResourceExhausted => "resource_exhausted",
            Self::LimitExceeded => "limit_exceeded",
            Self::SourceUnavailable => "source_unavailable",
            Self::EvidenceUnavailable => "evidence_unavailable",
            Self::NotAvailable => "not_available",
            Self::AccessRevoked => "access_revoked",
            Self::CursorExpired => "cursor_expired",
            Self::LineageChanged => "lineage_changed",
            Self::RecoveryRequired => "recovery_required",
            Self::ExtInvocationRefused => "ext_invocation_refused",
            Self::Internal => "internal",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == code)
    }

    /// Default `retryable` from the taxonomy table; the error's origin may override it.
    pub fn default_retryable(self) -> bool {
        matches!(
            self,
            Self::Superseded
                | Self::DeadlineExceeded
                | Self::ResourceExhausted
                | Self::SourceUnavailable
                | Self::Internal
        )
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Self::parse(&code)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown error code `{code}`")))
    }
}

/// One protocol error: the body of an `error` frame and the error type of this module.
///
/// Deserializing maps a code outside the v1 set to `internal` and keeps the original code
/// (and any original `data`) under `data.original_code` / `data.original_data`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtError {
    pub code: ErrorCode,
    pub retryable: bool,
    pub detail: String,
    pub data: Option<Value>,
}

impl ExtError {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            retryable: code.default_retryable(),
            detail: detail.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// A host-detected violation carrying a machine-readable `data.reason` token.
    pub fn protocol_violation(reason: &str, detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::ProtocolViolation, detail)
            .with_data(serde_json::json!({ "reason": reason }))
    }

    pub fn invalid_request(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidRequest, detail)
    }

    /// Builds an error from wire fields, applying the unknown-code and default rules.
    pub fn from_wire(
        code: &str,
        retryable: Option<bool>,
        detail: String,
        data: Option<Value>,
    ) -> Self {
        match ErrorCode::parse(code) {
            Some(code) => Self {
                code,
                retryable: retryable.unwrap_or(code.default_retryable()),
                detail,
                data,
            },
            None => {
                let mut kept = Map::new();
                kept.insert("original_code".into(), Value::String(code.into()));
                if let Some(original) = data {
                    kept.insert("original_data".into(), original);
                }
                Self {
                    code: ErrorCode::Internal,
                    retryable: retryable.unwrap_or(ErrorCode::Internal.default_retryable()),
                    detail,
                    data: Some(Value::Object(kept)),
                }
            }
        }
    }

    /// The machine-readable reason token in `data.reason`, when present.
    pub fn reason(&self) -> Option<&str> {
        self.data.as_ref()?.get("reason")?.as_str()
    }
}

impl fmt::Display for ExtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for ExtError {}

#[derive(Serialize)]
struct WireOut<'a> {
    code: ErrorCode,
    retryable: bool,
    detail: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: &'a Option<Value>,
}

#[derive(Deserialize)]
struct WireIn {
    code: String,
    #[serde(default)]
    retryable: Option<bool>,
    #[serde(default)]
    detail: String,
    #[serde(default)]
    data: Option<Value>,
}

impl Serialize for ExtError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        WireOut {
            code: self.code,
            retryable: self.retryable,
            detail: &self.detail,
            data: &self.data,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ExtError {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = WireIn::deserialize(deserializer)?;
        Ok(Self::from_wire(
            &wire.code,
            wire.retryable,
            wire.detail,
            wire.data,
        ))
    }
}
