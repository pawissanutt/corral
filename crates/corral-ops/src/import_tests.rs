use super::*;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "scope_amendment_tests.rs"]
mod scope_amendments;

const PLAN: &str = "---\ntitle: Widget spin\nproject: app\ntasks:\n  - key: api\n    title: Add Widget.spin\n    kind: logic\n    size: s\n    priority: P1\n    profile: omo\n    labels: [widget, api]\n    body: |\n      Implement spin.\n  - key: cli\n    title: Wire widget spin\n    kind: backend\n    size: xs\n    depends: [api, T-001]\n    body: Expose the API.\n  - key: docs\n    title: Document spin\n    project: docs\n    kind: docs\n    depends: [cli]\n---\n## Why\nKeep the widget turning.\n";

fn fixture() -> (tempfile::TempDir, Ctx, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    for project in ["app", "docs"] {
        store.config.projects.insert(
            project.into(),
            corral_core::Project {
                path: ".".into(),
                dispatch: Default::default(),
                default_profile: None,
                base: None,
                review: Default::default(),
                artifact_retention: None,
                post_land: Vec::new(),
            },
        );
    }
    store.save_config().unwrap();
    store
        .create("Existing API", Some("app"), Status::Done, "")
        .unwrap();
    let path = dir.path().join("plan.md");
    fs::write(&path, PLAN).unwrap();
    (dir, Ctx::from_store(store), path)
}

fn tree(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            result.insert(path.clone(), Vec::new());
            result.extend(tree(&path));
        } else {
            result.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    result
}

#[test]
fn imports_tracker_and_three_children_with_resolved_edges() {
    for ready in [false, true] {
        // Given a plan with local and existing dependencies and project overrides.
        let (_dir, ctx, path) = fixture();
        let source = PLAN.replace("project: app\n", &format!("project: app\nready: {ready}\n"));
        fs::write(&path, &source).unwrap();
        // When imported through the operation boundary.
        let result = import_plan(&ctx, &path, false, Origin::Owner).unwrap();
        // Then the persisted tracker, children and snapshot retain the plan.
        let tracker = ctx.store.get(&result.tracker.id).unwrap();
        assert_eq!(tracker.id, "T-002");
        assert!(tracker.tracker);
        assert_eq!(tracker.status, Status::Backlog);
        assert_eq!(tracker.children, ["T-003", "T-004", "T-005"]);
        assert_eq!(tracker.body, "## Why\nKeep the widget turning.\n");
        assert_eq!(
            tracker.source_digest,
            Some(crate::verification::digest(source.as_bytes()))
        );
        assert_eq!(tracker.artifacts.len(), 1);
        assert_eq!(tracker.artifacts[0].kind, ArtifactKind::Plan);
        assert_eq!(
            crate::artifact_read(&ctx, &tracker.artifacts[0]).unwrap(),
            source.as_bytes()
        );
        assert_eq!(ctx.store.list().unwrap().len(), 5);
        for child in &result.tasks {
            let saved = ctx.store.get(&child.id).unwrap();
            assert_eq!(saved.parent.as_deref(), Some("T-002"));
            assert_eq!(
                saved.status,
                if ready {
                    Status::Ready
                } else {
                    Status::Backlog
                }
            );
            assert!(!saved.tracker);
            assert_eq!(saved.origin, Some(Origin::Owner));
            assert!(!saved.depends_on.contains(&tracker.id));
        }
        let api = ctx.store.get("T-003").unwrap();
        assert_eq!(api.kind.as_deref(), Some("logic"));
        assert_eq!(api.size, Some(TaskSize::S));
        assert_eq!(api.priority, "P1");
        assert_eq!(api.profile.as_deref(), Some("omo"));
        assert_eq!(api.labels, ["widget", "api"]);
        assert_eq!(api.body, "Implement spin.\n");
        assert_eq!(
            ctx.store.get("T-004").unwrap().depends_on,
            ["T-003", "T-001"]
        );
        assert_eq!(
            ctx.store.get("T-005").unwrap().project.as_deref(),
            Some("docs")
        );
        assert_eq!(ctx.store.get("T-005").unwrap().depends_on, ["T-004"]);
    }
}

#[test]
fn task_id_shaped_local_key_and_existing_alias_resolve_separately() {
    // Given the review's three-task plan and an existing T-001 board card.
    let (_dir, ctx, path) = fixture();
    fs::write(
        &path,
        "---\ntitle: Distinct local and existing prerequisites\ntasks:\n  - key: T-001\n    title: New local prerequisite\n  - key: consumer\n    title: Consumer\n    depends: [T-001, T-1]\n  - key: docs\n    title: Document the consumer\n    depends: [consumer]\n---\nThe consumer depends on the newly imported task and the existing board task.\n",
    )
    .unwrap();
    for dry_run in [true, false] {
        let before = tree(ctx.store.dir());
        // When previewing and then publishing those distinct dependency identities.
        let result = import_plan(&ctx, &path, dry_run, Origin::Owner).unwrap();
        // Then the consumer retains both edges, and preview leaves the store unchanged.
        assert_eq!(result.tracker.id, "T-002");
        assert_eq!(result.tracker.children, ["T-003", "T-004", "T-005"]);
        assert_eq!(result.tasks[1].depends_on, ["T-003", "T-001"]);
        assert_eq!(result.tasks[2].depends_on, ["T-004"]);
        if dry_run {
            assert_eq!(tree(ctx.store.dir()), before);
        } else {
            assert_eq!(
                ctx.store.get("T-004").unwrap().depends_on,
                ["T-003", "T-001"]
            );
            assert_eq!(ctx.store.list().unwrap().len(), 5);
        }
    }
}

#[test]
fn invalid_imports_publish_nothing() {
    for invalid in [
        PLAN.replace("depends: [api, T-001]", "depends: [missing]"),
        PLAN.replace("depends: [api, T-001]", "depends: [T-999]"),
        PLAN.replace("project: app", "project: unknown"),
        PLAN.replace("project: docs", "project: unknown"),
        PLAN.replace("kind: logic", "kind: bogus"),
        PLAN.replace("size: s", "size: gigantic"),
        PLAN.replace("priority: P1", "priority: P9"),
        PLAN.replace("key: docs", "key: api"),
        PLAN.replace("title: Add Widget.spin", "title: ''"),
        PLAN.replace("    kind: logic", "    depends: [cli]\n    kind: logic"),
        PLAN.replace("depends: [api, T-001]", "depends: [api, api]"),
        PLAN.replace("depends: [api, T-001]", "depends: [T-001, T-1]"),
        PLAN.replace("title: Widget spin", "title: ''"),
        "---\ntitle: Empty\ntasks: []\n---\n".into(),
        "---\ntitle: Missing key\ntasks: [{title: Work}]\n---\n".into(),
        "---\ntitle: Missing title\ntasks: [{key: work}]\n---\n".into(),
        PLAN.replace("    kind: logic", "    typo: logic"),
        PLAN.replace("title: Widget spin", "title: Widget spin\napproved_scope:\n  goal: Ship\n  acceptance: []"),
        PLAN.replace("title: Widget spin", "title: Widget spin\napproved_scope:\n  goal: ''\n  acceptance: [{scenario: Check, expected: Pass}]"),
        PLAN.replace("title: Widget spin", "title: Widget spin\napproved_scope:\n  goal: Ship\n  acceptance: [{scenario: '', expected: Pass}]"),
        PLAN.replace("title: Widget spin", "title: Widget spin\napproved_scope:\n  goal: Ship\n  acceptance: [{scenario: Check, expected: Pass}]\n  delivery_target: {kind: integration, branch: 'bad..branch'}"),
        PLAN.replace("title: Widget spin", "title: Widget spin\napproved_scope:\n  goal: Ship\n  acceptance: [{scenario: Check, expected: Pass}]\n  delivery_target: {kind: integration, branch: main}"),
        PLAN.replace("title: Widget spin", "title: Widget spin\napproved_scope:\n  goal: Ship\n  acceptance: [{scenario: Check, expected: Pass}]"),
        PLAN.trim_end_matches("Keep the widget turning.\n")
            .replace("---\n## Why", "## Why"),
    ] {
        // Given a valid board and one invalid source.
        let (_dir, ctx, path) = fixture();
        fs::write(&path, &invalid).unwrap();
        let before = tree(ctx.store.dir());
        // When validation rejects the import.
        assert!(
            import_plan(&ctx, &path, false, Origin::Owner).is_err(),
            "{invalid}"
        );
        // Then no cards, events or artifacts were written.
        assert_eq!(tree(ctx.store.dir()), before, "{invalid}");
    }
}

#[test]
fn preview_writes_nothing_and_predicts_publication() {
    // Given even a missing dispatch lock, which a preview must not create.
    let (_dir, ctx, path) = fixture();
    fs::remove_file(ctx.store.dir().join("lock/dispatch.lock")).unwrap();
    let before = tree(ctx.store.dir());
    // When previewing the import.
    let preview = import_plan(&ctx, &path, true, Origin::Pm).unwrap();
    // Then the entire store is unchanged and its IDs/edges predict publication.
    assert!(preview.dry_run);
    assert_eq!(tree(ctx.store.dir()), before);
    let result = import_plan(&ctx, &path, false, Origin::Pm).unwrap();
    assert_eq!(preview.tracker.id, result.tracker.id);
    assert_eq!(preview.tracker.children, result.tracker.children);
    for (planned, saved) in preview.tasks.iter().zip(&result.tasks) {
        assert_eq!(planned.id, saved.id);
        assert_eq!(planned.depends_on, saved.depends_on);
        assert_eq!(saved.origin, Some(Origin::Pm));
    }
}

#[test]
fn duplicate_open_tracker_is_refused_without_writes() {
    // Given a previously imported source.
    let (_dir, ctx, path) = fixture();
    import_plan(&ctx, &path, false, Origin::Owner).unwrap();
    let before = tree(ctx.store.dir());
    // When the same bytes are imported or previewed again.
    for dry_run in [false, true] {
        assert!(import_plan(&ctx, &path, dry_run, Origin::Owner).is_err());
        // Then the original import remains the only one.
        assert_eq!(tree(ctx.store.dir()), before);
    }
}

#[test]
fn terminal_tracker_allows_a_new_import() {
    for status in [Status::Done, Status::Dropped] {
        // Given an import whose tracker has been terminalized by its owner.
        let (_dir, ctx, path) = fixture();
        let result = import_plan(&ctx, &path, false, Origin::Owner).unwrap();
        crate::move_task_with_decision(
            &ctx,
            &result.tracker.id,
            status,
            true,
            Some("Owner terminalizes this plan"),
            "owner",
            true,
        )
        .unwrap();
        // When the same source is imported again.
        let second = import_plan(&ctx, &path, false, Origin::Agent).unwrap();
        // Then the new import receives distinct IDs.
        assert_ne!(second.tracker.id, result.tracker.id);
        assert_eq!(ctx.store.list().unwrap().len(), 9);
    }
}

#[test]
fn malformed_task_filenames_reserve_ids_for_import() {
    // Given an unreadable task with a reserved high ID.
    let (_dir, ctx, path) = fixture();
    fs::write(ctx.store.dir().join("tasks/T-050-unreadable.md"), "broken").unwrap();
    // When allocating imported cards.
    let result = import_plan(&ctx, &path, false, Origin::Owner).unwrap();
    // Then no unreadable card is overwritten or aliased.
    assert_eq!(result.tracker.id, "T-051");
    assert_eq!(result.tracker.children, ["T-052", "T-053", "T-054"]);
}

#[test]
fn publication_failure_rolls_back_tasks_and_artifacts() {
    // Given an artifact destination that cannot be replaced.
    let (_dir, ctx, path) = fixture();
    let digest = crate::verification::digest(PLAN.as_bytes());
    let blocked = ctx
        .store
        .dir()
        .join(format!("artifacts/T-002/R0/{}-plan.md", &digest[..8]));
    fs::create_dir_all(&blocked).unwrap();
    let before = tree(ctx.store.dir());
    // When snapshot publication fails after task staging.
    assert!(import_plan(&ctx, &path, false, Origin::Owner).is_err());
    // Then every published task is rolled back, preserving the existing tree.
    assert_eq!(tree(ctx.store.dir()), before);
}

#[test]
fn concurrent_identical_imports_create_only_one_tracker() {
    // Given two clients importing the same source.
    let (_dir, ctx, path) = fixture();
    let barrier = std::sync::Barrier::new(2);
    // When both contend for batch admission.
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    import_plan(&ctx, &path, false, Origin::Owner).is_ok()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    // Then one wins and the other observes its digest under the lock.
    assert_eq!(outcomes.iter().filter(|success| **success).count(), 1);
    assert_eq!(ctx.store.list().unwrap().len(), 5);
}
