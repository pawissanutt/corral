//! A single native child record. No variant can carry an agent's accumulated history.
use super::*;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum NativeRecord {
    Generation(Box<AgentGeneration>),
    Receipt(RequestReceipt),
    Question(AgentQuestion),
    Turn(AgentTurn),
    Wake(WakeSubscription),
    Report(AgentReport),
    Job(JobRecord),
    Rewind(RewindRecord),
    UsageSession(UsageSession),
    BudgetRequest(BudgetRequest),
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
            Self::Rewind(_) => "rewinds",
            Self::UsageSession(_) => "usage_sessions",
            Self::BudgetRequest(_) => "budget_requests",
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
            Self::Rewind(r) => format!("id:{}", r.id),
            Self::UsageSession(s) => {
                let kind = match s.kind {
                    UsageSessionKind::Explore => "explore",
                    UsageSessionKind::Oracle => "oracle",
                    UsageSessionKind::Shadow => "shadow",
                    UsageSessionKind::Summary => "summary",
                    UsageSessionKind::WebSearch => "web_search",
                };
                format!("session:{}:{kind}:{}", s.generation, s.session_id)
            }
            Self::BudgetRequest(r) => format!("request:{}", r.request),
        }
    }
    /// Work that makes an otherwise settled container owe another reconciliation tick.
    pub fn pending(&self) -> bool {
        match self {
            Self::Receipt(r) => r.effects_pending() || r.recovery_update_due(),
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
                    index.push("turn:ended".into());
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
                if matches!(r.op.as_str(), "done" | "verdict")
                    && r.state == ReceiptState::Committed
                    && let Some(turn) = r.identity.turn
                {
                    index.push(format!("report:{}:{}:{turn}", r.op, r.identity.generation));
                }
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
            Self::Generation(r) => put(&mut state.generations, *r, |a, b| a.n == b.n),
            Self::Receipt(r) => put(&mut state.receipts, r, |a, b| a.request_id == b.request_id),
            Self::Question(r) => put(&mut state.questions, r, |a, b| a.key == b.key),
            Self::Turn(r) => put(&mut state.turns, r, |a, b| {
                (a.generation, a.seq) == (b.generation, b.seq)
            }),
            Self::Wake(r) => put(&mut state.wakes, r, |a, b| a.request_id == b.request_id),
            Self::Report(r) => put(&mut state.reports, r, |a, b| a.request_id == b.request_id),
            Self::Job(r) => put(&mut state.jobs, r, |a, b| a.id == b.id),
            Self::Rewind(r) => put(&mut state.rewinds, r, |a, b| a.id == b.id),
            Self::UsageSession(r) => put(&mut state.usage_sessions, r, |a, b| {
                a.generation == b.generation && a.kind == b.kind && a.session_id == b.session_id
            }),
            Self::BudgetRequest(r) => {
                put(&mut state.budget.requests, r, |a, b| a.request == b.request)
            }
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
            "rewinds" => serde_json::from_value(doc).map(Self::Rewind),
            "usage_sessions" => serde_json::from_value(doc).map(Self::UsageSession),
            "budget_requests" => serde_json::from_value(doc).map(Self::BudgetRequest),
            _ => bail!("unknown native record family {kind}"),
        };
        parsed.with_context(|| format!("decoding native {kind} record"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn boxed_generation_preserves_bytes_keys_roundtrip_and_folding() {
        let generation: AgentGeneration = serde_json::from_value(json!({
            "n": 3, "bind_nonce_sha256": "00".repeat(32), "pane": "p1",
            "created": "2026-10-23T00:00:00Z", "event_seq": 42,
            "todo": true, "assets_sha256": "11".repeat(32)
        }))
        .unwrap();
        let record = NativeRecord::Generation(Box::new(generation.clone()));
        let bytes = serde_json::to_vec(&generation).unwrap();
        assert_eq!(serde_json::to_vec(&record).unwrap(), bytes);
        assert_eq!(record.kind(), "generations");
        assert_eq!(record.token(), "gen:3");
        assert_eq!(record.metadata(), (false, vec!["gen:3".into()]));
        assert!(!record.pending());
        let decoded =
            NativeRecord::from_doc("generations", serde_json::from_slice(&bytes).unwrap()).unwrap();
        assert_eq!(decoded, record);
        let binding = serde_json::from_value(json!({
            "role": "worker", "store": "store", "project": "p", "generation": 3
        }))
        .unwrap();
        let mut state = AgentRunState::new(binding);
        assert!(decoded.clone().put(&mut state));
        assert_eq!(state.generations, vec![generation.clone()]);
        assert!(!decoded.put(&mut state));
        let mut updated = generation;
        updated.event_seq = 43;
        assert!(NativeRecord::Generation(Box::new(updated.clone())).put(&mut state));
        assert_eq!(state.generations, vec![updated.clone()]);
        assert!(!NativeRecord::Generation(Box::new(updated)).put(&mut state));
    }
}
