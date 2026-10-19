use super::*;

#[test]
fn queued_owner_plan_consent_cannot_submit_after_a_scope_amendment() {
    // Given explicit owner consent and a subscribed barrier before submission.
    let fx = Fx::new();
    let (plan, child) = plan(&fx);
    corral_ops::enqueue(&fx.ctx, &child.id, &Default::default()).unwrap();
    let barrier = Barrier::subscribe(&fx);
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| corral_ops::sync_at(&fx.fresh(), now()).unwrap());
        let mut release = barrier.arrival();
        let store = Store::open(fx.temp.path()).unwrap();
        let mut approved = plan.approved_scope.clone().unwrap();
        approved.goal = "Amended Plan requires new consent".into();
        // When the owner amends aggregate scope after preparation.
        store
            .amend_plan_scope(&plan.id, 0, approved, |_, _, _| Ok(()))
            .unwrap();
        release.write_all(b"R").unwrap();
        worker.join().unwrap();
    });
    drop(barrier);
    // Then no old consent is submitted, and recovery cannot create or submit another run.
    assert!(fx.records("submissions.jsonl").is_empty());
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();
    assert!(fx.records("submissions.jsonl").is_empty());
    assert_eq!(fx.ctx.store.get(&child.id).unwrap().runs.len(), 1);
}

fn plan(fx: &Fx) -> (corral_core::Task, corral_core::Task) {
    let source = fx.temp.path().join("plan.md");
    std::fs::write(
        &source,
        "---\ntitle: Execution Plan\nproject: app\nready: true\napproved_scope:\n  goal: Feature\n  acceptance: [{scenario: Check feature, expected: Pass}]\ntasks:\n  - {key: a, title: Plan member}\n---\n",
    ).unwrap();
    let imported =
        corral_ops::import_plan(&fx.ctx, &source, false, corral_core::Origin::Owner).unwrap();
    (imported.tracker, imported.tasks[0].clone())
}

#[test]
fn main_plan_dispatch_uses_main_instead_of_the_project_default() {
    // Given a main-target Plan in a project whose unrelated default is another branch.
    let mut fx = Fx::new();
    let (plan, child) = plan(&fx);
    let repo = fx.ctx.store.project_path("app").unwrap();
    fixture::git(&repo, &["branch", "other"]);
    fx.ctx.store.config.projects.get_mut("app").unwrap().base = Some("other".into());
    fx.ctx.store.save_config().unwrap();
    fx.grant(&[&plan.id]);
    // When the ordinary authorized Ready queue drains.
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();
    // Then the normal isolated run uses the Plan's target.
    let task = fx.ctx.store.get(&child.id).unwrap();
    assert_eq!(task.current_run().unwrap().base, "main");
    assert_eq!(fx.records("submissions.jsonl").len(), 1);
}

#[test]
fn explicit_off_target_dispatch_cannot_reserve_a_plan_run() {
    // Given an executable main-target member and a different existing branch.
    let fx = Fx::new();
    let (_plan, child) = plan(&fx);
    fixture::git(
        &fx.ctx.store.project_path("app").unwrap(),
        &["branch", "other"],
    );
    // When an explicit ordinary dispatch requests the wrong base.
    let result = corral_ops::dispatch(
        &fx.ctx,
        &child.id,
        &corral_ops::DispatchOpts {
            base: Some("other".into()),
            ..Default::default()
        },
    );
    // Then no run, worktree or agent submission is reserved.
    assert!(result.is_err());
    let task = fx.ctx.store.get(&child.id).unwrap();
    assert!(task.runs.is_empty());
    assert!(task.launch.is_none());
    assert!(fx.records("submissions.jsonl").is_empty());
}

#[test]
fn plan_pause_and_scope_revocation_at_submission_never_duplicate_or_widen_runs() {
    for change in ["pause", "scope", "hold"] {
        // Given exact Plan membership authorization and a pre-subscribed final IPC barrier.
        let mut fx = Fx::new();
        let (plan, child) = plan(&fx);
        let outside = fx.task("outside Plan", Status::Ready);
        fx.grant(&[&plan.id]);
        assert!(
            fx.ctx
                .store
                .effective_operating_policy()
                .unwrap()
                .allows(&child.id, OperatingActionKind::Dispatch)
        );
        let barrier = Barrier::subscribe(&fx);
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| corral_ops::sync_at(&fx.fresh(), now()).unwrap());
            let mut release = barrier.arrival();
            let prepared = fx.ctx.store.get(&child.id).unwrap();
            assert_eq!(prepared.runs.len(), 1);
            let original = authorization(&fx, &child.id, OperatingActionKind::Dispatch);
            let store = Store::open(fx.temp.path()).unwrap();
            // When a separate writer revokes authority after preparation, before submission.
            match change {
                "pause" => {
                    store.pause_operating_policy(true).unwrap();
                }
                "scope" => {
                    let mut approved = plan.approved_scope.clone().unwrap();
                    approved.goal = "Changed scope requires renewed consent".into();
                    store
                        .amend_plan_scope(&plan.id, 0, approved, |_, _, _| Ok(()))
                        .unwrap();
                }
                "hold" => {
                    store
                        .update(&plan.id, |task| {
                            task.dispatch_hold = true;
                            Ok(())
                        })
                        .unwrap();
                }
                _ => unreachable!(),
            }
            release.write_all(b"R").unwrap();
            worker.join().unwrap();
            // Then the operating resolver refuses submission and keeps the original reservation.
            assert!(fx.records("submissions.jsonl").is_empty(), "{change}");
            let refused = authorization(&fx, &child.id, OperatingActionKind::Dispatch);
            assert_eq!(refused.authorization, original.authorization);
            assert!(matches!(
                refused.disposition,
                SubmissionDisposition::AuthorizationRefused { .. }
            ));
        });
        drop(barrier);
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        assert!(fx.records("submissions.jsonl").is_empty());
        assert_eq!(fx.ctx.store.get(&child.id).unwrap().runs.len(), 1);
        assert!(fx.ctx.store.get(&outside.id).unwrap().runs.is_empty());
        // Restoring unchanged scope reuses its run. Changed scope needs a new action,
        // not a refreshed identity for the already prepared worktree.
        if change == "hold" {
            fx.ctx
                .store
                .update(&plan.id, |task| {
                    task.dispatch_hold = false;
                    Ok(())
                })
                .unwrap();
        }
        fx.grant(&[&plan.id]);
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        assert_eq!(fx.ctx.store.get(&child.id).unwrap().runs.len(), 1);
        assert_eq!(
            fx.records("submissions.jsonl").len(),
            usize::from(change != "scope"),
            "{change}"
        );
        assert!(fx.ctx.store.get(&outside.id).unwrap().runs.is_empty());
    }
}
