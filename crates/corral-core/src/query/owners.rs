//! Content metadata comes from its real owner, never a fabricated Task with empty history.
use super::{Predicate, SearchContext, contains, glob};
use crate::assistant::AssistantSession;
use crate::{ArtifactOwner, ArtifactOwnerSource, Status, Task};

#[derive(Clone, Copy)]
pub enum ContentOwner<'a> {
    Task(&'a Task),
    Assistant(&'a AssistantSession),
    Workspace,
}
impl<'a> From<&'a Task> for ContentOwner<'a> {
    fn from(task: &'a Task) -> Self {
        Self::Task(task)
    }
}
impl<'a> From<&'a ArtifactOwner> for ContentOwner<'a> {
    fn from(owner: &'a ArtifactOwner) -> Self {
        match &owner.source {
            ArtifactOwnerSource::Task(task) => Self::Task(task),
            ArtifactOwnerSource::Assistant(session) => Self::Assistant(session),
        }
    }
}
impl ContentOwner<'_> {
    pub(super) fn metadata_matches(
        self,
        predicate: &Predicate,
        context: SearchContext<'_>,
    ) -> bool {
        let (id, status, artifacts) = match self {
            Self::Assistant(session) => (
                session.id.as_str(),
                if session.ended.is_some() {
                    Status::Done
                } else {
                    Status::Backlog
                },
                !session.artifacts.is_empty(),
            ),
            Self::Workspace => ("", Status::Backlog, false),
            Self::Task(task) => return predicate.score(task, context).is_some(),
        };
        // Preserve the catalog's lifecycle labels. Sessions/workspace records have no task
        // project, run/model/harness, coding rounds or task cost/token aggregate.
        match predicate {
            Predicate::Status(wanted) => status == *wanted,
            Predicate::Id(value) => glob(value, &id.to_lowercase()),
            Predicate::Pinned => context.pinned,
            Predicate::HasArtifact => artifacts,
            Predicate::Sha(value) => contains(context.extra_text, value),
            Predicate::Project(_)
            | Predicate::Profile(_)
            | Predicate::Model(_)
            | Predicate::Harness(_)
            | Predicate::Cost(_)
            | Predicate::Rounds(_)
            | Predicate::Tokens(_)
            | Predicate::Text(_)
            | Predicate::Content(_, _)
            | Predicate::Kind(_)
            | Predicate::Since(_)
            | Predicate::Before(_) => false,
        }
    }
}
