//! Only heads, constant-size facts, pending questions and keyed session bindings are read here.
use anyhow::{Context, Result};
use corral_core::agent::ContainerRef;
use corral_core::{AutoStage, GateResult, RunHead, RunOutcome, Status, Store, TaskHead};
use corral_protocol::board::{BoardAgent, BoardGroup, BoardNow, BoardRow, CostBasis};
use std::collections::BTreeSet;

use super::rail;

pub(super) struct Computed {
    pub row: BoardRow,
    pub inputs: BTreeSet<String>,
}

pub(super) fn compute(store: &Store, head: &TaskHead) -> Result<Computed> {
    let mut inputs: BTreeSet<_> = head.depends_on.iter().cloned().collect();
    let plan = head.parent.clone().or_else(|| {
        head.plan.as_deref().map(|reference| {
            reference
                .split_once('@')
                .map_or(reference, |(id, _)| id)
                .to_owned()
        })
    });
    let parent = if let Some(id) = &plan {
        inputs.insert(id.clone());
        store
            .head_if_exists(id)
            .with_context(|| format!("reading plan {id}"))?
    } else {
        None
    };
    let goal = head
        .approved_scope
        .as_ref()
        .or_else(|| {
            parent
                .as_ref()
                .and_then(|head| head.approved_scope.as_ref())
        })
        .map(|scope| clip(&scope.goal, 80));
    // The ledger is the persisted output of ops::attention reconciliation, not another
    // classifier. In particular, no host/pane sampling or board-wide assessment happens here.
    let outstanding = head
        .attention
        .records
        .iter()
        .filter(|(_, record)| record.resolved_at.is_none());
    let attention = outstanding
        .clone()
        .take(4)
        .map(|(key, _)| key.0.clone())
        .collect();
    let questions = crate::questions::recorded_pending_views_ref(head)
        .with_context(|| format!("reading pending questions for {}", head.id))?;
    let owner_since = outstanding
        .filter(|(_, record)| record.responsible == corral_core::attention::Responsible::Owner)
        .map(|(_, record)| record.since)
        .chain(
            questions
                .iter()
                .filter(|q| q.pending && crate::questions::needs_owner(&q.question))
                .map(|q| q.question.asked.at),
        )
        .min();
    let held_reason = head.hold.as_ref().map(|hold| hold.reason.clone());
    let (group, code, text, since) = now(store, head, owner_since)?;
    let run = head.current_run.as_deref();
    let agent = agent(head, run)?;
    let (cost_usd, cost_basis) = cost(head);
    Ok(Computed {
        row: BoardRow {
            id: head.id.clone(),
            title: head.title.clone(),
            project: head.project.clone(),
            plan,
            goal,
            group,
            rail: rail::steps(head),
            now: BoardNow {
                code: code.into(),
                text: clip(&text, 60),
                since: since.map(|at| at.to_string()),
            },
            held_reason,
            questions: questions.iter().filter(|q| q.pending).count() as u64,
            attention,
            agent,
            cost_usd,
            cost_basis,
            // No head/fact carries an EventSeq. A change sequence is not an event sequence.
            last_event: None,
            rev: 0,
        },
        inputs,
    })
}

pub(super) fn clip(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

type Now = (BoardGroup, &'static str, String, Option<jiff::Timestamp>);

fn now(store: &Store, head: &TaskHead, owner_since: Option<jiff::Timestamp>) -> Result<Now> {
    let since = head.status_since;
    if let Some(park) = &head.park {
        return Ok((
            BoardGroup::Parked,
            "parked",
            format!("parked · {}", park.reason),
            Some(park.at),
        ));
    }
    if head.hold.is_some() || head.dispatch_hold {
        let reason = head
            .hold
            .as_ref()
            .map_or("dispatch hold", |hold| hold.reason.as_str());
        return Ok((
            BoardGroup::Held,
            "held",
            format!("held · {reason}"),
            head.hold.as_ref().map(|hold| hold.at).or(since),
        ));
    }
    if owner_since.is_some() || head.status == Status::NeedsMe {
        return Ok((
            BoardGroup::NeedsYou,
            "needs_you",
            "needs you".into(),
            owner_since.or(since),
        ));
    }
    if let Some(run) = head.last_plan_run.as_deref().filter(|run| run.holds_slot()) {
        return Ok((
            BoardGroup::Producing,
            "producing",
            "producing · plan".into(),
            Some(run.started),
        ));
    }
    if let Some(review) = head
        .last_plan
        .as_deref()
        .and_then(|plan| plan.current_review.as_ref())
        .filter(|review| review.verdict.is_none() && !review.cancelled)
    {
        return Ok((
            BoardGroup::Verifying,
            "reviewing",
            "reviewing".into(),
            Some(review.at),
        ));
    }
    if head.last_plan_run.as_deref().is_some_and(|run| {
        matches!(
            run.state,
            corral_core::PlanRunState::Submitted
                | corral_core::PlanRunState::AwaitingOwner
                | corral_core::PlanRunState::Escalated
                | corral_core::PlanRunState::Failed
        )
    }) {
        return Ok((BoardGroup::NeedsYou, "needs_you", "needs you".into(), since));
    }
    if let Some(run) = head.current_run.as_deref() {
        let stage = run.autopilot.as_ref();
        if let Some(stage) = stage {
            let next = match stage.stage {
                AutoStage::Gating => Some((BoardGroup::Verifying, "gating", "gating".into())),
                AutoStage::Reviewing => {
                    Some((BoardGroup::Verifying, "reviewing", "reviewing".into()))
                }
                AutoStage::Landing => Some((BoardGroup::Verifying, "landing", "landing".into())),
                AutoStage::Blocked => Some((
                    BoardGroup::Held,
                    "held",
                    format!(
                        "held · {}",
                        stage.reason.as_deref().unwrap_or("landing precondition")
                    ),
                )),
                AutoStage::Reviewed | AutoStage::Escalated => {
                    Some((BoardGroup::NeedsYou, "needs_you", "needs you".into()))
                }
                _ => None,
            };
            if let Some((group, code, text)) = next {
                return Ok((group, code, text, Some(stage.at)));
            }
        }
        if let Some(gate) = run
            .gate
            .as_ref()
            .filter(|gate| gate.result == GateResult::Running && !gate.process_lost)
        {
            return Ok((
                BoardGroup::Verifying,
                "gating",
                "gating".into(),
                Some(gate.at),
            ));
        }
        if let Some(review) = run
            .review
            .as_ref()
            .filter(|review| review.verdict.is_none() && !review.cancelled && !review.process_lost)
        {
            return Ok((
                BoardGroup::Verifying,
                "reviewing",
                "reviewing".into(),
                Some(review.at),
            ));
        }
        if head.status == Status::Running && run.outcome.is_none() {
            let phase = run.progress.as_ref().and_then(|progress| {
                progress.phases.iter().find(|phase| {
                    phase
                        .steps
                        .iter()
                        .any(|step| matches!(step.status, corral_core::StepState::InProgress))
                })
            });
            let text = phase.map_or_else(
                || "producing".into(),
                |phase| format!("producing · {}", phase.label),
            );
            // A phase has no publication timestamp; only the unqualified run start is known.
            return Ok((
                BoardGroup::Producing,
                "producing",
                text,
                phase.is_none().then_some(run.started),
            ));
        }
        if head.status == Status::Review || run.outcome == Some(RunOutcome::Done) {
            return Ok((BoardGroup::NeedsYou, "needs_you", "needs you".into(), since));
        }
    }
    for id in &head.depends_on {
        let dependency = store
            .head_if_exists(id)
            .with_context(|| format!("reading prerequisite {id}"))?;
        if dependency.is_none_or(|head| head.status != Status::Done) {
            return Ok((
                BoardGroup::Queued,
                "queued",
                format!("queued · after {id}"),
                since,
            ));
        }
    }
    Ok((BoardGroup::Queued, "queued", "queued".into(), since))
}

fn agent(head: &TaskHead, run: Option<&RunHead>) -> Result<Option<BoardAgent>> {
    if let Some(planner) = head
        .last_plan_run
        .as_deref()
        .filter(|planner| planner.holds_slot())
    {
        let session = native_session(head, planner.agent_host.as_deref(), || {
            ContainerRef::Planner {
                task: head.id.clone(),
                run: planner.id.clone(),
            }
        })?;
        return Ok(Some(BoardAgent {
            session,
            role: "planner".into(),
            profile: Some(planner.profile.clone()),
            runtime: None,
            machine: None,
        }));
    }
    if let Some(plan) = head.last_plan.as_deref()
        && let Some(review) = plan
            .current_review
            .as_ref()
            .filter(|review| review.verdict.is_none() && !review.cancelled)
    {
        let session = native_session(head, review.agent_host.as_deref(), || {
            ContainerRef::PlanReviewer {
                task: head.id.clone(),
                plan: plan.id.clone(),
                review: review.id.clone(),
            }
        })?;
        return Ok(Some(BoardAgent {
            session,
            role: "plan_reviewer".into(),
            profile: Some(review.reviewer.clone()),
            runtime: None,
            machine: None,
        }));
    }
    let Some(run) = run else {
        return Ok(None);
    };
    let reviewing = run.review.as_ref().is_some_and(|review| {
        review.verdict.is_none() && !review.cancelled && !review.process_lost
    });
    let (binding, role) = if reviewing {
        let Some(child) = run.current_child.as_deref() else {
            return Ok(Some(BoardAgent {
                session: None,
                role: "reviewer".into(),
                profile: None,
                runtime: None,
                machine: None,
            }));
        };
        (child, "reviewer")
    } else {
        (run, "worker")
    };
    let session = native_session(head, binding.agent_host.as_deref(), || {
        if reviewing {
            ContainerRef::Reviewer {
                task: head.id.clone(),
                run: run.id.clone(),
                review: binding.id.clone(),
            }
        } else {
            ContainerRef::Worker {
                task: head.id.clone(),
                run: run.id.clone(),
            }
        }
    })?
    .or_else(|| {
        binding
            .session
            .as_ref()
            .and_then(|session| session.id.clone())
    });
    Ok(Some(BoardAgent {
        session,
        role: role.into(),
        profile: Some(binding.profile.as_ref().unwrap_or(&binding.agent).clone()),
        runtime: None,
        machine: None,
    }))
}

fn native_session(
    head: &TaskHead,
    host: Option<&corral_core::AgentHostHead>,
    container: impl FnOnce() -> ContainerRef,
) -> Result<Option<String>> {
    let Some(host) = host.filter(|host| host.binding.generation > 0) else {
        return Ok(None);
    };
    Ok(head
        .agent_generation(&container(), host.binding.generation)
        .with_context(|| format!("reading agent session for {}", head.id))?
        .and_then(|generation| generation.bound.map(|bound| bound.session_id)))
}

fn cost(head: &TaskHead) -> (Option<f64>, Option<CostBasis>) {
    let mut total = head.summary.cost.clone();
    if let Some(run) = head.current_run.as_deref() {
        total.add(&run.facts.cost);
    }
    let basis = match (total.reported, total.estimated) {
        (Some(_), Some(_)) => Some(CostBasis::Mixed),
        (Some(_), None) => Some(CostBasis::Reported),
        (None, Some(_)) => Some(CostBasis::Estimated),
        (None, None) => None,
    };
    (
        basis.map(|_| total.reported.unwrap_or(0.0) + total.estimated.unwrap_or(0.0)),
        basis,
    )
}
