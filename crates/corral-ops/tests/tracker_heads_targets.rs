use corral_core::work::{self, Counter, HotScope};
use corral_core::{
    ApprovedScope, OperatingAuthority, OperatingSelection, PlanDeliveryTarget, Status, Store,
};

mod support;

#[test]
fn authorized_plan_member_target_checks_never_hydrate_its_parent() {
    let mut reports = Vec::new();
    for factor in [1, 10] {
        // Given an authorized Ready member of an approved, unclaimed integration Plan.
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        let output = support::command("git", root.path())
            .args(["init", "-q", "-b", "main"])
            .arg(&repo)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let mut store = Store::init(root.path()).unwrap();
        store.config.dispatch.paused = false;
        store.save_config().unwrap();
        let mut plan = store
            .create("Plan", Some("app"), Status::Backlog, "")
            .unwrap();
        let mut member = store
            .create("Member", Some("app"), Status::Ready, "")
            .unwrap();
        plan.created = "2026-10-27T00:00:00Z".parse().unwrap();
        plan.updated = plan.created;
        member.created = plan.created;
        member.updated = plan.updated;
        member.parent = Some(plan.id.clone());
        member.plan_required = Some(false);
        plan.tracker = true;
        plan.children = vec![member.id.clone()];
        plan.approved_scope = Some(ApprovedScope {
            goal: "Deliver member".into(),
            acceptance: vec![],
            approved_design: None,
            delivery_target: PlanDeliveryTarget::Integration {
                branch: "feature/plan".into(),
            },
        });
        store.fixture_put_task(&member).unwrap();
        store.fixture_put_task(&plan).unwrap();
        for _ in 0..factor * 10 {
            store.create("Old", Some("app"), Status::Done, "").unwrap();
            store
                .log_event(Some(&member.id), "history", serde_json::json!({}))
                .unwrap();
        }
        let preview = store
            .preview_operating_policy(&OperatingSelection {
                tasks: [plan.id.clone()].into_iter().collect(),
                authority: OperatingAuthority {
                    dispatch: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        store.config = store
            .confirm_operating_policy(
                &preview,
                "fixture owner",
                "2026-10-27T00:00:00Z".parse().unwrap(),
            )
            .unwrap();
        let ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
        let before = work::snapshot();

        // When progress takes normal operating authorization and target-readiness paths.
        let progress = {
            let _hot = HotScope::enter();
            corral_ops::plan_progress(&ctx, &plan.id).unwrap()
        };

        // Then the full-task algorithm's readiness result needs no parent hydration.
        assert_eq!(progress.counts.waiting, 1, "{progress:?}");
        assert_eq!(
            progress.work[0].reason.as_deref(),
            Some("ready for authorized dispatch")
        );
        assert_eq!(
            serde_json::to_value(&progress).unwrap(),
            serde_json::json!({
                "task": plan.id, "status": plan.status, "approved_scope": plan.approved_scope,
                "scope_revision": plan.approved_scope_revision,
                "counts": {"accepted": 0, "running": 0, "review": 0, "waiting": 1, "missing": 0},
                "work": [{
                    "task": member.id, "title": member.title, "status": member.status,
                "state": "waiting", "run": null, "reason": "ready for authorized dispatch",
                    "prerequisites": []
                }],
                "coverage": [], "unmapped_criteria": []
            })
        );
        let cells = work::snapshot().since(&before);
        assert_eq!(cells.unattributed_hydrations(), 0);
        assert_eq!(cells.get(Counter::ViewsOpened), 1);
        cells.check_identity().unwrap();
        reports.push(cells);
    }
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}
