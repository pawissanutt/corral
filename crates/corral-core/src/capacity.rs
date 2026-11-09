//! Independent scheduler budgets. Scheduling lanes never convey operating authority.
use serde::{Deserialize, Serialize};

pub(crate) mod activation;

pub(crate) fn limits_problems(limits: &crate::Limits) -> Vec<String> {
    if let Some(capacity) = &limits.capacity {
        return capacity.problems();
    }
    let mut problems = Vec::new();
    if limits.running == 0 {
        problems.push("limits.running must be > 0".into());
    }
    if limits
        .running_target
        .is_some_and(|t| t == 0 || t > limits.running)
    {
        problems.push("limits.running_target must be within 1..=limits.running".into());
    }
    if limits.review == 0 {
        problems.push("limits.review must be > 0".into());
    }
    problems
}

/// `false` is an explicit legacy-mode override, including over inherited user defaults.
/// `true` is refused: enabling requires all four explicit limits.
pub(crate) fn deserialize_optional<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<CapacityLimits>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Value {
        Limits(CapacityLimits),
        Disabled(bool),
    }
    match Option::<Value>::deserialize(deserializer)? {
        Some(Value::Limits(limits)) => Ok(Some(limits)),
        Some(Value::Disabled(false)) | None => Ok(None),
        Some(Value::Disabled(true)) => Err(serde::de::Error::custom(
            "capacity activation requires four explicit limits, not true",
        )),
    }
}

/// Explicit limits for the independent agent/gate scheduler. Legacy limits are not inferred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityLimits {
    pub unfinished: usize,
    pub agents: usize,
    pub gates: usize,
    pub adhoc_reserve: usize,
}

impl CapacityLimits {
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (key, value) in [
            ("unfinished", self.unfinished),
            ("agents", self.agents),
            ("gates", self.gates),
        ] {
            if value == 0 {
                out.push(format!("limits.capacity.{key} must be > 0"));
            }
        }
        if self.adhoc_reserve >= self.agents || self.adhoc_reserve >= self.unfinished {
            out.push(
                "limits.capacity.adhoc_reserve must be less than both agents and unfinished".into(),
            );
        }
        out
    }

    pub fn normal_agents(&self) -> usize {
        self.agents.saturating_sub(self.adhoc_reserve)
    }

    pub fn normal_unfinished(&self) -> usize {
        self.unfinished.saturating_sub(self.adhoc_reserve)
    }
}

impl crate::Limits {
    pub fn agent_limit(&self) -> usize {
        self.capacity.as_ref().map_or(self.running, |c| c.agents)
    }

    pub fn unfinished_limit(&self) -> usize {
        self.capacity.as_ref().map_or(self.wip, |c| c.unfinished)
    }
}

/// Durable scheduling classification, independent of who authorized an operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityLane {
    #[default]
    Normal,
    Adhoc,
}

impl CapacityLane {
    pub fn is_normal(&self) -> bool {
        *self == Self::Normal
    }
}

/// One independent resource pool's observed usage, including durable launch reservations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapacityPool {
    pub normal: usize,
    pub adhoc: usize,
}

impl CapacityPool {
    pub fn total(&self) -> usize {
        self.normal.saturating_add(self.adhoc)
    }

    pub fn add(&mut self, lane: CapacityLane, count: usize) {
        let used = match lane {
            CapacityLane::Normal => &mut self.normal,
            CapacityLane::Adhoc => &mut self.adhoc,
        };
        *used = used.saturating_add(count);
    }

    /// Remove a known existing claim when evaluating its replacement or continuation.
    pub fn credited(mut self, lane: CapacityLane, count: usize) -> Self {
        let used = match lane {
            CapacityLane::Normal => &mut self.normal,
            CapacityLane::Adhoc => &mut self.adhoc,
        };
        *used = used.saturating_sub(count);
        self
    }

    /// A normal admission must fit both ceilings; ad-hoc work never exceeds the hard ceiling.
    pub fn permits(&self, lane: CapacityLane, hard: usize, reserve: usize) -> bool {
        self.total() < hard
            && (lane == CapacityLane::Adhoc || self.normal < hard.saturating_sub(reserve))
    }
}

/// Durable unfinished-goal commitment. Legacy inference is used only before adoption.
pub fn task_committed(task: &crate::Task) -> bool {
    let planning = task.plan_runs.last();
    if task.tracker
        || matches!(task.status, crate::Status::Done | crate::Status::Dropped)
        || task.design && planning.is_some_and(|p| p.state == crate::PlanRunState::Approved)
    {
        return false;
    }
    task.capacity_started.unwrap_or_else(|| {
        legacy_started(
            task.status,
            task.current_run().map(|r| {
                (
                    r.outcome.is_some(),
                    r.seen_working || r.agent_host.is_some() || r.ended.is_some(),
                )
            }),
            planning.map(|p| {
                (
                    p.state,
                    p.agent_host.is_some() || p.herdr.is_some() || p.ended.is_some(),
                )
            }),
        )
    })
}

pub fn head_committed(head: &crate::TaskHead) -> bool {
    let planning = head.last_plan_run.as_ref();
    if head.tracker
        || matches!(head.status, crate::Status::Done | crate::Status::Dropped)
        || head.design && planning.is_some_and(|p| p.state == crate::PlanRunState::Approved)
    {
        return false;
    }
    head.capacity_started.unwrap_or_else(|| {
        legacy_started(
            head.status,
            head.current_run.as_ref().map(|r| {
                (
                    r.outcome.is_some(),
                    r.seen_working || r.agent_host.is_some() || r.ended.is_some(),
                )
            }),
            planning.map(|p| {
                (
                    p.state,
                    p.agent_host.is_some() || p.herdr.is_some() || p.ended.is_some(),
                )
            }),
        )
    })
}

fn legacy_started(
    status: crate::Status,
    coding: Option<(bool, bool)>,
    planning: Option<(crate::PlanRunState, bool)>,
) -> bool {
    let active_phase = matches!(
        status,
        crate::Status::Running | crate::Status::NeedsMe | crate::Status::Review
    );
    matches!(status, crate::Status::Running | crate::Status::Review)
        || coding.is_some_and(|(settled, observed)| settled || observed || active_phase)
        || planning.is_some_and(|(state, observed)| {
            observed
                || active_phase
                || !matches!(
                    state,
                    crate::PlanRunState::Planning | crate::PlanRunState::Cancelled
                )
        })
}

#[cfg(test)]
#[path = "capacity_tests.rs"]
mod tests;
