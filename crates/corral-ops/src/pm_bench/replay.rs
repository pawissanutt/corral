use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use corral_core::Event;

use super::{PmObservation, PmReplayStep, pm_select};

#[derive(Default)]
struct Board {
    pending: BTreeMap<String, PmObservation>,
    reviewing: BTreeSet<String>,
    ready: BTreeSet<String>,
    /// The reduced tier a task's gate events recorded; tasks without one keep the full-gate plan.
    reduced: BTreeMap<String, String>,
}

const FULL_REPAIR: &str = "Rebase onto current main, run the full gate, obtain fresh review.";

impl Board {
    fn stale(&self, task: &str, reason: String) -> PmObservation {
        let tier = self.reduced.get(task).cloned();
        let repair = match &tier {
            Some(tier) => format!(
                "Rebase onto current main. {} Obtain fresh review.",
                crate::prompt::tier_send_back(
                    tier,
                    &format!("corral gate {task} --wait"),
                    "corral itself runs the one full gate when this candidate lands on `main`",
                )
            ),
            None => FULL_REPAIR.into(),
        };
        PmObservation::StaleReview {
            task: task.into(),
            reason,
            repair,
            tier,
        }
    }
}

pub(super) fn task(observation: &PmObservation) -> Option<&str> {
    match observation {
        PmObservation::GateUntrusted { task, .. }
        | PmObservation::StaleReview { task, .. }
        | PmObservation::Question { task, .. }
        | PmObservation::EvidenceOnly { task, .. }
        | PmObservation::LoadFailure { task, .. }
        | PmObservation::ReviewInFlight { task, .. }
        | PmObservation::AutomaticWait { task, .. }
        | PmObservation::GateNeeded { task }
        | PmObservation::ReviewNeeded { task }
        | PmObservation::ReadyToLand { task }
        | PmObservation::Dispatch { task }
        | PmObservation::PlanDecision { task, .. }
        | PmObservation::Owner { task, .. } => Some(task),
        PmObservation::IdleIdeas { .. } => None,
    }
}

/// A ready candidate may not land while another recorded review is running.
pub(super) fn barrier(observations: &mut [PmObservation]) {
    let reviews: Vec<_> = observations
        .iter()
        .filter_map(|observation| match observation {
            PmObservation::ReviewInFlight { task, .. } => Some(task.clone()),
            _ => None,
        })
        .collect();
    if !reviews.is_empty() {
        for observation in observations {
            if let PmObservation::ReadyToLand { task } = observation {
                *observation = PmObservation::ReviewInFlight {
                    task: task.clone(),
                    reviews: reviews.clone(),
                };
            }
        }
    }
}

pub(super) fn replay(events: &[Event]) -> Result<Vec<PmReplayStep>> {
    let mut board = Board::default();
    let mut trace = Vec::with_capacity(events.len());
    for (index, event) in events.iter().enumerate() {
        board.apply(event);
        let mut observations: Vec<_> = board.pending.values().cloned().collect();
        for task in &board.reviewing {
            observations.push(PmObservation::ReviewInFlight {
                task: task.clone(),
                reviews: board.reviewing.iter().cloned().collect(),
            });
        }
        for task in &board.ready {
            observations.push(PmObservation::ReadyToLand { task: task.clone() });
        }
        barrier(&mut observations);
        let action = pm_select(&observations, "corral");
        trace.push(PmReplayStep {
            event_number: index + 1,
            event: event.clone(),
            observations,
            action,
        });
    }
    Ok(trace)
}

impl Board {
    fn apply(&mut self, event: &Event) {
        let Some(task) = event.task.as_deref() else {
            return;
        };
        let detail = &event.detail;
        if matches!(event.kind.as_str(), "gate_started" | "gate")
            && let Some(tier) = detail["tier"].as_str()
            && tier != corral_core::FULL_TIER
        {
            self.reduced.insert(task.into(), tier.into());
        }
        match event.kind.as_str() {
            "gate_started" | "changes_requested" | "replied" | "dispatched" => {
                self.pending.remove(task);
                self.ready.remove(task);
                self.reviewing.remove(task);
            }
            "gate" => {
                let observation = match detail["result"].as_str() {
                    Some("fail") => PmObservation::LoadFailure {
                        task: task.into(),
                        command: detail["failed_cmd"]
                            .as_str()
                            .unwrap_or("unrecorded command")
                            .into(),
                        alone_passed: None,
                        evidence: format!(
                            "gate failure at {}; isolated result absent from event",
                            event.ts
                        ),
                    },
                    Some("pass") => PmObservation::GateUntrusted {
                        task: task.into(),
                        log: "not included in event stream".into(),
                        problem:
                            "Pass event alone cannot establish the final per-attempt log result."
                                .into(),
                    },
                    _ => return,
                };
                self.pending.insert(task.into(), observation);
            }
            "done" => {
                self.pending
                    .insert(task.into(), PmObservation::GateNeeded { task: task.into() });
            }
            "review_started" => {
                // Starting a review is a historical fact, not certification of its gate.
                self.pending.remove(task);
                self.ready.remove(task);
                self.reviewing.insert(task.into());
            }
            "verdict" => {
                self.reviewing.remove(task);
                match detail["verdict"].as_str() {
                    Some("approve") => {
                        self.ready.insert(task.into());
                        self.pending.insert(task.into(), PmObservation::GateUntrusted {
                            task: task.into(),
                            log: "not included in event stream".into(),
                            problem: "A review verdict does not supply the missing per-attempt gate log.".into(),
                        });
                    }
                    Some("changes" | "escalate") => {
                        self.pending.insert(
                            task.into(),
                            PmObservation::Owner {
                                task: task.into(),
                                reason: format!("review verdict: {}", detail["verdict"]),
                            },
                        );
                    }
                    _ => {}
                }
            }
            "landing_stale" => {
                self.ready.remove(task);
                let reason = detail["reason"]
                    .as_str()
                    .unwrap_or("candidate identity is stale");
                let stale = self.stale(task, reason.into());
                self.pending.insert(task.into(), stale);
            }
            "approved" => {
                self.pending.remove(task);
                self.ready.remove(task);
                self.reviewing.remove(task);
                // Any other in-flight review is now bound to an older integrated base.
                let others: Vec<String> = self
                    .reviewing
                    .iter()
                    .chain(self.ready.iter())
                    .cloned()
                    .collect();
                for other in others {
                    let reason = format!(
                        "{task} landed at {} while this candidate was pending",
                        event.ts
                    );
                    let stale = self.stale(&other, reason);
                    self.pending.insert(other, stale);
                }
                self.ready.clear();
            }
            "needs_me" | "blocked" | "escalated" => {
                // These events do not establish an exact unanswered harness question.
                self.pending.insert(
                    task.into(),
                    PmObservation::Owner {
                        task: task.into(),
                        reason: detail["reason"]
                            .as_str()
                            .unwrap_or("owner attention; exact question unrecorded")
                            .into(),
                    },
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn event(task: &str, kind: &str, detail: serde_json::Value) -> Event {
        Event {
            ts: "2026-10-23T12:00:00Z".parse().unwrap(),
            task: Some(task.into()),
            kind: kind.into(),
            actor: None,
            via: None,
            detail,
        }
    }

    fn stale(task: &str, gate: serde_json::Value) -> PmObservation {
        let events = [
            event(task, "gate_started", gate),
            event(task, "review_started", json!({})),
            event("T-002", "approved", json!({})),
        ];
        let trace = replay(&events).unwrap();
        let last = trace.last().unwrap();
        last.observations
            .iter()
            .find(|o| matches!(o, PmObservation::StaleReview { .. }))
            .cloned()
            .unwrap()
    }

    #[test]
    fn a_reduced_tier_stale_review_names_its_tier_and_legacy_events_keep_the_full_gate_plan() {
        let PmObservation::StaleReview { repair, tier, .. } =
            stale("T-001", json!({"run": "R1", "tier": "quick"}))
        else {
            unreachable!()
        };
        assert_eq!(tier.as_deref(), Some("quick"));
        assert!(repair.contains("corral gate T-001 --wait"), "{repair}");
        assert!(repair.contains("it runs the quick tier"), "{repair}");
        assert!(repair.contains("under `Not verified:`"), "{repair}");
        assert!(!repair.contains("run the full gate"), "{repair}");

        let PmObservation::StaleReview { repair, tier, .. } = stale("T-001", json!({"run": "R1"}))
        else {
            unreachable!()
        };
        assert_eq!((repair.as_str(), tier), (FULL_REPAIR, None));
    }
}
