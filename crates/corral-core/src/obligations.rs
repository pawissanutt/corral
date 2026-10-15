//! Portable run obligations. Handles describe events; they never contain executable commands.
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationSource {
    Corral,
    Adapter,
    Worker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WaitHandle {
    /// Identity of the actual `corral gate` invocation, not a worker-selected result path.
    Gate { sha: String, pane: String },
    /// A declared harness channel. `wait_v1` declares bounded observation support, not a wake guarantee.
    Channel {
        channel_id: String,
        capability: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitRequest {
    pub version: u32,
    pub id: String,
    pub run: String,
    pub condition: String,
    pub owner: String,
    pub source: ObligationSource,
    pub deadline: Option<Timestamp>,
    pub handle: WaitHandle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequest {
    pub version: u32,
    pub id: String,
    pub run: String,
    pub question: String,
    pub requested_action: String,
    pub owner: String,
    pub source: ObligationSource,
    pub deadline: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitState {
    Pending,
    Ready,
    Failed,
    Cancelled,
    Lost,
    Expired,
    Unsupported,
    Unverifiable,
    MismatchedRun,
    DeliveryUnknown,
}

impl WaitState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Lost => "lost",
            Self::Expired => "expired",
            Self::Unsupported => "unsupported",
            Self::Unverifiable => "unverifiable",
            Self::MismatchedRun => "mismatched_run",
            Self::DeliveryUnknown => "delivery_unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitObservation {
    pub at: Timestamp,
    pub state: WaitState,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptOutcome {
    Ready,
    Failed,
    Cancelled,
    Lost,
    Answered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeDelivery {
    Acknowledged,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeReceipt {
    pub version: u32,
    pub id: String,
    pub run: String,
    pub outcome: ReceiptOutcome,
    pub delivery: ResumeDelivery,
    pub observed_at: Timestamp,
    pub answer: Option<String>,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitRecord {
    pub request: WaitRequest,
    pub registered_at: Timestamp,
    pub observation: WaitObservation,
    pub receipt: Option<ResumeReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub request: DecisionRequest,
    pub registered_at: Timestamp,
    pub observation: WaitObservation,
    pub receipt: Option<ResumeReceipt>,
}

impl DecisionRecord {
    pub fn pending(&self) -> bool {
        self.receipt.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunObligations {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waits: Vec<WaitRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<DecisionRecord>,
}

impl RunObligations {
    pub fn is_empty(&self) -> bool {
        self.waits.is_empty() && self.decisions.is_empty()
    }

    /// Exact owner questions take precedence over terminal heuristics.
    pub fn pending_decision(&self) -> Option<&DecisionRecord> {
        self.decisions.iter().find(|d| d.pending())
    }

    /// Compact shared readback for CLI, cards and detail, including diagnostic/uncertain states.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for d in &self.decisions {
            if d.pending() || d.observation.state == WaitState::DeliveryUnknown {
                lines.push(format!(
                    "decision {} [{}] ({}): {} Action: {}",
                    d.request.id,
                    d.observation.state.as_str(),
                    d.request.owner,
                    d.request.question,
                    d.request.requested_action
                ));
            }
        }
        for w in &self.waits {
            lines.push(format!(
                "wait {} [{}] ({}): {}",
                w.request.id,
                w.observation.state.as_str(),
                w.request.owner,
                w.request.condition
            ));
        }
        lines
    }
}
