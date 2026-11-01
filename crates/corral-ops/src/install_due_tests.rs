use super::*;
use corral_core::Store;

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(repo)
        .args([
            "-c",
            "user.name=Install test",
            "-c",
            "user.email=install@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[test]
fn older_installed_build_gets_one_install_event_per_landing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.projects.insert(
        "app".into(),
        serde_json::from_value(json!({"path":"."})).unwrap(),
    );
    let ctx = Ctx {
        store,
        resource_probe: crate::resources::FixedResourceProbe::arc(),
        herdr: corral_herdr::Herdr::with_bin("missing".into(), Some("corral-test".into())),
    };
    std::fs::create_dir_all(tmp.path().join("crates/corral")).unwrap();
    std::fs::write(
        tmp.path().join("crates/corral/Cargo.toml"),
        "[package]\nname = 'corral'\n",
    )
    .unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    git(tmp.path(), &["add", "crates/corral/Cargo.toml"]);
    git(tmp.path(), &["commit", "-qm", "installed"]);
    let old = git(tmp.path(), &["rev-parse", "HEAD"]);
    git(tmp.path(), &["commit", "--allow-empty", "-qm", "landing"]);
    let landed = git(tmp.path(), &["rev-parse", "HEAD"]);
    let task = ctx
        .store
        .create(
            "Self-hosted change",
            Some("app"),
            corral_core::Status::Review,
            "",
        )
        .unwrap();
    let run = serde_json::from_value(json!({
        "id":"R1", "agent":"generic", "worktree":tmp.path(), "branch":"corral/T-001",
        "base":"main", "base_sha":old, "started":"2026-10-30T00:00:00Z"
    }))
    .unwrap();
    let mut op = crate::hooks::prepare(&ctx, &task, &run, tmp.path(), &old, &landed)
        .unwrap()
        .unwrap();
    assert!(!op.ready());
    check_with(&ctx, &op, || panic!("not landed yet")).unwrap();
    op.landed = true;
    check_with(&ctx, &op, || old.clone()).unwrap();
    check_with(&ctx, &op, || old.clone()).unwrap();
    assert_eq!(
        ctx.store
            .events_shared()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "install_due")
            .count(),
        1
    );
    let mut next = op.clone();
    next.key = "landing-2".into();
    check_with(&ctx, &next, || landed.clone()).unwrap();
    git(
        tmp.path(),
        &["commit", "--allow-empty", "-qm", "newer installed build"],
    );
    let newer = git(tmp.path(), &["rev-parse", "HEAD"]);
    check_with(&ctx, &next, || newer).unwrap();
    next.landed = false;
    check_with(&ctx, &next, || {
        panic!("unlanded candidate cannot require installation")
    })
    .unwrap();
    next.landed = true;
    next.base = "plan-target".into();
    check_with(&ctx, &next, || {
        panic!("plan-target landing is not installed main")
    })
    .unwrap();
    assert_eq!(
        ctx.store
            .events_shared()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "install_due")
            .count(),
        1
    );

    // Neither half of the notice may escape a refused enclosing atomic unit.
    let mut refused = op.clone();
    refused.key = "refused".into();
    let result: anyhow::Result<()> = ctx.store.records_transaction(|| {
        check_with(&ctx, &refused, || "unknown".into())?;
        anyhow::bail!("refuse after staging both halves")
    });
    assert!(result.is_err());
    assert!(
        ctx.store
            .record_row("install-notices", &refused.key)
            .unwrap()
            .is_none()
    );
    assert!(
        !ctx.store
            .events_shared()
            .unwrap()
            .iter()
            .any(|event| event.kind == "install_due" && event.detail["operation"] == refused.key)
    );

    // Reach the same automatic landing completion used by production, with 1x/10x
    // unrelated exported history. Every cell, including byte remainders, stays equal.
    use corral_core::work::{self, Counter, HotScope};
    let mut baseline: Option<(work::WorkSnapshot, work::WorkSnapshot)> = None;
    for scale in [1, 10] {
        for n in 0..100 * scale {
            ctx.store
                .log_event(
                    None,
                    "historical-padding",
                    json!({"n":n,"body":"x".repeat(128)}),
                )
                .unwrap();
        }
        let mut growth = op.clone();
        growth.key = format!("growth-{scale:02}");
        growth.install_checked = false;
        let hot = HotScope::enter();
        let before = work::snapshot();
        crate::hooks::complete_landing(&ctx, &mut growth).unwrap();
        let first = work::snapshot().since(&before);
        let before = work::snapshot();
        check_with(&ctx, &growth, || "unknown".into()).unwrap();
        let replay = work::snapshot().since(&before);
        drop(hot);
        assert!(growth.ready());
        for snapshot in [&first, &replay] {
            snapshot.check_identity().unwrap();
            assert_eq!(snapshot.unattributed_hydrations(), 0);
            assert_eq!(snapshot.total(Counter::EventDecodes), 0);
            assert_eq!(snapshot.followup_calls.len(), 0);
        }
        if let Some((old_first, old_replay)) = &baseline {
            for counter in Counter::ALL {
                assert_eq!(
                    first.cells(counter),
                    old_first.cells(counter),
                    "first {counter:?}"
                );
                assert_eq!(
                    replay.cells(counter),
                    old_replay.cells(counter),
                    "replay {counter:?}"
                );
            }
        } else {
            baseline = Some((first, replay));
        }
        let reopened = Ctx::from_store(Store::open(tmp.path()).unwrap());
        check_with(&reopened, &growth, || "unknown".into()).unwrap();
        let events = reopened.store.events_shared().unwrap();
        let notices: Vec<_> = events
            .iter()
            .filter(|event| event.kind == "install_due" && event.detail["operation"] == growth.key)
            .collect();
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0].detail,
            json!({"project":growth.project,"run":growth.run,
            "operation":growth.key,"landed_sha":growth.landed_sha,
            "installed_sha":notices[0].detail["installed_sha"],
            "reason":"corral main advanced past the installed build; reinstall corral"})
        );
    }
}
