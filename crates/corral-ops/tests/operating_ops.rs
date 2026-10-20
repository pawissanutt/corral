#![cfg(unix)]

#[path = "operating_launch/fixture.rs"]
mod fixture;
#[path = "operating_ops/holds.rs"]
mod holds;
#[path = "operating_ops/withdrawal.rs"]
mod withdrawal;

use corral_core::{Accept, Status};
use fixture::{Fx, now};

#[test]
fn legacy_settings_and_future_work_do_not_supply_dispatch_authority() {
    // Given legacy-enabled automation and a finite grant for one task only.
    let mut fx = Fx::new();
    let included = fx.task("included", Status::Ready);
    let excluded = fx.task("excluded", Status::Ready);
    corral_ops::sync_at(&fx.ctx, now()).unwrap();
    assert!(fx.records("submissions.jsonl").is_empty());
    fx.grant(&[&included.id]);
    let future = fx.task("future", Status::Backlog);
    // When the existing queue and backlog executors run.
    corral_ops::sync_at(&fx.ctx, now()).unwrap();
    // Then only the exact confirmed task starts.
    assert_eq!(
        fx.ctx.store.get(&included.id).unwrap().status,
        Status::Running
    );
    for id in [&excluded.id, &future.id] {
        assert!(fx.ctx.store.get(id).unwrap().runs.is_empty());
    }
    assert_eq!(fx.records("submissions.jsonl").len(), 1);
}

#[test]
fn owner_queue_approval_starts_only_its_task_in_assist() {
    // Given Assist with two Ready tasks and a durable, one-task owner queue action.
    let fx = Fx::new();
    let included = fx.task("owner start", Status::Ready);
    let other = fx.task("unapproved", Status::Ready);
    corral_ops::enqueue(&fx.ctx, &included.id, &Default::default()).unwrap();
    // When a fresh process drains the queue.
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();
    // Then it consumes only the explicit action, not Ready membership or free slots.
    assert_eq!(
        fx.ctx.store.get(&included.id).unwrap().status,
        Status::Running
    );
    assert!(fx.ctx.store.get(&other.id).unwrap().runs.is_empty());
    assert_eq!(fx.records("submissions.jsonl").len(), 1);
}

#[test]
fn explicit_start_does_not_authorize_following_review_or_backlog() {
    // Given Assist and a direct owner-approved start.
    let fx = Fx::new();
    let task = fx.task("explicit start", Status::Ready);
    let other = fx.task("backlog", Status::Backlog);
    // When the owner starts exactly that task and sync observes it.
    fx.start(&task.id);
    fx.done(&task.id, "assist.txt");
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();
    // Then the run exists but no workspace review or additional-start grant exists.
    assert_eq!(fx.records("submissions.jsonl").len(), 1);
    assert!(fx.ctx.store.get(&other.id).unwrap().runs.is_empty());
    assert!(
        !fx.ctx
            .store
            .effective_operating_policy()
            .unwrap()
            .allows(&task.id, corral_core::OperatingActionKind::Review)
    );
}

#[test]
fn finite_dependency_chain_uses_bounded_repair_independent_review_and_verified_landing() {
    // Given A -> B with one failed gate and fully delegated, finite authority.
    let mut fx = Fx::new();
    fx.ctx.store.config.review.gate = Some(vec!["test -f repaired.txt".into()]);
    fx.ctx.store.save_config().unwrap();
    let first = fx.task("first", Status::Ready);
    let second = fx.task("second", Status::Backlog);
    fx.ctx
        .store
        .update(&second.id, |task| {
            task.depends_on.push(first.id.clone());
            Ok(())
        })
        .unwrap();
    fx.grant(&[&first.id, &second.id]);
    corral_ops::sync_at(&fx.ctx, now()).unwrap();
    assert!(fx.ctx.store.get(&second.id).unwrap().runs.is_empty());
    fx.done(&first.id, "first.txt");
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();
    let repairing = fx.ctx.store.get(&first.id).unwrap();
    assert_eq!(
        repairing.status,
        Status::Running,
        "{:#?}",
        fx.ctx.store.events().unwrap()
    );
    assert_eq!(repairing.rounds, 1);
    assert_eq!(fx.records("deliveries.jsonl").len(), 1);
    assert!(fx.ctx.store.get(&second.id).unwrap().runs.is_empty());

    // When the worker completes its bounded repair, a separate reviewer approves each candidate.
    fx.done(&first.id, "repaired.txt");
    for id in [&first.id, &second.id] {
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        let reviewed = fx.ctx.store.get(id).unwrap();
        let run = reviewed.current_run().unwrap();
        let review = run.review.as_ref().unwrap();
        assert_eq!(review.reviewer, "rev");
        assert_ne!(
            review.pane,
            run.herdr.as_ref().map(|herdr| herdr.pane_id.clone())
        );
        corral_ops::verdict_bound(
            &fx.fresh(),
            id,
            corral_core::Verdict::Approve,
            "Independent fixture approval",
            Some(&corral_ops::review_callback_token(review).unwrap()),
        )
        .unwrap();
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        assert_eq!(fx.ctx.store.get(id).unwrap().status, Status::Done);
        if id == &first.id {
            assert_eq!(
                fx.ctx.store.get(&second.id).unwrap().status,
                Status::Running
            );
            fx.done(&second.id, "second.txt");
        }
    }
    // Then both exact obligations are accepted with durable audit records on main.
    let repo = fx.temp.path().join("app");
    assert!(repo.join("first.txt").is_file());
    assert!(repo.join("second.txt").is_file());
    for id in [&first.id, &second.id] {
        let task = fx.ctx.store.get(id).unwrap();
        assert!(
            task.artifacts
                .iter()
                .any(|artifact| artifact.title == "Acceptance record")
        );
    }
    assert_eq!(fx.ctx.store.get(&first.id).unwrap().rounds, 1);
}

#[test]
fn holds_and_changed_contracts_do_not_block_independent_authorized_work() {
    // Given a held and an independent task in one finite grant.
    let mut fx = Fx::new();
    let held = fx.task("held", Status::Backlog);
    let changed = fx.task("changed", Status::Ready);
    let independent = fx.task("independent", Status::Ready);
    fx.ctx
        .store
        .update(&held.id, |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    fx.grant(&[&held.id, &changed.id, &independent.id]);
    fx.ctx
        .store
        .update(&changed.id, |task| {
            task.body.push_str(" changed");
            Ok(())
        })
        .unwrap();
    // When the board advances with available capacity.
    corral_ops::sync_at(&fx.ctx, now()).unwrap();
    // Then the real holds remain while unrelated delegated work proceeds.
    assert!(fx.ctx.store.get(&held.id).unwrap().dispatch_hold);
    assert!(fx.ctx.store.get(&held.id).unwrap().runs.is_empty());
    assert!(fx.ctx.store.get(&changed.id).unwrap().runs.is_empty());
    assert_eq!(
        fx.ctx.store.get(&independent.id).unwrap().status,
        Status::Running
    );
}

#[test]
fn explicit_owner_acceptance_is_not_removed_by_a_dispatch_grant() {
    // Given an explicit task acceptance restriction in an otherwise delegated workload.
    let mut fx = Fx::new();
    let owner = fx.task("owner acceptance", Status::Ready);
    fx.ctx
        .store
        .update(&owner.id, |task| {
            task.review = Some(Accept::Owner);
            Ok(())
        })
        .unwrap();
    fx.grant(&[&owner.id]);
    // When authority is resolved and the task starts.
    let policy = fx.ctx.store.effective_operating_policy().unwrap();
    corral_ops::sync_at(&fx.ctx, now()).unwrap();
    // Then checking/start authority does not imply acceptance.
    assert!(policy.allows(&owner.id, corral_core::OperatingActionKind::Gate));
    assert!(!policy.allows(&owner.id, corral_core::OperatingActionKind::Accept));
    assert_eq!(
        fx.ctx.store.get(&owner.id).unwrap().review,
        Some(Accept::Owner)
    );
}

#[test]
fn authentication_disposition_precedes_stale_review_when_main_moves() {
    // Given a synthetic owner-authentication blocker, never the owner's live task.
    let mut fx = Fx::new();
    let blocked = fx.task("authentication disposition", Status::Ready);
    fx.start(&blocked.id);
    fx.done(&blocked.id, "blocked.txt");
    corral_ops::gate_start(&fx.ctx, &blocked.id).unwrap();
    corral_ops::gate_collect(&fx.ctx, &blocked.id)
        .unwrap()
        .unwrap();
    let review = corral_ops::review_run(&fx.ctx, &blocked.id, Some("rev")).unwrap();
    corral_ops::verdict_bound(
        &fx.ctx,
        &blocked.id,
        corral_core::Verdict::Approve,
        "Independently reviewed before main moved",
        Some(&corral_ops::review_callback_token(&review).unwrap()),
    )
    .unwrap();
    fx.ctx
        .store
        .update(&blocked.id, |task| {
            task.status = Status::NeedsMe;
            task.current_run_mut().unwrap().blocked_excerpt =
                Some("Authentication failed; owner must refresh credentials".into());
            Ok(())
        })
        .unwrap();
    let independent = fx.task("independent scope", Status::Ready);
    fx.grant(&[&blocked.id, &independent.id]);
    let repo = fx.temp.path().join("app");
    std::fs::write(repo.join("main-moved.txt"), "independent main movement\n").unwrap();
    fixture::git(&repo, &["add", "-A"]);
    fixture::git(
        &repo,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "main moved"],
    );
    let before = fx.ctx.store.get(&blocked.id).unwrap();
    let submitted = fx.records("submissions.jsonl").len();
    // When repeated fresh syncs see stale integration data and free slots.
    for _ in 0..2 {
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
    }
    // Then no rebase, review, repair or restart crosses the real blocker.
    let retained = fx.ctx.store.get(&blocked.id).unwrap();
    assert_eq!(retained.status, Status::NeedsMe);
    assert_eq!(retained.rounds, 0);
    assert_eq!(retained.runs, before.runs);
    assert!(fx.records("deliveries.jsonl").is_empty());
    assert_eq!(fx.records("submissions.jsonl").len(), submitted + 1);
    assert_eq!(
        fx.ctx.store.get(&independent.id).unwrap().status,
        Status::Running
    );
}

#[test]
fn exact_reviewed_scopes_admit_published_children_without_expanding_the_grant() {
    // Given an independently reviewed plan whose exact labels are confirmed before publication.
    let mut fx = Fx::new();
    let parent = fx.task("bounded plan parent", Status::Ready);
    let source = fx.ctx.store.state_dir().join("scoped-plan.md");
    std::fs::write(&source, "---\ngoal: Ship bounded children\nnon_goals: []\ndecisions: []\naffected_paths: [first.txt, second.txt]\ncontracts: []\ndependencies:\n  - task: First\n    write_scope: [first.txt]\n    depends_on: []\n    acceptance: [{scenario: first, expected: works}]\n  - task: Second\n    write_scope: [second.txt]\n    depends_on: [First]\n    acceptance: [{scenario: second, expected: works}]\nacceptance: [{scenario: bounded, expected: accepted}]\nverification: [check]\nowner_decisions: []\n---\n").unwrap();
    let plan = corral_ops::plan_submit(&fx.ctx, &parent.id, &source).unwrap();
    let review = corral_ops::plan_review(&fx.ctx, &parent.id, Some("rev")).unwrap();
    corral_ops::plan_verdict(
        &fx.ctx,
        &parent.id,
        &corral_ops::PlanVerdict {
            plan: &plan.id,
            attempt: Some(&review.id),
            verdict: corral_core::Verdict::Approve,
            notes: "Independent scope review",
        },
    )
    .unwrap();
    let selection = corral_core::OperatingSelection {
        tasks: [parent.id.clone()].into(),
        plan_scopes: [(parent.id.clone(), ["First".into(), "Second".into()].into())].into(),
        authority: corral_core::OperatingAuthority {
            dispatch: true,
            accept: true,
            plan_accept: true,
        },
        ..Default::default()
    };
    let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", now())
        .unwrap();
    let original_config = std::fs::read(fx.ctx.store.config_path()).unwrap();
    // When explicit final plan acceptance publishes through the existing parent barrier.
    corral_ops::plan_verdict(
        &fx.ctx,
        &parent.id,
        &corral_ops::PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: corral_core::Verdict::Approve,
            notes: "Owner accepts the reviewed finite scopes",
        },
    )
    .unwrap();
    let parent = fx.ctx.store.get(&parent.id).unwrap();
    assert_eq!(parent.status, Status::Done);
    assert_eq!(parent.children.len(), 2);
    let unknown = fx.task("unconfirmed future child", Status::Ready);
    fx.ctx
        .store
        .update(&unknown.id, |task| {
            task.plan = Some(format!("{}@{}", parent.id, plan.id));
            Ok(())
        })
        .unwrap();
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();
    // Then only the exact first child starts; the parent and project names admit nothing else.
    assert_eq!(
        fx.ctx.store.get(&parent.children[0]).unwrap().status,
        Status::Running
    );
    assert!(
        fx.ctx
            .store
            .get(&parent.children[1])
            .unwrap()
            .runs
            .is_empty()
    );
    assert!(fx.ctx.store.get(&unknown.id).unwrap().runs.is_empty());
    assert!(
        !fx.ctx
            .store
            .effective_operating_policy()
            .unwrap()
            .allows(&unknown.id, corral_core::OperatingActionKind::Dispatch)
    );
    assert_eq!(
        std::fs::read(fx.ctx.store.config_path()).unwrap(),
        original_config
    );
}
