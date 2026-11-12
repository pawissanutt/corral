//! Which tasks one sync pass may take task effects on (dispatch, repair, review, landing, queue
//! admission, autopilot advance). Fact observation, recovery and notification stay workspace-wide.
use anyhow::Result;
use corral_core::TaskHead;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncScope {
    /// The ordinary workspace sync.
    All,
    /// Only tasks whose stored project equals this one. A project-less task belongs only to
    /// `Project(None)`, so a named project never reaches it.
    Project(Option<String>),
}

impl SyncScope {
    pub(crate) fn includes(&self, project: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Project(scope) => scope.as_deref() == project,
        }
    }

    /// The in-scope heads of a pass's read, in their order.
    pub(crate) fn heads(&self, mut heads: Vec<TaskHead>) -> Vec<TaskHead> {
        heads.retain(|head| self.includes(head.project.as_deref()));
        heads
    }

    /// A phase whose foreign-task effects cannot be filtered runs only in the workspace sync;
    /// a scoped pass skips it and the next ordinary sync runs it.
    pub(crate) fn workspace_only<T: Default>(
        &self,
        phase: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        match self {
            Self::All => phase(),
            Self::Project(_) => Ok(T::default()),
        }
    }
}
