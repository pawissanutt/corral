use super::*;
use corral_core::{AgentCmd, PlanRun, Profile, Store};

const PLAN: &str = "---\ngoal: Ship widget\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: spin, expected: clockwise}]\nverification: [check]\nowner_decisions: []\n---\n";

fn fixture() -> (tempfile::TempDir, Ctx, String, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("app")).unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.projects.insert(
        "app".into(),
        corral_core::Project {
            path: "app".into(),
            default_profile: None,
            base: None,
            review: Default::default(),
            artifact_retention: None,
            post_land: Vec::new(),
        },
    );
    store.config.agents.insert(
        "omo".into(),
        AgentCmd {
            command: "true {model_args}".into(),
        },
    );
    store.config.harnesses.get_mut("omo").unwrap().command = "true {model_args}".into();
    for (name, model) in [
        ("gpt-sol", "fixture-planner"),
        ("gpt-review", "fixture-reviewer"),
    ] {
        store.config.profiles.insert(
            name.into(),
            Profile {
                harness: "omo".into(),
                model: Some(model.into()),
                vendor: Some("openai".into()),
                effort: None,
                escalate_to: None,
                args: Vec::new(),
            },
        );
    }
    store.config.review.reviewer = Some("gpt-review".into());
    store.config.review.max_rounds = Some(1);
    store.save_config().unwrap();
    let task = store
        .create(
            "Design widget",
            Some("app"),
            Status::Running,
            "Widget design",
        )
        .unwrap();
    let output = store.state_dir().join("plan-output");
    std::fs::create_dir_all(&output).unwrap();
    let path = output.join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    store
        .update(&task.id, |t| {
            t.plan_required = Some(true);
            t.plan_runs.push(PlanRun {
                id: "D1".into(),
                profile: "gpt-sol".into(),
                harness: "omo".into(),
                model: Some("fixture-planner".into()),
                vendor: Some("openai".into()),
                effort: None,
                spec_revision: crate::plan_spec_revision(t)?,
                started: Timestamp::now(),
                ended: None,
                herdr: None,
                output_dir: output,
                submitted: None,
                rounds: 0,
                state: PlanRunState::Planning,
                pending_reply: None,
            });
            Ok(())
        })
        .unwrap();
    let engine = tmp.path().join("engine");
    std::fs::create_dir(&engine).unwrap();
    let bin = engine.join("herdr");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/plan_engine.sh"),
        &bin,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ctx = Ctx {
        store,
        herdr: corral_herdr::Herdr::with_bin(bin, Some("corral-test".into())),
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    crate::plan_submit(&ctx, &task.id, &path).unwrap();
    (tmp, ctx, task.id, path)
}

fn review(ctx: &Ctx, id: &str, verdict: Verdict) {
    advance(ctx, &ctx.store.get(id).unwrap(), Timestamp::now()).unwrap();
    let task = ctx.store.get(id).unwrap();
    let plan = task.plans.last().unwrap();
    let attempt = plan.reviews.last().unwrap();
    assert_ne!(attempt.reviewer, task.plan_runs[0].profile);
    crate::plan_verdict(
        ctx,
        id,
        &crate::PlanVerdict {
            plan: &plan.id,
            attempt: Some(&attempt.id),
            verdict,
            notes: "Review notes",
        },
    )
    .unwrap();
}

#[test]
fn plan_changes_relaunch_planner_and_stop_at_max_rounds() {
    // Given a submitted plan reviewed by a different model.
    let (_tmp, ctx, id, path) = fixture();
    review(&ctx, &id, Verdict::Changes);
    // When autopilot returns changes and then receives another rejected revision.
    advance(&ctx, &ctx.store.get(&id).unwrap(), Timestamp::now()).unwrap();
    let revised = ctx.store.get(&id).unwrap();
    assert_eq!(revised.plan_runs[0].rounds, 1);
    assert_eq!(revised.plan_runs[0].state, PlanRunState::Changes);
    assert_eq!(revised.status, Status::Running);
    crate::plan_submit(&ctx, &id, &path).unwrap();
    review(&ctx, &id, Verdict::Changes);
    advance(&ctx, &ctx.store.get(&id).unwrap(), Timestamp::now()).unwrap();
    // Then bounded retries escalate without a code candidate or code gate.
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.plan_runs[0].rounds, 1);
    assert_eq!(task.plan_runs[0].state, PlanRunState::Escalated);
    assert_eq!(task.status, Status::NeedsMe);
    assert!(task.runs.is_empty());
    assert!(task.launch.is_none());
    assert_eq!(task.plans.len(), 2);
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
}

#[test]
fn review_approval_waits_for_owner_without_holding_worker_slot() {
    // Given a planning session and independent reviewer.
    let (_tmp, ctx, id, _path) = fixture();
    // When the reviewer approves and autopilot reconciles.
    review(&ctx, &id, Verdict::Approve);
    advance(&ctx, &ctx.store.get(&id).unwrap(), Timestamp::now()).unwrap();
    // Then owner attention is visible, but no acceptance or worker slot exists.
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(task.plan_runs[0].state, PlanRunState::AwaitingOwner);
    assert!(task.plans[0].approval.is_none());
    assert!(task.runs.is_empty());
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
}

#[test]
fn planner_pane_loss_records_failure_and_releases_admission() {
    // Given a planner in its first active round.
    let (_tmp, ctx, id, _path) = fixture();
    ctx.store
        .update(&id, |t| {
            let run = t.plan_runs.last_mut().unwrap();
            run.state = PlanRunState::Planning;
            run.ended = None;
            t.status = Status::Running;
            Ok(())
        })
        .unwrap();
    let task = ctx.store.get(&id).unwrap();
    // When a valid inventory still has no pane after startup grace.
    let now = task.plan_runs[0].started + jiff::SignedDuration::from_secs(60);
    reconcile(&ctx, &task, &Default::default(), now).unwrap();
    // Then it is a failed plan session, not a phantom implementation run.
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(task.plan_runs[0].state, PlanRunState::Failed);
    assert_eq!(task.plan_runs[0].ended, Some(now));
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
}
