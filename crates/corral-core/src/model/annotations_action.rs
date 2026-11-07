//! Bounded typed mutation payloads shared by trusted annotation adapters.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAnnotation {
    pub expected_run: String,
    pub base: String,
    pub head: String,
    pub path: String,
    pub side: AnnotationSide,
    pub range: Option<LineRange>,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeAnnotation {
    pub thread: String,
    pub expected_revision: u32,
    #[serde(default)]
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnnotationAction {
    Open(OpenAnnotation),
    Reply(ChangeAnnotation),
    Resolve(ChangeAnnotation),
    Reopen(ChangeAnnotation),
}
impl AnnotationAction {
    pub fn kind(&self) -> AnnotationKind {
        match self {
            Self::Open(_) => AnnotationKind::Opened,
            Self::Reply(_) => AnnotationKind::Reply,
            Self::Resolve(_) => AnnotationKind::Resolved,
            Self::Reopen(_) => AnnotationKind::Reopened,
        }
    }
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Open(a) => require(
                !a.expected_run.is_empty()
                    && a.expected_run.len() <= 128
                    && a.base.len() <= 64
                    && a.head.len() <= 64
                    && a.path.len() <= 1024,
                AnnotationCode::InvalidPath,
                "candidate fields/path exceed input bounds",
            ),
            Self::Reply(a) | Self::Resolve(a) | Self::Reopen(a) => require(
                a.thread.len() <= 16,
                AnnotationCode::ThreadNotFound,
                "invalid thread identity",
            ),
        }
    }
    pub fn text(&self) -> &str {
        match self {
            Self::Open(a) => &a.text,
            Self::Reply(a) | Self::Resolve(a) | Self::Reopen(a) => &a.text,
        }
    }
}
