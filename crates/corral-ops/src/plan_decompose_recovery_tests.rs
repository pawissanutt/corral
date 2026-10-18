use super::*;

fn interrupted_replacement(initial_scopes: usize) -> (tempfile::TempDir, Ctx, String, PathBuf) {
    let scopes = if initial_scopes == 0 {
        Vec::new()
    } else {
        vec![scope("implementation", &[])]
    };
    let (dir, mut ctx, id) = fixture(scopes.clone());
    ctx.herdr.set_session(Some("corral-test".into()));
    ctx.store.config.herdr.session = Some("corral-test".into());
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
    let first = submit_reviewed_plan(&ctx, &id, &document, scopes);
    owner_accept(&ctx, &id, &first).unwrap();
    let second = submit_reviewed_plan(
        &ctx,
        &id,
        &document,
        vec![scope("a", &[]), scope("b", &["a"])],
    );
    let blocked = child_path(&ctx.store, "T-003");
    std::fs::create_dir(&blocked).unwrap();
    let error = owner_accept(&ctx, &id, &second).unwrap_err();
    assert!(format!("{error:#}").contains("publishing"));
    let parent = ctx.store.get(&id).unwrap();
    assert_eq!(parent.plan, Some(format!("{id}@P1")));
    assert!(parent.children.is_empty());
    assert!(parent.plans[1].approval.is_some());
    let staged = ctx.store.get("T-002").unwrap();
    assert_eq!(staged.plan, Some(format!("{id}@P2")));
    assert!(
        crate::queue::dependency_check(&ctx.store, &staged)
            .unwrap()
            .is_some()
    );
    std::fs::remove_dir(blocked).unwrap();
    let path = ctx
        .store
        .state_dir()
        .join(format!("plan-decompose-{id}.json"));
    (dir, ctx, id, path)
}

fn private_herdr_available() -> bool {
    if std::env::var("CORRAL_HERDR_SESSION").as_deref() != Ok("corral-test")
        || !std::env::var("HOME").is_ok_and(|home| home.starts_with("/tmp/corral-herdr-"))
    {
        eprintln!("skipping sync recovery: requires private scripts/test-herdr.sh fixture");
        return false;
    }
    let available = std::process::Command::new("herdr")
        .args(["--session", "corral-test", "workspace", "list"])
        .output()
        .is_ok_and(|output| output.status.success());
    if !available {
        eprintln!("skipping sync recovery: private corral-test server unavailable");
    }
    available
}

#[test]
fn publication_sync_resumes_zero_scope_self_plan_replacement() {
    if private_herdr_available() {
        assert_sync_recovery(0, false);
    }
}

#[test]
fn publication_sync_resumes_one_scope_self_plan_replacement() {
    if private_herdr_available() {
        assert_sync_recovery(1, false);
    }
}

#[test]
fn publication_sync_remaps_replacement_collisions_for_both_self_plans() {
    if private_herdr_available() {
        for initial_scopes in [0, 1] {
            assert_sync_recovery(initial_scopes, true);
        }
    }
}

fn assert_sync_recovery(initial_scopes: usize, collision: bool) {
    let (_dir, ctx, id, path) = interrupted_replacement(initial_scopes);
    let original: Journal = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let other = collision.then(|| {
        ctx.store
            .create("Independent", None, Status::Backlog, "Do not overwrite")
            .unwrap()
    });
    let other_bytes = other
        .as_ref()
        .map(|task| std::fs::read(&task.path).unwrap());
    let config = std::fs::read(ctx.store.config_path()).unwrap();

    // Only ordinary sync resumes the durable owner acceptance, with no re-acceptance.
    crate::sync(&ctx).unwrap();
    crate::sync(&ctx).unwrap();

    let parent = ctx.store.get(&id).unwrap();
    assert_eq!(parent.status, Status::Done);
    assert_eq!(parent.plan, Some(format!("{id}@P2")));
    let second = if collision { "T-004" } else { "T-003" };
    assert_eq!(parent.children, ["T-002", second]);
    assert_eq!(
        ctx.store.list().unwrap().len(),
        if collision { 4 } else { 3 }
    );
    assert_eq!(std::fs::read(ctx.store.config_path()).unwrap(), config);
    let journal: Journal = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(journal.backlink, original.backlink);
    assert_eq!(journal.document, original.document);
    assert_eq!(journal.artifact_sha256, original.artifact_sha256);
    assert_eq!(journal.source_contract, original.source_contract);
    assert_eq!(
        journal.projected_contract,
        operating_contract_revision(&parent).unwrap()
    );
    for (label, child_id) in [("a", "T-002"), ("b", second)] {
        let child = ctx.store.get(child_id).unwrap();
        assert_eq!(child.plan, parent.plan);
        assert_eq!(child.write_scope, [format!("{label}/**")]);
        assert_eq!(journal.scopes[label].task, child_id);
        assert_eq!(
            journal.scopes[label].contract_revision,
            operating_contract_revision(&child).unwrap()
        );
        assert_eq!(
            journal.scopes[label].scope_digest,
            crate::verification::digest(
                &serde_json::to_vec(
                    parent.plans[1]
                        .document
                        .dependencies
                        .iter()
                        .find(|s| s.task == label)
                        .unwrap()
                )
                .unwrap()
            )
        );
        assert!(child.runs.is_empty() && !child.queued);
        assert!(
            crate::queue::dependency_check(&ctx.store, &child)
                .unwrap()
                .is_none_or(|waiting| { label == "b" && !waiting.is_empty() })
        );
    }
    assert_eq!(
        ctx.store.get(second).unwrap().depends_on,
        [id, "T-002".into()]
    );
    if let Some(other) = other {
        assert_eq!(std::fs::read(&other.path).unwrap(), other_bytes.unwrap());
    }
}

#[test]
fn publication_recovery_rejects_foreign_changed_and_different_replacements() {
    for initial_scopes in [0, 1] {
        for change in [
            "foreign",
            "attachment",
            "source",
            "artifact",
            "document",
            "child",
            "staged",
            "different",
        ] {
            let (_dir, ctx, id, path) = interrupted_replacement(initial_scopes);
            let mut journal: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            match change {
                "foreign" => journal["backlink"] = "T-999@P2".into(),
                "attachment" => {
                    ctx.store
                        .update(&id, |task| {
                            task.plan = Some(format!("{id}@P0"));
                            let mut old = task.plans[0].clone();
                            old.id = "P0".into();
                            task.plans.insert(0, old);
                            Ok(())
                        })
                        .unwrap();
                }
                "source" => journal["source_contract"] = "foreign-contract".into(),
                "artifact" => journal["artifact_sha256"] = "foreign-artifact".into(),
                "document" => journal["document"]["goal"] = "Different plan".into(),
                "child" => {
                    let mut child =
                        parse_task(journal["children"][0].as_str().unwrap(), Path::new(""))
                            .unwrap();
                    child.body.push_str("\nUnreviewed child instructions");
                    journal["children"][0] = render_task(&child).into();
                }
                "staged" => {
                    ctx.store
                        .update("T-002", |task| {
                            task.write_scope.push("foreign/**".into());
                            Ok(())
                        })
                        .unwrap();
                }
                "different" => {
                    let document = ctx.store.get(&id).unwrap().plans[1].document.clone();
                    let plan = submit_reviewed_plan(
                        &ctx,
                        &id,
                        &document,
                        vec![scope("c", &[]), scope("d", &[])],
                    );
                    assert!(owner_accept(&ctx, &id, &plan).is_err());
                }
                _ => unreachable!(),
            }
            std::fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            let parent = ctx.store.get(&id).unwrap();
            let parent_bytes = std::fs::read(&parent.path).unwrap();
            let child_bytes = std::fs::read(child_path(&ctx.store, "T-002")).unwrap();
            let plan = parent.plans.last().unwrap().id.clone();
            assert!(
                apply(&ctx, &id, &plan).is_err(),
                "{initial_scopes}: {change}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "{change}");
            assert_eq!(
                std::fs::read(&parent.path).unwrap(),
                parent_bytes,
                "{change}"
            );
            assert_eq!(
                std::fs::read(child_path(&ctx.store, "T-002")).unwrap(),
                child_bytes,
                "{change}"
            );
            assert!(!child_path(&ctx.store, "T-003").exists());
        }
    }
}
