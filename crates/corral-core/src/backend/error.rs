//! Typed failures of the storage contract (plan T-253 sections 1-2).

use std::fmt;
use std::time::Duration;

use super::rows::ChangeSeq;

/// A stale precondition: the stored `found` differs from the `expected` the writer read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub subject: String,
    pub expected: String,
    pub found: String,
}

impl Conflict {
    pub fn new(
        subject: impl Into<String>,
        expected: impl fmt::Debug,
        found: impl fmt::Debug,
    ) -> Self {
        Self {
            subject: subject.into(),
            expected: format!("{expected:?}"),
            found: format!("{found:?}"),
        }
    }
}

/// Why an operation applied nothing.
#[derive(Debug)]
pub enum ApplyError {
    Conflict(Conflict),
    Precondition(String),
    Busy { retry_after: Duration },
    Io(anyhow::Error),
}

impl ApplyError {
    pub fn precondition(message: impl Into<String>) -> Self {
        Self::Precondition(message.into())
    }
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(c) => write!(
                f,
                "conflict on {}: expected {}, found {}",
                c.subject, c.expected, c.found
            ),
            Self::Precondition(message) => write!(f, "precondition failed: {message}"),
            Self::Busy { retry_after } => write!(f, "store busy; retry after {retry_after:?}"),
            Self::Io(error) => write!(f, "storage i/o: {error:#}"),
        }
    }
}

impl std::error::Error for ApplyError {}

impl From<Conflict> for ApplyError {
    fn from(conflict: Conflict) -> Self {
        Self::Conflict(conflict)
    }
}

/// Why a view read returned nothing.
#[derive(Debug)]
pub enum ViewError {
    /// The view is older than `view_max_age`; open a new view.
    Expired {
        age: Duration,
    },
    Io(anyhow::Error),
}

impl fmt::Display for ViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Expired { age } => write!(f, "read view expired after {age:?}"),
            Self::Io(error) => write!(f, "storage i/o: {error:#}"),
        }
    }
}

impl std::error::Error for ViewError {}

/// Why a change-feed read returned nothing.
#[derive(Debug)]
pub enum FeedError {
    /// The feed was pruned past the cursor; rebuild the derivation from `oldest`.
    Gap {
        oldest: ChangeSeq,
    },
    View(ViewError),
}

impl fmt::Display for FeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gap { oldest } => {
                write!(f, "change feed pruned; oldest retained is {}", oldest.0)
            }
            Self::View(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for FeedError {}

impl From<ViewError> for FeedError {
    fn from(error: ViewError) -> Self {
        Self::View(error)
    }
}
