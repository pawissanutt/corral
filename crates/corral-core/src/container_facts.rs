//! Constant-size facts of one container (plan T-253 section 4, "Container facts"): counts, sums
//! and pointers that replace reads of a container's nested history. Each is derived from the
//! assembled container by the functions here and stored in the container's row; the revision
//! counters (`children_rev`, `obligations_rev`) are the store's and are never derived.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    AggregateAttempt, CheckpointRecord, DecisionRecord, HerdrRef, PlanRecord, PlanRun,
    ProfileSwitch, Run, RunOutcome, UsageTotals, Verdict, WaitState,
};

/// Fixed-width encodings that keep summary and fact documents constant in size as counts grow:
/// counts as 20-digit strings and costs as the 16 hex digits of their exact bits. Plain JSON
/// numbers are still accepted on read.
pub(crate) mod fixed {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Count {
        Text(String),
        Number(u64),
    }

    fn count<E: serde::de::Error>(value: Count) -> Result<u64, E> {
        match value {
            Count::Number(n) => Ok(n),
            Count::Text(text) => text.parse().map_err(E::custom),
        }
    }

    pub fn serialize<S: Serializer>(n: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{n:020}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        count(Count::deserialize(d)?)
    }

    pub mod map {
        use super::*;

        pub fn serialize<S: Serializer>(
            map: &BTreeMap<String, u64>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeMap;
            let mut out = s.serialize_map(Some(map.len()))?;
            for (key, n) in map {
                out.serialize_entry(key, &format!("{n:020}"))?;
            }
            out.end()
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<BTreeMap<String, u64>, D::Error> {
            BTreeMap::<String, Count>::deserialize(d)?
                .into_iter()
                .map(|(key, n)| Ok((key, count(n)?)))
                .collect()
        }
    }

    pub mod opt {
        use super::*;

        pub fn serialize<S: Serializer>(n: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
            match n {
                Some(n) => s.serialize_str(&format!("{n:020}")),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
            Option::<Count>::deserialize(d)?.map(count).transpose()
        }
    }

    pub mod float {
        use super::*;

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Float {
            Bits(String),
            Number(f64),
        }

        pub fn serialize<S: Serializer>(n: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
            match n {
                Some(n) => s.serialize_str(&format!("{:016x}", n.to_bits())),
                None => s.serialize_none(),
            }
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
            Option::<Float>::deserialize(d)?
                .map(|value| match value {
                    Float::Number(n) => Ok(n),
                    Float::Bits(bits) => u64::from_str_radix(&bits, 16)
                        .map(f64::from_bits)
                        .map_err(serde::de::Error::custom),
                })
                .transpose()
        }
    }
}

/// Reported, estimate-only and unpriced spend, folded per usage entry by the board card's
/// `TaskCost` rule.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostTotals {
    #[serde(with = "fixed::float")]
    pub reported: Option<f64>,
    #[serde(with = "fixed::float")]
    pub estimated: Option<f64>,
    #[serde(with = "fixed")]
    pub unpriced_tokens: u64,
}

impl CostTotals {
    pub fn of_run(run: &Run) -> CostTotals {
        let mut cost = CostTotals::default();
        for entry in run.total_usage().iter().flat_map(|u| u.entries.iter()) {
            match (entry.reported_usd, entry.estimated_usd) {
                (Some(r), _) => *cost.reported.get_or_insert(0.0) += r,
                (None, Some(e)) => *cost.estimated.get_or_insert(0.0) += e,
                (None, None) => cost.unpriced_tokens += entry.total_tokens(),
            }
        }
        cost
    }

    /// Adds another subtotal, as the summary folds a demoted run.
    pub fn add(&mut self, other: &CostTotals) {
        if let Some(r) = other.reported {
            *self.reported.get_or_insert(0.0) += r;
        }
        if let Some(e) = other.estimated {
            *self.estimated.get_or_insert(0.0) += e;
        }
        self.unpriced_tokens += other.unpriced_tokens;
    }
}

/// The constant-size metadata of one checkpoint; its payload is the canonical `Checkpoint`
/// document stored once per `payload_digest` (plan section 4, "Checkpoints").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointMeta {
    /// 1-based position in the run's checkpoint list.
    #[serde(with = "fixed")]
    pub ordinal: u64,
    pub artifact: String,
    #[serde(with = "fixed")]
    pub revision: u64,
    pub captured: Timestamp,
    pub task: String,
    pub run: String,
    pub spec_revision: String,
    #[serde(with = "fixed")]
    pub progress_revision: u64,
    pub head: String,
    pub base: String,
    pub tree: String,
    pub payload_digest: String,
}

impl CheckpointMeta {
    pub fn of(ordinal: u64, record: &CheckpointRecord) -> serde_json::Result<CheckpointMeta> {
        let c = &record.checkpoint;
        Ok(CheckpointMeta {
            ordinal,
            artifact: record.artifact.clone(),
            revision: c.revision,
            captured: c.captured,
            task: c.input.task.clone(),
            run: c.input.run.clone(),
            spec_revision: c.input.spec_revision.clone(),
            progress_revision: c.input.progress_revision,
            head: c.input.head.clone(),
            base: c.input.base.clone(),
            tree: c.input.tree.clone(),
            payload_digest: payload_digest(&serde_json::to_value(c)?)?,
        })
    }
}

/// The content address of a checkpoint payload document.
pub fn payload_digest(payload: &serde_json::Value) -> serde_json::Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_string(payload)?.as_bytes())
    ))
}

/// One pane a container recorded (plan section 4, "Pane references and cleanup").
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PaneRef {
    /// The recording container: `run:<seq>/<child>`, `plan_run:<seq>`, `plan_review:<plan>/<n>`
    /// or `aggregate_review:<attempt>/<n>`.
    pub owner: String,
    /// `run`, `gate`, `plan_run`, `plan_review` or `aggregate_review`.
    pub kind: String,
    pub pane: String,
    /// `""` for legacy socket-less references.
    pub socket: String,
    pub workspace: Option<String>,
}

impl PaneRef {
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.owner, self.kind, self.pane)
    }

    fn of(owner: &str, kind: &str, herdr: &HerdrRef) -> PaneRef {
        PaneRef {
            owner: owner.to_owned(),
            kind: kind.to_owned(),
            pane: herdr.pane_id.clone(),
            socket: socket_text(herdr.socket.as_deref()),
            workspace: Some(herdr.workspace_id.clone()),
        }
    }
}

fn socket_text(socket: Option<&std::path::Path>) -> String {
    socket
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The panes one run (or review child) itself recorded: its agent pane and its gate pane.
pub fn run_panes(owner: &str, run: &Run) -> Vec<PaneRef> {
    own_panes(owner, run.herdr.as_ref(), run.gate.as_ref())
}

/// The panes of a run's own agent pane and gate record.
pub fn own_panes(
    owner: &str,
    herdr: Option<&HerdrRef>,
    gate: Option<&crate::GateRecord>,
) -> Vec<PaneRef> {
    let mut panes = Vec::new();
    if let Some(herdr) = herdr {
        panes.push(PaneRef::of(owner, "run", herdr));
    }
    if let Some(gate) = gate
        && let Some(pane) = &gate.pane
    {
        panes.push(PaneRef {
            owner: owner.to_owned(),
            kind: "gate".to_owned(),
            pane: pane.clone(),
            socket: socket_text(
                gate.socket
                    .as_deref()
                    .or(herdr.and_then(|h| h.socket.as_deref())),
            ),
            workspace: None,
        });
    }
    panes
}

pub fn plan_run_panes(owner: &str, run: &PlanRun) -> Vec<PaneRef> {
    run.herdr
        .iter()
        .map(|herdr| PaneRef::of(owner, "plan_run", herdr))
        .collect()
}

pub fn herdr_panes(owner: &str, kind: &str, herdr: Option<&HerdrRef>) -> Vec<PaneRef> {
    herdr
        .map(|herdr| vec![PaneRef::of(owner, kind, herdr)])
        .unwrap_or_default()
}

pub(crate) fn count_sockets(panes: impl IntoIterator<Item = PaneRef>) -> BTreeMap<String, u64> {
    let mut sockets = BTreeMap::new();
    for pane in panes {
        *sockets.entry(pane.socket).or_default() += 1;
    }
    sockets
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContainerFacts {
    /// Review children, plan reviews or aggregate reviews.
    #[serde(with = "fixed")]
    pub children: u64,
    /// Advances on every write that changes a child row's stored document.
    #[serde(with = "fixed")]
    pub children_rev: u64,
    #[serde(with = "fixed")]
    pub open_children: u64,
    pub first_child_started: Option<Timestamp>,
    /// Distinct harnesses of the run and its children, with their counts.
    #[serde(with = "fixed::map")]
    pub harnesses: BTreeMap<String, u64>,
    /// Children whose review asked for changes and that were not superseded.
    #[serde(with = "fixed")]
    pub changes_unsuperseded: u64,
    /// Plan reviews whose verdict asked for changes.
    #[serde(with = "fixed")]
    pub changes: u64,
    /// Plan or aggregate reviews still waiting for a verdict.
    #[serde(with = "fixed")]
    pub pending_children: u64,
    /// `Run::total_usage` of the run and its children, without entries.
    pub usage_total: Option<UsageTotals>,
    pub cost: CostTotals,
    #[serde(with = "fixed")]
    pub checkpoints: u64,
    pub latest_checkpoint: Option<CheckpointMeta>,
    #[serde(with = "fixed")]
    pub switches: u64,
    pub last_switch: Option<ProfileSwitch>,
    pub pending_decision: Option<DecisionRecord>,
    /// The 1-based position of `pending_decision` among the run's decisions.
    #[serde(with = "fixed::opt")]
    pub pending_decision_ordinal: Option<u64>,
    #[serde(with = "fixed")]
    pub waits: u64,
    #[serde(with = "fixed")]
    pub decisions: u64,
    #[serde(with = "fixed")]
    pub pending_waits: u64,
    /// Waits and decisions without a resume receipt.
    #[serde(with = "fixed")]
    pub open_obligations: u64,
    /// Advances on every write that changes a wait or decision row's stored document.
    #[serde(with = "fixed")]
    pub obligations_rev: u64,
    /// Pane references of the container and its children, per socket.
    #[serde(with = "fixed::map")]
    pub pane_refs: BTreeMap<String, u64>,
    #[serde(with = "fixed")]
    pub pending_questions: u64,
    #[serde(with = "fixed")]
    pub armed_wakes: u64,
    #[serde(with = "fixed")]
    pub agent_records: u64,
}

impl ContainerFacts {
    /// These facts with both revision counters cleared, for comparing with a derivation.
    pub fn without_revisions(&self) -> ContainerFacts {
        ContainerFacts {
            children_rev: 0,
            obligations_rev: 0,
            ..self.clone()
        }
    }

    /// The facts of a coding run (or review child) `owner` at `<seq>/<child>`.
    pub fn of_run(owner: &str, run: &Run) -> serde_json::Result<ContainerFacts> {
        let children = &run.review_runs;
        let mut harnesses = BTreeMap::new();
        for harness in std::iter::once(run)
            .chain(children)
            .filter_map(|r| r.harness.clone())
        {
            *harnesses.entry(harness).or_default() += 1;
        }
        let mut panes = run_panes(&format!("run:{owner}"), run);
        for (n, child) in children.iter().enumerate() {
            let child_owner = format!("run:{}", child_owner(owner, n + 1));
            panes.extend(run_panes(&child_owner, child));
        }
        let host = run.agent_host.as_deref();
        Ok(ContainerFacts {
            children: len_u64(children.len()),
            children_rev: 0,
            open_children: count(children.iter().filter(|c| c.ended.is_none())),
            first_child_started: children.first().map(|c| c.started),
            harnesses,
            changes_unsuperseded: count(children.iter().filter(|c| changes_unsuperseded(c))),
            changes: 0,
            pending_children: 0,
            usage_total: run.total_usage().map(UsageTotals::of),
            cost: CostTotals::of_run(run),
            checkpoints: len_u64(run.checkpoints.len()),
            latest_checkpoint: run
                .checkpoints
                .last()
                .map(|record| CheckpointMeta::of(len_u64(run.checkpoints.len()), record))
                .transpose()?,
            switches: len_u64(run.switches.len()),
            last_switch: run.switches.last().cloned(),
            pending_decision: run.obligations.pending_decision().cloned(),
            pending_decision_ordinal: run
                .obligations
                .decisions
                .iter()
                .position(DecisionRecord::pending)
                .map(|i| len_u64(i + 1)),
            waits: len_u64(run.obligations.waits.len()),
            decisions: len_u64(run.obligations.decisions.len()),
            pending_waits: count(
                run.obligations
                    .waits
                    .iter()
                    .filter(|w| w.observation.state == WaitState::Pending),
            ),
            open_obligations: count(run.obligations.waits.iter().filter(|w| w.receipt.is_none()))
                + count(
                    run.obligations
                        .decisions
                        .iter()
                        .filter(|d| d.receipt.is_none()),
                ),
            obligations_rev: 0,
            pane_refs: count_sockets(panes),
            pending_questions: host.map_or(0, |h| {
                count(h.questions.iter().filter(|q| q.state.pending()))
            }),
            armed_wakes: host.map_or(0, |h| {
                count(h.wakes.iter().filter(|w| {
                    matches!(
                        w.state,
                        crate::agent::WakeState::Armed | crate::agent::WakeState::PendingStart
                    )
                }))
            }),
            agent_records: host.map_or(0, |h| {
                len_u64(
                    h.generations.len()
                        + h.receipts.len()
                        + h.questions.len()
                        + h.turns.len()
                        + h.wakes.len()
                        + h.reports.len(),
                )
            }),
        })
    }

    pub fn of_plan_run(owner: &str, run: &PlanRun) -> ContainerFacts {
        ContainerFacts {
            pane_refs: count_sockets(plan_run_panes(&format!("plan_run:{owner}"), run)),
            ..ContainerFacts::default()
        }
    }

    pub fn of_plan(owner: &str, plan: &PlanRecord) -> ContainerFacts {
        let panes = plan.reviews.iter().enumerate().flat_map(|(n, review)| {
            herdr_panes(
                &format!("plan_review:{owner}/{:06}", n + 1),
                "plan_review",
                review.herdr.as_ref(),
            )
        });
        ContainerFacts {
            children: len_u64(plan.reviews.len()),
            changes: count(
                plan.reviews
                    .iter()
                    .filter(|r| r.verdict == Some(Verdict::Changes)),
            ),
            pending_children: count(plan.reviews.iter().filter(|r| r.verdict.is_none())),
            pane_refs: count_sockets(panes),
            ..ContainerFacts::default()
        }
    }

    pub fn of_attempt(owner: &str, attempt: &AggregateAttempt) -> ContainerFacts {
        let panes = attempt.reviews.iter().enumerate().flat_map(|(n, review)| {
            herdr_panes(
                &format!("aggregate_review:{owner}/{:06}", n + 1),
                "aggregate_review",
                review.herdr.as_ref(),
            )
        });
        ContainerFacts {
            children: len_u64(attempt.reviews.len()),
            pending_children: count(attempt.reviews.iter().filter(|r| r.pending())),
            pane_refs: count_sockets(panes),
            ..ContainerFacts::default()
        }
    }
}

/// The key of review child `n` (1-based) of the run at `owner`.
pub fn child_owner(owner: &str, n: usize) -> String {
    let run = owner.split('/').next().unwrap_or(owner);
    format!("{run}/{n:06}")
}

/// A review child that asked for changes and was not superseded counts toward the
/// implementation review round.
pub fn changes_unsuperseded(child: &Run) -> bool {
    child.outcome != Some(RunOutcome::Superseded)
        && child
            .review
            .as_ref()
            .is_some_and(|r| r.verdict == Some(Verdict::Changes))
}

pub(crate) fn len_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn count<T>(items: impl Iterator<Item = T>) -> u64 {
    len_u64(items.count())
}

impl UsageTotals {
    pub fn of(usage: crate::Usage) -> UsageTotals {
        UsageTotals {
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            cost_usd: usage.cost_usd,
            reported_usd: usage.reported_usd,
            estimated_usd: usage.estimated_usd,
            unpriced_tokens: usage.unpriced_tokens,
            cost_basis: usage.cost_basis,
        }
    }
}
