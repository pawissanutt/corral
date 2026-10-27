use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use corral_core::{
    Accept, ConfigLayer, ConfigLayers, ConfigSource, OperatingActionKind, OperatingAuthority,
    OperatingMode, OperatingPresentation, OperatingSelection, PolicySource, Status, Store, Task,
};
use jiff::Timestamp;
use tempfile::TempDir;

#[path = "operating/plans.rs"]
mod plans;
#[path = "operating/projects.rs"]
mod projects;
#[path = "operating/publication.rs"]
mod publication;

struct Fixture {
    root: TempDir,
    store: Store,
    task: Task,
}

fn clock() -> Timestamp {
    "2026-10-18T03:00:00Z".parse().unwrap()
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("app/.git")).unwrap();
        let store = Store::init(root.path()).unwrap();
        let task = store
            .create("Included", Some("app"), Status::Backlog, "Agreed work\n")
            .unwrap();
        Self { root, store, task }
    }

    fn selection(&self) -> OperatingSelection {
        OperatingSelection {
            tasks: BTreeSet::from([self.task.id.clone()]),
            authority: OperatingAuthority {
                dispatch: true,
                accept: true,
                plan_accept: true,
                aggregate_accept: false,
            },
            ..OperatingSelection::default()
        }
    }

    fn grant(&self) {
        let preview = self
            .store
            .preview_operating_policy(&self.selection())
            .unwrap();
        self.store
            .confirm_operating_policy(&preview, "owner", clock())
            .unwrap();
    }
}

#[test]
fn every_legacy_combination_loads_as_assist_without_rewriting_config() {
    // Given every legacy switch combination, including apparently permissive defaults.
    let fixture = Fixture::new();
    for autopilot in [None, Some(false), Some(true)] {
        for accept in [None, Some("owner"), Some("auto")] {
            for paused in [false, true] {
                for backlog in [false, true] {
                    for ideas in ["never", "suggest", "auto"] {
                        let mut text = format!(
                            "[profiles.omo]\nharness = 'generic'\n[harnesses.generic]\ncommand = 'true'\n[projects.app]\npath = 'app'\n[dispatch]\npaused = {paused}\nauto_backlog = {backlog}\npromote_ideas = '{ideas}'\n[review]\n"
                        );
                        if let Some(value) = autopilot {
                            text.push_str(&format!("autopilot = {value}\n"));
                        }
                        if let Some(value) = accept {
                            text.push_str(&format!(
                                "accept = '{value}'\nplan_accept = '{value}'\n"
                            ));
                        }
                        fs::write(fixture.store.config_path(), &text).unwrap();
                        // When a fresh consumer loads the legacy configuration.
                        let store = Store::open(fixture.root.path()).unwrap();
                        let effective = store.effective_operating_policy().unwrap();
                        // Then neither a legacy flag nor available capacity grants authority.
                        assert_eq!(effective.mode, OperatingMode::Assist);
                        assert!(effective.authorization.is_none());
                        assert!(!effective.allows(&fixture.task.id, OperatingActionKind::Dispatch));
                        assert_eq!(store.config.review.autopilot, autopilot);
                        assert_eq!(store.config.dispatch.paused, paused);
                        assert_eq!(store.config.dispatch.auto_backlog, backlog);
                        assert_eq!(fs::read_to_string(store.config_path()).unwrap(), text);
                    }
                }
            }
        }
    }
}

#[test]
fn project_preview_is_finite_and_does_not_include_ideas_or_future_work() {
    // Given project Backlog, Ready and Idea tasks.
    let fixture = Fixture::new();
    let ready = fixture
        .store
        .create("Ready", Some("app"), Status::Ready, "Ready\n")
        .unwrap();
    let idea = fixture
        .store
        .create("Idea", Some("app"), Status::Idea, "Idea\n")
        .unwrap();
    let selection = OperatingSelection {
        projects: BTreeSet::from(["app".into()]),
        authority: fixture.selection().authority,
        ..OperatingSelection::default()
    };
    let preview = fixture.store.preview_operating_policy(&selection).unwrap();
    assert_eq!(
        preview
            .policy
            .scope
            .tasks
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([fixture.task.id.clone(), ready.id])
    );
    // When the owner confirms that preview and another task is subsequently filed.
    fixture
        .store
        .confirm_operating_policy(&preview, "owner", clock())
        .unwrap();
    let future = fixture
        .store
        .create("Later", Some("app"), Status::Backlog, "Later\n")
        .unwrap();
    // Then the manifest is not a subscription.
    let policy = fixture.store.effective_operating_policy().unwrap();
    assert!(policy.allows(&fixture.task.id, OperatingActionKind::Dispatch));
    assert!(!policy.allows(&idea.id, OperatingActionKind::Dispatch));
    assert!(!policy.allows(&future.id, OperatingActionKind::Dispatch));
}

#[test]
fn empty_unknown_missing_and_excluded_scopes_cannot_be_confirmed() {
    // Given a valid workspace and explicitly unusable selections.
    let fixture = Fixture::new();
    let invalid = [
        OperatingSelection::default(),
        OperatingSelection {
            projects: BTreeSet::from(["unknown".into()]),
            ..fixture.selection()
        },
        OperatingSelection {
            tasks: BTreeSet::from(["T-999".into()]),
            ..fixture.selection()
        },
        OperatingSelection {
            exclusions: BTreeSet::from([fixture.task.id.clone()]),
            ..fixture.selection()
        },
    ];
    // When these selections are previewed.
    for selection in invalid {
        assert!(fixture.store.preview_operating_policy(&selection).is_err());
    }
    // Then no partial approval exists.
    assert!(fixture.store.operating_state().unwrap().approval.is_none());
}

#[test]
fn exclusions_are_explicit_and_never_gain_permission() {
    // Given a project selection with an explicitly excluded member.
    let fixture = Fixture::new();
    let excluded = fixture
        .store
        .create("Excluded", Some("app"), Status::Ready, "Excluded\n")
        .unwrap();
    let selection = OperatingSelection {
        projects: BTreeSet::from(["app".into()]),
        exclusions: BTreeSet::from([excluded.id.clone()]),
        ..fixture.selection()
    };
    let preview = fixture.store.preview_operating_policy(&selection).unwrap();
    // When that finite scope is confirmed.
    fixture
        .store
        .confirm_operating_policy(&preview, "owner", clock())
        .unwrap();
    // Then exclusions survive reload and remain unauthorized.
    let reopened = Store::open(fixture.root.path()).unwrap();
    assert!(
        reopened
            .config
            .automation
            .scope
            .exclusions
            .contains(&excluded.id)
    );
    assert!(
        !reopened
            .effective_operating_policy()
            .unwrap()
            .allows(&excluded.id, OperatingActionKind::Dispatch)
    );
}

#[test]
fn changed_task_contract_and_missing_member_fail_closed() {
    // Given two included tasks.
    let fixture = Fixture::new();
    let other = fixture
        .store
        .create("Other", Some("app"), Status::Ready, "Other\n")
        .unwrap();
    let selection = OperatingSelection {
        tasks: BTreeSet::from([fixture.task.id.clone(), other.id.clone()]),
        ..fixture.selection()
    };
    let preview = fixture.store.preview_operating_policy(&selection).unwrap();
    fixture
        .store
        .confirm_operating_policy(&preview, "owner", clock())
        .unwrap();
    // When one contract changes and the other disappears.
    fixture
        .store
        .update(&fixture.task.id, |task| {
            task.body.push_str("New requirement\n");
            Ok(())
        })
        .unwrap();
    fs::remove_file(other.path).unwrap();
    // Then neither old membership authorizes work.
    let effective = fixture.store.effective_operating_policy().unwrap();
    assert!(!effective.allows(&fixture.task.id, OperatingActionKind::Dispatch));
    assert!(!effective.allows(&other.id, OperatingActionKind::Dispatch));
    assert_eq!(
        effective.tasks.len(),
        2,
        "missing obligations remain inspectable"
    );
}

#[test]
fn replaced_repository_at_the_same_path_invalidates_membership() {
    // Given a confirmed repository identity.
    let fixture = Fixture::new();
    fixture.grant();
    // When the repository marker is replaced without changing the configured path.
    fs::rename(
        fixture.root.path().join("app/.git"),
        fixture.root.path().join("app/.git-old"),
    )
    .unwrap();
    fs::create_dir(fixture.root.path().join("app/.git")).unwrap();
    // Then the old grant cannot authorize the replacement repository.
    assert!(
        !fixture
            .store
            .effective_operating_policy()
            .unwrap()
            .allows(&fixture.task.id, OperatingActionKind::Dispatch)
    );
}

#[test]
fn changing_a_worktree_repository_pointer_invalidates_membership() {
    // Given a repository whose .git marker is a regular worktree pointer.
    let fixture = Fixture::new();
    let marker = fixture.root.path().join("app/.git");
    fs::remove_dir(&marker).unwrap();
    fs::write(&marker, "gitdir: ../original\n").unwrap();
    fixture.grant();
    // When its target changes in place without replacing the marker inode.
    fs::write(&marker, "gitdir: ../replaced\n").unwrap();
    // Then the original repository identity is no longer authorized.
    assert!(
        !fixture
            .store
            .effective_operating_policy()
            .unwrap()
            .allows(&fixture.task.id, OperatingActionKind::Dispatch)
    );
}

#[test]
fn changed_project_assignment_and_write_scope_invalidate_membership() {
    for change_project in [false, true] {
        // Given an exact task contract with two configured projects.
        let fixture = Fixture::new();
        fs::create_dir(fixture.root.path().join("other")).unwrap();
        fixture
            .store
            .edit_config(|doc| doc.set_str("projects.other.path", "other"))
            .unwrap();
        fixture.grant();
        // When the task moves project or widens its write scope.
        fixture
            .store
            .update(&fixture.task.id, |task| {
                if change_project {
                    task.project = Some("other".into());
                } else {
                    task.write_scope.push("**".into());
                }
                Ok(())
            })
            .unwrap();
        // Then the original member cannot gain authority.
        assert!(
            !fixture
                .store
                .effective_operating_policy()
                .unwrap()
                .allows(&fixture.task.id, OperatingActionKind::Dispatch)
        );
    }
}

#[test]
fn builtin_owner_default_is_distinct_from_every_explicit_owner_exception() {
    // Given a separately configured gate/reviewer and delegated acceptance.
    let fixture = Fixture::new();
    fixture
        .store
        .edit_config(|doc| {
            doc.set_str("review.gate", "[\"true\"]")?;
            doc.set_str("review.reviewer", "omo")
        })
        .unwrap();
    fixture.grant();
    let policy = fixture.store.effective_operating_policy().unwrap();
    assert!(policy.allows(&fixture.task.id, OperatingActionKind::Accept));
    assert!(policy.allows(&fixture.task.id, OperatingActionKind::AcceptPlan));
    assert_eq!(
        policy.tasks[&fixture.task.id].acceptance_source,
        PolicySource::Default
    );
    // When owner-only acceptance is explicit in the workspace, even below a task Auto override.
    fixture
        .store
        .edit_config(|doc| {
            doc.set_str("review.accept", "owner")?;
            doc.set_str("review.plan_accept", "owner")
        })
        .unwrap();
    fixture
        .store
        .update(&fixture.task.id, |task| {
            task.review = Some(Accept::Auto);
            Ok(())
        })
        .unwrap();
    // Then checking remains permitted, but neither acceptance is delegated.
    let policy = fixture.store.effective_operating_policy().unwrap();
    assert!(policy.allows(&fixture.task.id, OperatingActionKind::Gate));
    assert!(!policy.allows(&fixture.task.id, OperatingActionKind::Accept));
    assert!(!policy.allows(&fixture.task.id, OperatingActionKind::AcceptPlan));
    assert_eq!(
        policy.tasks[&fixture.task.id].acceptance_source,
        PolicySource::Task
    );
    assert_eq!(
        policy.tasks[&fixture.task.id].plan_acceptance_source,
        PolicySource::Global
    );
}

#[test]
fn explicit_task_and_project_owner_restrictions_remain_narrowing() {
    for layer in ["task", "project"] {
        // Given delegated acceptance and valid verification configuration.
        let fixture = Fixture::new();
        fixture
            .store
            .edit_config(|doc| {
                doc.set_str("review.gate", "[\"true\"]")?;
                doc.set_str("review.reviewer", "omo")?;
                doc.set_str("review.accept", "auto")
            })
            .unwrap();
        fixture.grant();
        // When an explicit lower scope requires owner acceptance.
        if layer == "task" {
            fixture
                .store
                .update(&fixture.task.id, |task| {
                    task.review = Some(Accept::Owner);
                    Ok(())
                })
                .unwrap();
        } else {
            fixture
                .store
                .edit_config(|doc| doc.set_str("projects.app.review.accept", "owner"))
                .unwrap();
        }
        // Then automatic acceptance is refused.
        assert!(
            !fixture
                .store
                .effective_operating_policy()
                .unwrap()
                .allows(&fixture.task.id, OperatingActionKind::Accept)
        );
    }
}

#[test]
fn legacy_false_and_dispatch_restrictions_never_expand_a_grant() {
    for restriction in [
        "review.autopilot",
        "dispatch.paused",
        "dispatch.auto_backlog",
    ] {
        // Given a valid scoped dispatch grant.
        let fixture = Fixture::new();
        fixture.grant();
        // When an explicit legacy restriction is configured.
        fixture
            .store
            .edit_config(|doc| {
                doc.set_str(
                    restriction,
                    if restriction == "dispatch.paused" {
                        "true"
                    } else {
                        "false"
                    },
                )
            })
            .unwrap();
        // Then the grant does not erase it.
        assert!(
            !fixture
                .store
                .effective_operating_policy()
                .unwrap()
                .allows(&fixture.task.id, OperatingActionKind::Dispatch)
        );
    }
}

#[test]
fn owner_attention_and_holds_precede_automatic_work() {
    // Given included work.
    let fixture = Fixture::new();
    fixture.grant();
    // When the owner holds automatic work.
    fixture
        .store
        .update(&fixture.task.id, |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let policy = fixture.store.effective_operating_policy().unwrap();
    for action in [
        OperatingActionKind::Dispatch,
        OperatingActionKind::Plan,
        OperatingActionKind::Gate,
        OperatingActionKind::Review,
        OperatingActionKind::PlanReview,
        OperatingActionKind::Repair,
        OperatingActionKind::Accept,
        OperatingActionKind::AcceptPlan,
    ] {
        assert!(!policy.allows(&fixture.task.id, action), "{action:?}");
    }
    // When the task needs an unclassified owner disposition.
    fixture
        .store
        .update(&fixture.task.id, |task| {
            task.status = Status::NeedsMe;
            Ok(())
        })
        .unwrap();
    // Then no mechanical gate or repair is inferred from capacity or stale results.
    let policy = fixture.store.effective_operating_policy().unwrap();
    assert!(!policy.allows(&fixture.task.id, OperatingActionKind::Gate));
    assert!(!policy.allows(&fixture.task.id, OperatingActionKind::Repair));
}

#[test]
fn unrelated_edits_survive_confirmation_but_relevant_changes_make_it_stale() {
    // Given a preview on a comment-bearing workspace.
    let fixture = Fixture::new();
    let text = fs::read_to_string(fixture.store.config_path()).unwrap();
    fs::write(
        fixture.store.config_path(),
        format!("# retained owner notes\n{text}"),
    )
    .unwrap();
    let preview = fixture
        .store
        .preview_operating_policy(&fixture.selection())
        .unwrap();
    // When another context changes an unrelated setting before confirmation.
    let other = Store::open(fixture.root.path()).unwrap();
    other
        .edit_config(|doc| doc.set_str("ui.done_max", "19"))
        .unwrap();
    fixture
        .store
        .confirm_operating_policy(&preview, "owner", clock())
        .unwrap();
    // Then confirmation preserves that edit and the comment.
    let config = fixture.store.config_layers().unwrap().config().unwrap();
    assert_eq!(config.ui.done_max, 19);
    assert!(
        fs::read_to_string(fixture.store.config_path())
            .unwrap()
            .starts_with("# retained owner notes\n")
    );
    let stale = fixture
        .store
        .preview_operating_policy(&fixture.selection())
        .unwrap();
    other
        .edit_config(|doc| doc.set_str("review.accept", "owner"))
        .unwrap();
    assert!(
        fixture
            .store
            .confirm_operating_policy(&stale, "owner", clock())
            .is_err()
    );
}

#[test]
fn stale_task_owner_default_presence_and_project_membership_reject_confirmation() {
    for change in ["contract", "owner", "membership", "policy"] {
        // Given a pending confirmation for the exact project membership.
        let fixture = Fixture::new();
        let selection = OperatingSelection {
            projects: BTreeSet::from(["app".into()]),
            ..fixture.selection()
        };
        let preview = fixture.store.preview_operating_policy(&selection).unwrap();
        // When an input relevant to consent changes.
        match change {
            "contract" => {
                fixture
                    .store
                    .update(&fixture.task.id, |task| {
                        task.body.push_str("Changed\n");
                        Ok(())
                    })
                    .unwrap();
            }
            "owner" => {
                fixture
                    .store
                    .update(&fixture.task.id, |task| {
                        task.review = Some(Accept::Owner);
                        Ok(())
                    })
                    .unwrap();
            }
            "membership" => {
                fixture
                    .store
                    .create("New", Some("app"), Status::Ready, "New\n")
                    .unwrap();
            }
            "policy" => {
                fixture.store.pause_operating_policy(true).unwrap();
            }
            _ => unreachable!(),
        }
        // Then the old preview cannot publish different authority.
        assert!(
            fixture
                .store
                .confirm_operating_policy(&preview, "owner", clock())
                .is_err(),
            "{change}"
        );
        assert!(fixture.store.operating_state().unwrap().approval.is_none());
    }
}

#[test]
fn approval_and_uncertain_presentation_survive_reload_and_scope_amendment() {
    // Given an unresolved presentation receipt, independent from approval.
    let fixture = Fixture::new();
    let receipt = OperatingPresentation {
        request_digest: "task-run-full-request".into(),
        delivery: "delivery-1".into(),
        confirmed: false,
        at: clock(),
    };
    fixture
        .store
        .record_operating_presentation("decision-1", receipt.clone())
        .unwrap();
    // When the scope is explicitly confirmed and then reopened.
    fixture.grant();
    let reopened = Store::open(fixture.root.path()).unwrap();
    let state = reopened.operating_state().unwrap();
    // Then the approval and separate presentation remain intact; loading does not acknowledge it.
    assert!(state.approval.is_some());
    assert_eq!(state.presentations["decision-1"], receipt);
    reopened
        .record_operating_presentation(
            "decision-1",
            OperatingPresentation {
                confirmed: true,
                ..receipt
            },
        )
        .unwrap();
    fixture.grant();
    assert!(fixture.store.operating_state().unwrap().presentations["decision-1"].confirmed);
}

#[test]
fn personal_configuration_cannot_supply_or_override_workspace_authority() {
    // Given an explicit personal layer attempting to grant authority.
    let fixture = Fixture::new();
    let personal = fixture.root.path().join("personal.toml");
    fs::write(
        &personal,
        "[automation]\nmode = 'autopilot'\n[automation.authority]\ndispatch = true\n",
    )
    .unwrap();
    // When layers are decoded, even beneath an overriding workspace value.
    let layers = ConfigLayers::load_paths(&fixture.store.config_path(), Some(&personal)).unwrap();
    // Then the forbidden layer is rejected.
    assert!(layers.config().is_err());
    assert!(
        fixture
            .store
            .edit_config_layer_from(ConfigLayer::User, ConfigSource::Cli, |doc| doc
                .set_str("automation.mode", "autopilot"))
            .is_err()
    );
    assert_eq!(
        fixture.store.effective_operating_policy().unwrap().mode,
        OperatingMode::Assist
    );
}

#[test]
fn malformed_fresh_authority_never_uses_a_cached_broader_policy() {
    // Given a store with a previously valid grant in its cache.
    let mut fixture = Fixture::new();
    fixture.grant();
    fixture.store.reload_config_if_changed().unwrap();
    // When a raw writer corrupts authority and ordinary reload retains the old cache.
    let text = fs::read_to_string(fixture.store.config_path()).unwrap();
    fs::write(
        fixture.store.config_path(),
        format!("{text}\n[automation.unrecognized]\nenabled = true\n"),
    )
    .unwrap();
    assert!(!fixture.store.reload_config_if_changed().unwrap());
    assert_eq!(
        fixture.store.config.automation.mode,
        OperatingMode::Autopilot
    );
    // Then the strict authorization boundary refuses rather than using that cache.
    assert!(fixture.store.effective_operating_policy().is_err());
}

#[test]
fn pause_resume_retains_grant_but_assist_requires_new_confirmation() {
    // Given an active grant.
    let fixture = Fixture::new();
    fixture.grant();
    let original = fixture.store.operating_state().unwrap().approval.unwrap();
    // When paused and explicitly resumed.
    fixture.store.pause_operating_policy(true).unwrap();
    assert!(
        !fixture
            .store
            .effective_operating_policy()
            .unwrap()
            .allows(&fixture.task.id, OperatingActionKind::Dispatch)
    );
    fixture.store.pause_operating_policy(false).unwrap();
    assert!(
        fixture
            .store
            .effective_operating_policy()
            .unwrap()
            .allows(&fixture.task.id, OperatingActionKind::Dispatch)
    );
    assert_eq!(
        fixture.store.operating_state().unwrap().approval.unwrap(),
        original
    );
    // Then switching to Assist withdraws the identity, so resume is not activation.
    fixture.store.assist_operating_policy().unwrap();
    assert_eq!(
        fixture.store.effective_operating_policy().unwrap().mode,
        OperatingMode::Assist
    );
    assert!(fixture.store.pause_operating_policy(false).is_err());
}

#[test]
fn comments_and_normal_progress_do_not_change_scope_contract_identity() {
    // Given a task manifest.
    let fixture = Fixture::new();
    fixture.grant();
    // When only scheduling progress changes, not its specification or scope.
    fixture
        .store
        .update(&fixture.task.id, |task| {
            task.status = Status::Ready;
            task.priority = "P0".into();
            task.queued = true;
            Ok(())
        })
        .unwrap();
    // Then the existing finite grant remains usable without granting another member.
    assert!(
        fixture
            .store
            .effective_operating_policy()
            .unwrap()
            .allows(&fixture.task.id, OperatingActionKind::Dispatch)
    );
}

fn reviewed_plan(task: &Task) -> corral_core::PlanRecord {
    corral_core::PlanRecord {
        id: "P1".into(),
        artifact: corral_core::Artifact {
            tier_override: None,
            compressed_bytes: None,
            purged_at: None,
            run: "D1".into(),
            kind: corral_core::ArtifactKind::Plan,
            title: "Reviewed".into(),
            path: "plan.md".into(),
            source: "plan.md".into(),
            sha256: "a".repeat(64),
            bytes: 1,
            added: clock(),
        },
        spec_revision: corral_core::plan_contract_revision(task).unwrap(),
        document: corral_core::PlanDocument {
            delivery_target: None,
            human_review: None,
            goal: "Finite work".into(),
            non_goals: Vec::new(),
            decisions: Vec::new(),
            affected_paths: vec!["src/".into()],
            contracts: Vec::new(),
            dependencies: vec![corral_core::PlanScope {
                task: "named-child".into(),
                write_scope: vec!["src/child.rs".into()],
                depends_on: Vec::new(),
                acceptance: Vec::new(),
                gate: None,
            }],
            acceptance: vec![corral_core::PlanAcceptance {
                criterion: None,
                scenario: "Child works".into(),
                expected: "Verified".into(),
            }],
            verification: vec!["cargo test".into()],
            owner_decisions: Vec::new(),
        },
        reviews: vec![corral_core::PlanReview {
            delivery: corral_core::inbox::DeliveryMode::Legacy,
            inbox: Default::default(),
            route: None,
            id: "PR1".into(),
            reviewer: "omo".into(),
            policy_digest: "review-policy".into(),
            verdict: Some(corral_core::Verdict::Approve),
            notes: None,
            at: clock(),
            herdr: None,
            pending_reply: None,
        }],
        run: Some("D1".into()),
        approval: None,
    }
}

#[test]
fn reviewed_plan_scope_grants_bind_exact_revision_labels_and_document() {
    // Given a separately reviewed plan with one exact scope label.
    let fixture = Fixture::new();
    fixture
        .store
        .update(&fixture.task.id, |task| {
            task.plans.push(reviewed_plan(task));
            Ok(())
        })
        .unwrap();
    let selection = OperatingSelection {
        plan_scopes: BTreeMap::from([(
            fixture.task.id.clone(),
            BTreeSet::from(["named-child".into()]),
        )]),
        ..fixture.selection()
    };
    let preview = fixture.store.preview_operating_policy(&selection).unwrap();
    assert_eq!(preview.policy.scope.plans[0].revision, "P1");
    assert_eq!(preview.policy.scope.plans[0].scopes.len(), 1);
    fixture
        .store
        .confirm_operating_policy(&preview, "owner", clock())
        .unwrap();
    // When an unapproved plan child is filed and the reviewed scope itself changes.
    let child = fixture
        .store
        .create("New child", Some("app"), Status::Backlog, "New\n")
        .unwrap();
    fixture
        .store
        .update(&fixture.task.id, |task| {
            task.plans[0].document.dependencies[0]
                .write_scope
                .push("src/unreviewed.rs".into());
            Ok(())
        })
        .unwrap();
    // Then neither the changed parent authorization nor automatic descendant admission is valid.
    let effective = fixture.store.effective_operating_policy().unwrap();
    assert!(!effective.allows(&fixture.task.id, OperatingActionKind::AcceptPlan));
    assert!(!effective.allows(&child.id, OperatingActionKind::Dispatch));
}

#[test]
fn unknown_and_unreviewed_plan_scopes_cannot_receive_a_grant() {
    for invalid in ["unknown", "unreviewed", "stale"] {
        // Given a plan with one deliberately invalid consent input.
        let fixture = Fixture::new();
        fixture
            .store
            .update(&fixture.task.id, |task| {
                let mut plan = reviewed_plan(task);
                if invalid == "unreviewed" {
                    plan.reviews.clear();
                }
                if invalid == "stale" {
                    plan.spec_revision = "old-spec".into();
                }
                task.plans.push(plan);
                Ok(())
            })
            .unwrap();
        let selection = OperatingSelection {
            plan_scopes: BTreeMap::from([(
                fixture.task.id.clone(),
                BTreeSet::from([if invalid == "unknown" {
                    "unknown"
                } else {
                    "named-child"
                }
                .into()]),
            )]),
            ..fixture.selection()
        };
        // When scope confirmation is prepared.
        assert!(
            fixture.store.preview_operating_policy(&selection).is_err(),
            "{invalid}"
        );
        // Then no grant can be published.
        assert!(fixture.store.operating_state().unwrap().approval.is_none());
    }
}
