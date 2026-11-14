//! Bounded device-owned uploads; links remain opaque and never carry file paths.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const ATTACHMENT_BEGIN: &str = "attachment.begin";
pub const ATTACHMENT_PUT: &str = "attachment.put";
pub const ATTACHMENT_DISCARD: &str = "attachment.discard";
pub const ASSISTANT_ATTACH_V1: &str = "assistant.attach.v1";
pub const ATTACHMENT_IMAGE_MAX: u64 = 2 * 1024 * 1024;
pub const ATTACHMENT_TEXT_MAX: u64 = 64 * 1024;
pub const ATTACHMENT_CHUNK: usize = 36 * 1024;
pub const ATTACHMENTS_PER_MESSAGE: usize = 4;
pub const ATTACHMENT_NAME_MAX: usize = 160;
pub const ATTACHMENT_URL_MAX: usize = 2048;
pub const DEVICE_UPLOADS_MAX: usize = 8;
pub const DEVICE_UPLOAD_DAY_BYTES: u64 = 64 * 1024 * 1024;
pub const UPLOAD_TTL_SECS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    File,
    Link,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentBegin {
    pub upload: String,
    pub kind: AttachmentKind,
    pub name: String,
    pub media_type: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentPut {
    pub upload: String,
    pub offset: u64,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentDiscard {
    pub upload: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UploadAck {
    pub upload: String,
    pub received: u64,
    pub bytes: u64,
    pub ready: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentFact {
    pub id: String,
    pub kind: AttachmentKind,
    pub name: String,
    pub media_type: Option<String>,
    pub bytes: Option<u64>,
    pub sha256: Option<String>,
    pub url: Option<String>,
}
