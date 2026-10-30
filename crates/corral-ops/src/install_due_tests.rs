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
}
