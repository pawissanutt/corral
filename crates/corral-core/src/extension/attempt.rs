//! Context attempt states and terminal outcomes (plan T-381 sections 4.6 and 6.2).

use super::error::ErrorCode;
use serde::{Deserialize, Serialize};

/// Reason recorded when recovery finds the host or child process gone.
pub const PROCESS_LOST: &str = "process_lost";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AttemptState {
    Starting,
    Running,
    Accepted,
    Superseded,
    Cancelled,
    DeadlineExceeded,
    Failed {
        code: ErrorCode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum TerminalOutcome {
    Accepted,
    Superseded,
    Cancelled,
    DeadlineExceeded,
    Failed {
        code: ErrorCode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl TerminalOutcome {
    /// A failure outcome. Codes with a dedicated outcome (`superseded`, `cancelled`,
    /// `deadline_exceeded`) map to it, so each terminal fact has one representation.
    pub fn failed(code: ErrorCode, reason: Option<&str>) -> Self {
        match code {
            ErrorCode::Superseded => Self::Superseded,
            ErrorCode::Cancelled => Self::Cancelled,
            ErrorCode::DeadlineExceeded => Self::DeadlineExceeded,
            code => Self::Failed {
                code,
                reason: reason.map(str::to_string),
            },
        }
    }

    pub fn process_lost() -> Self {
        Self::failed(ErrorCode::Internal, Some(PROCESS_LOST))
    }
}

impl From<TerminalOutcome> for AttemptState {
    fn from(outcome: TerminalOutcome) -> Self {
        match outcome {
            TerminalOutcome::Accepted => Self::Accepted,
            TerminalOutcome::Superseded => Self::Superseded,
            TerminalOutcome::Cancelled => Self::Cancelled,
            TerminalOutcome::DeadlineExceeded => Self::DeadlineExceeded,
            TerminalOutcome::Failed { code, reason } => Self::Failed { code, reason },
        }
    }
}

impl AttemptState {
    pub fn outcome(&self) -> Option<TerminalOutcome> {
        match self {
            Self::Starting | Self::Running => None,
            Self::Accepted => Some(TerminalOutcome::Accepted),
            Self::Superseded => Some(TerminalOutcome::Superseded),
            Self::Cancelled => Some(TerminalOutcome::Cancelled),
            Self::DeadlineExceeded => Some(TerminalOutcome::DeadlineExceeded),
            Self::Failed { code, reason } => Some(TerminalOutcome::Failed {
                code: *code,
                reason: reason.clone(),
            }),
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.outcome().is_some()
    }

    /// `starting -> running -> terminal`. A starting attempt may also end without running
    /// (cancel before publication, deadline, host loss), but only `running` can be accepted
    /// or superseded. Terminal states never change.
    pub fn can_transition_to(&self, next: &AttemptState) -> bool {
        match self {
            Self::Starting => matches!(
                next,
                Self::Running | Self::Cancelled | Self::DeadlineExceeded | Self::Failed { .. }
            ),
            Self::Running => next.is_terminal(),
            _ => false,
        }
    }
}
