use super::*;
use corral_core::{
    ApprovedScope, PlanApproval, PlanScope, operating_contract_revision, render_task,
};
use sha2::{Digest, Sha256};

fn hash(value: &impl serde::Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_string(value).unwrap().as_bytes())
    )
}

fn published(with_tracker: bool, partial: bool) -> (Fixture, Vec<Task>, Option<Task>) {
    let fixture = Fixture::new();
    let store = &fixture.store;
    let source = store.dir().join("approved-plan.md");
    let parent = store
        .update(&fixture.task.id, |task| {
            let mut plan = reviewed_plan(task);
            plan.run = None;
            plan.document.dependencies.push(PlanScope {
                task: "other-child".into(),
                write_scope: vec!["src/other.rs".into()],
                depends_on: Vec::new(),
                acceptance: Vec::new(),
                gate: None,
            });
            let bytes = format!(
                "---\n{}---\n",
                serde_yaml_ng::to_string(&plan.document).unwrap()
            );
            fs::write(&source, &bytes).unwrap();
            plan.artifact.sha256 = format!("{:x}", Sha256::digest(bytes.as_bytes()));
            plan.artifact.path = "approved-plan.md".into();
            plan.artifact.source = source.to_str().unwrap().into();
            let policy = corral_core::plan_policy::policy_digest(&store.config, task).unwrap();
            plan.reviews[0].policy_digest = policy.clone();
            plan.reviews[0].reviewer = "owner".into();
            plan.approval = Some(PlanApproval {
                by: "owner".into(),
                at: clock(),
                policy_digest: policy,
                authorization: None,
            });
            task.plans.push(plan);
            Ok(())
        })
        .unwrap();
    let plan = parent.plans.last().unwrap();
    let selection = OperatingSelection {
        plan_scopes: BTreeMap::from([(
            parent.id.clone(),
            plan.document
                .dependencies
                .iter()
                .take(if partial { 1 } else { 2 })
                .map(|scope| scope.task.clone())
                .collect(),
        )]),
        ..fixture.selection()
    };
    let preview = store.preview_operating_policy(&selection).unwrap();
    store
        .confirm_operating_policy(&preview, "owner", clock())
        .unwrap();
    let backlink = format!("{}@{}", parent.id, plan.id);
    let tracker = with_tracker.then(|| {
        store
            .create("Execution Plan", Some("app"), Status::Backlog, "")
            .unwrap()
    });
    let children = plan
        .document
        .dependencies
        .iter()
        .map(|scope| {
            let child = store
                .create(&scope.task, Some("app"), Status::Backlog, "Bounded work\n")
                .unwrap();
            store
                .update(&child.id, |task| {
                    task.parent = Some(tracker.as_ref().unwrap_or(&parent).id.clone());
                    task.plan = Some(backlink.clone());
                    task.write_scope = scope.write_scope.clone();
                    Ok(())
                })
                .unwrap()
        })
        .collect::<Vec<_>>();
    let tracker = tracker.map(|tracker| {
        let mut tracker = store
            .update(&tracker.id, |task| {
                task.tracker = true;
                task.parent = Some(parent.id.clone());
                task.children = children.iter().map(|task| task.id.clone()).collect();
                task.source_digest = Some(plan.artifact.sha256.clone());
                Ok(())
            })
            .unwrap();
        tracker.approved_scope = Some(ApprovedScope {
            goal: plan.document.goal.clone(),
            acceptance: plan.document.acceptance.clone(),
            delivery_target: plan.document.delivery_target.clone().unwrap_or_default(),
            approved_design: Some(backlink.clone()),
        });
        fs::write(&tracker.path, render_task(&tracker)).unwrap();
        tracker
    });
    let projected = store
        .update(&parent.id, |task| {
            task.status = Status::Done;
            task.plan = Some(backlink.clone());
            task.children = children.iter().map(|task| task.id.clone()).collect();
            Ok(())
        })
        .unwrap();
    let scopes = plan
        .document
        .dependencies
        .iter()
        .zip(&children)
        .map(|(scope, child)| {
            (
                scope.task.clone(),
                serde_json::json!({
                    "task": child.id,
                    "scope_digest": hash(scope),
                    "contract_revision": operating_contract_revision(child).unwrap(),
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let journal = serde_json::json!({
        "backlink": backlink,
        "document": plan.document,
        "source_contract": operating_contract_revision(&parent).unwrap(),
        "projected_contract": operating_contract_revision(&projected).unwrap(),
        "artifact_sha256": plan.artifact.sha256,
        "scopes": scopes,
        "tracker": tracker.as_ref().map(render_task),
    });
    fs::create_dir_all(store.state_dir()).unwrap();
    fs::write(
        store
            .state_dir()
            .join(format!("plan-decompose-{}.json", parent.id)),
        serde_json::to_string(&journal).unwrap(),
    )
    .unwrap();
    (fixture, children, tracker)
}

#[test]
fn scoped_publication_keeps_legacy_and_tracker_children_working_with_partial_grants() {
    for with_tracker in [false, true] {
        for partial in [false, true] {
            let (fixture, children, _) = published(with_tracker, partial);
            let effective = fixture.store.effective_operating_policy().unwrap();
            assert!(
                effective.allows(&children[0].id, OperatingActionKind::Dispatch),
                "{:?}",
                effective.reasons
            );
            assert_eq!(
                effective.allows(&children[1].id, OperatingActionKind::Dispatch),
                !partial
            );
            assert!(
                fixture
                    .store
                    .published_operating_member(&children[0])
                    .unwrap()
                    .is_some()
            );
        }
    }
}

#[test]
fn published_member_repair_submission_survives_reviewer_policy_change() {
    use corral_core::{OperatingActionIdentity, OperatingOrigin, SubmissionOutcome};
    use std::cell::Cell;

    for with_tracker in [false, true] {
        let (mut fixture, children, _) = published(with_tracker, false);
        fixture.store = Store::open(fixture.root.path()).unwrap();
        fixture.store.config.agents.insert(
            "reviewer".into(),
            corral_core::AgentCmd {
                command: "true".into(),
            },
        );
        fixture.store.config.review.reviewer = Some("reviewer".into());
        fixture.store.save_config().unwrap();
        let effective = fixture.store.effective_operating_policy().unwrap();
        let authorization = fixture
            .store
            .authorize_operating_action(
                OperatingActionIdentity {
                    task: children[0].id.clone(),
                    run: "R1".into(),
                    attempt: "repair-1".into(),
                    kind: OperatingActionKind::Repair,
                    candidate: Some(corral_core::CandidateIdentity {
                        version: 2,
                        task: children[0].id.clone(),
                        run: "R1".into(),
                        attempt: "gate-1".into(),
                        head: "head".into(),
                        tree: "tree".into(),
                        integration_base: "base".into(),
                        dispatch_base: "base".into(),
                        spec_digest: "spec".into(),
                        policy_digest: "policy".into(),
                        inputs_digest: "inputs".into(),
                        environment: vec![],
                        gate_tier: None,
                    }),
                    plan: None,
                    round: None,
                },
                OperatingOrigin::Automatic {
                    authorization: effective.authorization.unwrap(),
                    revision: fixture
                        .store
                        .config_layers()
                        .unwrap()
                        .config()
                        .unwrap()
                        .automation
                        .revision,
                },
            )
            .unwrap();
        let submitted = Cell::new(false);
        let outcome = fixture
            .store
            .submit_operating_action(
                &authorization,
                &authorization.action,
                |_, task| {
                    task.queued = true;
                    Ok(())
                },
                || {
                    submitted.set(true);
                    Ok(())
                },
            )
            .unwrap();
        assert!(
            matches!(outcome, SubmissionOutcome::Submitted(())),
            "{outcome:?}"
        );
        assert!(submitted.get());
        assert!(fixture.store.get(&children[0].id).unwrap().queued);
    }
}

#[test]
fn published_tracker_scope_identity_hold_and_lifecycle_guard_derived_authority() {
    for change in [
        "scope", "target", "children", "hold", "dropped", "progress", "excluded",
    ] {
        let (fixture, children, tracker) = published(true, false);
        let tracker = tracker.unwrap();
        assert!(
            fixture
                .store
                .effective_operating_policy()
                .unwrap()
                .allows(&children[0].id, OperatingActionKind::Dispatch)
        );
        if change == "scope" || change == "target" {
            let mut scope = tracker.approved_scope.clone().unwrap();
            // Removing the design link permits an explicit amendment; the old
            // publication must still not confer authority on the amended Plan.
            scope.approved_design = None;
            if change == "scope" {
                scope.goal.push_str(" changed");
            } else {
                scope.delivery_target = corral_core::PlanDeliveryTarget::Integration {
                    branch: "delivery".into(),
                };
            }
            fixture
                .store
                .amend_plan_scope(&tracker.id, 0, scope, |_, _, _| Ok(()))
                .unwrap();
        } else if change == "excluded" {
            let selection = OperatingSelection {
                exclusions: BTreeSet::from([tracker.id.clone()]),
                plan_scopes: BTreeMap::from([(
                    fixture.task.id.clone(),
                    BTreeSet::from(["named-child".into(), "other-child".into()]),
                )]),
                ..fixture.selection()
            };
            let preview = fixture.store.preview_operating_policy(&selection).unwrap();
            fixture
                .store
                .confirm_operating_policy(&preview, "owner", clock())
                .unwrap();
        } else {
            fixture
                .store
                .update(&tracker.id, |task| {
                    match change {
                        "children" => {
                            task.children.pop();
                        }
                        "hold" => task.dispatch_hold = true,
                        "dropped" => task.status = Status::Dropped,
                        "progress" => {
                            task.status = Status::Done;
                            task.rounds = 3;
                        }
                        _ => unreachable!(),
                    }
                    Ok(())
                })
                .unwrap();
        }
        let effective = fixture.store.effective_operating_policy().unwrap();
        for child in &children {
            assert_eq!(
                effective.allows(&child.id, OperatingActionKind::Dispatch),
                change == "progress",
                "{change}: {:?}",
                effective.reasons
            );
        }
    }
}
