use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use corral_core::Event;

use super::{PmObservation, PmReplayStep, pm_select};

#[derive(Default)]
struct Board {
    pending: BTreeMap<String, PmObservation>,
    reviewing: BTreeSet<String>,
    ready: BTreeSet<String>,
    /// The reduced tier the current run's gate events selected; tasks without one keep the
    /// full-gate plan.
    reduced: BTreeMap<String, ReducedGate>,
}

/// A reduced-tier selection as gate events record it: the run, the tier and, when the receipt
/// identity carried one, the binding source that says whose contract it is.
struct ReducedGate {
    run: Option<String>,
    tier: String,
    source: Option<String>,
}

const FULL_REPAIR: &str = "Rebase onto current main, run the full gate, obtain fresh review.";
const UNKNOWN_DESTINATION: &str = "these events do not establish the candidate's destination, so corral runs its landing tier itself";

impl ReducedGate {
    /// Members bind a plan scope, a member-gate record or the plan_member policy; review rounds
    /// are main-bound. A gate recorded without a binding names no destination.
    fn landing(&self) -> (&'static str, String) {
        match self.source.as_deref() {
            Some(s)
                if s.starts_with("plan:")
                    || s.starts_with("member-gate:")
                    || s == "policy:plan_member" =>
            {
                (
                    "Rebase onto its current Plan target.",
                    crate::prompt::plan_target_landing(None, &self.tier),
                )
            }
            Some("policy:review_rounds") => (
                "Rebase onto current main.",
                crate::prompt::branch_landing(corral_core::FULL_TIER, None),
            ),
            _ => (
                "Rebase onto its current destination.",
                UNKNOWN_DESTINATION.into(),
            ),
        }
    }
}

impl Board {
    fn stale(&self, task: &str, reason: String) -> PmObservation {
        let reduced = self.reduced.get(task);
        let repair = match reduced {
            Some(gate) => {
                let (rebase, landing) = gate.landing();
                let gate_cmd = format!("corral gate {task} --wait");
                let text = crate::prompt::tier_send_back(&gate.tier, &gate_cmd, &landing);
                format!("{rebase} {text} Obtain fresh review.")
            }
            None => FULL_REPAIR.into(),
        };
        PmObservation::StaleReview {
            task: task.into(),
            reason,
            repair,
            tier: reduced.map(|gate| gate.tier.clone()),
        }
    }

    /// A reduced gate selects its tier; a member's full binding, a gate without a tier, or another
    /// run's gate ends it. Only an unbound full gate of the same run, a main landing gate, keeps
    /// the review-round tier.
    fn record_gate(&mut self, task: &str, detail: &serde_json::Value) {
        let run = detail["run"].as_str();
        let binding = &detail["identity"]["gate_tier"];
        let tier = detail["tier"].as_str().or_else(|| binding["tier"].as_str());
        match tier {
            Some(tier) if tier != corral_core::FULL_TIER => {
                let gate = ReducedGate {
                    run: run.map(str::to_owned),
                    tier: tier.to_owned(),
                    source: binding["source"].as_str().map(str::to_owned),
                };
                self.reduced.insert(task.to_owned(), gate);
            }
            Some(_)
                if binding.is_null()
                    && self
                        .reduced
                        .get(task)
                        .is_some_and(|gate| gate.run.as_deref() == run) => {}
            _ => {
                self.reduced.remove(task);
            }
        }
    }
}

pub(super) fn task(observation: &PmObservation) -> Option<&str> {
    observation.task()
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
            "gate_started" | "gate" => self.record_gate(task, detail),
            "dispatched" => {
                self.reduced.remove(task);
            }
            _ => {}
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
    use serde_json::{Value, json};

    use super::*;

    fn event(task: &str, kind: &str, detail: Value) -> Event {
        Event {
            ts: "2026-10-23T12:00:00Z".parse().unwrap(),
            task: Some(task.into()),
            kind: kind.into(),
            actor: None,
            via: None,
            detail,
        }
    }

    fn bound(run: &str, tier: &str, source: &str) -> Value {
        json!({"run": run, "tier": tier,
            "identity": {"gate_tier": {"tier": tier, "source": source, "selection_sha256": "s"}}})
    }

    /// The StaleReview a sibling landing raises after `events` (review started on the last run).
    fn stale_after(task: &str, run: &str, mut events: Vec<Event>) -> (String, Option<String>) {
        events.push(event(task, "review_started", json!({"run": run})));
        events.push(event("T-999", "approved", json!({})));
        let trace = replay(&events).unwrap();
        let observation = trace
            .last()
            .unwrap()
            .observations
            .iter()
            .find_map(|o| match o {
                PmObservation::StaleReview {
                    task: t,
                    repair,
                    tier,
                    ..
                } if t == task => Some((repair.clone(), tier.clone())),
                _ => None,
            });
        observation.unwrap()
    }

    fn assert_quick(repair: &str, tier: Option<&str>, landing: &str) {
        assert_eq!(tier, Some("quick"));
        for needle in [
            "corral gate T-1 --wait",
            "it runs the quick tier",
            "under `Not verified:`",
            landing,
        ] {
            assert!(repair.contains(needle), "{needle:?} missing: {repair}");
        }
        assert!(!repair.contains("run the full gate"), "{repair}");
    }

    #[test]
    fn a_main_bound_quick_replay_names_quick_and_keeps_it_across_its_full_landing_gate() {
        let quick = bound("R1", "quick", "policy:review_rounds");
        let landing = json!({"run": "R1", "tier": "full", "identity": {"head": "h"}});
        let (repair, tier) = stale_after(
            "T-1",
            "R1",
            vec![
                event("T-1", "gate_started", quick),
                event("T-1", "gate_started", landing),
            ],
        );
        assert_quick(
            &repair,
            tier.as_deref(),
            "the one full gate when this candidate lands on its main branch",
        );
        assert!(repair.starts_with("Rebase onto current main."), "{repair}");
    }

    #[test]
    fn a_quick_member_replay_lands_on_its_plan_target_not_main() {
        let member = bound("R1", "quick", "member-gate:T-900@1");
        let gate = event("T-1", "gate_started", member);
        let (repair, tier) = stale_after("T-1", "R1", vec![gate]);
        assert_quick(
            &repair,
            tier.as_deref(),
            "this member lands on its Plan target on the quick tier",
        );
        assert!(!repair.contains("main"), "{repair}");
    }

    #[test]
    fn an_unbound_quick_gate_states_that_its_destination_is_unknown() {
        let gate = event("T-1", "gate_started", json!({"run": "R1", "tier": "quick"}));
        let (repair, tier) = stale_after("T-1", "R1", vec![gate]);
        assert_quick(&repair, tier.as_deref(), UNKNOWN_DESTINATION);
    }

    #[test]
    fn a_new_run_a_full_member_binding_or_an_untiered_gate_ends_the_reduced_tier() {
        let quick = || event("T-1", "gate_started", json!({"run": "R1", "tier": "quick"}));
        let cases = [
            // A new dispatch, then an untiered gate of that run.
            vec![
                quick(),
                event("T-1", "dispatched", json!({"run": "R2"})),
                event("T-1", "gate_started", json!({"run": "R2"})),
            ],
            // An owner amendment to a full member gate.
            vec![
                quick(),
                event(
                    "T-1",
                    "gate_started",
                    bound("R1", "full", "member-gate:T-900@2"),
                ),
            ],
            // A same-run gate without a tier after tiers were disabled.
            vec![
                quick(),
                event("T-1", "gate", json!({"run": "R1", "result": "pass"})),
            ],
            // An unbound full gate of another run.
            vec![
                quick(),
                event("T-1", "gate_started", json!({"run": "R2", "tier": "full"})),
            ],
        ];
        for events in cases {
            let run = events.last().unwrap().detail["run"]
                .as_str()
                .unwrap()
                .to_owned();
            let (repair, tier) = stale_after("T-1", &run, events);
            assert_eq!((repair.as_str(), tier), (FULL_REPAIR, None));
        }
    }

    #[test]
    fn landing_stale_uses_the_current_selection_and_legacy_events_keep_the_full_gate_plan() {
        let stale = |events: Vec<Event>| {
            let trace = replay(&events).unwrap();
            match trace.last().unwrap().observations.first() {
                Some(PmObservation::StaleReview { repair, tier, .. }) => {
                    (repair.clone(), tier.clone())
                }
                other => panic!("{other:?}"),
            }
        };
        let landing_stale = || {
            event(
                "T-1",
                "landing_stale",
                json!({"run": "R1", "reason": "moved"}),
            )
        };
        let quick = event(
            "T-1",
            "gate_started",
            bound("R1", "quick", "plan:T-900@P1#a@d"),
        );
        let (repair, tier) = stale(vec![quick, landing_stale()]);
        assert_quick(&repair, tier.as_deref(), "Plan target");
        let legacy = event("T-1", "gate_started", json!({"run": "R1"}));
        assert_eq!(
            stale(vec![legacy, landing_stale()]),
            (FULL_REPAIR.to_owned(), None)
        );
    }
}
