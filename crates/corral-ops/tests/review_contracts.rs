//! PRF-08 behavioral fixtures: task-aware reviewer routing, candidate-bound packets, evidence
//! enforcement and bounded stronger review, against the synchronous fake herdr.
#![cfg(unix)] // Fixtures execute the symlinked POSIX fake engine.

use std::collections::BTreeSet;
use std::path::Path;

mod support;

use corral_core::{
    ArtifactKind, Config, Harness, Profile, ProfileEligibility, ProfileMetadata, ProfileRole,
    ReviewCapability, ReviewContractKind, ReviewIndependence, ReviewRecord, ReviewRoute,
    SelectionGuidance, Status, Store, Verdict,
};
use corral_herdr::Herdr;
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome, StrongerOutcome};
use serde_json::Value;

const SPEC: &str = "## Goal\nChange the widget.\n\n## Acceptance\n- The widget changes.\n";

fn git(dir: &Path, args: &[&str]) {
    let out = support::command("git", dir)
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
}

fn reviewer(model: &str, vendor: &str, roles: &[ProfileRole]) -> Profile {
    Profile {
        metadata: Some(ProfileMetadata {
            display_name: model.into(),
            purpose: "Review".into(),
            guidance: SelectionGuidance::default(),
            instructions: format!("Custom guidance for {model}"),
            eligibility: ProfileEligibility {
                enabled: true,
                automatic: true,
                roles: roles.iter().copied().collect::<BTreeSet<_>>(),
                projects: BTreeSet::new(),
            },
            revision: "r1".into(),
            evidence: None,
            candidates: vec![],
            fallback: None,
        }),
        harness: "h".into(),
        model: Some(model.into()),
        vendor: Some(vendor.into()),
        effort: None,
        args: vec![],
        escalate_to: None,
    }
}

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
}

fn fixture(configure: impl FnOnce(&mut Config)) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("herdr");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/reviewer_ops/herdr.sh"),
        &bin,
    )
    .unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir_all(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "corral test"]);
    git(&app, &["config", "user.email", "corral@test.invalid"]);
    std::fs::write(app.join("README.md"), "hello\n").unwrap();
    git(&app, &["add", "-A"]);
    git(&app, &["commit", "-q", "-m", "init"]);
    let mut store = Store::init(tmp.path()).unwrap();
    let config = &mut store.config;
    config.harnesses.insert(
        "h".into(),
        Harness {
            command: "true {model_args}".into(),
            flags: Some("claude".into()),
            model_flag: None,
            effort_flag: None,
            efforts: vec![],
            prompt: None,
        },
    );
    use ProfileRole::{Implementation, ImplementationReview, PlanReview};
    let mut author = reviewer("sonnet", "anthropic", &[Implementation]);
    author.metadata = None;
    config.profiles.insert("author".into(), author);
    config.profiles.insert(
        "design".into(),
        reviewer("opus", "openai", &[PlanReview, ImplementationReview]),
    );
    config.profiles.insert(
        "impl".into(),
        reviewer("haiku", "openai", &[ImplementationReview]),
    );
    config.profiles.insert(
        "visual".into(),
        reviewer("haiku", "openai", &[ImplementationReview]),
    );
    config.profiles.insert(
        "same".into(),
        reviewer("sonnet", "anthropic", &[ImplementationReview]),
    );
    config.review.gate = Some(vec!["echo gate-ok".into()]);
    config.review.reviewer = Some("auto".into());
    for (kind, profile) in [
        (ReviewContractKind::Plan, "design"),
        (ReviewContractKind::Implementation, "impl"),
        (ReviewContractKind::ComplexImplementation, "design"),
        (ReviewContractKind::VisualImplementation, "visual"),
    ] {
        config.routing.review_profiles.insert(kind, profile.into());
    }
    configure(config);
    store.save_config().unwrap();
    let ctx = Ctx {
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        store,
        herdr: Herdr::with_bin(bin, None),
    };
    Fx { _tmp: tmp, ctx }
}

fn candidate(fx: &Fx, kind: &str, labels: &[&str], file: &str) -> String {
    let ctx = &fx.ctx;
    let task = ctx
        .store
        .create("Change the widget", Some("app"), Status::Ready, SPEC)
        .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.kind = Some(kind.into());
            t.labels = labels.iter().map(|l| l.to_string()).collect();
            Ok(())
        })
        .unwrap();
    let opts = DispatchOpts {
        profile: Some("author".into()),
        ..DispatchOpts::default()
    };
    let DispatchOutcome::Started { task, .. } = corral_ops::dispatch(ctx, &task.id, &opts).unwrap()
    else {
        panic!("expected Started");
    };
    let worktree = task.current_run().unwrap().worktree.clone();
    let path = worktree.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "changed\n").unwrap();
    corral_git::commit_all(&worktree, "Record review fixture").unwrap();
    let identity = corral_ops::progress_readback(ctx, &task.id).unwrap();
    corral_ops::save_checkpoint(
        ctx,
        &task.id,
        &corral_core::CheckpointUpdate {
            task: identity.task,
            run: identity.run,
            spec_revision: identity.spec_revision,
            expected_revision: 0,
            progress_revision: 0,
            head: identity.head,
            base: identity.base,
            tree: identity.tree,
            criterion_ids: vec![],
            evidence_refs: vec![],
            next_action: "Review the candidate".into(),
            decisions: vec![],
            waits: vec![],
            resources: vec![],
        },
    )
    .unwrap();
    corral_ops::done(ctx, &task.id, "widget changed; Verified: all tests pass").unwrap();
    corral_ops::gate_start(ctx, &task.id).unwrap();
    corral_ops::gate_wait(ctx, &task.id).unwrap();
    task.id
}

fn route(record: &ReviewRecord) -> &ReviewRoute {
    record
        .route
        .as_deref()
        .expect("routed review records its route")
}

fn attempt(record: &ReviewRecord) -> String {
    record.identity.as_ref().unwrap().attempt.clone()
}

fn packet(fx: &Fx, id: &str) -> Value {
    let task = fx.ctx.store.get(id).unwrap();
    let run = task.current_run().unwrap();
    let child = run.review_runs.last().unwrap();
    let file = fx
        .ctx
        .store
        .prompts_dir()
        .join(format!("{id}-{}-{}-review-packet.json", run.id, child.id));
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

fn recorded(fx: &Fx, id: &str) -> ReviewRecord {
    fx.ctx
        .store
        .get(id)
        .unwrap()
        .current_run()
        .unwrap()
        .review
        .clone()
        .unwrap()
}

#[test]
fn routine_implementation_routes_to_the_implementation_mapping() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let record = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let route = route(&record);
    assert_eq!(record.reviewer, "impl");
    assert_eq!(
        route.requirement.contract.kind,
        ReviewContractKind::Implementation
    );
    assert_eq!(route.requirement.capability, ReviewCapability::Routine);
    assert_eq!(route.profile_source, "auto_mapping");
    assert_eq!(route.mapping, Some(ReviewContractKind::Implementation));
    assert_eq!(route.profile_revision, "r1");
    assert_eq!(route.independence, ReviewIndependence::DifferentVendor);
    assert_eq!(
        route.contract_digest,
        corral_ops::review_contracts::contract_digest(ReviewContractKind::Implementation)
    );
    assert!(route.packet_digest.is_some());
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(
        task.current_run().unwrap().agent,
        "author",
        "selection is recorded separately from the implementation profile"
    );
}

#[test]
fn packet_binds_the_candidate_and_never_treats_worker_prose_as_pass() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let record = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let packet = packet(&fx, &id);
    let bound = &packet["bound"];
    assert_eq!(
        packet["digest"],
        route(&record).packet_digest.clone().unwrap()
    );
    assert_eq!(bound["candidate"]["head"], record.sha);
    assert_eq!(bound["requirement"]["contract"]["kind"], "implementation");
    assert_eq!(
        bound["evidence"]["worker_summary"]["authority"],
        "worker_assertion"
    );
    assert_eq!(bound["evidence"]["worker_summary"]["machine_pass"], false);
    assert_eq!(
        bound["evidence"]["gate"]["authority"],
        "corral_gate_receipt"
    );
    assert_eq!(bound["evidence"]["reusable"], serde_json::json!(["gate"]));
    assert_eq!(bound["gaps"], serde_json::json!([]));
    assert!(
        bound["object"]["changed_paths"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "src/widget.rs")
    );
    let diff = std::fs::read(bound["object"]["diff_file"].as_str().unwrap()).unwrap();
    assert!(String::from_utf8_lossy(&diff).contains("+changed"));
    assert_eq!(
        bound["reviewer"]["instructions"],
        "Custom guidance for haiku"
    );
    let approved = corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "criteria shown").unwrap();
    assert_eq!(approved.verdict, Some(Verdict::Approve));
}

#[test]
fn concurrency_label_on_a_tiny_docs_task_selects_strong_review() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "docs", &["concurrency"], "notes.md");
    let record = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let route = route(&record);
    assert_eq!(
        route.requirement.contract.kind,
        ReviewContractKind::ComplexImplementation
    );
    assert_eq!(route.requirement.capability, ReviewCapability::Strong);
    assert_eq!(record.reviewer, "design");
}

#[test]
fn touching_a_risk_path_selects_strong_review_without_labels() {
    let fx = fixture(|config| config.review.risk_paths = Some(vec!["locks/**".into()]));
    let id = candidate(&fx, "logic", &[], "locks/mutex.rs");
    let record = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let route = route(&record);
    assert_eq!(
        route.requirement.contract.kind,
        ReviewContractKind::ComplexImplementation
    );
    assert_eq!(route.signals.risk_paths, vec!["locks/mutex.rs".to_string()]);
    assert_eq!(record.reviewer, "design");
}

#[test]
fn ui_approval_without_screenshot_requests_changes() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "ui", &[], "web/app.tsx");
    let record = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    assert_eq!(record.reviewer, "visual");
    assert_eq!(
        route(&record).requirement.contract.kind,
        ReviewContractKind::VisualImplementation
    );
    assert_eq!(
        packet(&fx, &id)["bound"]["gaps"][0]["code"],
        "visual_evidence_missing"
    );
    let saved = corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "looks fine").unwrap();
    assert_eq!(saved.verdict, Some(Verdict::Changes));
}

#[test]
fn ui_approval_with_screenshot_evidence_is_recorded() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "ui", &[], "web/app.tsx");
    let shot = fx._tmp.path().join("shot.png");
    std::fs::write(&shot, b"\x89PNG\r\n\x1a\nfixture").unwrap();
    corral_ops::add_artifact(
        &fx.ctx,
        &id,
        &shot,
        Some(ArtifactKind::Screenshot),
        Some("UI"),
    )
    .unwrap();
    corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    assert_eq!(packet(&fx, &id)["bound"]["gaps"], serde_json::json!([]));
    let saved = corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "inspected").unwrap();
    assert_eq!(saved.verdict, Some(Verdict::Approve));
}

#[test]
fn prose_deliverable_uses_the_prose_contract_and_routine_mapping() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "docs", &[], "docs/guide.md");
    let record = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let route = route(&record);
    assert_eq!(route.requirement.contract.kind, ReviewContractKind::Prose);
    assert_eq!(route.mapping, Some(ReviewContractKind::Implementation));
    assert_eq!(record.reviewer, "impl");
    let saved = corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "claims verified").unwrap();
    assert_eq!(saved.verdict, Some(Verdict::Approve));
}

#[test]
fn explicit_pin_keeps_its_reviewer_and_escalates_a_capability_mismatch() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &["permissions"], "src/auth.rs");
    let record = corral_ops::review_run(&fx.ctx, &id, Some("impl")).unwrap();
    let route = route(&record);
    assert_eq!(record.reviewer, "impl");
    assert_eq!(route.profile_source, "explicit_override");
    assert_eq!(route.mapping, None);
    assert_eq!(
        route.requirement.contract.kind,
        ReviewContractKind::ComplexImplementation
    );
    assert!(route.capability_mismatch);
    let saved = corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "fine").unwrap();
    assert_eq!(saved.verdict, Some(Verdict::Escalate));
}

#[test]
fn same_model_reviewer_is_a_disclosed_fresh_session() {
    let fx = fixture(|config| {
        config
            .routing
            .review_profiles
            .insert(ReviewContractKind::Implementation, "same".into());
    });
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let record = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let route = route(&record);
    assert_eq!(
        route.independence,
        ReviewIndependence::SameModelFreshSession
    );
    assert!(!route.warnings.is_empty());
}

#[test]
fn cross_family_requirement_still_refuses_a_same_vendor_auto_reviewer() {
    let fx = fixture(|config| {
        config.review.require_cross_family = Some(true);
        config
            .routing
            .review_profiles
            .insert(ReviewContractKind::Implementation, "same".into());
    });
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let error = corral_ops::review_run(&fx.ctx, &id, None).unwrap_err();
    assert!(format!("{error:#}").contains("cross-family"), "{error:#}");
}

#[test]
fn changed_reviewer_profile_revision_stales_the_packet() {
    let mut fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    fx.ctx
        .store
        .config
        .profiles
        .get_mut("impl")
        .and_then(|p| p.metadata.as_mut())
        .unwrap()
        .revision = "r2".into();
    let error = corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "ok").unwrap_err();
    let text = format!("{error:#}");
    assert!(
        text.contains("stale review") && text.contains("profile_revision"),
        "{text}"
    );
    assert_eq!(recorded(&fx, &id).verdict, None);
}

#[test]
fn stronger_review_supersedes_once_then_escalates() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let routine = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let outcome = corral_ops::request_stronger_review(
        &fx.ctx,
        &id,
        &attempt(&routine),
        "The lock ordering argument in src/widget.rs:1 needs design-level analysis.",
    )
    .unwrap();
    let StrongerOutcome::Rerouted { superseded, .. } = outcome else {
        panic!("expected reroute: {outcome:?}");
    };
    let task = fx.ctx.store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    let old = run.review_runs.iter().find(|c| c.id == superseded).unwrap();
    assert_eq!(old.outcome, Some(corral_core::RunOutcome::Superseded));
    assert_eq!(old.review.as_ref().unwrap().verdict, None);
    if run.review.is_none() {
        corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    }
    let strong = recorded(&fx, &id);
    let strong_route = route(&strong);
    assert_eq!(strong.reviewer, "design");
    assert_eq!(
        strong_route.requirement.capability,
        ReviewCapability::Strong
    );
    assert_eq!(
        strong_route.escalated_from.as_deref(),
        Some(superseded.as_str())
    );
    let findings = &packet(&fx, &id)["bound"]["previous_findings"];
    assert_eq!(findings[0]["attempt"], superseded);
    assert_eq!(findings[0]["superseded"], true);

    let again =
        corral_ops::request_stronger_review(&fx.ctx, &id, &attempt(&strong), "Still uncertain.")
            .unwrap();
    assert!(
        matches!(again, StrongerOutcome::Escalated { .. }),
        "{again:?}"
    );
    assert_eq!(recorded(&fx, &id).verdict, Some(Verdict::Escalate));
    let reviews = fx
        .ctx
        .store
        .get(&id)
        .unwrap()
        .current_run()
        .unwrap()
        .review_runs
        .len();
    assert_eq!(reviews, 2, "no parallel panel or unbounded chain");
}

#[test]
fn pinned_reviewer_cannot_switch_itself_to_a_stronger_review() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let pinned = corral_ops::review_run(&fx.ctx, &id, Some("impl")).unwrap();
    let outcome =
        corral_ops::request_stronger_review(&fx.ctx, &id, &attempt(&pinned), "Need more depth.")
            .unwrap();
    assert!(
        matches!(outcome, StrongerOutcome::Escalated { .. }),
        "{outcome:?}"
    );
    assert_eq!(recorded(&fx, &id).verdict, Some(Verdict::Escalate));
}

#[test]
fn legacy_profile_policy_keeps_its_reviewer_with_a_task_aware_contract() {
    let fx = fixture(|config| config.review.reviewer = Some("design".into()));
    let id = candidate(&fx, "test", &["flaky"], "tests/widget.rs");
    let record = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let route = route(&record);
    assert_eq!(record.reviewer, "design");
    assert_eq!(
        route.requirement.contract.kind,
        ReviewContractKind::TestOnly
    );
    assert_eq!(route.profile_source, "workspace_review_config");
    assert_eq!(route.mapping, None);
}
