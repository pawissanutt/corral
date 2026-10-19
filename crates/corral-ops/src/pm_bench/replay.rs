use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use corral_core::Event;

use super::{PmObservation, PmReplayStep, pm_select};

#[derive(Default)]
struct Board {
    pending: BTreeMap<String, PmObservation>,
    reviewing: BTreeSet<String>,
    ready: BTreeSet<String>,
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
                self.pending.insert(
                    task.into(),
                    PmObservation::StaleReview {
                        task: task.into(),
                        reason: detail["reason"]
                            .as_str()
                            .unwrap_or("candidate identity is stale")
                            .into(),
                        repair: "Rebase onto current main, run the full gate, obtain fresh review."
                            .into(),
                    },
                );
            }
            "approved" => {
                self.pending.remove(task);
                self.ready.remove(task);
                self.reviewing.remove(task);
                // Any other in-flight review is now bound to an older integrated base.
                for other in self.reviewing.iter().chain(self.ready.iter()) {
                    self.pending.insert(
                        other.clone(),
                        PmObservation::StaleReview {
                            task: other.clone(),
                            reason: format!(
                                "{task} landed at {} while this candidate was pending",
                                event.ts
                            ),
                            repair:
                                "Rebase onto current main, run the full gate, obtain fresh review."
                                    .into(),
                        },
                    );
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
