use super::*;
use corral_core::{Task, operating_contract_revision};

fn reviewed(ctx: &Ctx, id: &str, path: &std::path::Path, scope: Option<&str>) -> String {
    let document = match scope {
        None => PLAN.into(),
        Some(name) => PLAN.replace(
            "dependencies: []",
            &format!(
                "dependencies:\n  - task: {name}\n    write_scope: [{name}/**]\n    depends_on: []\n    acceptance: []"
            ),
        ),
    };
    std::fs::write(path, document).unwrap();
    let plan = pending(ctx, id, path);
    plan_verdict(
        ctx,
        id,
        &PlanVerdict {
            plan: &plan,
            attempt: Some("PR1"),
            verdict: Verdict::Approve,
            notes: "Independent approval",
        },
    )
    .unwrap();
    plan
}

fn owner_accept(ctx: &Ctx, id: &str, plan: &str) {
    plan_verdict(
        ctx,
        id,
        &PlanVerdict {
            plan,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Owner accepts",
        },
    )
    .unwrap();
}

fn interrupted(
    initial: Option<&str>,
    replacement: Option<&str>,
    automatic: bool,
) -> (tempfile::TempDir, Ctx, Task, PathBuf) {
    let (root, ctx, id, path) = fixture();
    let first = reviewed(&ctx, &id, &path, initial);
    owner_accept(&ctx, &id, &first);
    let second = reviewed(&ctx, &id, &path, replacement);
    if automatic {
        ctx.store
            .update(&id, |task| {
                task.status = Status::Review;
                Ok(())
            })
            .unwrap();
        let preview = ctx
            .store
            .preview_operating_policy(&corral_core::OperatingSelection {
                tasks: [id.clone()].into(),
                plan_scopes: [(id.clone(), [replacement.unwrap().to_string()].into())].into(),
                authority: corral_core::OperatingAuthority {
                    dispatch: true,
                    accept: false,
                    plan_accept: true,
                    aggregate_accept: false,
                },
                ..Default::default()
            })
            .unwrap();
        ctx.store
            .confirm_operating_policy(&preview, "owner", clock())
            .unwrap();
    }
    let source = ctx.store.get(&id).unwrap();
    if automatic {
        corral_ops::sync_at(&ctx, clock()).unwrap();
    } else {
        owner_accept(&ctx, &id, &second);
    }
    let completed = ctx.store.get(&id).unwrap();
    assert_eq!(completed.plan.as_deref(), Some("T-001@P2"));
    assert_eq!(
        completed.plans[1].approval.as_ref().unwrap().by,
        if automatic { "reviewer" } else { "owner" }
    );
    let journal_path = ctx
        .store
        .state_dir()
        .join(format!("plan-decompose-{id}.json"));
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
    assert!(journal["children"].as_array().unwrap().is_empty());
    assert_eq!(
        journal["scopes"].as_object().unwrap().len(),
        usize::from(replacement.is_some())
    );
    assert_eq!(
        journal["source_contract"],
        operating_contract_revision(&source).unwrap()
    );

    // Reconstruct the durable prefix after journal rename, before the final
    // projected parent save. Acceptance and the exact new journal remain durable.
    ctx.store
        .update(&id, |task| {
            task.plan = source.plan.clone();
            task.write_scope = source.write_scope.clone();
            task.queued = source.queued;
            task.dispatch_plan = source.dispatch_plan.clone();
            Ok(())
        })
        .unwrap();
    let prefix = ctx.store.get(&id).unwrap();
    assert_eq!(
        journal["source_contract"],
        operating_contract_revision(&prefix).unwrap()
    );
    assert_eq!(prefix.plans[1].approval, completed.plans[1].approval);
    let mut fresh = ctx.clone();
    fresh.store = Store::open(root.path()).unwrap();
    (root, fresh, completed, journal_path)
}

fn clock() -> jiff::Timestamp {
    "2026-10-18T10:00:00Z".parse().unwrap()
}

fn sync_twice(ctx: &Ctx) {
    corral_ops::sync_at(ctx, clock()).unwrap();
    corral_ops::sync_at(ctx, clock()).unwrap();
}

fn assert_recovery(initial: Option<&str>, replacement: Option<&str>, automatic: bool) {
    // Given the exact durable interruption prefix from real accepted plans.
    let (_root, ctx, completed, journal) = interrupted(initial, replacement, automatic);
    let journal_bytes = std::fs::read(&journal).unwrap();
    let config = std::fs::read(ctx.store.config_path()).unwrap();

    // When a reopened store performs ordinary sync, then repeats it.
    sync_twice(&ctx);

    // Then the same approval finishes publication without another verdict or child.
    let after = ctx.store.get(&completed.id).unwrap();
    assert_eq!(
        after.plan, completed.plan,
        "accepted replacement must recover without another owner decision"
    );
    assert_eq!(after.write_scope, completed.write_scope);
    assert_eq!(after.plans[1].approval, completed.plans[1].approval);
    assert!(approved_plan(&ctx, &after).is_ok());
    assert!(after.children.is_empty() && after.runs.is_empty());
    assert!(!after.queued && after.dispatch_plan.is_none());
    assert_eq!(ctx.store.list().unwrap().len(), 1);
    assert_eq!(std::fs::read(journal).unwrap(), journal_bytes);
    assert_eq!(std::fs::read(ctx.store.config_path()).unwrap(), config);
}

#[test]
fn interrupted_zero_scope_replacement_control_recovers() {
    assert_recovery(None, None, false);
}

#[test]
fn interrupted_single_scope_replacement_of_zero_scope_recovers() {
    assert_recovery(None, Some("replacement"), false);
}

#[test]
fn interrupted_single_scope_replacement_of_single_scope_recovers() {
    assert_recovery(Some("original"), Some("replacement"), false);
}

#[test]
fn scoped_automatic_single_scope_replacement_recovers() {
    assert_recovery(None, Some("replacement"), true);
    assert_recovery(Some("original"), Some("replacement"), true);
}

#[test]
fn self_projection_restart_rejects_changed_publication_identities() {
    for initial in [None, Some("original")] {
        for automatic in [false, true] {
            for change in [
                "attachment",
                "source",
                "backlink",
                "artifact",
                "document",
                "projected",
                "children",
                "missing",
                "extra",
                "task",
                "digest",
                "contract",
            ] {
                // Given an interrupted accepted self-projection with one changed identity.
                let (_root, ctx, completed, path) =
                    interrupted(initial, Some("replacement"), automatic);
                let mut journal: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                match change {
                    "attachment" => {
                        ctx.store
                            .update(&completed.id, |task| {
                                task.plan = Some("T-999@P1".into());
                                Ok(())
                            })
                            .unwrap();
                    }
                    "source" => journal["source_contract"] = "foreign-source".into(),
                    "backlink" => journal["backlink"] = "T-999@P2".into(),
                    "artifact" => journal["artifact_sha256"] = "foreign-artifact".into(),
                    "document" => journal["document"]["goal"] = "Unreviewed plan".into(),
                    "projected" => journal["projected_contract"] = "foreign-projection".into(),
                    "children" => journal["children"] = serde_json::json!(["foreign-child"]),
                    "missing" => journal["scopes"] = serde_json::json!({}),
                    "extra" => {
                        journal["scopes"]["unknown"] = journal["scopes"]["replacement"].clone();
                    }
                    "task" => journal["scopes"]["replacement"]["task"] = "T-999".into(),
                    "digest" => {
                        journal["scopes"]["replacement"]["scope_digest"] = "foreign-scope".into();
                    }
                    "contract" => {
                        journal["scopes"]["replacement"]["contract_revision"] =
                            "foreign-contract".into();
                    }
                    _ => unreachable!(),
                }
                std::fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
                let bytes = std::fs::read(&path).unwrap();
                let before = ctx.store.get(&completed.id).unwrap();

                // When ordinary sync encounters the changed publication, then repeats.
                sync_twice(&ctx);

                // Then it retains the same acceptance without attaching or rewriting it.
                let after = ctx.store.get(&completed.id).unwrap();
                assert_eq!(after.plan, before.plan, "{initial:?} {automatic} {change}");
                assert_eq!(after.write_scope, before.write_scope, "{change}");
                assert_eq!(after.plans[1].approval, completed.plans[1].approval);
                assert!(after.children.is_empty() && after.runs.is_empty());
                assert_eq!(ctx.store.list().unwrap().len(), 1);
                assert_eq!(std::fs::read(path).unwrap(), bytes, "{change}");
            }
        }
    }
}

#[test]
fn automatic_self_projection_restart_rechecks_fresh_authority() {
    for initial in [None, Some("original")] {
        for change in ["pause", "hold", "assist", "scope", "authority", "contract"] {
            // Given an accepted automatic replacement, then a separate-context revocation.
            let (root, ctx, completed, path) = interrupted(initial, Some("replacement"), true);
            let other = Store::open(root.path()).unwrap();
            match change {
                "pause" => {
                    other.pause_operating_policy(true).unwrap();
                }
                "hold" => {
                    other
                        .update(&completed.id, |task| {
                            task.dispatch_hold = true;
                            Ok(())
                        })
                        .unwrap();
                }
                "assist" => {
                    other.assist_operating_policy().unwrap();
                }
                "scope" => {
                    other
                        .edit_config(|doc| doc.unset("automation.scope.tasks").map(|_| ()))
                        .unwrap();
                }
                "authority" => {
                    other
                        .edit_config(|doc| doc.set_str("automation.authority.plan_accept", "false"))
                        .unwrap();
                }
                "contract" => {
                    other
                        .update(&completed.id, |task| {
                            task.write_scope.push("changed/**".into());
                            Ok(())
                        })
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let before = ctx.store.get(&completed.id).unwrap();
            let bytes = std::fs::read(&path).unwrap();

            // When the previously opened context syncs with newly narrowed authority.
            sync_twice(&ctx);

            // Then publication stays uncommitted and its exact approval is retained.
            let after = ctx.store.get(&completed.id).unwrap();
            assert_eq!(after.plan, before.plan, "{initial:?} {change}");
            assert_eq!(after.write_scope, before.write_scope);
            assert_eq!(after.plans[1].approval, completed.plans[1].approval);
            assert!(after.children.is_empty() && after.runs.is_empty());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);

            // Restoring the same paused/held grant needs no new acceptance.
            match change {
                "pause" => {
                    other.pause_operating_policy(false).unwrap();
                }
                "hold" => {
                    other
                        .update(&completed.id, |task| {
                            task.dispatch_hold = false;
                            Ok(())
                        })
                        .unwrap();
                }
                "assist" | "scope" | "authority" | "contract" => continue,
                _ => unreachable!(),
            }
            sync_twice(&ctx);
            let restored = ctx.store.get(&completed.id).unwrap();
            assert_eq!(restored.plan, completed.plan, "{initial:?} {change}");
            assert_eq!(restored.write_scope, completed.write_scope);
            assert_eq!(restored.plans[1].approval, completed.plans[1].approval);
        }
    }
}
