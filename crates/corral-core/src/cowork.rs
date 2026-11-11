//! Completion responsibility is separate from immutable co-work provenance.
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};

/// Co-work source provenance, with either a run-bound legacy instruction or a versioned
/// completion scope. New scopes carry a minimum-reader fence in their review tag.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Cowork {
    #[serde(default)]
    pub completion: Option<crate::CompletionScope>,
    pub session: String,
    pub scratch: String,
    pub run: String,
    pub review: CoworkReview,
    /// The owner confirmed this instruction. Legacy true means Ship; new intent is explicit.
    #[serde(default)]
    pub owner_confirmed: bool,
    /// The owner's waiver of independent review for one finished run. Older readers ignore
    /// it and still require review; every new finish or transfer clears it.
    #[serde(default)]
    pub review_waiver: Option<ReviewWaiver>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoworkReview {
    None,
    Required,
    /// Minimum-reader fence: old controllers must not mistake Verify for legacy Ship.
    CompletionV1,
    /// Minimum reader for goal-bounded transfer/admission. Older controllers must refuse
    /// rather than treating a saved or plan-only transfer as an ordinary completion.
    TransferV1,
}

/// The owner landed this run after its gate without independent review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewWaiver {
    pub run: String,
    pub reason: String,
    pub at: jiff::Timestamp,
}

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

// Fail before persistence if a producer forgets the old-reader fence. Merely adding an
// optional intent field would let an old controller deserialize Verify as legacy Ship.
impl Serialize for crate::Cowork {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.completion.is_some()
            != matches!(
                self.review,
                crate::CoworkReview::CompletionV1 | crate::CoworkReview::TransferV1
            )
        {
            return Err(serde::ser::Error::custom(
                "completion scope requires the completion_v1 review fence",
            ));
        }
        let mut value = serializer.serialize_struct(
            "Cowork",
            4 + usize::from(self.owner_confirmed)
                + usize::from(self.completion.is_some())
                + usize::from(self.review_waiver.is_some()),
        )?;
        if let Some(scope) = &self.completion {
            value.serialize_field("completion", scope)?;
        }
        value.serialize_field("session", &self.session)?;
        value.serialize_field("scratch", &self.scratch)?;
        value.serialize_field("run", &self.run)?;
        value.serialize_field("review", &self.review)?;
        if self.owner_confirmed {
            value.serialize_field("owner_confirmed", &true)?;
        }
        if let Some(waiver) = &self.review_waiver {
            value.serialize_field("review_waiver", waiver)?;
        }
        value.end()
    }
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

    /// Only a confirmed Ship finish still driven by its assistant can skip review, and only
    /// for the exact run the owner waived.
    pub fn review_waived_for(&self, run: &str) -> bool {
        self.shipping()
            && self.completion.is_some()
            && self.assistant_driver().is_some()
            && self.review_waiver.as_ref().is_some_and(|w| w.run == run)
    }
}
