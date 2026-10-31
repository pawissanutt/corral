//! Read-only artifact catalog metadata over real task or assistant-session owners.
//! This view is never accepted by scheduling, native callbacks or task mutation APIs.
use crate::assistant::AssistantSession;
use crate::{Artifact, Status, Task};
use jiff::Timestamp;

#[derive(Debug, Clone)]
pub enum ArtifactOwnerSource {
    Task(Box<Task>),
    Assistant(Box<AssistantSession>),
}

#[derive(Debug, Clone)]
pub struct ArtifactOwner {
    pub id: String,
    pub title: String,
    pub project: Option<String>,
    /// Catalog lifecycle labels: an ended session is Done; an active session is Backlog.
    /// This does not create a task or authorize any task operation for a session.
    pub status: Status,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub artifacts: Vec<Artifact>,
    pub source: ArtifactOwnerSource,
}

impl ArtifactOwner {
    pub fn task(&self) -> Option<&Task> {
        match &self.source {
            ArtifactOwnerSource::Task(task) => Some(task),
            ArtifactOwnerSource::Assistant(_) => None,
        }
    }
    pub fn session(&self) -> Option<&AssistantSession> {
        match &self.source {
            ArtifactOwnerSource::Assistant(session) => Some(session),
            ArtifactOwnerSource::Task(_) => None,
        }
    }
}

impl From<Task> for ArtifactOwner {
    fn from(task: Task) -> Self {
        Self {
            id: task.id.clone(),
            title: task.title.clone(),
            project: task.project.clone(),
            status: task.status,
            created: task.created,
            updated: task.updated,
            artifacts: task.artifacts.clone(),
            source: ArtifactOwnerSource::Task(Box::new(task)),
        }
    }
}
impl From<AssistantSession> for ArtifactOwner {
    fn from(session: AssistantSession) -> Self {
        Self {
            id: session.id.clone(),
            title: session.summary.clone(),
            project: None,
            status: if session.ended.is_some() {
                Status::Done
            } else {
                Status::Backlog
            },
            created: session.created,
            updated: session.ended.unwrap_or(session.created),
            artifacts: session.artifacts.clone(),
            source: ArtifactOwnerSource::Assistant(Box::new(session)),
        }
    }
}
