//! JSON-RPC 2.0 envelopes. Method payloads live in the protocol's typed modules.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const JSONRPC: &str = "2.0";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request<P = Value> {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    pub params: P,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification<P = Value> {
    pub jsonrpc: String,
    pub method: String,
    pub params: P,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response<T = Value> {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(flatten)]
    pub outcome: ResponseOutcome<T>,
}

/// A response has exactly one result or error. A successful null result is preserved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseOutcome<T = Value> {
    Result { result: T },
    Error { error: Error },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthenticated,
    Revoked,
    UnsupportedVersion,
    MethodNotFound,
    InvalidParams,
    TooLarge,
    NotFound,
    Conflict,
    Refused,
    ConfirmRequired,
    Busy,
    Unavailable,
    Expired,
    Internal,
    #[serde(other)]
    Other,
}
