//! A single native child record. No variant can carry an agent's accumulated history.
use super::*;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum NativeRecord {
    Generation(AgentGeneration),
    Receipt(RequestReceipt),
    Question(AgentQuestion),
    Turn(AgentTurn),
    Wake(WakeSubscription),
    Report(AgentReport),
    Job(JobRecord),
}

impl NativeRecord {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Generation(_) => "generations",
            Self::Receipt(_) => "receipts",
            Self::Question(_) => "questions",
            Self::Turn(_) => "turns",
            Self::Wake(_) => "wakes",
            Self::Report(_) => "reports",
            Self::Job(_) => "jobs",
        }
    }
    pub fn token(&self) -> String {
        match self {
            Self::Generation(g) => format!("gen:{}", g.n),
            Self::Receipt(r) => format!("request:{}", r.request_id),
            Self::Question(q) => format!("key:{}", q.key),
            Self::Turn(t) => format!("turn:{}:{}", t.generation, t.seq),
            Self::Wake(w) => format!("request:{}", w.request_id),
            Self::Report(r) => format!("request:{}", r.request_id),
            Self::Job(j) => format!("id:{}", j.id),
        }
    }
    /// Work that makes an otherwise settled container owe another reconciliation tick.
    pub fn pending(&self) -> bool {
        match self {
            Self::Receipt(r) => {
                matches!(
                    r.state,
                    ReceiptState::Reserved | ReceiptState::Queued | ReceiptState::Uncertain
                ) || !r.post_pending.is_empty()
            }
            Self::Question(q) => q.recorded_pending(),
            Self::Job(j) => j.needs_reap(),
            _ => false,
        }
    }
    pub(crate) fn metadata(&self) -> (bool, Vec<String>) {
        let mut index = vec![self.token()];
        let open = match self {
            Self::Generation(_) | Self::Report(_) => false,
            Self::Turn(t) => {
                index.push(format!("generation:{}", t.generation));
                if let Some(ended) = t.ended {
                    index.push(format!("ended:{}:{ended}", t.generation));
                }
                t.ended.is_none()
            }
            Self::Wake(w) => {
                let active = matches!(w.state, WakeState::Armed | WakeState::PendingStart);
                if active {
                    index.push("wake:active".into());
                }
                active
            }
            Self::Receipt(r) => {
                if r.result["queued"] == true
                    && let Some(effect) = &r.effect
                {
                    index.push(format!("queued:effect:{}", effect.reference));
                }
                if r.op == "gate" && r.state == ReceiptState::Queued {
                    index.push("gate:queued".into());
                }
                self.pending()
            }
            _ => self.pending(),
        };
        if self.pending() {
            index.push("native:pending".into());
        }
        (open, index)
    }
    pub(crate) fn put(self, state: &mut AgentRunState) -> bool {
        fn put<T: PartialEq>(rows: &mut Vec<T>, value: T, same: impl Fn(&T, &T) -> bool) -> bool {
            if let Some(before) = rows.iter_mut().find(|row| same(row, &value)) {
                if *before == value {
                    return false;
                }
                *before = value;
            } else {
                rows.push(value);
            }
            true
        }
        match self {
            Self::Generation(r) => put(&mut state.generations, r, |a, b| a.n == b.n),
            Self::Receipt(r) => put(&mut state.receipts, r, |a, b| a.request_id == b.request_id),
            Self::Question(r) => put(&mut state.questions, r, |a, b| a.key == b.key),
            Self::Turn(r) => put(&mut state.turns, r, |a, b| {
                (a.generation, a.seq) == (b.generation, b.seq)
            }),
            Self::Wake(r) => put(&mut state.wakes, r, |a, b| a.request_id == b.request_id),
            Self::Report(r) => put(&mut state.reports, r, |a, b| a.request_id == b.request_id),
            Self::Job(r) => put(&mut state.jobs, r, |a, b| a.id == b.id),
        }
    }

    pub(crate) fn from_doc(kind: &str, doc: Value) -> Result<Self> {
        let parsed = match kind {
            "generations" => serde_json::from_value(doc).map(Self::Generation),
            "receipts" => serde_json::from_value(doc).map(Self::Receipt),
            "questions" => serde_json::from_value(doc).map(Self::Question),
            "turns" => serde_json::from_value(doc).map(Self::Turn),
            "wakes" => serde_json::from_value(doc).map(Self::Wake),
            "reports" => serde_json::from_value(doc).map(Self::Report),
            "jobs" => serde_json::from_value(doc).map(Self::Job),
            _ => bail!("unknown native record family {kind}"),
        };
        parsed.with_context(|| format!("decoding native {kind} record"))
    }
}
