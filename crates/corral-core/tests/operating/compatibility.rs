use super::*;

#[test]
fn child_only_grant_freezes_owning_plan_without_sibling_or_recursive_authority() {
    for change in ["goal", "criteria", "target", "removed-scope", "progress"] {
        let fixture = PlanFixture::new();
        let child = &fixture.children[0];
        let selection = OperatingSelection {
            tasks: BTreeSet::from([child.id.clone()]),
            ..fixture.fixture.selection()
        };
        let preview = fixture
            .store()
            .preview_operating_policy(&selection)
            .unwrap();
        assert_eq!(
            preview
                .policy
                .scope
                .tasks
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([child.id.clone(), fixture.plan.id.clone()])
        );
        fixture
            .store()
            .confirm_operating_policy(&preview, "owner", clock())
            .unwrap();
        assert!(fixture.allows(&child.id));
        assert!(!fixture.allows(&fixture.children[1].id));
        let descendant = fixture
            .store()
            .create("Descendant", Some("app"), Status::Backlog, "")
            .unwrap();
        fixture
            .store()
            .update(&descendant.id, |task| {
                task.parent = Some(child.id.clone());
                Ok(())
            })
            .unwrap();
        assert!(!fixture.allows(&descendant.id));
        if change == "progress" {
            fixture
                .store()
                .update(&fixture.plan.id, |task| {
                    task.status = Status::Done;
                    task.rounds = 1;
                    Ok(())
                })
                .unwrap();
        } else if change == "removed-scope" {
            let mut plan = fixture.plan.clone();
            plan.approved_scope = None;
            fs::write(&plan.path, corral_core::render_task(&plan)).unwrap();
        } else {
            let mut scope = fixture.plan.approved_scope.clone().unwrap();
            match change {
                "goal" => scope.goal.push_str(" changed"),
                "criteria" => scope.acceptance[0].expected.push_str(" changed"),
                "target" => {
                    scope.delivery_target = PlanDeliveryTarget::Integration {
                        branch: "delivery".into(),
                    }
                }
                _ => unreachable!(),
            }
            fixture
                .store()
                .amend_plan_scope(&fixture.plan.id, 0, scope, |_, _, _| Ok(()))
                .unwrap();
        }
        assert_eq!(fixture.allows(&child.id), change == "progress", "{change}");
        assert!(!fixture.allows(&fixture.children[1].id));
    }
}

#[test]
fn child_only_legacy_selection_has_no_execution_plan_anchor() {
    let fixture = PlanFixture::new();
    let mut legacy = fixture.plan.clone();
    legacy.approved_scope = None;
    fs::write(&legacy.path, corral_core::render_task(&legacy)).unwrap();
    let selection = OperatingSelection {
        tasks: BTreeSet::from([fixture.children[0].id.clone()]),
        ..fixture.fixture.selection()
    };
    let preview = fixture
        .store()
        .preview_operating_policy(&selection)
        .unwrap();
    assert_eq!(preview.policy.scope.tasks.len(), 1);
    fixture
        .store()
        .confirm_operating_policy(&preview, "owner", clock())
        .unwrap();
    assert!(fixture.allows(&fixture.children[0].id));
    assert!(!fixture.allows(&fixture.children[1].id));
}

#[test]
fn legacy_tracking_parent_is_not_an_execution_plan_but_keeps_reciprocity_checks() {
    let fixture = PlanFixture::new();
    let mut legacy = fixture.plan.clone();
    legacy.approved_scope = None;
    fs::write(&legacy.path, corral_core::render_task(&legacy)).unwrap();
    assert!(fixture.store().execution_plan(&legacy).unwrap().is_none());
    assert!(
        fixture
            .store()
            .execution_plan(&fixture.children[0])
            .unwrap()
            .is_none()
    );
    fixture
        .store()
        .update(&legacy.id, |task| {
            task.children.remove(0);
            Ok(())
        })
        .unwrap();
    assert!(
        fixture
            .store()
            .execution_plan(&fixture.children[0])
            .is_err()
    );
}

#[test]
fn project_preview_skips_legacy_tracker_expansion_and_preserves_ordinary_work() {
    let fixture = PlanFixture::new();
    let mut legacy = fixture.plan.clone();
    legacy.approved_scope = None;
    fs::write(&legacy.path, corral_core::render_task(&legacy)).unwrap();
    fixture
        .store()
        .update(&fixture.children[1].id, |task| {
            task.status = Status::Idea;
            Ok(())
        })
        .unwrap();
    let unrelated = fixture
        .store()
        .create("Ordinary work", Some("app"), Status::Backlog, "")
        .unwrap();
    let selection = OperatingSelection {
        projects: BTreeSet::from(["app".into()]),
        tasks: BTreeSet::new(),
        ..fixture.fixture.selection()
    };
    let preview = fixture
        .store()
        .preview_operating_policy(&selection)
        .unwrap();
    assert!(preview.policy.scope.tasks.contains_key(&legacy.id));
    assert!(
        !preview
            .policy
            .scope
            .tasks
            .contains_key(&fixture.children[1].id)
    );
    fixture
        .store()
        .confirm_operating_policy(&preview, "owner", clock())
        .unwrap();
    assert!(fixture.allows(&fixture.children[0].id));
    assert!(fixture.allows(&unrelated.id));
    assert!(!fixture.allows(&legacy.id));
}
