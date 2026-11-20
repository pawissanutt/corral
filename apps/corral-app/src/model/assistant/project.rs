//! The default filing project is orthogonal to the record Ask is about.
use crate::model::{Id, projects::Scope};
use corral_protocol::assistant::ContextRef;

/// The project Ask files under by default: the one in scope.
pub fn default_project(scope: &Scope) -> Option<Id> {
    scope.project().cloned()
}

pub fn project_context(id: &Id) -> ContextRef {
    ContextRef {
        kind: "project".into(),
        id: id.to_string(),
        rev: None,
    }
}
