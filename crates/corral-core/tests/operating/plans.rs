use super::*;
use corral_core::{
    ApprovedScope, OperatingActionIdentity, OperatingOrigin, PlanAcceptance, PlanDeliveryTarget,
    SubmissionOutcome, operating_contract_revision,
};
use std::cell::Cell;

#[path = "compatibility.rs"]
mod compatibility;

struct PlanFixture {
    fixture: Fixture,
    plan: Task,
    children: Vec<Task>,
}

impl PlanFixture {
    fn new() -> Self {
        let fixture = Fixture::new();
        let children = (0..2)
            .map(|index| {
                fixture
                    .store
                    .create(
                        &format!("Child {index}"),
                        Some("app"),
                        Status::Backlog,
                        "Work\n",
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut plan = fixture
            .store
            .update(&fixture.task.id, |task| {
                task.tracker = true;
                task.children = children.iter().map(|child| child.id.clone()).collect();
                Ok(())
            })
            .unwrap();
        // Model the initial imported file, not an amendment through generic editing.
        plan.approved_scope = Some(ApprovedScope {
            goal: "Deliver bounded work".into(),
            acceptance: vec![PlanAcceptance {
                criterion: Some("C1".into()),
                scenario: "Children complete".into(),
                expected: "Verified work".into(),
            }],
            delivery_target: PlanDeliveryTarget::Main,
            approved_design: None,
        });
        fs::write(&plan.path, corral_core::render_task(&plan)).unwrap();
        let children = children
            .into_iter()
            .map(|child| {
                fixture
                    .store
                    .update(&child.id, |child| {
                        child.parent = Some(plan.id.clone());
                        Ok(())
                    })
                    .unwrap()
            })
            .collect();
        Self {
            fixture,
            plan,
            children,
        }
    }

    fn store(&self) -> &Store {
        &self.fixture.store
    }

    fn grant(&self) {
        self.fixture.grant();
    }

    fn allows(&self, id: &str) -> bool {
        self.store()
            .effective_operating_policy()
            .unwrap()
            .allows(id, OperatingActionKind::Dispatch)
    }

    fn automatic(&self) -> corral_core::ActionAuthorization {
        let effective = self.store().effective_operating_policy().unwrap();
        self.store()
            .authorize_operating_action(
                OperatingActionIdentity {
                    task: self.children[0].id.clone(),
                    run: "R1".into(),
                    attempt: "launch-1".into(),
                    kind: OperatingActionKind::Dispatch,
                    candidate: None,
                    plan: None,
                    round: None,
                },
                OperatingOrigin::Automatic {
                    authorization: effective.authorization.unwrap(),
                    revision: self
                        .store()
                        .config_layers()
                        .unwrap()
                        .config()
                        .unwrap()
                        .automation
                        .revision,
                },
            )
            .unwrap()
    }
}

#[test]
fn selecting_plan_freezes_direct_children_and_not_future_descendants() {
    let fixture = PlanFixture::new();
    let preview = fixture
        .store()
        .preview_operating_policy(&fixture.fixture.selection())
        .unwrap();
    assert_eq!(preview.policy.scope.tasks.len(), 3);
    assert!(preview.policy.scope.plans.is_empty());
    fixture
        .store()
        .confirm_operating_policy(&preview, "owner", clock())
        .unwrap();
    assert!(
        !fixture.allows(&fixture.plan.id),
        "tracker is not an executor"
    );
    for child in &fixture.children {
        assert!(fixture.allows(&child.id));
    }
    let grandchild = fixture
        .store()
        .create("Later", Some("app"), Status::Backlog, "Later\n")
        .unwrap();
    fixture
        .store()
        .update(&grandchild.id, |task| {
            task.parent = Some(fixture.children[0].id.clone());
            Ok(())
        })
        .unwrap();
    assert!(!fixture.allows(&grandchild.id));
    assert!(fixture.allows(&fixture.children[0].id));
}

#[test]
fn explicit_child_exclusion_and_nonrecursive_tracker_selection_are_finite() {
    let fixture = PlanFixture::new();
    let selection = OperatingSelection {
        exclusions: BTreeSet::from([fixture.children[1].id.clone()]),
        ..fixture.fixture.selection()
    };
    let preview = fixture
        .store()
        .preview_operating_policy(&selection)
        .unwrap();
    assert_eq!(preview.policy.scope.tasks.len(), 2);
    fixture
        .store()
        .confirm_operating_policy(&preview, "owner", clock())
        .unwrap();
    assert!(fixture.allows(&fixture.children[0].id));
    assert!(!fixture.allows(&fixture.children[1].id));

    let grandchild = fixture
        .store()
        .create("Nested", Some("app"), Status::Backlog, "")
        .unwrap();
    let mut nested = fixture.store().get(&fixture.children[0].id).unwrap();
    nested.tracker = true;
    nested.approved_scope = fixture.plan.approved_scope.clone();
    nested.children = vec![grandchild.id.clone()];
    fs::write(&nested.path, corral_core::render_task(&nested)).unwrap();
    fixture
        .store()
        .update(&grandchild.id, |task| {
            task.parent = Some(nested.id.clone());
            Ok(())
        })
        .unwrap();
    let preview = fixture
        .store()
        .preview_operating_policy(&fixture.fixture.selection())
        .unwrap();
    assert!(!preview.policy.scope.tasks.contains_key(&grandchild.id));
}

#[test]
fn changed_plan_membership_requires_a_fresh_confirmation() {
    let fixture = PlanFixture::new();
    let preview = fixture
        .store()
        .preview_operating_policy(&fixture.fixture.selection())
        .unwrap();
    fixture
        .store()
        .update(&fixture.plan.id, |task| {
            task.children.pop();
            Ok(())
        })
        .unwrap();
    assert!(
        fixture
            .store()
            .confirm_operating_policy(&preview, "owner", clock())
            .is_err()
    );
    assert!(
        fixture
            .store()
            .operating_state()
            .unwrap()
            .approval
            .is_none()
    );
}

#[test]
fn plan_scope_target_membership_and_imported_parent_changes_deny_children() {
    for change in [
        "goal",
        "target",
        "membership",
        "parent",
        "child",
        "missing",
        "missing-plan",
        "tracker",
    ] {
        let fixture = PlanFixture::new();
        fixture.grant();
        match change {
            "goal" | "target" => {
                let mut scope = fixture.plan.approved_scope.clone().unwrap();
                if change == "goal" {
                    scope.goal.push_str(" and more");
                } else {
                    scope.delivery_target = PlanDeliveryTarget::Integration {
                        branch: "delivery".into(),
                    };
                }
                fixture
                    .store()
                    .amend_plan_scope(&fixture.plan.id, 0, scope, |_, _, _| Ok(()))
                    .unwrap();
            }
            "membership" | "tracker" => {
                fixture
                    .store()
                    .update(&fixture.plan.id, |task| {
                        if change == "membership" {
                            task.children.pop();
                        } else {
                            task.tracker = false;
                        }
                        Ok(())
                    })
                    .unwrap();
            }
            "parent" | "child" => {
                fixture
                    .store()
                    .update(&fixture.children[0].id, |task| {
                        if change == "parent" {
                            task.parent = None;
                        } else {
                            task.body.push_str("New scope\n");
                        }
                        Ok(())
                    })
                    .unwrap();
            }
            "missing" => fs::remove_file(&fixture.children[0].path).unwrap(),
            "missing-plan" => fs::remove_file(&fixture.plan.path).unwrap(),
            _ => unreachable!(),
        }
        for child in &fixture.children {
            assert!(!fixture.allows(&child.id), "{change}: {}", child.id);
        }
    }
}

#[test]
fn plan_progress_retains_consent_but_holds_and_exclusions_narrow_it() {
    for restriction in ["progress", "hold", "dropped", "needs-me", "excluded"] {
        let fixture = PlanFixture::new();
        if restriction == "excluded" {
            let selection = OperatingSelection {
                tasks: fixture
                    .children
                    .iter()
                    .map(|task| task.id.clone())
                    .collect(),
                exclusions: BTreeSet::from([fixture.plan.id.clone()]),
                ..fixture.fixture.selection()
            };
            let preview = fixture
                .store()
                .preview_operating_policy(&selection)
                .unwrap();
            fixture
                .store()
                .confirm_operating_policy(&preview, "owner", clock())
                .unwrap();
        } else {
            fixture.grant();
        }
        fixture
            .store()
            .update(&fixture.plan.id, |task| {
                match restriction {
                    "progress" => {
                        task.status = Status::Done;
                        task.priority = "P0".into();
                        task.rounds = 2;
                    }
                    "hold" => task.dispatch_hold = true,
                    "dropped" => task.status = Status::Dropped,
                    "needs-me" => task.status = Status::NeedsMe,
                    "excluded" => {}
                    _ => unreachable!(),
                }
                Ok(())
            })
            .unwrap();
        for child in &fixture.children {
            assert_eq!(
                fixture.allows(&child.id),
                restriction == "progress",
                "{restriction}"
            );
        }
    }
}

#[test]
fn invalid_plan_membership_cannot_be_previewed_or_resolved() {
    for change in [
        "missing",
        "parent",
        "project",
        "duplicate",
        "second-owner",
        "unlisted",
    ] {
        let fixture = PlanFixture::new();
        match change {
            "missing" => fs::remove_file(&fixture.children[0].path).unwrap(),
            "parent" | "project" => {
                fixture
                    .store()
                    .update(&fixture.children[0].id, |task| {
                        if change == "parent" {
                            task.parent = None;
                        } else {
                            task.project = None;
                        }
                        Ok(())
                    })
                    .unwrap();
            }
            "duplicate" | "unlisted" => {
                fixture
                    .store()
                    .update(&fixture.plan.id, |task| {
                        if change == "duplicate" {
                            task.children.push(task.children[0].clone());
                        } else {
                            task.children.pop();
                        }
                        Ok(())
                    })
                    .unwrap();
            }
            "second-owner" => {
                let other = fixture
                    .store()
                    .create("Other Plan", Some("app"), Status::Backlog, "")
                    .unwrap();
                fixture
                    .store()
                    .update(&other.id, |task| {
                        task.tracker = true;
                        task.children = fixture.plan.children.clone();
                        Ok(())
                    })
                    .unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            fixture
                .store()
                .preview_operating_policy(&fixture.fixture.selection())
                .is_err(),
            "{change}"
        );
        assert!(
            fixture
                .store()
                .execution_plan(&fixture.children[0])
                .is_err(),
            "{change}"
        );
    }
}

#[test]
fn execution_plan_distinguishes_imported_members_from_legacy_parents() {
    let fixture = PlanFixture::new();
    assert_eq!(
        fixture
            .store()
            .execution_plan(&fixture.children[0])
            .unwrap()
            .unwrap()
            .id,
        fixture.plan.id
    );
    assert_eq!(
        fixture
            .store()
            .execution_plan(&fixture.plan)
            .unwrap()
            .unwrap()
            .id,
        fixture.plan.id
    );
    let mut changed = fixture.children[0].clone();
    changed.parent = None;
    assert_ne!(
        operating_contract_revision(&changed).unwrap(),
        operating_contract_revision(&fixture.children[0]).unwrap()
    );
    fixture
        .store()
        .update(&fixture.plan.id, |task| {
            task.tracker = false;
            Ok(())
        })
        .unwrap();
    assert!(
        fixture
            .store()
            .execution_plan(&fixture.children[0])
            .unwrap()
            .is_none()
    );
}

#[test]
fn pause_or_membership_change_before_submission_prevents_ipc_and_replay_is_single() {
    for change in ["pause", "hold", "membership", "none"] {
        let fixture = PlanFixture::new();
        fixture.grant();
        let authorization = fixture.automatic();
        match change {
            "pause" => {
                fixture.store().pause_operating_policy(true).unwrap();
            }
            "hold" | "membership" => {
                fixture
                    .store()
                    .update(&fixture.plan.id, |task| {
                        if change == "hold" {
                            task.dispatch_hold = true;
                        } else {
                            task.children.pop();
                        }
                        Ok(())
                    })
                    .unwrap();
            }
            "none" => {}
            _ => unreachable!(),
        }
        let prepared = Cell::new(0);
        let submitted = Cell::new(0);
        let submit = || {
            fixture.store().submit_operating_action(
                &authorization,
                &authorization.action,
                |_, task| {
                    prepared.set(prepared.get() + 1);
                    task.queued = true;
                    Ok(())
                },
                || {
                    submitted.set(submitted.get() + 1);
                    Ok(())
                },
            )
        };
        let outcome = submit().unwrap();
        if change == "none" {
            assert!(matches!(outcome, SubmissionOutcome::Submitted(())));
            assert!(submit().is_err());
            assert_eq!((prepared.get(), submitted.get()), (1, 1));
        } else {
            assert!(matches!(
                outcome,
                SubmissionOutcome::AuthorizationRefused(_)
            ));
            assert_eq!((prepared.get(), submitted.get()), (0, 0));
            assert!(!fixture.store().get(&fixture.children[0].id).unwrap().queued);
        }
    }
}
