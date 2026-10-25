//! Canonical lifecycle projection, stable identities, and bounded read regressions.
use std::collections::BTreeMap;

use corral_core::attention::{Assessment, Responsible, SinceSource};
use corral_core::inbox::{Inbox, MessageBinding, Post, Recipient, Urgency};
use corral_core::{
    Config, EffectivePolicy, EffectiveTaskPolicy, OperatingMode, PlanRecord, PlanRun, PlanRunState,
    Run, Status, Store, Task,
};
use corral_ops::Ctx;
use corral_ops::attention::{Neighborhood, assess, board};
use jiff::Timestamp;
use serde_json::json;

fn at() -> Timestamp {
    "2026-10-25T23:00:00Z".parse().unwrap()
}

fn run() -> Run {
    serde_json::from_value(json!({
        "id":"R1","agent":"fixture","worktree":"/not-a-repository",
        "branch":"corral/T","base":"main","base_sha":"abc",
        "started":"2026-10-25T20:00:00Z","agent_state":"working","delivery":"inbox"
    }))
    .unwrap()
}

fn planner(state: PlanRunState) -> PlanRun {
    serde_json::from_value(json!({
        "id":"P1","profile":"fixture","harness":"fixture","model":null,
        "vendor":null,"effort":null,"spec_revision":"spec",
        "started":"2026-10-25T20:00:00Z","ended":null,"herdr":null,
        "output_dir":"/fixture","submitted":null,"rounds":0,"state":state,
        "pending_reply":null
    }))
    .unwrap()
}

fn plan() -> PlanRecord {
    serde_json::from_value(json!({
        "id":"P1","spec_revision":"spec",
        "artifact":{
            "run":"P1","kind":"plan","title":"Design","path":"plan.md",
            "source":"/fixture/plan.md","sha256":"a","bytes":1,"added":"2026-10-25T20:01:00Z"
        },
        "document":{
            "goal":"A tested change","non_goals":[],"decisions":[],
            "affected_paths":["src/main.rs"],"contracts":[],"dependencies":[],
            "acceptance":[{"scenario":"works","expected":"done"}],
            "verification":["cargo test"],"owner_decisions":[]
        }
    }))
    .unwrap()
}

fn policy(tasks: &[Task], config: &Config) -> EffectivePolicy {
    EffectivePolicy {
        mode: OperatingMode::Autopilot,
        paused: false,
        revision: "test".into(),
        authorization: Some("grant".into()),
        reasons: Vec::new(),
        tasks: tasks
            .iter()
            .map(|task| {
                (
                    task.id.clone(),
                    EffectiveTaskPolicy {
                        review: config.task_review_policy(task),
                        acceptance_source: corral_core::PolicySource::Task,
                        plan_acceptance_source: corral_core::PolicySource::Task,
                        refusals: BTreeMap::new(),
                    },
                )
            })
            .collect(),
    }
}

fn project(store: &mut Store) {
    store
        .edit_config(|doc| doc.set_str("projects.app.path", "."))
        .unwrap();
    store.reload_config_if_changed().unwrap();
}

fn evaluate(task: &Task, config: &Config) -> Assessment {
    let tasks = vec![task.clone()];
    let policy = policy(&tasks, config);
    assess(task, &Neighborhood::new(&tasks, config, &policy), at())
}

fn shape(assessment: &Assessment) -> Vec<(Option<String>, Option<Timestamp>)> {
    assessment
        .conditions
        .iter()
        .map(|row| (row.key.as_ref().map(ToString::to_string), row.since))
        .collect()
}

fn question(key: &str, timestamp: &str) -> corral_core::agent::AgentQuestion {
    serde_json::from_value(json!({
        "version":1,"key":key,"revision":1,"kind":"clarification","blocking":false,
        "text":"Which constraint applies?","state":"open",
        "asked":{"request_id":key,"payload_sha256":"sha","session_id":"session","generation":1,"at":timestamp},
        "wake":{"route":"none","key":key,"state":"pending"}
    })).unwrap()
}

#[test]
fn lifecycle_table_covers_normal_executors_and_waits_without_stalls() {
    // Given one canonical record per normal lifecycle row.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    project(&mut store);
    let base = store
        .create("Lifecycle", Some("app"), Status::Ready, "")
        .unwrap();
    let mut rows: Vec<(String, Task, &str)> = Vec::new();
    let mut add = |name: &str, task: Task, expected| rows.push((name.into(), task, expected));
    add("fresh ready", base.clone(), "dispatcher");
    for state in [PlanRunState::Planning, PlanRunState::Changes] {
        let mut task = base.clone();
        task.status = Status::Running;
        task.plan_runs.push(planner(state));
        add("active planner", task, "planner");
    }
    let mut pending = base.clone();
    pending.status = Status::Review;
    pending.plans.push(plan());
    add("plan review pending", pending.clone(), "plan_pilot");
    let review = serde_json::from_value(json!({
        "id":"PR1","reviewer":"rev","policy_digest":"policy","at":"2026-10-25T20:02:00Z",
        "herdr":{"pane_id":"reviewer","workspace_id":"review"}
    }))
    .unwrap();
    pending.plans[0].reviews.push(review);
    add("live plan reviewer", pending.clone(), "plan_reviewer");
    pending.plans[0].reviews[0].verdict = Some(corral_core::Verdict::Approve);
    pending.plan_runs.push(planner(PlanRunState::AwaitingOwner));
    add("plan owner", pending.clone(), "plan_awaiting_owner");
    pending.plan_runs[0].state = PlanRunState::Approved;
    pending.plans[0].approval = Some(
        serde_json::from_value(json!({
            "by":"owner","at":"2026-10-25T20:03:00Z","policy_digest":"policy"
        }))
        .unwrap(),
    );
    add("decomposition", pending, "plan_pilot");
    for stage in [
        "reserved",
        "worktree",
        "creating_pane",
        "pane",
        "submitting",
    ] {
        let mut task = base.clone();
        task.runs.push(run());
        task.launch = Some(serde_json::from_value(json!({"stage":stage,"reserved":true})).unwrap());
        add(stage, task, "launcher");
    }
    let mut failed = base.clone();
    failed.runs.push(run());
    failed.launch = Some(
        serde_json::from_value(
            json!({"stage":"submitting","reserved":true,"error":"authorization refused"}),
        )
        .unwrap(),
    );
    add("failed launch", failed, "launch_failed");
    for work in [
        corral_core::ReviewWork::Gate,
        corral_core::ReviewWork::Reviewer,
    ] {
        let mut task = base.clone();
        task.status = Status::Review;
        task.runs.push(run());
        task.runs[0].outcome = Some(corral_core::RunOutcome::Done);
        task.review_wait = Some(work);
        add("review capacity", task, "review_capacity");
    }
    for (name, fields, expected) in [
        (
            "gate",
            json!({"outcome":"done","gate":{"result":"running","at":"2026-10-25T20:04:00Z","sha":"abc"}}),
            "gate",
        ),
        (
            "reviewer",
            json!({"outcome":"done","review":{"reviewer":"rev","sha":"abc","at":"2026-10-25T20:05:00Z","pane":"reviewer"}}),
            "reviewer",
        ),
        ("autopilot", json!({"outcome":"done"}), "autopilot"),
        ("manual review", json!({}), "manual_review_open_run"),
        (
            "owner accept",
            json!({"outcome":"done","autopilot":{"stage":"reviewed","at":"2026-10-25T20:06:00Z"},"review":{"reviewer":"rev","sha":"abc","at":"2026-10-25T20:05:00Z","verdict":"approve"}}),
            "owner_accept",
        ),
        (
            "landing blocked",
            json!({"outcome":"done","autopilot":{"stage":"blocked","at":"2026-10-25T20:06:00Z","reason":"target dirty"}}),
            "landing_blocked",
        ),
    ] {
        let mut task = base.clone();
        task.status = Status::Review;
        let mut worker = serde_json::to_value(run()).unwrap();
        worker
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        task.runs.push(serde_json::from_value(worker).unwrap());
        add(name, task, expected);
    }
    let mut held = base.clone();
    held.dispatch_hold = true;
    add("owner hold", held, "dispatch_hold");
    let mut forbidden = base.clone();
    forbidden.project = Some("missing".into());
    add("project not allowed", forbidden, "project_not_allowed");
    let mut dependency = base.clone();
    dependency.depends_on.push("T-999".into());
    add("prerequisite", dependency, "prerequisite");
    let mut worker = base.clone();
    worker.status = Status::Running;
    worker.runs.push(run());
    add("live worker", worker.clone(), "worker");
    worker.status = Status::NeedsMe;
    worker.runs[0].agent_state = Some("blocked".into());
    add("live blocked worker", worker, "worker");
    for state in [
        PlanRunState::Failed,
        PlanRunState::Escalated,
        PlanRunState::Cancelled,
    ] {
        let mut task = base.clone();
        task.status = Status::NeedsMe;
        let mut failed = planner(state);
        failed.ended = Some(at());
        task.plan_runs.push(failed);
        add("failed planner", task, "plan_failed");
    }
    let mut owner = base.clone();
    owner.status = Status::NeedsMe;
    add("plain needs me", owner, "needs_me_other");
    for held in [false, true] {
        let mut task = base.clone();
        task.status = Status::Review;
        task.infra_wait=Some(serde_json::from_value(json!({
            "version":1,"class":"contention","marker":"index.lock","path":"/repo/.git/index.lock",
            "first_seen":"2026-10-25T20:00:00Z","first_mtime":null,
            "held_at":if held {Some("2026-10-25T20:05:00Z")} else {None}
        })).unwrap());
        add(
            "infrastructure wait",
            task,
            if held { "infra_hold" } else { "infra_wait" },
        );
    }
    let mut aggregate = base.clone();
    aggregate.status = Status::Running;
    aggregate.tracker = true;
    let identity = corral_core::CandidateIdentity {
        version: 1,
        task: base.id.clone(),
        run: "aggregate".into(),
        attempt: "A1".into(),
        head: "abc".into(),
        tree: "tree".into(),
        integration_base: "abc".into(),
        dispatch_base: "abc".into(),
        spec_digest: "spec".into(),
        policy_digest: "policy".into(),
        inputs_digest: "inputs".into(),
        environment: Vec::new(),
        gate_tier: None,
    };
    let mut attempt: corral_core::AggregateAttempt = serde_json::from_value(json!({
        "id":"A1","identity":identity,"target":{"kind":"main"},"code":true,"members":[],
        "started":"2026-10-25T20:04:00Z",
        "gate":{"result":"running","at":"2026-10-25T20:04:00Z","sha":"abc"}
    }))
    .unwrap();
    aggregate.aggregate_attempts.push(attempt.clone());
    add("aggregate in progress", aggregate.clone(), "aggregate_gate");
    attempt.gate = None;
    aggregate.aggregate_attempts[0] = attempt;
    add("aggregate owner", aggregate, "aggregate_awaiting_owner");
    // When each row is assessed without any process or history reads.
    for (name, task, expected) in rows {
        let result = evaluate(&task, &store.config);
        // Then its declared normal executor or condition exists, never a fallback stall.
        assert!(
            result
                .progress
                .as_ref()
                .is_some_and(|p| p.executor == expected)
                || result
                    .conditions
                    .iter()
                    .any(|condition| condition.kind == expected),
            "{name}: {result:?}"
        );
        assert!(
            !result
                .conditions
                .iter()
                .any(|condition| condition.kind == "unexpected_stall"),
            "{name}"
        );
        for condition in &result.conditions {
            assert!(!condition.reason_code.is_empty());
            assert!(!condition.resume.is_empty(), "{name}: {condition:?}");
            if matches!(
                condition.class,
                corral_core::WaitClass::Capacity
                    | corral_core::WaitClass::Hold
                    | corral_core::WaitClass::Prerequisite
            ) {
                assert!(condition.key.is_none());
            }
        }
        let mut unrelated = task.clone();
        unrelated.body.push_str("\nAn unrelated comment.");
        unrelated.updated = at();
        if let Some(run) = unrelated.current_run_mut() {
            run.usage_note = Some("refreshed telemetry".into());
        }
        assert_eq!(
            shape(&result),
            shape(&evaluate(&unrelated, &store.config)),
            "{name}"
        );
    }
    let mut paused = store.config.clone();
    paused.dispatch.paused = true;
    assert!(
        evaluate(&base, &paused)
            .conditions
            .iter()
            .any(|c| c.kind == "dispatch_paused")
    );
    let mut full = store.config.clone();
    full.limits.review = 0;
    let mut pending = base.clone();
    pending.status = Status::Review;
    pending.plans.push(plan());
    pending.plans[0].reviews.push(
        serde_json::from_value(json!({
            "id":"PR1","reviewer":"rev","policy_digest":"p","at":"2026-10-25T20:02:00Z"
        }))
        .unwrap(),
    );
    assert!(
        evaluate(&pending, &full)
            .conditions
            .iter()
            .any(|c| c.kind == "plan_review_capacity")
    );
}

#[test]
fn overlapping_question_delivery_and_decision_keys_resolve_independently() {
    // Given two questions, one pending decision and a terminal owner message on a live worker.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create_initialized("Overlap", None, Status::Running, "", |task| {
            let mut worker = run();
            let mut host =
                corral_core::agent::AgentRunState::new(corral_core::agent::AgentBinding {
                    role: corral_core::agent::AgentRole::Worker,
                    store: "store".into(),
                    project: "app".into(),
                    generation: 1,
                    health: corral_core::agent::HostHealth::Healthy,
                    callback: None,
                    closing: false,
                });
            host.questions = vec![
                question(&format!("{}/R1/Q1", task.id), "2026-10-25T20:00:00Z"),
                question(&format!("{}/R1/Q2", task.id), "2026-10-25T20:01:00Z"),
            ];
            worker.agent_host = Some(Box::new(host));
            worker.obligations.decisions.push(serde_json::from_value(json!({
            "request":{"version":1,"id":"D1","run":"R1","question":"Owner choice",
                "requested_action":"Present the decision","owner":"owner","source":"worker"},
            "registered_at":"2026-10-25T21:00:00Z",
            "observation":{"at":"2026-10-25T21:00:00Z","state":"pending","detail":"awaiting owner"},
            "receipt":null
        }))?);
            task.runs.push(worker);
            Ok(())
        })
        .unwrap();
    let inbox = Inbox::new(
        &store,
        Recipient::Run {
            task: task.id.clone(),
            run: "R1".into(),
        },
    )
    .unwrap();
    let message = inbox
        .post(Post {
            kind: "owner_note",
            urgency: Urgency::Wake,
            author: "owner",
            body: "Original instruction",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: corral_core::inbox::MAX_PAYLOAD,
        })
        .unwrap();
    inbox
        .mark_terminal(
            message.seq,
            corral_core::inbox::TerminalState::Cancelled,
            "T-128-style stale retire",
            "fixture",
        )
        .unwrap();
    let ctx = Ctx::from_store(store.clone());
    let first = board(&ctx, at())
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == task.id)
        .unwrap();
    assert_eq!(
        first.assessment.conditions.len(),
        4,
        "{:?}",
        first.assessment
    );
    assert_eq!(
        first.assessment.primary.as_ref().unwrap().kind,
        "decision_pending"
    );
    let retained = first
        .assessment
        .conditions
        .iter()
        .filter(|row| !row.key.as_ref().unwrap().0.contains("/Q1@"))
        .map(|row| (row.key.clone(), row.since))
        .collect::<Vec<_>>();
    // When only the oldest question is answered in its canonical record.
    ctx.store
        .update(&task.id, |task| {
            let q = &mut task.runs[0].agent_host.as_mut().unwrap().questions[0];
            q.state = corral_core::agent::QuestionState::Answered;
            Ok(())
        })
        .unwrap();
    // Then exactly that condition disappears; the other independent keys and ages stay.
    let second = board(&ctx, at())
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == task.id)
        .unwrap();
    assert_eq!(second.assessment.conditions.len(), 3);
    assert_eq!(
        retained,
        second
            .assessment
            .conditions
            .iter()
            .map(|row| (row.key.clone(), row.since))
            .collect::<Vec<_>>()
    );
}

#[cfg(unix)]
#[path = "operating_launch/fixture.rs"]
mod operating_fixture;

#[cfg(unix)]
#[test]
fn scoped_pm_waits_share_attention_and_ledger_writes_keep_action_identity() {
    use corral_core::{NoticeClaim, NoticeFact};
    // Given a scoped grant with an executable Ready task and an owner-held task.
    let mut fx = operating_fixture::Fx::new();
    let task = fx.task("Executable", Status::Ready);
    let held = fx.task("Waiting", Status::Ready);
    let independent = fx.task("Unscoped completed candidate", Status::Ready);
    fx.start(&independent.id);
    fx.done(&independent.id, "independent.txt");
    fx.ctx
        .store
        .update(&held.id, |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    fx.grant(&[&task.id, &held.id]);
    let before = corral_ops::pm_workset(&fx.fresh()).unwrap();
    assert_eq!(fx.records("submissions.jsonl").len(), 1);
    let action = before
        .actions
        .iter()
        .find(|action| action.action.task.as_deref() == Some(&task.id))
        .unwrap()
        .id
        .clone();
    let primary = before
        .assessments
        .iter()
        .find(|row| row.task == held.id)
        .unwrap()
        .assessment
        .primary
        .as_ref()
        .unwrap();
    assert_eq!(
        before
            .waits
            .iter()
            .find(|row| row.task.as_deref() == Some(&held.id))
            .unwrap()
            .evidence[0],
        primary.reason
    );
    let timestamp = fx.ctx.store.get(&task.id).unwrap().updated;
    // When real ledger-only writes occur on the target.
    fx.ctx
        .store
        .update_attention(&task.id, |ledger| {
            ledger.push_notice(NoticeClaim {
                version: 1,
                batch_id: "fixture".into(),
                producer: "fixture".into(),
                source: "fixture".into(),
                claimed_at: at(),
                claim_ids: vec!["fixture".into()],
                coverage: Vec::new(),
                exemption_basis: None,
                outcome: None,
                pending: vec![NoticeFact {
                    fact_id: "fixture/open".into(),
                    kind: "notification_claimed".into(),
                    detail: json!({}),
                }],
            })
        })
        .unwrap();
    let current = corral_ops::pm_workset(&fx.fresh()).unwrap();
    // Then authority identity is unchanged, but a non-attention write invalidates it.
    assert_eq!(fx.ctx.store.get(&task.id).unwrap().updated, timestamp);
    assert!(current.actions.iter().any(|row| row.id == action));
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.body.push_str("\nChanged contract");
            Ok(())
        })
        .unwrap();
    assert!(
        !corral_ops::pm_workset(&fx.fresh())
            .unwrap()
            .actions
            .iter()
            .any(|row| row.id == action)
    );
}
#[test]
fn t128_reviewing_without_reviewer_has_stable_canonical_stall_age() {
    // Given the T-128 canonical stage, with prose-only parking and no live reviewer.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("T128", None, Status::Review, "Parked pending re-audit")
        .unwrap();
    let mut worker = run();
    worker.outcome = Some(corral_core::RunOutcome::Done);
    worker.autopilot = Some(
        serde_json::from_value(json!({"stage":"reviewing","at":"2026-10-25T20:21:36Z"})).unwrap(),
    );
    task.runs.push(worker);
    // When it is assessed.
    let a = evaluate(&task, &store.config);
    let condition = a.primary.unwrap();
    // Then the unmatched stage, PM responsibility, anchor and honest age are exposed.
    assert_eq!(condition.kind, "unexpected_stall");
    assert_eq!(condition.responsible, Responsible::Pm);
    assert_eq!(
        condition.since,
        Some("2026-10-25T20:21:36Z".parse().unwrap())
    );
    assert_eq!(condition.age_secs, Some(9504));
    assert_eq!(condition.since_source, SinceSource::Canonical);
    assert!(condition.reason.contains("stage=reviewing"));
}

#[test]
fn t318_infrastructure_escalation_keeps_gate_recovery_evidence() {
    // Given an old infrastructure escalation with the gate still retained.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("T318", None, Status::NeedsMe, "").unwrap();
    let mut worker = run();
    worker.outcome = Some(corral_core::RunOutcome::Done);
    worker.autopilot=Some(serde_json::from_value(json!({
        "stage":"escalated","at":"2026-10-25T22:43:09Z","reason":"integration preflight error: index.lock"
    })).unwrap());
    worker.gate = Some(
        serde_json::from_value(
            json!({"result":"pass","at":"2026-10-25T20:00:00Z","sha":"retained-sha"}),
        )
        .unwrap(),
    );
    task.runs.push(worker);
    // When attention is projected.
    let condition = evaluate(&task, &store.config).primary.unwrap();
    // Then the PM sees the old canonical time and the existing recovery owner, not invented permission.
    assert_eq!(condition.kind, "infra_escalation");
    assert_eq!(
        condition.since,
        Some("2026-10-25T22:43:09Z".parse().unwrap())
    );
    assert_eq!(condition.responsible, Responsible::Pm);
    assert!(condition.next_action.description.contains("T-377"));
    assert!(condition.next_action.description.contains("retained-sha"));
}

#[test]
fn unknown_inventory_never_turns_stale_liveness_into_lost_agent() {
    // Given a Running task whose old pane observation said gone.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Unknown inventory", None, Status::Running, "")
        .unwrap();
    let mut worker = run();
    worker.agent_state = Some("gone".into());
    task.runs.push(worker);
    let tasks = vec![task.clone()];
    let policy = policy(&tasks, &store.config);
    let n = Neighborhood::new(&tasks, &store.config, &policy);
    // When inventory is unavailable, the classifier uses no new absence evidence.
    let a = assess(&task, &n, at());
    // Then the worker remains progressing with unknown liveness, not a new stall.
    assert_eq!(a.progress.unwrap().liveness, "unknown");
    assert!(
        !a.conditions
            .iter()
            .any(|c| c.kind == "agent_lost" || c.kind == "unexpected_stall")
    );
}

#[test]
fn unrelated_store_writes_and_restart_preserve_keys_but_new_stage_changes_them() {
    // Given a canonical escalation persisted through the real Store.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create_initialized("Stable", None, Status::NeedsMe, "", |task| {
            let mut worker = run();
            worker.outcome = Some(corral_core::RunOutcome::Done);
            worker.autopilot = Some(serde_json::from_value(json!({
                "stage":"escalated","at":"2026-10-25T20:00:00Z","reason":"round limit"
            }))?);
            task.runs.push(worker);
            Ok(())
        })
        .unwrap();
    let before = evaluate(&task, &store.config);
    // When unrelated comments/body, usage and timestamp-only writes occur and the context restarts.
    store
        .update(&task.id, |task| {
            task.body.push_str("\nUnrelated comment.");
            Ok(())
        })
        .unwrap();
    store
        .update(&task.id, |task| {
            task.runs[0].usage_note = Some("refreshed".into());
            Ok(())
        })
        .unwrap();
    store.update(&task.id, |_| Ok(())).unwrap();
    let reopened = Store::open(tmp.path()).unwrap();
    let after = evaluate(&reopened.get(&task.id).unwrap(), &reopened.config);
    // Then identities and since survive, while a new canonical episode changes its key.
    assert_eq!(shape(&before), shape(&after));
    let new = reopened
        .update(&task.id, |task| {
            task.runs[0].autopilot.as_mut().unwrap().at = "2026-10-25T21:00:00Z".parse()?;
            Ok(())
        })
        .unwrap();
    assert_ne!(shape(&after), shape(&evaluate(&new, &reopened.config)));
}

#[test]
fn legacy_status_age_is_unknown_until_durable_discovery_exists() {
    // Given a legacy Needs me row with no canonical status timestamp.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Legacy", None, Status::NeedsMe, "").unwrap();
    task.status_since = None;
    // When it is assessed without a ledger and later with a persisted first-discovery record.
    let initial = evaluate(&task, &store.config).primary.unwrap();
    assert_eq!(initial.since_source, SinceSource::Discovered);
    assert!(initial.since.is_none());
    assert!(initial.age_secs.is_none());
    let discovered = "2026-10-25T22:00:00Z".parse().unwrap();
    task.attention.records.insert(
        initial.key.clone().unwrap(),
        corral_core::AttentionRecord {
            kind: initial.kind,
            responsible: initial.responsible,
            since: discovered,
            since_source: SinceSource::Discovered,
            first_recorded: discovered,
            opened_at: discovered,
            pm_live_at_open: None,
            levels: Default::default(),
            handling: None,
            first_handled_at: None,
            resolved_at: None,
        },
    );
    // Then age is based on first discovery, not created/updated or an invented status time.
    let recorded = evaluate(&task, &store.config).primary.unwrap();
    assert_eq!(recorded.since, Some(discovered));
    assert_eq!(recorded.age_secs, Some(3600));
}

#[test]
fn hot_board_reads_ignore_event_git_and_settled_history_growth() {
    use corral_core::work::{Work, count};
    // Given one live owner header, with unrelated history kept outside it.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create_initialized("Active", None, Status::Running, "", |task| {
            task.runs.push(run());
            Ok(())
        })
        .unwrap();
    let recipient = Recipient::Run {
        task: task.id.clone(),
        run: "R1".into(),
    };
    let inbox = Inbox::new(&store, recipient).unwrap();
    let post = |body| Post {
        kind: "owner_note",
        urgency: Urgency::Wake,
        subject: None,
        max_payload: corral_core::inbox::MAX_PAYLOAD,
        body,
        author: "owner",
        binding: MessageBinding::default(),
    };
    inbox.post(post("Current instruction")).unwrap();
    let ctx = Ctx::from_store(store.clone());
    let grow_history = |tasks: std::ops::Range<u32>, messages: std::ops::Range<u64>| {
        for index in messages {
            std::fs::write(
                inbox.message_path(index),
                b"history is deliberately not decodable",
            )
            .unwrap();
        }
        for index in tasks {
            let mut done = task.clone();
            done.id = format!("T-{index:03}");
            done.status = Status::Done;
            done.title = format!("History {index}");
            done.path = Default::default();
            done.runs.clear();
            ctx.store.save(&done).unwrap();
        }
        ctx.store.list_shared().unwrap();
    };
    grow_history(100..101, 100..200);
    let baseline_reads = count(Work::InboxRecordRead);
    let baseline_visits = count(Work::BoardVisit);
    let first = board(&ctx, at()).unwrap();
    let baseline_reads = count(Work::InboxRecordRead) - baseline_reads;
    let baseline_visits = count(Work::BoardVisit) - baseline_visits;
    let active = first
        .tasks
        .iter()
        .find(|row| row.task == task.id)
        .unwrap()
        .clone();
    let event_path = ctx.store.dir().join("events.jsonl");
    std::fs::remove_file(&event_path).unwrap();
    std::fs::create_dir(&event_path).unwrap();
    // Growth occurs before warm counting; task-list parsing itself remains the Store's responsibility.
    // Historical inbox files are not reachable from the retained header.
    grow_history(101..110, 200..1100);
    let reads = count(Work::InboxRecordRead);
    let parses = count(Work::TaskParse);
    let events = count(Work::EventRead);
    let bodies = count(Work::InboxMessageRead);
    let envelopes = count(Work::InboxEnvelopeRead);
    let lists = count(Work::InboxList);
    let visits = count(Work::BoardVisit);
    // When a warm board is built with an unreadable event log and invalid Git checkout.
    let after = board(&ctx, at()).unwrap();
    // Then unfinished assessments stay byte-equal and hot work is bounded by one current identity.
    assert_eq!(
        after.tasks.iter().find(|row| row.task == task.id).unwrap(),
        &active
    );
    assert_eq!(count(Work::TaskParse) - parses, 0);
    assert_eq!(count(Work::EventRead) - events, 0);
    assert_eq!(count(Work::InboxMessageRead) - bodies, 0);
    assert_eq!(count(Work::InboxEnvelopeRead) - envelopes, 0);
    assert_eq!(count(Work::InboxList) - lists, 0);
    assert_eq!(count(Work::InboxRecordRead) - reads, baseline_reads);
    assert_eq!(count(Work::BoardVisit) - visits, baseline_visits);
    assert_eq!(baseline_visits, 1);
    assert!(count(Work::InboxRecordRead) - reads <= 5);
}
