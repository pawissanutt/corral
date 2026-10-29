use super::*;
use corral_core::{EffectiveTaskPolicy, OperatingMode, Run};
use serde_json::json;

fn at() -> Timestamp {
    "2026-10-29T08:00:00Z".parse().unwrap()
}

fn task(status: Status) -> Task {
    corral_core::parse_task(
        "---\nid: T-550\ntitle: Fixture\nstatus: running\ncreated: 2026-10-29T08:00:00Z\nupdated: 2026-10-29T08:00:00Z\n---\n",
        std::path::Path::new("T-550.md"),
    )
    .map(|mut task| {
        task.status = status;
        task.status_since = Some(at());
        task
    })
    .unwrap()
}

fn worker() -> Run {
    serde_json::from_value(json!({
        "id":"R1", "agent":"fixture", "worktree":"/fixture", "branch":"fixture",
        "base":"main", "base_sha":"abc", "started":at(), "agent_state":"working"
    }))
    .unwrap()
}

fn policy(task: &Task, config: &Config, refused: &[Action]) -> EffectivePolicy {
    EffectivePolicy {
        mode: OperatingMode::Autopilot,
        paused: false,
        revision: "fixture".into(),
        authorization: Some("grant".into()),
        reasons: Vec::new(),
        tasks: BTreeMap::from([(
            task.id.clone(),
            EffectiveTaskPolicy {
                review: config.task_review_policy(task),
                acceptance_source: corral_core::PolicySource::Task,
                plan_acceptance_source: corral_core::PolicySource::Task,
                refusals: refused
                    .iter()
                    .map(|action| (*action, vec!["stale authorization".into()]))
                    .collect(),
            },
        )]),
    }
}

fn assessment(task: &Task, dependencies: &[Task], refused: &[Action]) -> Assessment {
    assessment_with(task, dependencies, refused, |_| {})
}

fn assessment_with(
    task: &Task,
    dependencies: &[Task],
    refused: &[Action],
    configure: impl FnOnce(&mut Neighborhood<'_>),
) -> Assessment {
    // Given a frozen task neighborhood and a fixed clock, with no external evidence reads.
    let config = Config::default_for(std::path::Path::new("/fixture"));
    let policy = policy(task, &config, refused);
    let tasks = std::iter::once(task.clone())
        .chain(dependencies.iter().cloned())
        .collect::<Vec<_>>();
    let mut n = Neighborhood::new(&tasks, &config, &policy);
    configure(&mut n);
    let before = corral_core::work::Work::ALL.map(corral_core::work::count);
    // When the shared attention projection runs.
    let result = assess(task, &n, at());
    // Then only its one documented attention visit is recorded.
    for (index, work) in corral_core::work::Work::ALL.iter().enumerate() {
        assert_eq!(
            corral_core::work::count(*work),
            before[index] + u64::from(*work == corral_core::work::Work::AttentionVisit)
        );
    }
    result
}

fn waiting() -> Task {
    let mut task = task(Status::Running);
    let mut run = worker();
    run.obligations.waits.push(
        serde_json::from_value(json!({
            "request": {
                "version":1, "id":"W1", "run":"R1", "condition":"Dependency lands",
                "owner":"worker", "source":"worker",
                "handle":{"kind":"channel","channel_id":"task:T-551:landed","capability":"wait_v1"}
            },
            "registered_at":at(),
            "observation":{"at":at(),"state":"pending","detail":"old observation"}
        }))
        .unwrap(),
    );
    task.runs.push(run);
    task
}

#[test]
fn native_parked_wait_overrides_working_progress() {
    // Given a native parked run whose last agent observation still says working.
    let task = waiting();
    let mut dependency = task.clone();
    dependency.id = "T-551".into();
    // When assessed against the canonical unfinished dependency.
    let a = assessment(&task, &[dependency], &[]);
    // Then it cannot claim worker progress while waiting.
    assert!(a.progress.is_none(), "{a:#?}");
    assert_eq!(a.conditions[0].kind, "obligation_wait");
    assert_eq!(a.conditions[0].since, Some(at()));
    assert_eq!(
        a.conditions[0].next_action.recipient.as_deref(),
        Some("T-551 (running)")
    );
    let stopped = corral_core::attention::blocked(&task, &a).unwrap();
    assert_eq!(stopped.unblocker(), "T-551 (running)");
}

#[test]
fn landed_dependency_without_receipt_uses_fresh_status() {
    // Given an old pending observation and a dependency now canonically Done.
    let task = waiting();
    let mut dependency = task.clone();
    dependency.id = "T-551".into();
    dependency.status = Status::Done;
    // When assessed without mutating the native obligation.
    let a = assessment(&task, &[dependency], &[]);
    // Then the missing receipt, not the stale pending observation, explains the stop.
    let condition = &a.conditions[0];
    assert_eq!(condition.reason_code, "wait_receipt_missing");
    assert_eq!(
        condition.next_action.recipient.as_deref(),
        Some("T-551 (done)")
    );
    assert_eq!(condition.resume[0].satisfied, Some(false));
    assert_eq!(condition.resume[1].kind, "task_status");
    assert_eq!(condition.resume[1].target.as_deref(), Some("T-551"));
    assert_eq!(condition.resume[1].expected.as_deref(), Some("done"));
    assert_eq!(condition.resume[1].satisfied, Some(true));
    assert_eq!(condition.responsible, Responsible::Pm);
    assert_eq!(condition.unblocker(), "PM");
}

fn plan() -> Task {
    let mut task = task(Status::Running);
    task.plans.push(
        serde_json::from_value(json!({
            "id":"D1", "spec_revision":"fixture",
            "artifact":{"run":"P1","kind":"report","title":"Plan","path":"plan.md",
                "source":"/fixture/plan.md","sha256":"abc","bytes":0,"added":at()},
            "document":{"goal":"fixture","non_goals":[],"decisions":[],"affected_paths":[],
                "contracts":[],"dependencies":[],"acceptance":[],"verification":[],"owner_decisions":[]},
            "reviews":[{"id":"pr1","reviewer":"fixture","policy_digest":"fixture","at":at()}]
        }))
        .unwrap(),
    );
    task
}

#[test]
fn recorded_plan_review_without_reviewer_is_stopped() {
    // Given a recorded independent review attempt with no launched reviewer.
    let task = plan();
    // When the launch authority remains valid.
    let a = assessment(&task, &[], &[]);
    // Then the missing executor is actionable rather than indefinite pilot progress.
    assert!(a.progress.is_none(), "{a:#?}");
    assert_eq!(a.primary.unwrap().kind, "plan_reviewer_missing");
}

#[test]
fn ordinary_worker_and_future_progression_are_not_blocked() {
    for status in [Status::Ready, Status::Backlog, Status::Running] {
        // Given scheduled admission or a normally working run.
        let mut task = task(status);
        task.queued = true;
        if status == Status::Running {
            task.runs.push(worker());
        }
        // When assessed, then admission conditions do not turn into stopped execution.
        let a = assessment(&task, &[], &[]);
        assert!(
            corral_core::attention::blocked(&task, &a).is_none(),
            "{a:?}"
        );
    }
    // Given an authorized future initial plan-review launch, not an interrupted attempt.
    let mut task = plan();
    task.plans[0].reviews.clear();
    // When assessed, then the plan pilot remains the executor.
    let a = assessment(&task, &[], &[]);
    assert_eq!(a.progress.as_ref().unwrap().executor, "plan_pilot");
    assert!(corral_core::attention::blocked(&task, &a).is_none());
}

#[test]
fn escalation_silent_turn_and_launch_failure_are_blocked() {
    for reason in ["escalation", "silent_turn", "launch_failed"] {
        // Given a concrete stopping fact at an exact timestamp.
        let mut task = task(Status::NeedsMe);
        let mut run = worker();
        match reason {
            "escalation" => {
                run.autopilot = Some(
                    serde_json::from_value(json!({
                        "stage":"escalated","at":at(),"reason":"Owner disposition needed"
                    }))
                    .unwrap(),
                );
            }
            "silent_turn" => {
                run.idle_no_report = true;
                run.idle_since = Some(at());
            }
            "launch_failed" => {
                task.launch = Some(
                    serde_json::from_value(json!({
                        "stage":"submitting","reserved":true,"error":"Launch rejected"
                    }))
                    .unwrap(),
                );
            }
            _ => unreachable!(),
        }
        task.runs.push(run);
        // When classified, then the shared API selects the actual stopping condition.
        let a = assessment(&task, &[], &[]);
        let stopped = corral_core::attention::blocked(&task, &a).unwrap();
        assert_eq!(stopped.kind, reason);
        assert_eq!(stopped.since, Some(at()));
        assert_eq!(
            stopped.unblocker(),
            if reason == "escalation" {
                "Owner"
            } else {
                "PM"
            }
        );
    }
}

#[test]
fn plan_review_refusal_is_owner_authority_not_pm_recovery() {
    // Given a missing reviewer whose operating grant is stale.
    let task = plan();
    // When the frozen policy refuses PlanReview, then owner authority takes precedence.
    let a = assessment(&task, &[], &[Action::PlanReview]);
    let stopped = corral_core::attention::blocked(&task, &a).unwrap();
    assert_eq!(stopped.kind, "authority_blocked");
    assert_eq!(stopped.responsible, Responsible::Owner);
    assert_eq!(stopped.unblocker(), "Owner");
}

fn aggregate() -> Task {
    let mut task = task(Status::Running);
    task.tracker = true;
    let signals = corral_core::ReviewSignals::for_task(&task, corral_core::ReviewSubjectKind::Plan);
    let requirement = corral_core::review_requirement(&signals);
    let route: corral_core::ReviewRoute = serde_json::from_value(json!({
        "version":1, "requirement":requirement, "contract_digest":"fixture",
        "signals":signals, "profile":"reviewer", "profile_revision":"fixture",
        "profile_source":"task_override", "independence":"unknown",
        "capability_mismatch":false, "round":1, "scope":"full"
    }))
    .unwrap();
    let identity = json!({
        "version":1, "task":task.id, "run":"aggregate", "attempt":"A1",
        "head":"abc", "tree":"tree", "integration_base":"abc", "dispatch_base":"abc",
        "spec_digest":"spec", "policy_digest":"policy", "inputs_digest":"inputs",
        "environment":[]
    });
    task.aggregate_attempts.push(
        serde_json::from_value(json!({
            "id":"A1", "identity":identity, "target":{"kind":"main"}, "code":false,
            "members":[], "started":at(),
            "reviews":[{
                "id":"A1-R1", "reviewer":"reviewer", "route":route,
                "identity":identity, "evidence_digest":"evidence", "requested":at()
            }]
        }))
        .unwrap(),
    );
    task
}

#[test]
fn aggregate_missing_and_recorded_nonlive_reviewers_require_pm_recovery() {
    for abandoned in [false, true] {
        // Given a recorded aggregate reviewer that never launched or exited without a verdict.
        let mut task = aggregate();
        if abandoned {
            let review = &mut task.aggregate_attempts[0].reviews[0];
            review.herdr = Some(
                serde_json::from_value(json!({
                    "pane_id":"gone", "workspace_id":"fixture"
                }))
                .unwrap(),
            );
            review.abandoned = Some("reviewer exited before verdict".into());
        }
        // When assessed against allowed aggregate authority.
        let a = assessment(&task, &[], &[]);
        // Then missing execution is PM work, not owner acceptance or phantom progress.
        let stopped = corral_core::attention::blocked(&task, &a).unwrap();
        assert!(a.progress.is_none());
        assert_eq!(stopped.kind, "aggregate_reviewer_missing");
        assert_eq!(stopped.responsible, Responsible::Pm);
        assert_eq!(stopped.since, Some(at()));
    }
}

#[test]
fn aggregate_live_reviewer_and_gate_are_not_blocked() {
    for gate in [false, true] {
        // Given a launched reviewer or a running aggregate gate.
        let mut task = aggregate();
        if gate {
            task.aggregate_attempts[0].gate = Some(
                serde_json::from_value(json!({
                    "sha":"abc", "at":at(), "result":"running"
                }))
                .unwrap(),
            );
        } else {
            task.aggregate_attempts[0].reviews[0].herdr = Some(
                serde_json::from_value(json!({"pane_id":"reviewer","workspace_id":"fixture"}))
                    .unwrap(),
            );
        }
        // When projected, then future automatic completion is not a stopped task.
        let a = assessment(&task, &[], &[]);
        assert!(a.progress.is_some());
        assert!(
            corral_core::attention::blocked(&task, &a).is_none(),
            "{a:#?}"
        );
    }
}

#[test]
fn aggregate_missing_reviewer_preserves_stale_authority_refusal() {
    // Given a missing reviewer and a stale aggregate authorization.
    let task = aggregate();
    // When the current frozen policy refuses aggregate recovery.
    let a = assessment(&task, &[], &[Action::AcceptAggregate]);
    // Then it cannot advertise an authorized PM continuation.
    let stopped = corral_core::attention::blocked(&task, &a).unwrap();
    assert_eq!(stopped.kind, "authority_blocked");
    assert_eq!(stopped.responsible, Responsible::Owner);
    assert_eq!(
        stopped.resume[0].expected.as_deref(),
        Some("AcceptAggregate")
    );
}

#[test]
fn satisfied_native_receipt_allows_capacity_wait_without_blocking() {
    // Given the native wait has a receipt and is queued to reacquire its coding slot.
    let mut task = waiting();
    let run = &mut task.runs[0];
    run.obligations.waits[0].receipt = Some(
        serde_json::from_value(json!({
            "version":1,"id":"W1","run":"R1","outcome":"ready","delivery":"acknowledged",
            "observed_at":at(),"answer":null
        }))
        .unwrap(),
    );
    run.obligations.slot_wait =
        Some(serde_json::from_value(json!({"since":at(),"on":["T-551"]})).unwrap());
    // When assessed, then normal slot reacquisition is not a stopped native wait.
    let a = assessment(&task, &[], &[]);
    assert!(
        corral_core::attention::blocked(&task, &a).is_none(),
        "{a:?}"
    );
}

#[test]
fn generic_unmet_native_wait_is_corral_owned_and_not_worker_progress() {
    // Given a native wait that names a harness channel rather than another task.
    let mut task = waiting();
    task.runs[0].obligations.waits[0].request.handle = corral_core::WaitHandle::Channel {
        channel_id: "external-operation".into(),
        capability: Some("wait_v1".into()),
    };
    // When assessed, then the unmet wait still stops execution without inventing a dependency.
    let a = assessment(&task, &[], &[]);
    let stopped = corral_core::attention::blocked(&task, &a).unwrap();
    assert!(a.progress.is_none());
    assert_eq!(stopped.kind, "obligation_wait");
    assert_eq!(stopped.reason_code, "wait_pending");
    assert_eq!(stopped.unblocker(), "corral");
    assert_eq!(stopped.resume.len(), 1);
}

#[test]
fn needs_me_disk_key_and_terminal_exclusions_remain_stable() {
    // Given the historical disk status and an outstanding stopping fact.
    let mut task = waiting();
    task.status = Status::NeedsMe;
    assert_eq!(
        serde_json::to_value(task.status).unwrap(),
        json!("needs_me")
    );
    let a = assessment(&task, &[], &[]);
    // When viewing terminal or unscheduled statuses, then the old assessment cannot block them.
    for status in [Status::Idea, Status::Done, Status::Dropped] {
        task.status = status;
        assert!(corral_core::attention::blocked(&task, &a).is_none());
    }
}

#[test]
fn retained_missing_identity_stops_plan_and_aggregate_reviewers() {
    let since: Timestamp = "2026-10-29T08:01:00Z".parse().unwrap();
    for aggregate_review in [false, true] {
        // Given a recorded launched reviewer and a retained missing identity, not a live probe.
        let mut task = if aggregate_review {
            aggregate()
        } else {
            plan()
        };
        let herdr = serde_json::from_value(json!({
            "pane_id":"reviewer", "workspace_id":"fixture"
        }))
        .unwrap();
        if aggregate_review {
            task.aggregate_attempts[0].reviews[0].herdr = Some(herdr);
        } else {
            task.plans[0].reviews[0].herdr = Some(herdr);
        }
        // When no recorded loss exists, then unknown liveness does not invent a stop.
        let unknown = assessment(&task, &[], &[]);
        assert!(corral_core::attention::blocked(&task, &unknown).is_none());
        // When the exact pane@started identity is missing, then its canonical time owns the stop.
        let a = assessment_with(&task, &[], &[], |n| {
            n.missing_reviewers
                .insert(format!("reviewer@{}", at()), since);
        });
        let stopped = corral_core::attention::blocked(&task, &a).unwrap();
        assert_eq!(
            stopped.kind,
            if aggregate_review {
                "aggregate_reviewer_missing"
            } else {
                "plan_reviewer_missing"
            }
        );
        assert_eq!(stopped.since, Some(since));
        assert_eq!(stopped.responsible, Responsible::Pm);
        assert!(a.progress.is_none());
        // A loss from another launch of the same pane cannot poison this reviewer.
        let stale = assessment_with(&task, &[], &[], |n| {
            n.missing_reviewers
                .insert(format!("reviewer@{since}"), since);
        });
        assert!(corral_core::attention::blocked(&task, &stale).is_none());
    }
}

#[test]
fn stale_prepared_authorization_stops_otherwise_authorized_plan_pilot() {
    // Given no failed effective-policy action, but an exact prepared journal refusal.
    let mut task = plan();
    task.plans[0].reviews.clear();
    let since: Timestamp = "2026-10-29T07:59:00Z".parse().unwrap();
    // When the snapshot carries that refusal, then it overrides future pilot progress.
    let a = assessment_with(&task, &[], &[], |n| {
        n.prepared_refusals
            .insert(task.id.clone(), (since, "prepared policy changed".into()));
    });
    let stopped = corral_core::attention::blocked(&task, &a).unwrap();
    assert_eq!(stopped.kind, "authority_blocked");
    assert_eq!(stopped.reason_code, "authority_blocked");
    assert_eq!(stopped.since, Some(since));
    assert_eq!(stopped.responsible, Responsible::Owner);
    assert_eq!(stopped.resume[0].expected.as_deref(), Some("PlanReview"));
    assert!(a.progress.is_none());
}

#[test]
fn nonblocking_question_preserves_worker_but_blocking_question_stops_it() {
    for blocking in [false, true] {
        // Given a working worker with one canonical question of known blocking semantics.
        let mut task = task(Status::Running);
        let mut run = worker();
        let mut host = corral_core::agent::AgentRunState::new(corral_core::agent::AgentBinding {
            role: corral_core::agent::AgentRole::Worker,
            store: "fixture".into(),
            project: "fixture".into(),
            generation: 1,
            health: corral_core::agent::HostHealth::Healthy,
            callback: None,
            closing: false,
        });
        host.questions.push(
            serde_json::from_value(json!({
                "version":1,"key":"T-550/R1/Q1","revision":1,"kind":"clarification",
                "blocking":blocking,"text":"Which constraint?","state":"open",
                "asked":{"request_id":"ask","payload_sha256":"sha","session_id":"session",
                    "generation":1,"at":at()},
                "wake":{"route":"none","key":"wake","state":"pending"}
            }))
            .unwrap(),
        );
        run.agent_host = Some(Box::new(host));
        task.runs.push(run);
        // When projected, then only the actual suspension becomes Blocked.
        let a = assessment(&task, &[], &[]);
        assert_eq!(
            corral_core::attention::blocked(&task, &a).is_some(),
            blocking
        );
        assert_eq!(a.progress.is_none(), blocking);
        assert_eq!(a.conditions[0].kind, "question_open");
        if !blocking {
            // An unrelated native suspension cannot promote this nonblocking question.
            task.runs[0].obligations = waiting().runs[0].obligations.clone();
            let suspended = assessment(&task, &[], &[]);
            assert_eq!(
                corral_core::attention::blocked(&task, &suspended)
                    .unwrap()
                    .kind,
                "obligation_wait"
            );
        }
    }
}

#[test]
fn recorded_worker_loss_needs_no_live_inventory_probe() {
    for retained in [false, true] {
        // Given either a synced gone worker or an exact retained missing launch identity.
        let mut task = task(Status::Running);
        let mut run = worker();
        if retained {
            run.herdr = Some(
                serde_json::from_value(json!({
                    "pane_id":"worker","workspace_id":"fixture"
                }))
                .unwrap(),
            );
        } else {
            run.agent_state = Some("gone".into());
        }
        task.runs.push(run);
        // When inventory is known or retained loss is exact, then no new probe is needed.
        let a = assessment_with(&task, &[], &[], |n| {
            if retained {
                n.missing_reviewers.insert(format!("worker@{}", at()), at());
            } else {
                n.inventory_known = true;
            }
        });
        let stopped = corral_core::attention::blocked(&task, &a).unwrap();
        assert_eq!(stopped.kind, "agent_lost");
        assert_eq!(stopped.responsible, Responsible::Pm);
        assert!(a.progress.is_none());
    }
}

#[test]
fn silent_turn_age_uses_idle_onset_not_older_running_status() {
    // Given a long-running task whose silent turn started only a minute ago.
    let mut task = task(Status::Running);
    task.status_since = Some("2026-10-29T04:00:00Z".parse().unwrap());
    let mut run = worker();
    run.idle_no_report = true;
    run.idle_since = Some("2026-10-29T07:59:00Z".parse().unwrap());
    task.runs.push(run);
    // When projected at the fixed clock, then Blocked age starts at canonical idle onset.
    let a = assessment(&task, &[], &[]);
    let stopped = corral_core::attention::blocked(&task, &a).unwrap();
    assert_eq!(stopped.kind, "silent_turn");
    assert_eq!(stopped.since, task.runs[0].idle_since);
    assert_eq!(stopped.age_secs, Some(60));
}

#[test]
fn unrelated_failed_planner_cannot_mask_current_review() {
    for ownership in ["none", "run", "submitted"] {
        for live in [false, true] {
            // Given a failed planner and a latest submitted plan, with explicit ownership variants.
            let mut task = plan();
            let run = serde_json::from_value(json!({
                "id":"D1","profile":"fixture","harness":"fixture","spec_revision":"fixture",
                "started":at(),"ended":at(),"output_dir":"/fixture","rounds":0,
                "state":"failed","submitted": if ownership == "submitted" { Some("D1") } else { None }
            })).unwrap();
            task.plan_runs.push(run);
            if ownership == "run" {
                task.plans[0].run = Some("D1".into());
            }
            if live {
                task.plans[0].reviews[0].herdr = Some(
                    serde_json::from_value(json!({"pane_id":"reviewer","workspace_id":"fixture"}))
                        .unwrap(),
                );
            }
            // When assessed, then only failure of this plan's own planner can mask its review.
            let a = assessment(&task, &[], &[]);
            let stopped = corral_core::attention::blocked(&task, &a);
            if ownership != "none" {
                assert_eq!(stopped.unwrap().kind, "plan_failed");
            } else if live {
                assert!(stopped.is_none(), "{a:?}");
                assert_eq!(a.progress.unwrap().executor, "plan_reviewer");
            } else {
                assert_eq!(stopped.unwrap().kind, "plan_reviewer_missing");
            }
        }
    }
}

#[test]
fn review_escalation_has_concrete_condition_without_fallback() {
    // Given an ended review candidate escalated at an exact canonical time.
    let mut task = task(Status::Review);
    let mut run = worker();
    run.outcome = Some(corral_core::RunOutcome::Done);
    run.autopilot = Some(
        serde_json::from_value(json!({
            "stage":"escalated","at":at(),"reason":"Owner disposition needed"
        }))
        .unwrap(),
    );
    task.runs.push(run);
    // When assessed, then the concrete escalation replaces an unmatched fallback stall.
    let a = assessment(&task, &[], &[]);
    let stopped = corral_core::attention::blocked(&task, &a).unwrap();
    assert_eq!(stopped.kind, "escalation");
    assert_eq!(stopped.responsible, Responsible::Owner);
    assert_eq!(a.conditions.len(), 1);
}
