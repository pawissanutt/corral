use std::path::Path;

use corral_core::{
    Accept, Config, MergeMode, PolicySource, ReviewPolicy, Reviewer, Status, Store, validate_glob,
};

fn config(toml_text: &str) -> Config {
    toml::from_str(toml_text).unwrap()
}

mod operating_plan_decisions {
    use std::collections::BTreeSet;
    use std::fs;

    use corral_core::{
        Accept, Artifact, ArtifactKind, OperatingActionKind, OperatingAuthority, OperatingMode,
        OperatingPresentation, OperatingSelection, PlanApproval, PlanDocument, PlanRecord,
        PlanReview, Status, Store, Verdict,
    };
    use jiff::Timestamp;
    use sha2::{Digest, Sha256};

    const PLAN: &str = "---\ngoal: Storage\nnon_goals: []\ndecisions: []\naffected_paths: [storage.rs]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: save, expected: persisted}]\nverification: [cargo test]\nowner_decisions: [Choose storage, Choose export]\n---\n";
    const NOTES: &str = "Resolved: Choose storage => SQLite\nResolved: Choose export => JSON";

    fn clock() -> Timestamp {
        "2026-10-18T04:00:00Z".parse().unwrap()
    }

    fn fixture() -> (tempfile::TempDir, Store, String) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        store
            .edit_config(|doc| {
                doc.set_str("projects.app.path", ".")?;
                doc.set_str("review.plan_required", "true")?;
                doc.set_str("review.reviewer", "none")?;
                doc.set_str("review.accept", "owner")
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
        let source = root.path().join("plan.md");
        let snapshot = store.dir().join("artifacts/owner-plan.md");
        fs::write(&source, PLAN).unwrap();
        fs::write(&snapshot, PLAN).unwrap();
        // Existing owner-only plan-policy tuple: no planner and no reviewer profile.
        let policy_digest = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    true,
                    false,
                    Accept::Owner,
                    false,
                    None::<()>,
                    "owner",
                    None::<()>,
                ))
                .unwrap()
            )
        );
        store
            .update(&task.id, |task| {
                task.plans.push(PlanRecord {
                    id: "P1".into(),
                    artifact: Artifact {
                        tier_override: None,
                        compressed_bytes: None,
                        purged_at: None,
                        run: "D1".into(),
                        kind: ArtifactKind::Plan,
                        title: "Owner plan".into(),
                        path: "artifacts/owner-plan.md".into(),
                        source: source.to_string_lossy().into_owned(),
                        sha256: format!("{:x}", Sha256::digest(PLAN.as_bytes())),
                        bytes: u64::try_from(PLAN.len()).unwrap(),
                        added: clock(),
                    },
                    spec_revision: corral_core::plan_contract_revision(task).unwrap(),
                    document: PlanDocument::parse(PLAN).unwrap(),
                    reviews: vec![PlanReview {
                        delivery: corral_core::inbox::DeliveryMode::Legacy,
                        inbox: Default::default(),
                        route: None,
                        id: "PR1".into(),
                        reviewer: "owner".into(),
                        policy_digest: policy_digest.clone(),
                        verdict: Some(Verdict::Approve),
                        notes: Some(NOTES.into()),
                        at: clock(),
                        herdr: None,
                        pending_reply: None,
                    }],
                    run: None,
                    approval: Some(PlanApproval {
                        authorization: None,
                        by: "owner".into(),
                        at: clock(),
                        policy_digest: policy_digest.clone(),
                    }),
                });
                task.plan = Some(format!("{}@P1", task.id));
                Ok(())
            })
            .unwrap();
        (root, store, task.id)
    }

    fn grant(store: &Store, id: &str) {
        let preview = store
            .preview_operating_policy(&OperatingSelection {
                tasks: BTreeSet::from([id.into()]),
                authority: OperatingAuthority {
                    dispatch: true,
                    accept: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        store
            .confirm_operating_policy(&preview, "owner", clock())
            .unwrap();
    }

    #[test]
    fn resolved_owner_plan_decisions_allow_freshly_scoped_dispatch_after_reload() {
        // Given an accepted single-task plan retaining its immutable questions and owner answers.
        let (root, store, id) = fixture();
        let questions = store.get(&id).unwrap().plans[0]
            .document
            .owner_decisions
            .clone();
        grant(&store, &id);
        // When a new consumer resolves the fresh operating grant.
        let reopened = Store::open(root.path()).unwrap();
        let effective = reopened.effective_operating_policy().unwrap();
        // Then implementation is authorized without deleting questions or waiving owner acceptance.
        assert_eq!(effective.mode, OperatingMode::Autopilot);
        assert!(
            effective.allows(&id, OperatingActionKind::Dispatch),
            "{:?}",
            effective.tasks[&id].refusals
        );
        assert!(!effective.allows(&id, OperatingActionKind::Accept));
        assert_eq!(
            reopened.get(&id).unwrap().plans[0].document.owner_decisions,
            questions
        );
    }

    #[test]
    fn unresolved_or_invalid_plan_resolution_evidence_remains_blocking() {
        for invalid in [
            "unanswered",
            "incomplete",
            "empty-answer",
            "nonowner-review",
            "nonowner-approval",
            "changed-verdict",
            "stale-spec",
            "stale-policy",
            "changed-policy",
            "mismatched-policy",
            "detached",
            "changed-source",
            "changed-snapshot",
            "replaced-review",
            "presentation-only",
        ] {
            // Given an otherwise valid owner-approved plan with one invalid resolution input.
            let (root, store, id) = fixture();
            store
                .update(&id, |task| {
                    let plan = &mut task.plans[0];
                    match invalid {
                        "unanswered" | "presentation-only" => {
                            plan.approval = None;
                            plan.reviews[0].notes = None;
                        }
                        "incomplete" => {
                            plan.reviews[0].notes =
                                Some("Resolved: Choose storage => SQLite".into())
                        }
                        "empty-answer" => {
                            plan.reviews[0].notes = Some(
                                "Resolved: Choose storage => SQLite\nResolved: Choose export => "
                                    .into(),
                            )
                        }
                        "nonowner-review" => plan.reviews[0].reviewer = "pm".into(),
                        "nonowner-approval" => plan.approval.as_mut().unwrap().by = "pm".into(),
                        "changed-verdict" => plan.reviews[0].verdict = Some(Verdict::Changes),
                        "stale-spec" => task.body.push_str("Changed specification\n"),
                        "stale-policy" => {
                            plan.reviews[0].policy_digest = "old-policy".into();
                            plan.approval.as_mut().unwrap().policy_digest = "old-policy".into();
                        }
                        "mismatched-policy" => {
                            plan.reviews[0].policy_digest = "different-policy".into()
                        }
                        "changed-policy" => {}
                        "detached" => task.plan = None,
                        "changed-source" => fs::write(&plan.artifact.source, "changed source")?,
                        "changed-snapshot" => {
                            fs::write(store.dir().join(&plan.artifact.path), "changed snapshot")?
                        }
                        "replaced-review" => {
                            let mut pending = plan.reviews[0].clone();
                            pending.id = "PR2".into();
                            pending.verdict = None;
                            plan.reviews.push(pending);
                        }
                        _ => unreachable!(),
                    }
                    Ok(())
                })
                .unwrap();
            if invalid == "changed-policy" {
                store
                    .edit_config(|doc| doc.set_str("review.reviewer", "omo"))
                    .unwrap();
            }
            if invalid == "presentation-only" {
                store
                    .record_operating_presentation(
                        "plan-question",
                        OperatingPresentation {
                            request_digest: "Choose storage".into(),
                            delivery: "presented".into(),
                            confirmed: true,
                            at: clock(),
                        },
                    )
                    .unwrap();
            }
            grant(&store, &id);
            // When effective authority is recomputed from disk, not from a presentation or old approval.
            let effective = Store::open(root.path())
                .unwrap()
                .effective_operating_policy()
                .unwrap();
            // Then even a fresh scope grant cannot turn invalid resolution evidence into consent.
            assert_eq!(effective.mode, OperatingMode::Autopilot);
            assert!(
                !effective.allows(&id, OperatingActionKind::Dispatch),
                "{invalid}"
            );
        }
    }
}

#[test]
fn default_policy_is_the_manual_flow() {
    let p = ReviewPolicy::default();
    assert!(p.gate.is_empty());
    assert_eq!(p.reviewer, Reviewer::None);
    assert!(!p.require_cross_family);
    assert_eq!(p.accept, Accept::Owner);
    assert_eq!(p.max_rounds, 2);
    assert_eq!(p.merge, MergeMode::Rebase);
    assert!(p.remove_worktree);
    assert!(p.owner_paths.is_empty());

    let c = Config::default_for(Path::new("/nonexistent"));
    assert_eq!(c.review_policy(None, None), ReviewPolicy::default());
    let serialized: toml::Table = c.to_toml().parse().unwrap();
    assert!(!serialized.contains_key("review"), "{}", c.to_toml());
    c.validate_review().unwrap();
}

#[test]
fn cross_family_policy_inherits_and_explicit_off_preserves_review_requirements() {
    // Given an existing configured review and an opt-in global policy with a project opt-out.
    let c = config(
        r#"
        [review]
        reviewer = "pm"
        gate = ["cargo test"]
        accept = "owner"
        plan_accept = "owner"
        require_cross_family = true
        [projects.app]
        path = "."
        [projects.app.review]
        require_cross_family = false
        "#,
    );
    // When each layer resolves and round-trips.
    let global = c.review_policy(None, None);
    let project = c.review_policy(Some("app"), None);
    let again: Config = toml::from_str(&c.to_toml()).unwrap();
    // Then only reviewer diversity changes, not gates, reviewers or owner acceptance.
    assert!(global.cross_family_required());
    assert!(!project.cross_family_required());
    assert_eq!(project.reviewer, global.reviewer);
    assert_eq!(project.gate, global.gate);
    assert_eq!(project.accept, Accept::Owner);
    assert_eq!(project.plan_accept, Accept::Owner);
    assert_eq!(again, c);
    assert_eq!(
        c.review_policy_sources(None, None).require_cross_family,
        PolicySource::Global
    );
    assert_eq!(
        c.review_policy_sources(Some("app"), None)
            .require_cross_family,
        PolicySource::Project
    );
    assert_eq!(
        config("")
            .review_policy_sources(None, None)
            .require_cross_family,
        PolicySource::Default
    );
}

#[test]
fn family_checks_apply_only_to_explicit_diversity_requirements() {
    for setting in [
        "",
        "require_cross_family = false",
        "require_cross_family = true",
    ] {
        // Given default, explicit off, and explicit on settings.
        let c = config(&format!("[review]\n{setting}"));
        let p = c.review_policy(None, None);
        // When validating same-family, cross-family, and unknown identities.
        let same = p.check_reviewer_family(Some(&"openai".into()), Some(&"openai".into()));
        let different = p.check_reviewer_family(Some(&"openai".into()), Some(&"anthropic".into()));
        let unknown = p.check_reviewer_family(None, None);
        // Then only explicit opt-in requires known and distinct families.
        assert_eq!(same.is_err(), p.require_cross_family);
        assert!(different.is_ok());
        assert_eq!(unknown.is_err(), p.require_cross_family);
    }
    let p = config("[review]\nreviewer = 'cross-vendor'\nrequire_cross_family = false")
        .review_policy(None, None);
    assert!(p.cross_family_required());
    assert!(
        p.check_reviewer_family(Some(&"openai".into()), Some(&"openai".into()))
            .is_err()
    );
}

#[test]
fn policy_resolves_global_then_project_then_task() {
    let c = config(
        r#"
        [review]
        gate = ["cargo test"]
        reviewer = "pm"
        accept = "auto"
        max_rounds = 3

        [projects.app]
        path = "app"
        [projects.app.review]
        gate = ["just check"]
        accept = "owner"
        merge = "ff"
        owner_paths = [".github/**"]

        [projects.lib]
        path = "lib"
        "#,
    );
    let global = c.review_policy(Some("lib"), None);
    assert_eq!(global.gate, vec!["cargo test"]);
    assert_eq!(global.reviewer, Reviewer::Pm);
    assert_eq!(global.accept, Accept::Auto);
    assert_eq!(global.max_rounds, 3);
    assert_eq!(global.merge, MergeMode::Rebase);

    let project = c.review_policy(Some("app"), None);
    assert_eq!(project.gate, vec!["just check"]);
    assert_eq!(project.reviewer, Reviewer::Pm);
    assert_eq!(project.accept, Accept::Owner);
    assert_eq!(project.max_rounds, 3);
    assert_eq!(project.merge, MergeMode::Ff);
    assert_eq!(project.owner_paths, vec![".github/**"]);

    let task = c.review_policy(Some("app"), Some(Accept::Auto));
    assert_eq!(task.accept, Accept::Auto);
    assert_eq!(task.gate, vec!["just check"]);
    assert_eq!(c.review_policy(None, None).gate, vec!["cargo test"]);

    let s = c.review_policy_sources(Some("app"), Some(Accept::Auto));
    assert_eq!(s.gate, PolicySource::Project);
    assert_eq!(s.reviewer, PolicySource::Global);
    assert_eq!(s.accept, PolicySource::Task);
    assert_eq!(s.max_rounds, PolicySource::Global);
    assert_eq!(s.merge, PolicySource::Project);
    assert_eq!(s.owner_paths, PolicySource::Project);
    assert_eq!(s.autopilot, PolicySource::Default);
    let s = c.review_policy_sources(Some("lib"), None);
    assert_eq!(
        (s.gate, s.accept, s.merge),
        (
            PolicySource::Global,
            PolicySource::Global,
            PolicySource::Default
        )
    );
    assert_eq!(
        PolicySource::Project.label(Some("app")),
        "[projects.app.review]"
    );
}

#[test]
fn review_config_round_trips_through_toml() {
    let c = config(
        r#"
        [review]
        gate = ["true"]
        [projects.app]
        path = "app"
        [projects.app.review]
        accept = "auto"
        "#,
    );
    let again: Config = toml::from_str(&c.to_toml()).unwrap();
    assert_eq!(again, c);
}

#[test]
fn validation_rejects_unknown_reviewers_bad_globs_and_empty_commands() {
    let c = config(
        r#"
        [profiles.deep]
        harness = "claude"

        [review]
        reviewer = "nobody"
        owner_paths = ["/abs/**", "src/a**b", "ok/**/*.toml"]

        [projects.app]
        path = "app"
        [projects.app.review]
        reviewer = "deep"
        gate = ["  "]
        owner_paths = ["x/{a,b}"]
        "#,
    );
    let err = format!("{:#}", c.validate_review().unwrap_err());
    assert!(
        err.contains("[review]: unknown reviewer profile 'nobody'"),
        "{err}"
    );
    assert!(err.contains("\"/abs/**\" must be relative"), "{err}");
    assert!(err.contains("'**' must be a whole path segment"), "{err}");
    assert!(!err.contains("ok/**/*.toml"), "{err}");
    assert!(!err.contains("'deep'"), "{err}");
    assert!(
        err.contains("[projects.app.review]: gate has an empty command"),
        "{err}"
    );
    assert!(err.contains("braces are not supported"), "{err}");

    let ok = config(
        r#"
        [profiles.deep]
        harness = "claude"
        [review]
        reviewer = "deep"
        owner_paths = [".github/**", "**/Cargo.toml"]
        "#,
    );
    ok.validate_review().unwrap();
    for bad in ["", "a//b", "../x", "a/[ab]"] {
        assert!(validate_glob(bad).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn autopilot_switch_defaults_on_and_is_workspace_wide() {
    assert!(config("").autopilot_enabled());
    assert!(!config("[review]\nautopilot = false\n").autopilot_enabled());
    let per_project =
        config("[projects.app]\npath = \"app\"\n[projects.app.review]\nautopilot = false\n");
    let err = format!("{:#}", per_project.validate_review().unwrap_err());
    assert!(
        err.contains("[projects.app.review]: autopilot is workspace-wide"),
        "{err}"
    );
}

#[test]
fn store_set_autopilot_edits_corral_toml_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    let file = store.dir().join("corral.toml");
    let text = format!(
        "# keep me\n{}\n[review] # policy\naccept = \"owner\" # the owner lands\n",
        std::fs::read_to_string(&file).unwrap()
    );
    std::fs::write(&file, &text).unwrap();

    store.set_autopilot(false).unwrap();
    assert!(!store.config.autopilot_enabled());
    let saved = std::fs::read_to_string(&file).unwrap();
    assert!(
        saved.starts_with("# keep me\n")
            && saved.contains(
                "[review] # policy\naccept = \"owner\" # the owner lands\nautopilot = false\n"
            ),
        "{saved}"
    );
    assert!(!Store::open(tmp.path()).unwrap().config.autopilot_enabled());

    store.set_autopilot(true).unwrap();
    assert!(Store::open(tmp.path()).unwrap().config.autopilot_enabled());
}

#[test]
fn unknown_review_keys_are_parse_errors() {
    let err = toml::from_str::<Config>("[review]\naccpet = \"auto\"\n").unwrap_err();
    assert!(err.to_string().contains("accpet"), "{err}");
    assert!(toml::from_str::<Config>("[review]\naccept = \"maybe\"\n").is_err());
}

#[test]
fn task_review_frontmatter_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let t = store.create("Auto task", None, Status::Ready, "").unwrap();
    assert_eq!(t.review, None);
    let text = std::fs::read_to_string(&t.path).unwrap();
    assert!(!text.contains("review:"), "{text}");
    store
        .update(&t.id, |t| {
            t.review = Some(Accept::Auto);
            Ok(())
        })
        .unwrap();
    let text = std::fs::read_to_string(&t.path).unwrap();
    assert!(text.contains("review: auto"), "{text}");
    assert_eq!(store.get(&t.id).unwrap().review, Some(Accept::Auto));
}
