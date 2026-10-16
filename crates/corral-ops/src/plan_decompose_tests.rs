use super::*;
use corral_core::{Artifact, ArtifactKind, PlanApproval, PlanRecord, PlanScope};

fn acceptance(id: &str) -> PlanAcceptance {
    PlanAcceptance {
        criterion: Some(id.into()),
        scenario: format!("exercise {id}"),
        expected: format!("{id} works"),
    }
}

fn scope(task: &str, depends_on: &[&str]) -> PlanScope {
    PlanScope {
        task: task.into(),
        write_scope: vec![format!("{task}/**")],
        depends_on: depends_on.iter().map(|s| (*s).into()).collect(),
        acceptance: Vec::new(),
    }
}

fn fixture(scopes: Vec<PlanScope>) -> (tempfile::TempDir, Ctx, String) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store
        .create("Design feature", None, Status::Backlog, "Original spec\n")
        .unwrap();
    let now = jiff::Timestamp::now();
    store
        .update(&task.id, |task| {
            task.design = true;
            task.plan_required = Some(true);
            task.plans.push(PlanRecord {
                id: "P1".into(),
                artifact: Artifact {
                    run: "P1".into(),
                    kind: ArtifactKind::Plan,
                    title: "Plan".into(),
                    path: "unused-plan.md".into(),
                    source: "unused-source.md".into(),
                    sha256: "unused".into(),
                    bytes: 0,
                    added: now,
                    tier_override: None,
                    compressed_bytes: None,
                    purged_at: None,
                },
                spec_revision: "accepted-before-projection".into(),
                document: PlanDocument {
                    goal: "Ship feature".into(),
                    non_goals: vec!["Unrelated changes".into()],
                    decisions: vec!["Keep interfaces stable".into()],
                    affected_paths: vec!["src/**".into()],
                    contracts: vec!["Use the shared interface".into()],
                    dependencies: scopes,
                    acceptance: vec![acceptance("C1"), acceptance("C2")],
                    verification: vec!["Run focused tests".into()],
                    owner_decisions: Vec::new(),
                },
                reviews: Vec::new(),
                run: None,
                approval: Some(PlanApproval {
                    by: "owner".into(),
                    at: now,
                    policy_digest: "accepted-policy".into(),
                }),
            });
            Ok(())
        })
        .unwrap();
    (dir, Ctx::from_store(store), task.id)
}

#[test]
fn zero_or_one_scope_attaches_plan_without_children() {
    for scopes in [Vec::new(), vec![scope("implementation", &[])]] {
        let (_dir, ctx, id) = fixture(scopes.clone());
        let before = ctx.store.get(&id).unwrap();
        let route = ctx
            .store
            .config
            .route_task(&before, &Default::default(), |_| None)
            .unwrap();
        ctx.store
            .update(&id, |t| {
                t.queued = true;
                t.dispatch_plan = Some(corral_core::DispatchPlan {
                    plan: true,
                    explicit_plan: true,
                    choice: corral_core::RetryPlan {
                        profile: route.profile.clone(),
                        model: route.model.clone(),
                        effort: route.effort.clone(),
                        switch: None,
                    },
                    route,
                    base: None,
                    mode: Default::default(),
                });
                Ok(())
            })
            .unwrap();
        apply(&ctx, &id, "P1").unwrap();
        apply(&ctx, &id, "P1").unwrap();
        let task = ctx.store.get(&id).unwrap();
        assert_eq!(task.plan.as_deref(), Some("T-001@P1"));
        assert_eq!(task.status, before.status);
        assert!(task.children.is_empty());
        assert!(!task.queued);
        assert!(task.dispatch_plan.is_none());
        assert_eq!(ctx.store.list().unwrap().len(), 1);
        if let Some(scope) = scopes.first() {
            assert_eq!(task.write_scope, scope.write_scope);
        }
        assert!(
            !ctx.store
                .state_dir()
                .join("plan-decompose-T-001.json")
                .exists()
        );
    }
}

#[test]
fn decomposes_scopes_preserving_contracts_and_mapping_dependencies() {
    let mut first = scope("library", &[]);
    first.acceptance = vec![acceptance("C1")];
    let (_dir, ctx, id) = fixture(vec![first, scope("client", &["library"])]);
    let prerequisite = ctx
        .store
        .create("Prerequisite", None, Status::Backlog, "")
        .unwrap();
    ctx.store
        .update(&id, |task| {
            task.depends_on.push(prerequisite.id.clone());
            Ok(())
        })
        .unwrap();
    apply(&ctx, &id, "P1").unwrap();
    let parent = ctx.store.get(&id).unwrap();
    assert_eq!(parent.status, Status::Done);
    assert!(parent.runs.is_empty());
    assert!(parent.launch.is_none());
    assert_eq!(parent.children.len(), 2);
    let library = ctx.store.get(&parent.children[0]).unwrap();
    let client = ctx.store.get(&parent.children[1]).unwrap();
    assert_eq!(library.title, "library");
    assert_eq!(library.write_scope, ["library/**"]);
    assert_eq!(library.depends_on, [prerequisite.id.clone(), id.clone()]);
    assert_eq!(
        client.depends_on,
        [prerequisite.id, id.clone(), library.id.clone()]
    );
    assert!(library.body.contains("C1 works"));
    assert!(!library.body.contains("C2 works"));
    assert!(client.body.contains("C1 works"));
    assert!(client.body.contains("C2 works"));
    for child in [&library, &client] {
        assert_eq!(child.status, Status::Backlog);
        assert_eq!(child.plan.as_deref(), Some("T-001@P1"));
        assert_eq!(child.plan_required, Some(false));
        assert!(!child.design);
        assert!(child.children.is_empty());
        assert!(child.plans.is_empty());
        assert!(child.plan_runs.is_empty());
        assert!(child.runs.is_empty());
        assert!(child.body.contains("Use the shared interface"));
        assert!(child.body.contains("Run focused tests"));
    }
    let files: Vec<_> = parent
        .children
        .iter()
        .map(|id| std::fs::read(ctx.store.get(id).unwrap().path).unwrap())
        .collect();
    apply(&ctx, &id, "P1").unwrap();
    assert_eq!(ctx.store.list().unwrap().len(), 4);
    for (index, child) in parent.children.iter().enumerate() {
        assert_eq!(
            std::fs::read(ctx.store.get(child).unwrap().path).unwrap(),
            files[index]
        );
    }
}

#[test]
fn invalid_graphs_have_no_task_or_journal_side_effects() {
    for scopes in [
        vec![scope("a", &["missing"]), scope("b", &[])],
        vec![scope("a", &["b"]), scope("b", &["a"])],
        vec![scope("a", &["a"]), scope("b", &[])],
        vec![scope("a", &[]), scope("a", &[])],
        vec![scope("a", &["b", "b"]), scope("b", &[])],
        vec![scope("", &[]), scope("b", &[])],
    ] {
        let (_dir, ctx, id) = fixture(scopes);
        let before = std::fs::read(ctx.store.get(&id).unwrap().path).unwrap();
        assert!(apply(&ctx, &id, "P1").is_err());
        assert_eq!(ctx.store.list().unwrap().len(), 1);
        assert_eq!(
            std::fs::read(ctx.store.get(&id).unwrap().path).unwrap(),
            before
        );
        assert!(
            !ctx.store
                .state_dir()
                .join("plan-decompose-T-001.json")
                .exists()
        );
    }
}

#[test]
fn refuses_review_only_acceptance_and_old_plan_ids() {
    let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &[])]);
    assert!(apply(&ctx, &id, "P0").is_err());
    ctx.store
        .update(&id, |task| {
            task.plans[0].approval = None;
            Ok(())
        })
        .unwrap();
    assert!(apply(&ctx, &id, "P1").is_err());
    assert_eq!(ctx.store.list().unwrap().len(), 1);
}

#[test]
fn rejects_invalid_inherited_dependency_graph_before_creating_children() {
    for missing in [true, false] {
        let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &[])]);
        let prerequisite = ctx
            .store
            .create("Existing prerequisite", None, Status::Backlog, "")
            .unwrap();
        ctx.store
            .update(&prerequisite.id, |task| {
                task.depends_on
                    .push(if missing { "T-999".into() } else { id.clone() });
                Ok(())
            })
            .unwrap();
        ctx.store
            .update(&id, |task| {
                task.depends_on.push(prerequisite.id.clone());
                Ok(())
            })
            .unwrap();
        assert!(apply(&ctx, &id, "P1").is_err());
        assert_eq!(ctx.store.list().unwrap().len(), 2);
        assert!(
            !ctx.store
                .state_dir()
                .join("plan-decompose-T-001.json")
                .exists()
        );
    }
}

#[test]
fn scope_criteria_preserve_live_requirements_with_fresh_child_identity() {
    let mut first = scope("a", &[]);
    first.acceptance = vec![acceptance("C1")];
    let (_dir, ctx, id) = fixture(vec![first, scope("b", &[])]);
    ctx.store
        .update(&id, |task| {
            task.completion = Some(corral_core::CriteriaManifest {
                version: 1,
                spec_digest: crate::verification::spec_digest(task)?,
                reviewed_by: "owner".into(),
                audited: true,
                require_gate: true,
                owner_decisions: Vec::new(),
                criteria: ["C1", "C2"]
                    .into_iter()
                    .map(|id| corral_core::Criterion {
                        id: id.into(),
                        required: true,
                        live: true,
                        scenario: "Original scenario".into(),
                        expected: "Original expected".into(),
                    })
                    .collect(),
            });
            Ok(())
        })
        .unwrap();
    apply(&ctx, &id, "P1").unwrap();
    let child = ctx.store.get("T-002").unwrap();
    let manifest = child.completion.as_ref().unwrap();
    assert_eq!(manifest.criteria.len(), 1);
    assert!(manifest.criteria[0].live);
    assert!(manifest.criteria[0].required);
    assert_eq!(manifest.criteria[0].scenario, "exercise C1");
    assert_eq!(
        manifest.spec_digest,
        crate::verification::spec_digest(&child).unwrap()
    );
    assert_eq!(
        ctx.store
            .get("T-003")
            .unwrap()
            .completion
            .unwrap()
            .criteria
            .len(),
        2
    );
}

#[test]
fn interrupted_projection_resumes_without_duplicate_children() {
    let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &["a"])]);
    // Deterministically fail publishing the second child, after the journal and
    // first child exist. No timers, hooks, servers or live herdr are involved.
    let blocked = child_path(&ctx.store, "T-003");
    std::fs::create_dir(&blocked).unwrap();
    assert!(apply(&ctx, &id, "P1").is_err());
    let parent = ctx.store.get(&id).unwrap();
    assert_ne!(parent.status, Status::Done);
    assert!(parent.children.is_empty());
    let staged = ctx.store.get("T-002").unwrap();
    assert!(staged.depends_on.contains(&id));
    assert!(staged.runs.is_empty());
    assert!(
        crate::queue::dependency_check(&ctx.store, &staged)
            .unwrap()
            .is_some()
    );
    std::fs::remove_dir(blocked).unwrap();
    apply(&ctx, &id, "P1").unwrap();
    apply(&ctx, &id, "P1").unwrap();
    assert_eq!(ctx.store.list().unwrap().len(), 3);
    assert_eq!(ctx.store.get(&id).unwrap().children, ["T-002", "T-003"]);
}

#[test]
fn recovery_remaps_reserved_id_taken_by_another_writer() {
    let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &["a"])]);
    let blocked = child_path(&ctx.store, "T-003");
    std::fs::create_dir(&blocked).unwrap();
    assert!(apply(&ctx, &id, "P1").is_err());
    std::fs::remove_dir(blocked).unwrap();
    let other = ctx
        .store
        .create(
            "Independent task",
            None,
            Status::Backlog,
            "Do not overwrite",
        )
        .unwrap();
    assert_eq!(other.id, "T-003");
    apply(&ctx, &id, "P1").unwrap();
    assert_eq!(
        ctx.store.get("T-003").unwrap().body.trim(),
        "Do not overwrite"
    );
    let parent = ctx.store.get(&id).unwrap();
    assert_eq!(parent.children, ["T-002", "T-004"]);
    assert_eq!(
        ctx.store.get("T-004").unwrap().depends_on,
        ["T-001", "T-002"]
    );
    assert_eq!(ctx.store.list().unwrap().len(), 4);
}

#[test]
fn concurrent_approval_calls_serialize_on_the_dispatch_lock() {
    let (_dir, ctx, id) = fixture(vec![scope("a", &[]), scope("b", &["a"])]);
    std::thread::scope(|scope| {
        let one = scope.spawn(|| apply(&ctx, &id, "P1"));
        let two = scope.spawn(|| apply(&ctx, &id, "P1"));
        one.join().unwrap().unwrap();
        two.join().unwrap().unwrap();
    });
    assert_eq!(ctx.store.list().unwrap().len(), 3);
    assert_eq!(ctx.store.get(&id).unwrap().children, ["T-002", "T-003"]);
}
