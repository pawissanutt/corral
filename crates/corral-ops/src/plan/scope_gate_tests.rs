use super::*;
use corral_core::{
    Artifact, ArtifactKind, ExpectedFailure, MemberGateRecord, PlanAcceptance, PlanApproval,
    PlanRecord, PlanScope, Status, TierPolicy,
};

use crate::Ctx;

fn gate(tier: &str, tests: &[&str], expected: &[&str]) -> ScopeGate {
    ScopeGate {
        tier: tier.into(),
        tests: tests.iter().map(|t| (*t).into()).collect(),
        expected_failures: expected
            .iter()
            .map(|test| ExpectedFailure {
                test: (*test).into(),
                reason: "fails until cli lands; restored by cli".into(),
            })
            .collect(),
    }
}

fn scope(task: &str, depends_on: &[&str], gate: Option<ScopeGate>) -> PlanScope {
    PlanScope {
        task: task.into(),
        write_scope: vec![format!("{task}/**")],
        depends_on: depends_on.iter().map(|s| (*s).into()).collect(),
        acceptance: Vec::new(),
        gate,
    }
}

fn scopes() -> Vec<PlanScope> {
    vec![
        scope(
            "api",
            &[],
            Some(gate(
                "quick",
                &["package(corral-ops)"],
                &["corral-ops::gate_ops tests::quick_gate_runs"],
            )),
        ),
        scope("cli", &["api"], Some(gate("full", &[], &[]))),
        scope("docs", &["cli"], None),
    ]
}

/// Accepts and publishes an owner-approved design `T-001@P1` with the given delivery target.
fn published(target: PlanDeliveryTarget) -> (tempfile::TempDir, Ctx, String) {
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
                    delivery_target: Some(target),
                    goal: "Ship feature".into(),
                    non_goals: Vec::new(),
                    decisions: Vec::new(),
                    affected_paths: vec!["src/**".into()],
                    contracts: Vec::new(),
                    dependencies: scopes(),
                    acceptance: vec![PlanAcceptance {
                        criterion: None,
                        scenario: "exercise".into(),
                        expected: "works".into(),
                    }],
                    verification: vec!["Run focused tests".into()],
                    owner_decisions: Vec::new(),
                },
                reviews: Vec::new(),
                run: None,
                approval: Some(PlanApproval {
                    by: "owner".into(),
                    at: now,
                    policy_digest: "accepted-policy".into(),
                    authorization: None,
                }),
            });
            Ok(())
        })
        .unwrap();
    let ctx = Ctx::from_store(store);
    crate::plan_decompose::apply(&ctx, &task.id, "P1").unwrap();
    (dir, ctx, task.id)
}

fn integration() -> PlanDeliveryTarget {
    PlanDeliveryTarget::Integration {
        branch: "feature/widget".into(),
    }
}

/// Published member IDs by their scope label, read once before any child edit.
fn members(ctx: &Ctx, design: &str) -> BTreeMap<String, String> {
    let parent = ctx.store.get(design).unwrap();
    parent
        .children
        .iter()
        .map(|id| (ctx.store.get(id).unwrap().title, id.clone()))
        .collect()
}

fn resolved(ctx: &Ctx, id: &str) -> Result<Option<MemberGate>> {
    resolve(&ctx.store, &ctx.store.get(id).unwrap())
}

fn scope_source(design: &str, label: &str) -> String {
    let scope = scopes().into_iter().find(|s| s.task == label).unwrap();
    let digest = crate::verification::digest(&serde_json::to_vec(&scope).unwrap());
    format!("plan:{design}@P1#{label}@{digest}")
}

fn edit_journal(ctx: &Ctx, design: &str, edit: impl FnOnce(&mut serde_json::Value)) {
    let path = ctx
        .store
        .state_dir()
        .join(format!("plan-decompose-{design}.json"));
    let mut journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    edit(&mut journal);
    std::fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
}

fn named_error(result: Result<Option<MemberGate>>) -> ScopeGateError {
    result
        .unwrap_err()
        .downcast::<ScopeGateError>()
        .expect("a named member gate resolution error")
}

#[test]
fn published_scope_gates_resolve_through_the_journal_binding_despite_child_edits() {
    // Given an accepted two-plus-scope integration plan whose scopes declare gates, published.
    let (_dir, ctx, design) = published(integration());
    let members = members(&ctx, &design);
    let expected = |label: &str| {
        let gate = scopes()
            .into_iter()
            .find(|s| s.task == label)
            .unwrap()
            .gate
            .unwrap();
        (gate, scope_source(&design, label))
    };
    let before = resolved(&ctx, &members["api"]).unwrap().unwrap();
    assert_eq!(
        (before.gate.clone(), before.source.clone()),
        expected("api")
    );
    let cli = resolved(&ctx, &members["cli"]).unwrap().unwrap();
    assert_eq!((cli.gate, cli.source), expected("cli"));
    // When someone renames api's child to cli's label and rewrites its labels and body.
    ctx.store
        .update(&members["api"], |task| {
            task.title = "cli".into();
            task.labels = vec!["cli".into(), "docs".into()];
            task.body = "scope: cli\ngate: full\n".into();
            Ok(())
        })
        .unwrap();
    // Then the resolved gate and its source do not change.
    assert_eq!(resolved(&ctx, &members["api"]).unwrap().unwrap(), before);
}

#[test]
fn missing_or_mismatched_publication_binding_is_a_named_error() {
    let (_dir, ctx, design) = published(integration());
    let members = members(&ctx, &design);
    // Given cli's binding removed from the journal and api's digest replaced.
    edit_journal(&ctx, &design, |journal| {
        journal["scopes"].as_object_mut().unwrap().remove("cli");
        journal["scopes"]["api"]["scope_digest"] = "foreign-scope".into();
    });
    // Then each member gets its named error rather than a fallback gate.
    assert_eq!(
        named_error(resolved(&ctx, &members["cli"])),
        ScopeGateError::MissingBinding {
            member: members["cli"].clone(),
            design: format!("{design}@P1"),
        }
    );
    assert_eq!(
        named_error(resolved(&ctx, &members["api"])),
        ScopeGateError::ScopeDigestMismatch {
            member: members["api"].clone(),
            design: format!("{design}@P1"),
            label: "api".into(),
        }
    );
    // And a member whose own plan link was rewritten has no binding either.
    ctx.store
        .update(&members["docs"], |task| {
            task.plan = Some(format!("{design}@P9"));
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        named_error(resolved(&ctx, &members["docs"])),
        ScopeGateError::MissingBinding { .. }
    ));
}

#[test]
fn precedence_is_owner_record_then_plan_scope_then_policy_then_full() {
    let (_dir, mut ctx, design) = published(integration());
    let members = members(&ctx, &design);
    let tracker = ctx
        .store
        .get(&members["api"])
        .unwrap()
        .parent
        .clone()
        .unwrap();
    let owner_gate = gate("lint", &["test(widget)"], &["corral-ops::x tests::y"]);
    ctx.store
        .update(&tracker, |task| {
            task.member_gates.insert(
                members["api"].clone(),
                MemberGateRecord {
                    revision: 2,
                    gate: owner_gate.clone(),
                    reason: "narrow api".into(),
                    by: "owner".into(),
                    at: jiff::Timestamp::now(),
                    history: Vec::new(),
                },
            );
            Ok(())
        })
        .unwrap();
    // The owner record outranks api's plan scope gate.
    let api = resolved(&ctx, &members["api"]).unwrap().unwrap();
    assert_eq!(api.gate, owner_gate);
    assert_eq!(api.source, format!("member-gate:{tracker}@2"));
    assert_eq!(api.tracker, tracker);
    // A declared scope gate outranks policy.
    ctx.store.config.review.tier_policy = Some(TierPolicy {
        plan_member: Some("quick".into()),
        review_rounds: None,
    });
    let cli = resolved(&ctx, &members["cli"]).unwrap().unwrap();
    assert_eq!(cli.source, scope_source(&design, "cli"));
    // A scope without a gate uses tier_policy.plan_member, with no tests or expected failures.
    let docs = resolved(&ctx, &members["docs"]).unwrap().unwrap();
    assert_eq!(
        (docs.gate, docs.source.as_str()),
        (gate("quick", &[], &[]), "policy:plan_member")
    );
    // Without a policy it is full, still a member binding.
    ctx.store.config.review.tier_policy = None;
    let docs = resolved(&ctx, &members["docs"]).unwrap().unwrap();
    assert_eq!(
        (docs.gate, docs.source.as_str()),
        (gate("full", &[], &[]), "default:full")
    );
}

#[test]
fn imported_tracker_members_take_tests_and_expected_failures_only_from_an_owner_record() {
    // Given an imported integration tracker, which has no approved design.
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    store.config.projects.insert(
        "app".into(),
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
    store.save_config().unwrap();
    let path = dir.path().join("import.md");
    std::fs::write(
        &path,
        "---\ntitle: Widget\nproject: app\napproved_scope:\n  goal: Ship\n  acceptance: [{scenario: Check, expected: Pass}]\n  delivery_target: {kind: integration, branch: feature/widget}\ntasks:\n  - key: api\n    title: Add api\n  - key: cli\n    title: Add cli\n    depends: [api]\n---\n",
    )
    .unwrap();
    let mut ctx = Ctx::from_store(store);
    let imported =
        crate::import::import_plan(&ctx, &path, false, corral_core::Origin::Owner).unwrap();
    let tracker = imported.tracker.id.clone();
    let api = ctx.store.get(&tracker).unwrap().children[0].clone();
    assert_eq!(
        ctx.store
            .get(&tracker)
            .unwrap()
            .approved_scope
            .unwrap()
            .approved_design,
        None
    );
    // And a member whose own plan link names some design.
    ctx.store
        .update(&api, |task| {
            task.plan = Some("T-999@P1".into());
            Ok(())
        })
        .unwrap();
    ctx.store.config.review.tier_policy = Some(TierPolicy {
        plan_member: Some("quick".into()),
        review_rounds: None,
    });
    // Then the member uses tier_policy.plan_member with no tests or expected failures.
    let resolved_api = resolved(&ctx, &api).unwrap().unwrap();
    assert_eq!(
        (resolved_api.gate, resolved_api.source.as_str()),
        (gate("quick", &[], &[]), "policy:plan_member")
    );
    // And only an owner record supplies them.
    let owner_gate = gate("quick", &["test(api)"], &["corral-ops::api tests::slow"]);
    ctx.store
        .update(&tracker, |task| {
            task.member_gates.insert(
                api.clone(),
                MemberGateRecord {
                    revision: 1,
                    gate: owner_gate.clone(),
                    reason: "imported member".into(),
                    by: "owner".into(),
                    at: jiff::Timestamp::now(),
                    history: Vec::new(),
                },
            );
            Ok(())
        })
        .unwrap();
    let resolved_api = resolved(&ctx, &api).unwrap().unwrap();
    assert_eq!(
        (resolved_api.gate, resolved_api.source),
        (owner_gate, format!("member-gate:{tracker}@1"))
    );
}

#[test]
fn main_delivery_members_and_unplanned_tasks_have_no_member_gate() {
    let (_dir, ctx, design) = published(PlanDeliveryTarget::Main);
    for id in members(&ctx, &design).values() {
        assert_eq!(resolved(&ctx, id).unwrap(), None);
    }
    assert_eq!(resolved(&ctx, &design).unwrap(), None);
}

#[test]
fn plan_submit_rejects_a_scope_gate_naming_an_undefined_tier() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    store.config.review.gate_tiers = Some(BTreeMap::from([("quick".into(), vec!["true".into()])]));
    store.save_config().unwrap();
    let task = store.create("Plan", None, Status::Ready, "Spec").unwrap();
    let ctx = Ctx::from_store(store);
    let path = dir.path().join("plan.md");
    let plan = |tier: &str| {
        format!(
            "---\ngoal: Plan\nnon_goals: []\ndecisions: []\naffected_paths: [a.rs]\ncontracts: []\n\
             delivery_target: {{kind: integration, branch: feature/widget}}\n\
             dependencies:\n\
             - {{task: api, write_scope: [a.rs], depends_on: [], gate: {{tier: {tier}}}}}\n\
             - {{task: cli, write_scope: [b.rs], depends_on: [api]}}\n\
             acceptance: [{{scenario: check, expected: passes}}]\nverification: [check]\nowner_decisions: []\n---\n"
        )
    };
    // When the scope gate names a tier the project does not define.
    std::fs::write(&path, plan("turbo")).unwrap();
    let error = format!(
        "{:#}",
        crate::plan_submit(&ctx, &task.id, &path).unwrap_err()
    );
    // Then submission is rejected naming the scope label and the tier, and nothing is saved.
    assert!(
        error.contains("scope \"api\"") && error.contains("\"turbo\""),
        "{error}"
    );
    assert!(ctx.store.get(&task.id).unwrap().plans.is_empty());
    // A defined tier and the reserved full tier are accepted.
    for tier in ["quick", "full"] {
        std::fs::write(&path, plan(tier)).unwrap();
        crate::plan_submit(&ctx, &task.id, &path).unwrap();
    }
}
