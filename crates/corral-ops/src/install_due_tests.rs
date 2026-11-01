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
fn absent_installed_commit_requires_install() {
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
    std::fs::write(tmp.path().join("crates/corral/Cargo.toml"), "fixture").unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    git(tmp.path(), &["commit", "--allow-empty", "-qm", "landing"]);
    let old = git(tmp.path(), &["rev-parse", "HEAD"]);
    git(tmp.path(), &["commit", "--allow-empty", "-qm", "next"]);
    let landed = git(tmp.path(), &["rev-parse", "HEAD"]);
    let task = ctx
        .store
        .create("Landing", Some("app"), corral_core::Status::Review, "")
        .unwrap();
    let run = serde_json::from_value(json!({
        "id":"R1", "agent":"generic", "worktree":tmp.path(), "branch":"corral/T-001",
        "base":"main", "base_sha":landed, "started":"2026-10-30T00:00:00Z"
    }))
    .unwrap();
    let mut op = crate::hooks::prepare(&ctx, &task, &run, tmp.path(), &old, &landed)
        .unwrap()
        .unwrap();
    op.landed = true;
    let missing = "0123456789012345678901234567890123456789";
    check_with(&ctx, &op, || missing.into()).unwrap();
    let events = ctx.store.events_shared().unwrap();
    let notice = events
        .iter()
        .find(|event| event.kind == "install_due")
        .unwrap();
    assert_eq!(notice.detail["installed_sha"], missing);
    op.key = "unknown-build".into();
    check_with(&ctx, &op, || "unknown".into()).unwrap();
    assert!(ctx.store.events_shared().unwrap().iter().any(|event| {
        event.kind == "install_due" && event.detail["operation"] == "unknown-build"
    }));
}

#[test]
fn automatic_install_notice_is_atomic_and_history_independent() {
    for identity in ["unknown", "0123456789012345678901234567890123456789"] {
        for history in [1, 100] {
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
            std::fs::write(tmp.path().join("crates/corral/Cargo.toml"), "fixture").unwrap();
            git(tmp.path(), &["init", "-q", "-b", "main"]);
            git(tmp.path(), &["commit", "--allow-empty", "-qm", "old"]);
            let old = git(tmp.path(), &["rev-parse", "HEAD"]);
            git(tmp.path(), &["commit", "--allow-empty", "-qm", "landing"]);
            let landed = git(tmp.path(), &["rev-parse", "HEAD"]);
            let task = ctx
                .store
                .create("Landing", Some("app"), corral_core::Status::Review, "")
                .unwrap();
            let run = serde_json::from_value(json!({
                "id":"R1", "agent":"generic", "worktree":tmp.path(), "branch":"corral/T-001",
                "base":"main", "base_sha":old, "started":"2026-10-30T00:00:00Z"
            }))
            .unwrap();
            let mut op = crate::hooks::prepare(&ctx, &task, &run, tmp.path(), &old, &landed)
                .unwrap()
                .unwrap();
            op.landed = true;
            for n in 0..history {
                ctx.store
                    .log_event(None, "historical-padding", json!({"n": n}))
                    .unwrap();
            }
            check_with(&ctx, &op, || identity.into()).unwrap();
            check_with(&ctx, &op, || identity.into()).unwrap();
            let reopened = Ctx::from_store(Store::open(tmp.path()).unwrap());
            check_with(&reopened, &op, || identity.into()).unwrap();
            let events = reopened.store.events_shared().unwrap();
            let notices: Vec<_> = events
                .iter()
                .filter(|e| e.kind == "install_due" && e.detail["operation"] == op.key)
                .collect();
            assert_eq!(notices.len(), 1, "identity={identity} history={history}");
            assert_eq!(
                notices[0].detail,
                json!({
                    "project": op.project, "run": op.run, "operation": op.key,
                    "landed_sha": op.landed_sha, "installed_sha": identity,
                    "reason": "corral main advanced past the installed build; reinstall corral"
                })
            );
        }
    }
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
}
