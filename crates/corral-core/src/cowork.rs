//! Completion responsibility is separate from immutable co-work provenance.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionIntent {
    Verify,
    Ship,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompletionDriver {
    Assistant { session: String, scratch: String },
    Board,
}

/// A task-scoped instruction. Transfers change the driver, not the candidate or repair budget.
/// Unknown versions and changed contracts must refuse continuation, never fall back to Ship.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionScope {
    pub version: u32,
    pub contract_revision: String,
    pub intent: CompletionIntent,
    pub driver: CompletionDriver,
    pub instruction: String,
}

impl crate::Cowork {
    /// Legacy finishes are run-bound. Versioned instructions survive ordinary run replacement;
    /// their exact task contract is rechecked by the operating resolver before every effect.
    pub fn applies_to(&self, run: &str) -> bool {
        self.completion.is_some() || self.run == run
    }

    /// A transferred source candidate is still workerless. A later board run (or an
    /// explicitly materialized worker pane) holds ordinary worker capacity.
    pub fn has_worker(&self, current_run: &str, recorded_worker: bool) -> bool {
        self.assistant_driver().is_none() && (self.run != current_run || recorded_worker)
    }

    pub fn assistant_driver(&self) -> Option<(&str, &str)> {
        match self.completion.as_ref().map(|scope| &scope.driver) {
            Some(CompletionDriver::Board) => None,
            Some(CompletionDriver::Assistant { session, scratch }) => Some((session, scratch)),
            None => Some((&self.session, &self.scratch)),
        }
    }

    pub fn shipping(&self) -> bool {
        self.owner_confirmed
            && self
                .completion
                .as_ref()
                .is_none_or(|scope| scope.version == 1 && scope.intent == CompletionIntent::Ship)
    }
}
