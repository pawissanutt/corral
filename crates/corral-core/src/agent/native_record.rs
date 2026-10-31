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
                    UsageSessionKind::Summary => "summary",
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
