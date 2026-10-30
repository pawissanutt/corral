use super::*;
use corral_core::{ApprovedScope, Origin, PlanDeliveryTarget};

fn journal(ctx: &Ctx, id: &str) -> Journal {
    serde_json::from_slice(
        &std::fs::read(
            ctx.store
                .state_dir()
                .join(format!("plan-decompose-{id}.json")),
        )
        .unwrap(),
    )
    .unwrap()
}

fn published_tracker(ctx: &Ctx, id: &str) -> Task {
    let staged = parse_task(journal(ctx, id).tracker.as_deref().unwrap(), Path::new("")).unwrap();
    ctx.store.get(&staged.id).unwrap()
}

#[test]
fn progress_maps_published_scope_criteria_without_a_completion_manifest() {
    // Given an independently reviewed design whose scopes cover only C1.
    let (_dir, mut ctx, id) = fixture(Vec::new());
    ctx.store.config.agents.insert(
        "reviewer".into(),
        corral_core::AgentCmd {
            command: "true".into(),
        },
    );
    ctx.store.config.review.reviewer = Some("reviewer".into());
    ctx.store.save_config().unwrap();
    let document = ctx.store.get(&id).unwrap().plans[0].document.clone();
    ctx.store
        .update(&id, |task| {
            task.plans.clear();
            Ok(())
        })
        .unwrap();
    let mut a = scope("api", &[]);
    a.acceptance = vec![acceptance("C1")];
    let mut b = scope("cli", &["api"]);
    b.acceptance = vec![acceptance("C1")];
    let revision = submit_reviewed_plan(&ctx, &id, &document, vec![a, b]);
    owner_accept(&ctx, &id, &revision).unwrap();
    let tracker = published_tracker(&ctx, &id);
    // When the shared projection reads the published tracker rather than a design history count.
    let progress = crate::plan_progress(&ctx, &tracker.id).unwrap();
    // Then both exact children map C1, C2 remains unmapped, and design publication is non-code.
    assert_eq!(progress.coverage[0].tasks, tracker.children);
    assert_eq!(progress.unmapped_criteria, [acceptance("C2")]);
    for work in &progress.work {
        let design = work
            .prerequisites
            .iter()
            .find(|dependency| dependency.task == id)
            .unwrap();
        assert!(design.satisfied);
        assert_eq!(
            design.design_publication.as_deref(),
            Some(format!("{id}@{revision}").as_str())
        );
        assert!(design.receipt_sha.is_none());
    }
}

#[test]
fn an_added_member_joins_the_published_design_and_its_tracker_together() {
    // Given a published multi-scope decomposition and an unstarted fix task.
    let (_dir, ctx, id) = fixture(vec![scope("api", &[]), scope("cli", &["api"])]);
    apply(&ctx, &id, "P1").unwrap();
    let tracker = published_tracker(&ctx, &id);
    let project = tracker.project.as_deref();
    let fix = ctx.store.create("Fix", project, Status::Backlog, "");
    let fix = fix.unwrap().id;
    // When the owner adds the fix to the Plan.
    crate::plan_add_member(&ctx, &tracker.id, &fix).unwrap();
    // Then the design and the tracker list the same members, ending with the fix.
    let design = ctx.store.get(&id).unwrap();
    let tracker = ctx.store.get(&tracker.id).unwrap();
    assert_eq!(design.children, tracker.children);
    assert_eq!(tracker.children.last(), Some(&fix));
    let member = ctx.store.get(&fix).unwrap();
    let plan = ctx.store.execution_plan(&member).unwrap().unwrap();
    assert_eq!(plan.id, tracker.id);
}

#[test]
fn approved_decomposition_publishes_one_plan_without_rewriting_design_history() {
    for target in [
        PlanDeliveryTarget::Main,
        PlanDeliveryTarget::Integration {
            branch: "feature/widget".into(),
        },
    ] {
        // Given an accepted multi-scope design and retained owner history.
        let (_dir, ctx, id) = fixture(vec![scope("api", &[]), scope("cli", &["api"])]);
        let before = ctx
            .store
            .update(&id, |task| {
                task.plans[0].document.delivery_target = Some(target.clone());
                Ok(())
            })
            .unwrap();
        // When publication is requested twice.
        apply(&ctx, &id, "P1").unwrap();
        apply(&ctx, &id, "P1").unwrap();
        // Then one tracker owns exactly the children and approved aggregate scope.
        let parent = ctx.store.get(&id).unwrap();
        let tracker = published_tracker(&ctx, &id);
        assert_eq!(parent.plans, before.plans);
        assert_eq!(parent.plan_runs, before.plan_runs);
        assert_eq!(parent.body, before.body);
        assert!(!parent.tracker);
        assert_eq!(tracker.children, parent.children);
        assert_eq!(tracker.parent.as_deref(), Some(id.as_str()));
        assert_eq!(tracker.status, Status::Backlog);
        assert_eq!(tracker.source_digest.as_deref(), Some("unused"));
        assert_eq!(
            tracker.approved_scope,
            Some(ApprovedScope {
                goal: "Ship feature".into(),
                acceptance: before.plans[0].document.acceptance.clone(),
                delivery_target: target,
                approved_design: Some(format!("{id}@P1")),
            })
        );
        assert!(
            tracker.completion.is_none() && tracker.runs.is_empty() && tracker.plans.is_empty()
        );
        for child in &tracker.children {
            let child = ctx.store.get(child).unwrap();
            assert_eq!(child.parent.as_ref(), Some(&tracker.id));
            assert_eq!(child.plan, parent.plan);
        }
        assert_eq!(
            ctx.store
                .list()
                .unwrap()
                .iter()
                .filter(|task| task.tracker)
                .count(),
            1
        );
        assert!(crate::tracker::check_action(&tracker, "dispatch").is_err());
    }
}

#[test]
fn tracker_publication_failure_resumes_from_same_receipt_and_ids() {
    // Given failure at each file boundary, including the tracker itself.
    for blocked_id in ["T-004", "T-002", "T-003"] {
        let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &["a"])]);
        let blocked = child_path(&ctx.store, blocked_id);
        std::fs::create_dir(&blocked).unwrap();
        assert!(apply(&ctx, &id, "P1").is_err());
        let receipt = journal(&ctx, &id);
        assert!(ctx.store.get(&id).unwrap().children.is_empty());
        for child in ctx
            .store
            .list()
            .unwrap()
            .iter()
            .filter(|task| task.plan.is_some())
        {
            assert!(
                crate::queue::dependency_check(&ctx.store, child)
                    .unwrap()
                    .is_some()
            );
        }
        std::fs::remove_dir(blocked).unwrap();
        // When the exact accepted publication is resumed.
        apply(&ctx, &id, "P1").unwrap();
        apply(&ctx, &id, "P1").unwrap();
        // Then the original IDs and approved scope survive without duplication.
        let after = journal(&ctx, &id);
        assert_eq!(after.tracker, receipt.tracker);
        assert_eq!(after.children, receipt.children);
        assert_eq!(ctx.store.list().unwrap().len(), 4);
        assert_eq!(published_tracker(&ctx, &id).children, ["T-002", "T-003"]);
    }
}

#[test]
fn collision_before_tracker_publication_remaps_children_without_stealing_ids() {
    // Given a journal but no published files, then another writer taking a child ID.
    let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &["a"])]);
    let blocked = child_path(&ctx.store, "T-004");
    std::fs::create_dir(&blocked).unwrap();
    assert!(apply(&ctx, &id, "P1").is_err());
    std::fs::remove_dir(blocked).unwrap();
    let other = ctx
        .store
        .create("Unrelated", None, Status::Backlog, "Keep")
        .unwrap();
    assert_eq!(other.id, "T-002");
    let other_bytes = std::fs::read(&other.path).unwrap();
    // When resumed through the existing collision-remapping receipt.
    apply(&ctx, &id, "P1").unwrap();
    apply(&ctx, &id, "P1").unwrap();
    // Then only the reserved unpublished child moves, with its edges and tracker updated.
    let tracker = published_tracker(&ctx, &id);
    assert_eq!(tracker.id, "T-004");
    assert_eq!(tracker.children, ["T-005", "T-003"]);
    assert_eq!(
        ctx.store.get("T-003").unwrap().depends_on,
        [id, "T-005".into()]
    );
    assert_eq!(std::fs::read(&other.path).unwrap(), other_bytes);
    assert_eq!(ctx.store.list().unwrap().len(), 5);
    for child in &tracker.children {
        assert_eq!(
            ctx.store.get(child).unwrap().parent,
            Some(tracker.id.clone())
        );
    }
}

#[test]
fn interrupted_remap_receipt_accepts_only_exact_earlier_tracker_contract() {
    // Given an ID remap interrupted before its first child write.
    for changed in [false, true] {
        let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &["a"])]);
        let tracker_path = child_path(&ctx.store, "T-004");
        std::fs::create_dir(&tracker_path).unwrap();
        assert!(apply(&ctx, &id, "P1").is_err());
        std::fs::remove_dir(&tracker_path).unwrap();
        ctx.store
            .create("First collision", None, Status::Backlog, "")
            .unwrap();
        let child_path = child_path(&ctx.store, "T-005");
        std::fs::create_dir(&child_path).unwrap();
        assert!(apply(&ctx, &id, "P1").is_err());
        std::fs::remove_dir(&child_path).unwrap();
        let other = ctx
            .store
            .create("Second collision", None, Status::Backlog, "")
            .unwrap();
        assert_eq!(other.id, "T-005");
        let old_tracker = std::fs::read(&tracker_path).unwrap();
        // When the next remap receipt is durable but the tracker rewrite hasn't happened.
        let mut receipt = journal(&ctx, &id);
        remap_collisions(&ctx.store, &ctx.store.list().unwrap(), &mut receipt).unwrap();
        for (scope, text) in receipt.document.dependencies.iter().zip(&receipt.children) {
            let child = parse_task(text, Path::new("")).unwrap();
            let binding = receipt.scopes.get_mut(&scope.task).unwrap();
            binding.task = child.id.clone();
            binding.contract_revision = operating_contract_revision(&child).unwrap();
        }
        write_atomic(
            &ctx.store
                .state_dir()
                .join(format!("plan-decompose-{id}.json")),
            &serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&tracker_path).unwrap(), old_tracker);
        if changed {
            // Simulate a raw out-of-band edit, not the now-protected store API.
            let mut task = ctx.store.get("T-004").unwrap();
            task.approved_scope
                .as_mut()
                .unwrap()
                .goal
                .push_str(" unapproved");
            std::fs::write(&tracker_path, render_task(&task)).unwrap();
        }
        // Then exact earlier files resume, but a later unapproved edit is refused.
        let resumed = apply(&ctx, &id, "P1");
        if changed {
            assert!(resumed.unwrap_err().to_string().contains("was modified"));
        } else {
            resumed.unwrap();
            let tracker = published_tracker(&ctx, &id);
            assert_eq!(tracker.id, "T-004");
            assert_eq!(tracker.children, ["T-006", "T-003"]);
            assert_eq!(ctx.store.list().unwrap().len(), 6);
            assert_eq!(
                ctx.store.get("T-003").unwrap().depends_on,
                [id, "T-006".into()]
            );
        }
    }
}

#[test]
fn imported_approved_plan_has_the_same_tracker_membership_shape() {
    // Given approved scope obtained from a published design.
    let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &["a"])]);
    apply(&ctx, &id, "P1").unwrap();
    let tracker = published_tracker(&ctx, &id);
    let mut approved_scope = tracker.approved_scope.clone().unwrap();
    // Imported owner-authored plans need not have a design-run source.
    approved_scope.approved_design = None;
    let path = ctx.store.root().join("import.md");
    let yaml = serde_yaml_ng::to_string(&approved_scope).unwrap();
    let yaml = yaml
        .lines()
        .map(|line| format!("  {line}\n"))
        .collect::<String>();
    std::fs::write(&path, format!("---\ntitle: Imported Plan\napproved_scope:\n{yaml}tasks:\n  - key: a\n    title: a\n  - key: b\n    title: b\n    depends: [a]\n---\nOwner design\n")).unwrap();
    // When imported through the normal operation.
    let result = crate::import_plan(&ctx, &path, false, Origin::Owner).unwrap();
    // Then its tracker and children expose the same approved-scope and membership fields.
    assert_eq!(result.tracker.approved_scope, Some(approved_scope));
    assert!(result.tracker.tracker && result.tracker.source_digest.is_some());
    assert_eq!(
        result.tracker.children,
        result
            .tasks
            .iter()
            .map(|task| task.id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(result.tracker.status, tracker.status);
    assert_eq!(result.tasks[1].depends_on, [result.tasks[0].id.clone()]);
    for child in result.tasks {
        assert_eq!(child.parent.as_ref(), Some(&result.tracker.id));
        assert!(!child.tracker);
    }
}

#[test]
fn imported_design_link_binds_scope_and_project_to_current_approval() {
    // Given a real snapshotted design with a valid current approval.
    let (_dir, ctx, id) = scoped_fixture(&["a", "b"]);
    apply(&ctx, &id, "P1").unwrap();
    let document = ctx.store.get(&id).unwrap().plans[0].document.clone();
    let approved = ApprovedScope {
        goal: document.goal,
        acceptance: document.acceptance,
        delivery_target: PlanDeliveryTarget::Main,
        approved_design: Some(format!("{id}@P1")),
    };
    let path = ctx.store.root().join("linked-import.md");
    let source = |scope: &ApprovedScope, project: &str| {
        let yaml = serde_yaml_ng::to_string(scope)
            .unwrap()
            .lines()
            .map(|line| format!("  {line}\n"))
            .collect::<String>();
        format!(
            "---\ntitle: Linked Plan\n{project}approved_scope:\n{yaml}tasks:\n  - key: work\n    title: Work\n---\n"
        )
    };
    for change in ["goal", "acceptance", "target", "project"] {
        let mut changed = approved.clone();
        match change {
            "goal" => changed.goal.push_str(" without approval"),
            "acceptance" => changed.acceptance[0].expected.push_str(" changed"),
            "target" => {
                changed.delivery_target = PlanDeliveryTarget::Integration {
                    branch: "feature/other".into(),
                }
            }
            "project" => {}
            _ => unreachable!(),
        }
        std::fs::write(
            &path,
            source(
                &changed,
                if change == "project" {
                    ""
                } else {
                    "project: app\n"
                },
            ),
        )
        .unwrap();
        let error = crate::import_plan(&ctx, &path, false, Origin::Owner).unwrap_err();
        assert!(
            format!("{error:#}").contains("differs from the linked design revision"),
            "{change}: {error:#}"
        );
        assert_eq!(ctx.store.list().unwrap().len(), 4);
    }
    std::fs::write(&path, source(&approved, "project: app\n")).unwrap();
    // When the exact scope and source revision are imported.
    let result = crate::import_plan(&ctx, &path, false, Origin::Owner).unwrap();
    // Then the immutable design link is retained as metadata, not a second design history.
    assert_eq!(result.tracker.approved_scope, Some(approved));
    assert!(result.tracker.plans.is_empty());
}

#[test]
fn all_done_children_leave_scoped_tracker_for_aggregate_acceptance_and_keep_history() {
    // Given published approved scope whose child tasks have all finished.
    let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &[])]);
    apply(&ctx, &id, "P1").unwrap();
    let before = published_tracker(&ctx, &id);
    let design = ctx.store.get(&id).unwrap();
    for child in &before.children {
        ctx.store
            .update(child, |task| {
                task.status = Status::Done;
                Ok(())
            })
            .unwrap();
    }
    // When the existing tracker reconciliation observes aggregate progress.
    assert!(
        crate::tracker::reconcile_one(&ctx, &before.id)
            .unwrap()
            .is_none()
    );
    // Then acceptance criteria hold it for aggregate acceptance, without rewriting
    // approved scope or design acceptance.
    let after = ctx.store.get(&before.id).unwrap();
    assert!(after.plan_acceptance_required());
    assert_eq!(after.status, before.status);
    assert_eq!(after.approved_scope, before.approved_scope);
    assert_eq!(ctx.store.get(&id).unwrap(), design);
    assert!(after.completion.is_none());
}

#[test]
fn stale_approved_design_link_is_refused_without_publication() {
    // Given a newer approved design than the import's reference.
    let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &[])]);
    ctx.store
        .update(&id, |task| {
            let mut replacement = task.plans[0].clone();
            replacement.id = "P2".into();
            task.plans.push(replacement);
            Ok(())
        })
        .unwrap();
    let path = ctx.store.root().join("stale-import.md");
    std::fs::write(&path, format!("---\ntitle: Stale\napproved_scope:\n  goal: Ship\n  acceptance: [{{scenario: Check, expected: Pass}}]\n  approved_design: {id}@P1\ntasks:\n  - key: work\n    title: Work\n---\n")).unwrap();
    let before = ctx.store.list().unwrap();
    // When an import or publication tries to use the replaced revision.
    let error = crate::import_plan(&ctx, &path, false, Origin::Owner).unwrap_err();
    let publication_error = apply(&ctx, &id, "P1").unwrap_err();
    // Then both refuse the stale identity explicitly, without filing tasks.
    assert!(format!("{error:#}").contains("stale approved design revision"));
    assert!(format!("{publication_error:#}").contains("stale plan revision"));
    assert_eq!(ctx.store.list().unwrap(), before);
}

#[test]
fn legacy_trackers_and_task_only_stores_do_not_gain_approved_scope() {
    // Given original task files with none of the new metadata.
    let (_dir, ctx, id) = fixture(Vec::new());
    let ordinary = ctx.store.get(&id).unwrap();
    let mut legacy = ordinary.clone();
    legacy.tracker = true;
    legacy.children = vec!["T-999".into()];
    for original in [ordinary, legacy] {
        let text = render_task(&original);
        // When old frontmatter is loaded and round-tripped.
        let loaded = parse_task(&text, Path::new("legacy.md")).unwrap();
        // Then only an effective default exists: no stored target, completion or scope is invented.
        assert!(loaded.approved_scope.is_none());
        assert!(loaded.completion.is_none());
        assert_eq!(loaded.status, original.status);
        assert_eq!(loaded.tracker, original.tracker);
        assert!(!render_task(&loaded).contains("approved_scope:"));
    }
}

#[test]
fn historical_unfinished_receipt_publishes_tracker_without_changing_child_ids() {
    // Given the exact T-144 receipt shape and an already-written first child.
    let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &["a"])]);
    let blocked = child_path(&ctx.store, "T-004");
    std::fs::create_dir(&blocked).unwrap();
    assert!(apply(&ctx, &id, "P1").is_err());
    std::fs::remove_dir(blocked).unwrap();
    let mut receipt = journal(&ctx, &id);
    receipt.tracker = None;
    receipt.previous_contracts.clear();
    for (scope, text) in receipt
        .document
        .dependencies
        .iter()
        .zip(&mut receipt.children)
    {
        let mut child = parse_task(text, Path::new("")).unwrap();
        child.parent = None;
        receipt
            .scopes
            .get_mut(&scope.task)
            .unwrap()
            .contract_revision = operating_contract_revision(&child).unwrap();
        *text = render_task(&child);
    }
    let value = serde_json::to_value(&receipt).unwrap();
    assert!(value.get("tracker").is_none() && value.get("previous_contracts").is_none());
    write_atomic(
        &ctx.store
            .state_dir()
            .join(format!("plan-decompose-{id}.json")),
        &serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    write_atomic(
        &child_path(&ctx.store, "T-002"),
        receipt.children[0].as_bytes(),
    )
    .unwrap();
    // When the old accepted publication resumes through the same apply operation.
    apply(&ctx, &id, "P1").unwrap();
    apply(&ctx, &id, "P1").unwrap();
    // Then its original children gain one tracker backlink without duplicate allocation.
    let tracker = published_tracker(&ctx, &id);
    assert_eq!(tracker.children, ["T-002", "T-003"]);
    assert_eq!(ctx.store.list().unwrap().len(), 4);
    for child in &tracker.children {
        assert_eq!(
            ctx.store.get(child).unwrap().parent,
            Some(tracker.id.clone())
        );
    }
}

#[test]
fn completed_historical_decomposition_is_not_reopened_or_backfilled() {
    // Given a completed historical decomposition with ordinary linked children.
    let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &["a"])]);
    apply(&ctx, &id, "P1").unwrap();
    let tracker = published_tracker(&ctx, &id);
    std::fs::remove_file(&tracker.path).unwrap();
    let mut receipt = journal(&ctx, &id);
    receipt.tracker = None;
    receipt.previous_contracts.clear();
    for (scope, text) in receipt
        .document
        .dependencies
        .iter()
        .zip(&mut receipt.children)
    {
        let child = parse_task(text, Path::new("")).unwrap();
        let saved = ctx
            .store
            .update(&child.id, |child| {
                child.parent = None;
                Ok(())
            })
            .unwrap();
        receipt
            .scopes
            .get_mut(&scope.task)
            .unwrap()
            .contract_revision = operating_contract_revision(&saved).unwrap();
        *text = render_task(&saved);
    }
    write_atomic(
        &ctx.store
            .state_dir()
            .join(format!("plan-decompose-{id}.json")),
        &serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    let parent = ctx.store.get(&id).unwrap();
    // When publication is requested again after its historical terminal barrier.
    apply(&ctx, &id, "P1").unwrap();
    // Then no new tracker, scope, or lifecycle fact is invented.
    let after = ctx.store.get(&id).unwrap();
    assert_eq!(after.status, Status::Done);
    assert_eq!(after.children, parent.children);
    assert_eq!(after.plans, parent.plans);
    assert_eq!(
        operating_contract_revision(&after).unwrap(),
        operating_contract_revision(&parent).unwrap()
    );
    assert_eq!(ctx.store.list().unwrap().len(), 3);
    assert!(
        ctx.store
            .list()
            .unwrap()
            .iter()
            .all(|task| !task.tracker && task.approved_scope.is_none())
    );
}
