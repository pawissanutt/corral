//! Version-one owner annotations. Canonical rows, never specification or evidence inputs.
use anyhow::Result;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[cfg(test)]
#[path = "model/annotations_tests.rs"]
mod tests;

#[path = "model/annotations_action.rs"]
mod action;
pub use action::*;
#[path = "model/annotations_view.rs"]
mod view;
pub use view::*;

pub const NOTE_BYTES: usize = 4096;
pub const THREAD_ENTRIES: u32 = 64;
pub const TASK_THREADS: u32 = 256;
pub const TASK_TEXT_BYTES: u32 = 256 * 1024;
pub const RESPONSE_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationCode {
    NotOwner,
    RoleNotPermitted,
    OutOfScope,
    StaleCandidate,
    CandidateNotCurrent,
    CandidateUnavailable,
    InvalidPath,
    PathOutOfScope,
    InvalidRange,
    BinaryRange,
    FileTooLarge,
    InvalidText,
    ThreadNotFound,
    ThreadRevisionConflict,
    InvalidTransition,
    RequestKeyConflict,
    AnnotationLimit,
    UnsupportedVersion,
    InvalidCursor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnnotationError {
    pub code: AnnotationCode,
    pub message: String,
    pub detail: Value,
}
impl std::fmt::Display for AnnotationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}
impl std::error::Error for AnnotationError {}
pub fn annotation_error(code: AnnotationCode, message: &str, detail: Value) -> anyhow::Error {
    AnnotationError {
        code,
        message: message.into(),
        detail,
    }
    .into()
}
pub fn require(ok: bool, code: AnnotationCode, message: &str) -> Result<()> {
    if !ok {
        return Err(annotation_error(code, message, Value::Null));
    }
    Ok(())
}
pub fn version(v: u32) -> Result<()> {
    require(
        v == 1,
        AnnotationCode::UnsupportedVersion,
        "annotation version must be 1",
    )
}
/// Check the version before decoding fields a future writer may have added.
pub fn document_version(doc: &Value) -> Result<()> {
    require(
        doc.get("version").and_then(Value::as_u64) == Some(1),
        AnnotationCode::UnsupportedVersion,
        "annotation document version must be 1",
    )?;
    if let Some(anchor) = doc.get("anchor") {
        document_version(anchor)?;
    }
    if let Some(author) = doc.get("author") {
        document_version(author)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationSide {
    New,
    Old,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}
impl LineRange {
    pub fn validate(self) -> Result<()> {
        require(
            self.start > 0 && self.end >= self.start && self.end - self.start < 400,
            AnnotationCode::InvalidRange,
            "range must be 1-based, inclusive and at most 400 lines",
        )
    }
}
pub fn validate_path(path: &str) -> Result<()> {
    require(
        !path.is_empty()
            && path.len() <= 1024
            && !path.starts_with('/')
            && !path.contains(['\\', '\0', ':'])
            && path
                .split('/')
                .all(|p| !p.is_empty() && p != "." && p != ".." && !p.eq_ignore_ascii_case(".git")),
        AnnotationCode::InvalidPath,
        "expected a repository-relative path outside .git",
    )
}
pub fn validate_key(key: &str) -> Result<()> {
    require(
        !key.is_empty()
            && key.len() <= 128
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b)),
        AnnotationCode::InvalidText,
        "request key must be 1..128 ASCII letters, digits or ._:-",
    )
}
pub fn validate_text(text: &str, required: bool) -> Result<()> {
    require(
        text.len() <= NOTE_BYTES,
        AnnotationCode::AnnotationLimit,
        "note exceeds 4 KiB",
    )?;
    require(
        !text.contains('\0') && (!required || !text.trim().is_empty()),
        AnnotationCode::InvalidText,
        "note must contain non-whitespace text and no NUL",
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationAnchor {
    pub version: u32,
    pub task: String,
    pub run: String,
    pub base: String,
    pub head: String,
    pub tree: String,
    pub path: String,
    pub side: AnnotationSide,
    pub blob: Option<String>,
    pub range: Option<LineRange>,
    pub excerpt: String,
    pub excerpt_truncated: bool,
    pub excerpt_sha256: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnnotationSurface {
    Cli,
    PiPanel {
        container: crate::agent::ContainerRef,
        session: String,
        generation: u64,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnnotationAuthor {
    Owner {
        version: u32,
        surface: AnnotationSurface,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationRequest {
    pub key: String,
    pub payload_sha256: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationState {
    Open,
    Resolved,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationKind {
    Opened,
    Reply,
    Resolved,
    Reopened,
}
impl AnnotationKind {
    pub fn state(self, previous: AnnotationState) -> AnnotationState {
        match self {
            Self::Opened | Self::Reopened => AnnotationState::Open,
            Self::Resolved => AnnotationState::Resolved,
            Self::Reply => previous,
        }
    }
    pub fn event(self) -> &'static str {
        match self {
            Self::Opened => "annotation_opened",
            Self::Reply => "annotation_replied",
            Self::Resolved => "annotation_resolved",
            Self::Reopened => "annotation_reopened",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationEntry {
    pub version: u32,
    pub seq: u32,
    pub kind: AnnotationKind,
    /// Projection after this immutable entry, also the original retry result's state.
    pub state: AnnotationState,
    pub author: AnnotationAuthor,
    pub at: Timestamp,
    pub text: String,
    pub request: AnnotationRequest,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationThread {
    pub version: u32,
    pub id: String,
    pub anchor: AnnotationAnchor,
    pub revision: u32,
    pub state: AnnotationState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<AnnotationEntry>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnnotationSummary {
    pub threads: u32,
    pub entries: u32,
    pub text_bytes: u32,
    pub next_thread: u32,
}
impl AnnotationSummary {
    pub fn admit(&self, revision: u32, text: &str, new: bool) -> Result<()> {
        require(
            !new || self.threads < TASK_THREADS,
            AnnotationCode::AnnotationLimit,
            "task exceeds 256 threads",
        )?;
        require(
            revision < THREAD_ENTRIES,
            AnnotationCode::AnnotationLimit,
            "thread exceeds 64 entries",
        )?;
        require(
            u64::from(self.text_bytes) + text.len() as u64 <= u64::from(TASK_TEXT_BYTES),
            AnnotationCode::AnnotationLimit,
            "task note text exceeds 256 KiB",
        )
    }
    pub fn of(threads: &[AnnotationThread]) -> Result<Self> {
        let mut out = Self::default();
        let mut ids = std::collections::BTreeSet::new();
        let mut keys = std::collections::BTreeSet::new();
        for thread in threads {
            thread.validate()?;
            require(
                ids.insert(thread.id.clone()),
                AnnotationCode::InvalidTransition,
                "duplicate thread",
            )?;
            let n = thread.number()?;
            out.next_thread = out.next_thread.max(n + 1);
            out.threads += 1;
            for entry in &thread.entries {
                require(
                    keys.insert(entry.request.key.clone()),
                    AnnotationCode::RequestKeyConflict,
                    "duplicate key",
                )?;
                out.entries += 1;
                out.text_bytes += entry.text.len() as u32;
            }
        }
        require(
            out.threads <= TASK_THREADS && out.text_bytes <= TASK_TEXT_BYTES,
            AnnotationCode::AnnotationLimit,
            "task annotation capacity exceeded",
        )?;
        Ok(out)
    }
}
impl AnnotationThread {
    pub fn number(&self) -> Result<u32> {
        self.id
            .strip_prefix('N')
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|n| *n > 0 && *n <= TASK_THREADS)
            .ok_or_else(|| {
                annotation_error(
                    AnnotationCode::InvalidTransition,
                    "invalid thread id",
                    json!(self.id),
                )
            })
    }
    pub fn validate(&self) -> Result<()> {
        version(self.version)?;
        version(self.anchor.version)?;
        self.number()?;
        validate_path(&self.anchor.path)?;
        if let Some(range) = self.anchor.range {
            range.validate()?;
        }
        require(
            self.anchor.excerpt.len() <= 2048 && self.anchor.excerpt.lines().count() <= 40,
            AnnotationCode::AnnotationLimit,
            "stored excerpt exceeds bound",
        )?;
        require(
            self.revision > 0
                && self.revision as usize == self.entries.len()
                && self.revision <= THREAD_ENTRIES,
            AnnotationCode::InvalidTransition,
            "thread revision projection differs from entries",
        )?;
        let mut state = AnnotationState::Open;
        for (i, entry) in self.entries.iter().enumerate() {
            version(entry.version)?;
            let AnnotationAuthor::Owner { version: v, .. } = &entry.author;
            version(*v)?;
            validate_key(&entry.request.key)?;
            validate_text(
                &entry.text,
                matches!(entry.kind, AnnotationKind::Opened | AnnotationKind::Reply),
            )?;
            require(
                entry.seq as usize == i + 1 && ((i == 0) == (entry.kind == AnnotationKind::Opened)),
                AnnotationCode::InvalidTransition,
                "invalid entry ordinal/kind",
            )?;
            require(
                !matches!(
                    (entry.kind, state),
                    (AnnotationKind::Resolved, AnnotationState::Resolved)
                        | (AnnotationKind::Reopened, AnnotationState::Open)
                ),
                AnnotationCode::InvalidTransition,
                "invalid transition",
            )?;
            state = entry.kind.state(state);
            require(
                entry.state == state,
                AnnotationCode::InvalidTransition,
                "invalid entry state projection",
            )?;
        }
        require(
            self.state == state,
            AnnotationCode::InvalidTransition,
            "invalid thread state projection",
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnnotationResult {
    pub task: String,
    pub thread: String,
    pub entry: u32,
    pub revision: u32,
    pub state: AnnotationState,
    pub replayed: bool,
}
