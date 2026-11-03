//! Operational holds do not change lifecycle, task contracts or worker sessions.
use anyhow::{Result, ensure};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskHold {
    pub reason: String,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<crate::ReviewWork>,
    /// Only focus-owned holds are automatically released. Manual holds survive focus clear.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
}

impl TaskHold {
    pub fn refusal(&self) -> String {
        format!("Held: {}", self.reason)
    }

    pub fn require_clear<T: crate::TaskFieldsRef + ?Sized>(task: &T) -> Result<()> {
        ensure!(
            task.hold().is_none(),
            "{}: {}",
            task.id(),
            task.hold().as_ref().map_or_else(String::new, Self::refusal)
        );
        Ok(())
    }
}
