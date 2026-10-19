use super::*;
use corral_core::{ApprovedScope, PlanAcceptance, PlanDeliveryTarget};

fn approved_scope() -> ApprovedScope {
    ApprovedScope {
        goal: "Approved goal".into(),
        acceptance: vec![PlanAcceptance {
            criterion: None,
            scenario: "Run checks".into(),
            expected: "Pass".into(),
        }],
        delivery_target: PlanDeliveryTarget::Main,
        approved_design: None,
    }
}

fn design(ctx: &Ctx, approve: bool) -> String {
    let task = ctx
        .store
        .create("Design", Some("app"), Status::Backlog, "")
        .unwrap();
    let scope = approved_scope();
    let document = PlanDocument {
        delivery_target: None,
        goal: scope.goal,
        acceptance: scope.acceptance,
        non_goals: Vec::new(),
        decisions: Vec::new(),
        affected_paths: vec!["widget.rs".into()],
        contracts: Vec::new(),
        dependencies: Vec::new(),
        verification: vec!["cargo test".into()],
        owner_decisions: Vec::new(),
    };
    let path = ctx.store.root().join(format!("design-{}.md", task.id));
    fs::write(
        &path,
        format!("---\n{}---\n", serde_yaml_ng::to_string(&document).unwrap()),
    )
    .unwrap();
    let plan = crate::plan_submit(ctx, &task.id, &path).unwrap();
    if approve {
        crate::plan_verdict(
            ctx,
            &task.id,
            &crate::PlanVerdict {
                plan: &plan.id,
                attempt: None,
                verdict: corral_core::Verdict::Approve,
                notes: "Owner accepts fixture scope",
            },
        )
        .unwrap();
    }
    format!("{}@{}", task.id, plan.id)
}

fn plan_fixture() -> (tempfile::TempDir, Ctx, Task, PathBuf) {
    let (dir, ctx, path) = fixture();
    let mut scope = approved_scope();
    scope.approved_design = Some(design(&ctx, true));
    fs::write(&path, format!("---\ntitle: Plan\nproject: app\napproved_scope: {}\ntasks:\n  - {{key: a, title: A}}\n  - {{key: b, title: B, depends: [a]}}\n---\nOriginal tracker body\n",
        serde_json::to_string(&scope).unwrap())).unwrap();
    let tracker = import_plan(&ctx, &path, false, Origin::Owner)
        .unwrap()
        .tracker;
    let tracker = ctx.store.get(&tracker.id).unwrap();
    (dir, ctx, tracker, path)
}

fn amendment(path: &Path, scope: &ApprovedScope) {
    fs::write(
        path,
        format!(
            "---\napproved_scope: {}\n---\n",
            serde_json::to_string(scope).unwrap()
        ),
    )
    .unwrap();
}

fn saved_files(ctx: &Ctx) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files: BTreeMap<_, _> = ctx
        .store
        .list()
        .unwrap()
        .into_iter()
        .map(|task| (task.path.clone(), fs::read(task.path).unwrap()))
        .collect();
    let events = ctx.store.dir().join("events.jsonl");
    files.insert(events.clone(), fs::read(events).unwrap());
    files
}

#[test]
fn current_revision_amendment_preserves_identity_members_and_previous_approval() {
    // Given an existing Plan linked to a real owner-approved design.
    let (_dir, ctx, before, path) = plan_fixture();
    let children = before
        .children
        .iter()
        .map(|id| ctx.store.get(id).unwrap())
        .collect::<Vec<_>>();
    let mut scope = approved_scope();
    scope.goal = "Owner amended goal".into();
    scope.delivery_target = PlanDeliveryTarget::Integration {
        branch: "feature/cohesive".into(),
    };
    amendment(&path, &scope);
    // When the current revision is explicitly amended.
    let after = amend_plan_scope(&ctx, &before.id, &path, 0).unwrap();
    // Then scope advances once, while original approval and all task membership remain.
    assert_eq!(after.id, before.id);
    assert_eq!(after.path, before.path);
    assert_eq!(after.parent, before.parent);
    assert_eq!(after.children, before.children);
    assert_eq!(after.source_digest, before.source_digest);
    assert_eq!(after.body, before.body);
    assert_eq!(after.plans, before.plans);
    assert_eq!(after.status, before.status);
    assert_eq!(after.approved_scope, Some(scope));
    assert_eq!(after.approved_scope_revision, 1);
    assert_eq!(after.approved_scope_history.len(), 1);
    assert_eq!(after.approved_scope_history[0].revision, 0);
    assert_eq!(
        after.approved_scope_history[0].approved_scope,
        before.approved_scope
    );
    assert_eq!(ctx.store.get(&before.id).unwrap(), after);
    assert_eq!(
        children,
        before
            .children
            .iter()
            .map(|id| ctx.store.get(id).unwrap())
            .collect::<Vec<_>>()
    );
}

#[test]
fn stale_scope_amendment_leaves_all_saved_cards_and_events_unchanged() {
    // Given a Plan whose scope was already advanced by another amendment.
    let (_dir, ctx, before, path) = plan_fixture();
    amendment(&path, &approved_scope());
    amend_plan_scope(&ctx, &before.id, &path, 0).unwrap();
    let saved = saved_files(&ctx);
    // When a stale writer still expects revision zero.
    let error = amend_plan_scope(&ctx, &before.id, &path, 0).unwrap_err();
    // Then it receives an explicit stale error without changing any saved task or event.
    assert!(format!("{error:#}").contains("stale Plan scope amendment"));
    assert_eq!(saved_files(&ctx), saved);
}

#[test]
fn invalid_or_unapproved_amendment_design_is_refused_without_saved_changes() {
    // Given existing approved scope and an additional design that has not been approved.
    let (_dir, ctx, before, path) = plan_fixture();
    let pending = design(&ctx, false);
    let original = before
        .approved_scope
        .as_ref()
        .unwrap()
        .approved_design
        .as_ref()
        .unwrap();
    for link in [original.replace("@P1", "@P0"), pending] {
        let mut scope = approved_scope();
        scope.approved_design = Some(link);
        amendment(&path, &scope);
        let saved = saved_files(&ctx);
        // When a current-revision amendment supplies an invalid or unapproved link.
        let error = amend_plan_scope(&ctx, &before.id, &path, 0).unwrap_err();
        // Then the bad approval is explicitly refused, not stored as a new revision.
        assert!(format!("{error:#}").contains("stale approved design revision"));
        assert_eq!(saved_files(&ctx), saved);
    }
}

#[test]
fn generic_store_writes_cannot_replace_scope_revision_or_history() {
    // Given an existing Plan whose history is owned by the amendment boundary.
    for field in ["scope", "revision", "history"] {
        let (_dir, ctx, before, _path) = plan_fixture();
        let saved = saved_files(&ctx);
        // When an ordinary writer attempts to forge one of the protected fields.
        let error = ctx
            .store
            .update(&before.id, |task| {
                match field {
                    "scope" => {
                        task.approved_scope.as_mut().unwrap().approved_design =
                            Some("T-999@P0".into())
                    }
                    "revision" => task.approved_scope_revision = 7,
                    "history" => {
                        task.approved_scope_history
                            .push(corral_core::ApprovedScopeRevision {
                                revision: 0,
                                approved_scope: None,
                                superseded_at: task.created,
                            })
                    }
                    _ => unreachable!(),
                }
                Ok(())
            })
            .unwrap_err();
        // Then only the explicit operation can write scope and its approval history.
        assert!(format!("{error:#}").contains("revision-guarded amendment"));
        assert_eq!(saved_files(&ctx), saved);
    }
}

#[test]
fn concurrent_scope_amendments_compare_and_swap_the_current_revision() {
    // Given two writers with the same initial revision.
    let (_dir, ctx, before, path) = plan_fixture();
    amendment(&path, &approved_scope());
    // When they race through the supported operation.
    let results = std::thread::scope(|threads| {
        let one = threads.spawn(|| amend_plan_scope(&ctx, &before.id, &path, 0));
        let two = threads.spawn(|| amend_plan_scope(&ctx, &before.id, &path, 0));
        [one.join().unwrap(), two.join().unwrap()]
    });
    // Then precisely one revision and previous-approval record are published.
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| format!("{error:#}").contains("stale Plan scope amendment"))
    );
    let after = ctx.store.get(&before.id).unwrap();
    assert_eq!(after.approved_scope_revision, 1);
    assert_eq!(after.approved_scope_history.len(), 1);
    assert_eq!(after.children, before.children);
}

#[test]
fn legacy_cross_project_tracker_cannot_gain_approved_scope_by_amendment() {
    // Given a legacy tracking-only import whose children span projects.
    let (_dir, ctx, path) = fixture();
    let tracker = import_plan(&ctx, &path, false, Origin::Owner)
        .unwrap()
        .tracker;
    amendment(&path, &approved_scope());
    let saved = saved_files(&ctx);
    // When an amendment tries to give that mixed membership approved Plan scope.
    let error = amend_plan_scope(&ctx, &tracker.id, &path, 0).unwrap_err();
    // Then the same one-project rule as initial publication refuses without altering files.
    assert!(format!("{error:#}").contains("one project"));
    assert_eq!(saved_files(&ctx), saved);
}
