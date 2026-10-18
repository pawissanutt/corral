use super::PLAN;
use corral_core::{Status, Store, Verdict};
use corral_ops::{PlanVerdict, approved_plan, plan_submit, plan_verdict};

const ANSWERS: &str = "Resolved: Choose storage => SQLite\nResolved: Choose export => JSON";

fn manifest_fixture() -> (
    tempfile::TempDir,
    corral_ops::Ctx,
    String,
    std::path::PathBuf,
) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    store
        .edit_config(|doc| {
            doc.set_str("projects.app.path", ".")?;
            doc.set_str("review.plan_required", "true")?;
            doc.set_str("review.reviewer", "none")
        })
        .unwrap();
    let task = store
        .create(
            "Storage",
            Some("app"),
            Status::Ready,
            "Implement storage.\n",
        )
        .unwrap();
    let ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
    corral_ops::register_criteria(
        &ctx,
        &task.id,
        &corral_core::CriteriaManifest {
            version: 1,
            spec_digest: corral_ops::completion_spec_digest(&task).unwrap(),
            reviewed_by: "owner".into(),
            audited: true,
            require_gate: false,
            criteria: vec![corral_core::Criterion {
                id: "C1".into(),
                required: true,
                live: false,
                scenario: "widget --spin".into(),
                expected: "clockwise".into(),
            }],
            owner_decisions: vec!["Choose storage".into(), "Choose export".into()],
        },
    )
    .unwrap();
    let path = root.path().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    (root, ctx, task.id, path)
}

fn confirm_scope(store: &Store, id: &str) {
    let preview = store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: std::collections::BTreeSet::from([id.into()]),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                plan_accept: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    store
        .confirm_operating_policy(&preview, "owner", "2026-10-18T04:00:00Z".parse().unwrap())
        .unwrap();
}

fn authorize(store: &Store, id: &str, kind: corral_core::OperatingActionKind) -> bool {
    let config = store.config_layers().unwrap().config().unwrap();
    store
        .authorize_operating_action(
            corral_core::OperatingActionIdentity {
                task: id.into(),
                run: "R1".into(),
                attempt: format!("{kind:?}"),
                kind,
                candidate: None,
                plan: Some("P1".into()),
                round: None,
            },
            corral_core::OperatingOrigin::Automatic {
                authorization: config.automation.authorization.unwrap(),
                revision: config.automation.revision,
            },
        )
        .is_ok()
}

fn decomposed_fixture() -> (tempfile::TempDir, corral_ops::Ctx, String, String) {
    let (root, ctx, parent, path) = manifest_fixture();
    std::fs::write(
        &path,
        PLAN.replace(
            "dependencies: []",
            "dependencies:\n  - task: First\n    write_scope: [src/first.rs]\n    depends_on: []\n  - task: Second\n    write_scope: [src/second.rs]\n    depends_on: []",
        ),
    )
    .unwrap();
    let plan = plan_submit(&ctx, &parent, &path).unwrap();
    let review = plan_verdict(
        &ctx,
        &parent,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: ANSWERS,
        },
    )
    .unwrap();
    assert_eq!(review.verdict, Some(Verdict::Approve));
    let task = ctx.store.get(&parent).unwrap();
    assert_eq!(task.status, Status::Done);
    assert_eq!(task.children.len(), 2);
    let child = ctx.store.get(&task.children[0]).unwrap();
    assert!(child.plans.is_empty());
    assert_eq!(child.plan, Some(format!("{parent}@{}", plan.id)));
    assert_eq!(
        child.completion.as_ref().unwrap().owner_decisions,
        ["Choose storage", "Choose export"]
    );
    (root, ctx, parent, child.id)
}

#[test]
fn resolved_parent_answers_authorize_only_the_freshly_scoped_child_after_reload() {
    use corral_core::OperatingActionKind::Dispatch;
    // Given real owner acceptance and decomposition with inherited immutable manifest questions.
    let (root, ctx, parent, child) = decomposed_fixture();
    confirm_scope(&ctx.store, &child);
    // When the exact child grant and parent approval are read by a fresh consumer.
    let reopened = Store::open(root.path()).unwrap();
    let effective = reopened.effective_operating_policy().unwrap();
    // Then the durable answers permit the child, not unconfirmed siblings or the parent.
    assert!(
        effective.allows(&child, Dispatch),
        "{:?}",
        effective.tasks[&child].refusals
    );
    assert!(authorize(&reopened, &child, Dispatch));
    let parent = reopened.get(&parent).unwrap();
    assert!(!effective.allows(&parent.id, Dispatch));
    assert!(!effective.allows(&parent.children[1], Dispatch));
    assert!(reopened.get(&child).unwrap().plans.is_empty());
    assert_eq!(
        reopened
            .get(&child)
            .unwrap()
            .completion
            .unwrap()
            .owner_decisions,
        ["Choose storage", "Choose export"]
    );
}

#[test]
fn invalid_parent_answers_cannot_authorize_a_freshly_scoped_child() {
    use corral_core::OperatingActionKind::Dispatch;
    for invalid in [
        "missing-parent",
        "missing-backlink",
        "wrong-parent",
        "wrong-revision",
        "unlisted-child",
        "detached-parent",
        "incomplete-publication",
        "missing-approval",
        "nonowner-approval",
        "nonowner-review",
        "unanswered",
        "incomplete",
        "empty-answer",
        "stale-spec",
        "stale-manifest",
        "stale-policy",
        "changed-source",
        "changed-snapshot",
        "replaced-plan",
        "replaced-review",
        "presentation-only",
        "new-child-question",
        "pending-child-decision",
        "pending-parent-decision",
    ] {
        // Given real publication of children from an owner-answered parent plan.
        let (root, ctx, parent, child) = decomposed_fixture();
        ctx.store
            .update(&parent, |task| {
                let plan = &mut task.plans[0];
                match invalid {
                    "unlisted-child" => task.children.retain(|id| id != &child),
                    "detached-parent" => task.plan = None,
                    "incomplete-publication" => task.status = Status::Ready,
                    "missing-approval" => plan.approval = None,
                    "nonowner-approval" => plan.approval.as_mut().unwrap().by = "pm".into(),
                    "nonowner-review" => plan.reviews.last_mut().unwrap().reviewer = "pm".into(),
                    "unanswered" | "presentation-only" => {
                        plan.reviews.last_mut().unwrap().notes = None
                    }
                    "incomplete" => {
                        plan.reviews.last_mut().unwrap().notes =
                            Some("Resolved: Choose storage => SQLite".into())
                    }
                    "empty-answer" => {
                        plan.reviews.last_mut().unwrap().notes = Some(
                            "Resolved: Choose storage => SQLite\nResolved: Choose export => "
                                .into(),
                        )
                    }
                    "stale-spec" => task.body.push_str("Changed parent specification\n"),
                    "stale-manifest" => task
                        .completion
                        .as_mut()
                        .unwrap()
                        .owner_decisions
                        .push("Choose backup".into()),
                    "stale-policy" => plan.approval.as_mut().unwrap().policy_digest.clear(),
                    "changed-source" => std::fs::write(&plan.artifact.source, "Changed source")?,
                    "changed-snapshot" => std::fs::write(
                        ctx.store.dir().join(&plan.artifact.path),
                        "Changed snapshot",
                    )?,
                    "replaced-plan" => {
                        let mut replacement = plan.clone();
                        replacement.id = "P2".into();
                        task.plans.push(replacement);
                    }
                    "replaced-review" => {
                        let mut review = plan.reviews.last().unwrap().clone();
                        review.id = "PR2".into();
                        review.verdict = None;
                        plan.reviews.push(review);
                    }
                    "missing-parent"
                    | "missing-backlink"
                    | "wrong-parent"
                    | "wrong-revision"
                    | "new-child-question"
                    | "pending-child-decision"
                    | "pending-parent-decision" => {}
                    _ => unreachable!(),
                }
                Ok(())
            })
            .unwrap();
        let decision_task = if invalid == "pending-parent-decision" {
            &parent
        } else {
            &child
        };
        ctx.store
            .update(decision_task, |task| {
                match invalid {
                    "missing-backlink" => task.plan = None,
                    "wrong-parent" => task.plan = Some(format!("{child}@P1")),
                    "wrong-revision" => task.plan = Some(format!("{parent}@P99")),
                    "new-child-question" => task
                        .completion
                        .as_mut()
                        .unwrap()
                        .owner_decisions
                        .push("Choose backup".into()),
                    "pending-child-decision" | "pending-parent-decision" => {
                        task.runs.push(serde_json::from_value(serde_json::json!({
                            "id": "R1", "agent": "omo", "worktree": root.path(),
                            "branch": "fixture", "base": "main", "base_sha": "fixture",
                            "started": "2026-10-18T04:00:00Z",
                            "obligations": {"decisions": [{
                                "request": {
                                    "version": 1, "id": "choose-storage", "run": "R1",
                                    "question": "Choose storage", "requested_action": "choose",
                                    "owner": "owner", "source": "worker", "deadline": null
                                },
                                "registered_at": "2026-10-18T04:00:00Z",
                                "observation": {
                                    "at": "2026-10-18T04:00:00Z",
                                    "state": "pending", "detail": "Waiting"
                                },
                                "receipt": null
                            }]}
                        }))?)
                    }
                    _ => {}
                }
                Ok(())
            })
            .unwrap();
        if invalid == "missing-parent" {
            std::fs::remove_file(ctx.store.get(&parent).unwrap().path).unwrap();
        }
        if invalid == "presentation-only" {
            ctx.store
                .record_operating_presentation(
                    "parent-question",
                    corral_core::OperatingPresentation {
                        request_digest: "Choose storage".into(),
                        delivery: "presented".into(),
                        confirmed: true,
                        at: "2026-10-18T04:00:00Z".parse().unwrap(),
                    },
                )
                .unwrap();
        }
        // When a fresh grant is confirmed after invalidating exactly one evidence class.
        confirm_scope(&ctx.store, &child);
        let reopened = Store::open(root.path()).unwrap();
        let effective = reopened.effective_operating_policy().unwrap();
        // Then grant/presentation consent cannot replace valid, attached owner answers.
        assert!(!effective.allows(&child, Dispatch), "{invalid}");
        assert!(!authorize(&reopened, &child, Dispatch), "{invalid}");
        assert!(!reopened.state_dir().join("action-authorizations").exists());
    }
}

#[test]
fn manifest_owner_decisions_without_valid_answers_refuse_fresh_automatic_authority() {
    use corral_core::OperatingActionKind::{AcceptPlan, Dispatch};
    for invalid in [
        "no-plan",
        "unanswered",
        "incomplete",
        "empty-answer",
        "stale-spec",
        "stale-manifest",
        "stale-policy",
        "nonowner-review",
        "nonowner-approval",
        "detached",
        "replaced-review",
        "presentation-only",
    ] {
        // Given actual manifest registration and an empty-question imported plan, when present.
        let (root, ctx, id, path) = manifest_fixture();
        if invalid != "no-plan" {
            let plan = plan_submit(&ctx, &id, &path).unwrap();
            assert!(plan.document.owner_decisions.is_empty());
            let notes = match invalid {
                "unanswered" | "presentation-only" => "Owner looked at the question",
                "incomplete" => "Resolved: Choose storage => SQLite",
                "empty-answer" => "Resolved: Choose storage => SQLite\nResolved: Choose export => ",
                _ => ANSWERS,
            };
            let verdict = plan_verdict(
                &ctx,
                &id,
                &PlanVerdict {
                    plan: &plan.id,
                    attempt: None,
                    verdict: Verdict::Approve,
                    notes,
                },
            )
            .unwrap();
            let unanswered = matches!(
                invalid,
                "unanswered" | "incomplete" | "empty-answer" | "presentation-only"
            );
            assert_eq!(
                verdict.verdict,
                Some(if unanswered {
                    Verdict::Escalate
                } else {
                    Verdict::Approve
                })
            );
            ctx.store
                .update(&id, |task| {
                    let plan = &mut task.plans[0];
                    match invalid {
                        "stale-spec" => task.body.push_str("Changed specification\n"),
                        "stale-manifest" => task
                            .completion
                            .as_mut()
                            .unwrap()
                            .owner_decisions
                            .push("Choose backup".into()),
                        "stale-policy" => plan.approval.as_mut().unwrap().policy_digest.clear(),
                        "nonowner-review" => {
                            plan.reviews.last_mut().unwrap().reviewer = "pm".into()
                        }
                        "nonowner-approval" => plan.approval.as_mut().unwrap().by = "pm".into(),
                        "detached" => task.plan = None,
                        "replaced-review" => {
                            let mut review = plan.reviews.last().unwrap().clone();
                            review.id = "PR2".into();
                            review.verdict = None;
                            plan.reviews.push(review);
                        }
                        "unanswered" | "incomplete" | "empty-answer" | "presentation-only" => {}
                        _ => unreachable!(),
                    }
                    Ok(())
                })
                .unwrap();
            assert!(approved_plan(&ctx, &ctx.store.get(&id).unwrap()).is_err());
        }
        if invalid == "presentation-only" {
            ctx.store
                .record_operating_presentation(
                    "manifest-question",
                    corral_core::OperatingPresentation {
                        request_digest: "Choose storage".into(),
                        delivery: "presented".into(),
                        confirmed: true,
                        at: "2026-10-18T04:00:00Z".parse().unwrap(),
                    },
                )
                .unwrap();
        }
        // When the owner confirms a fresh scope and a new consumer resolves it from disk.
        confirm_scope(&ctx.store, &id);
        let reopened = Store::open(root.path()).unwrap();
        let effective = reopened.effective_operating_policy().unwrap();
        assert_eq!(effective.mode, corral_core::OperatingMode::Autopilot);
        // Then scope consent is not a design answer, and no automatic preparation is journaled.
        for action in [Dispatch, AcceptPlan] {
            assert!(!effective.allows(&id, action), "{invalid}: {action:?}");
            assert!(!authorize(&reopened, &id, action), "{invalid}: {action:?}");
        }
        assert!(!reopened.state_dir().join("action-authorizations").exists());
    }
}

#[test]
fn owner_resolved_manifest_decisions_allow_fresh_automatic_authority_after_reload() {
    use corral_core::OperatingActionKind::{AcceptPlan, Dispatch};
    // Given manifest-only questions resolved through the actual owner acceptance workflow.
    let (root, ctx, id, path) = manifest_fixture();
    let plan = plan_submit(&ctx, &id, &path).unwrap();
    let verdict = plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: ANSWERS,
        },
    )
    .unwrap();
    assert_eq!(verdict.verdict, Some(Verdict::Approve));
    assert!(
        approved_plan(&ctx, &ctx.store.get(&id).unwrap())
            .unwrap()
            .is_some()
    );
    confirm_scope(&ctx.store, &id);
    // When the confirmed scope and durable owner evidence are reloaded.
    let reopened = Store::open(root.path()).unwrap();
    let effective = reopened.effective_operating_policy().unwrap();
    // Then both policy and authorization journal admit the answered task without deleting questions.
    for action in [Dispatch, AcceptPlan] {
        assert!(
            effective.allows(&id, action),
            "{:?}",
            effective.tasks[&id].refusals
        );
        assert!(authorize(&reopened, &id, action));
    }
    let task = reopened.get(&id).unwrap();
    assert!(task.plans[0].document.owner_decisions.is_empty());
    assert_eq!(
        task.completion.unwrap().owner_decisions,
        ["Choose storage", "Choose export"]
    );
}

#[test]
fn owner_resolved_single_task_plan_allows_fresh_scoped_implementation() {
    use corral_core::{OperatingActionKind, OperatingAuthority, OperatingSelection};
    // Given the existing explicit owner-verdict flow, with no planner or agent submission.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    store
        .edit_config(|doc| {
            doc.set_str("projects.app.path", ".")?;
            doc.set_str("review.plan_required", "true")?;
            doc.set_str("review.reviewer", "none")
        })
        .unwrap();
    let task = store
        .create(
            "Storage",
            Some("app"),
            Status::Ready,
            "Implement storage.\n",
        )
        .unwrap();
    let ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
    let path = root.path().join("plan.md");
    std::fs::write(
        &path,
        PLAN.replace("owner_decisions: []", "owner_decisions: [Choose storage]"),
    )
    .unwrap();
    let plan = plan_submit(&ctx, &task.id, &path).unwrap();
    let review = plan_verdict(
        &ctx,
        &task.id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Resolved: Choose storage => SQLite",
        },
    )
    .unwrap();
    assert_eq!(review.verdict, Some(Verdict::Approve));
    let accepted = ctx.store.get(&task.id).unwrap();
    assert!(approved_plan(&ctx, &accepted).unwrap().is_some());
    let preview = ctx
        .store
        .preview_operating_policy(&OperatingSelection {
            tasks: std::collections::BTreeSet::from([task.id.clone()]),
            authority: OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    ctx.store
        .confirm_operating_policy(&preview, "owner", "2026-10-18T04:00:00Z".parse().unwrap())
        .unwrap();
    // When a fresh consumer resolves the explicitly confirmed task scope.
    let reopened = Store::open(root.path()).unwrap();
    let effective = reopened.effective_operating_policy().unwrap();
    // Then the durable owner answer permits dispatch without deleting its immutable question.
    assert!(
        effective.allows(&task.id, OperatingActionKind::Dispatch),
        "{:?}",
        effective.tasks[&task.id].refusals
    );
    assert_eq!(
        reopened.get(&task.id).unwrap().plans[0]
            .document
            .owner_decisions,
        ["Choose storage"]
    );
}
