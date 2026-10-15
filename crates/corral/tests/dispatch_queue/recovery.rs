use corral_core::Status;

use crate::queue_support::Fixture;

#[test]
fn foreign_hook_and_sync_cannot_recover_an_unattached_launch() {
    // Given a launch killed after external pane creation but before attachment is saved.
    let f = Fixture::new();
    let task = f.task("app", Status::Ready);
    f.fault("after workspace create");
    let output = f
        .command(&["dispatch", &task.id])
        .env("CRASH_LAUNCH", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let before = f.ctx.store.get(&task.id).unwrap();
    assert!(before.current_run().unwrap().herdr.is_none());
    assert!(before.launch.is_some());
    assert_eq!(
        before.launch.as_ref().unwrap().socket.as_ref(),
        Some(&f.ctx.herdr.socket_path().unwrap())
    );
    let events = serde_json::to_value(f.ctx.store.events().unwrap()).unwrap();
    let inventory = f.state();
    let slots = corral_ops::slots(&f.ctx).unwrap();

    // When another server supplies a valid non-empty inventory to hook or manual sync.
    for verb in ["hook", "sync"] {
        let output = f
            .command(&[verb])
            .env("CORRAL_HERDR_SESSION", "foreign-session")
            .output()
            .unwrap();
        // Then even recovery is a silent no-op with no slot, state, event or engine changes.
        assert!(output.status.success(), "{verb}: {output:?}");
        assert!(output.stdout.is_empty(), "{verb}: {output:?}");
        assert!(output.stderr.is_empty(), "{verb}: {output:?}");
        assert_eq!(f.ctx.store.get(&task.id).unwrap(), before);
        assert_eq!(
            serde_json::to_value(f.ctx.store.events().unwrap()).unwrap(),
            events
        );
        assert_eq!(f.state(), inventory);
        assert_eq!(corral_ops::slots(&f.ctx).unwrap(), slots);
    }
}

#[test]
fn preflight_refusal_releases_claim_before_worktree_or_pane_creation() {
    let f = Fixture::new();
    let task = f.task("app", Status::Ready);
    f.fault("claimed workspace list");

    // The adapter refuses only after the durable reservation is visible.
    assert!(!f.cli(&["dispatch", &task.id]).status.success());
    let refused = f.ctx.store.get(&task.id).unwrap();
    assert_eq!(refused.runs.len(), 1);
    let claim = refused.launch.as_ref().unwrap();
    assert!(!claim.reserved);
    assert!(
        claim
            .error
            .as_ref()
            .unwrap()
            .contains("claimed workspace list")
    );
    assert!(!refused.current_run().unwrap().worktree.exists());
    assert_eq!(f.state()["creates"], 0);
    assert_eq!(f.state()["submits"], 0);
    assert_eq!(corral_ops::slots(&f.ctx).unwrap(), (0, 1));
    let events = f.ctx.store.events().unwrap();
    let claimed = events
        .iter()
        .position(|e| e.kind == "dispatch_claimed")
        .unwrap();
    let released = events
        .iter()
        .position(|e| e.kind == "dispatch_released")
        .unwrap();
    assert!(claimed < released);

    // The released slot serves another project, then recovery reuses the same run.
    let other = f.task("other", Status::Ready);
    f.ok(&["dispatch", &other.id]);
    f.ok(&[
        "done",
        &other.id,
        "--summary",
        "Verified: fixture\nNot verified: none",
    ]);
    f.ok(&["dispatch", &task.id]);
    let resumed = f.ctx.store.get(&task.id).unwrap();
    assert_eq!(resumed.runs.len(), 1);
    assert_eq!(resumed.status, Status::Running);
    assert!(resumed.launch.is_none());
    assert_eq!(f.state()["creates"], 2);
    assert_eq!(f.state()["submits"], 2);
}

#[test]
fn crashed_external_boundaries_recover_without_duplicate_run_or_pane() {
    for point in [
        "before workspace create",
        "after workspace create",
        "before pane report-metadata",
        "after pane report-metadata",
        "before pane run",
        "after pane run",
    ] {
        // Given a real CLI process killed by its external adapter at a precise boundary.
        let f = Fixture::new();
        let task = f.task("app", Status::Ready);
        f.fault(point);
        let out = f
            .command(&["dispatch", &task.id])
            .env("CRASH_LAUNCH", "1")
            .output()
            .unwrap();
        assert!(!out.status.success(), "{point}");
        assert!(
            f.ctx.store.get(&task.id).unwrap().launch.is_some(),
            "{point}"
        );

        // When independent sync processes recover and then reconcile again.
        f.ok(&["sync"]);
        f.ok(&["sync"]);

        // Then the journal is released, and the single run/pane is retained.
        let task = f.ctx.store.get(&task.id).unwrap();
        assert!(task.launch.is_none(), "{point}");
        assert_eq!(task.runs.len(), 1, "{point}");
        assert_eq!(f.state()["creates"], 1, "{point}");
        let submitted = if point == "before pane run" { 0 } else { 1 };
        assert_eq!(f.state()["submits"], submitted, "{point}");
        let expected = if point.ends_with("pane run") {
            Status::NeedsMe
        } else {
            Status::Running
        };
        assert_eq!(task.status, expected, "{point}");
        assert_eq!(corral_ops::slots(&f.ctx).unwrap(), (1, 1));
    }
}

#[test]
fn failed_launch_releases_capacity_and_resumes_the_same_reserved_run() {
    for point in [
        "before workspace create",
        "after workspace create",
        "before pane report-metadata",
    ] {
        let f = Fixture::new();
        let task = f.task("app", Status::Ready);
        f.fault(point);
        assert!(!f.cli(&["dispatch", &task.id]).status.success());
        let failed = f.ctx.store.get(&task.id).unwrap();
        assert!(!failed.launch.as_ref().unwrap().reserved);
        assert_eq!(corral_ops::slots(&f.ctx).unwrap(), (0, 1));

        f.ok(&["sync"]);
        let resumed = f.ctx.store.get(&task.id).unwrap();
        assert_eq!(resumed.runs.len(), 1);
        assert_eq!(resumed.status, Status::Running);
        assert_eq!(f.state()["creates"], 1);
        assert_eq!(f.state()["submits"], 1);
        let events = f.ctx.store.events().unwrap();
        assert!(events.iter().any(|e| e.kind == "dispatch_released"));
    }
}

#[test]
fn dequeue_of_failed_launch_closes_its_pane_and_releases_authorization() {
    let f = Fixture::new();
    let task = f.task("app", Status::Ready);
    f.fault("before pane report-metadata");
    assert!(!f.cli(&["dispatch", &task.id]).status.success());
    f.ok(&["queue", "remove", &task.id]);
    f.ok(&["queue", "admit", "app"]);
    f.ok(&["sync"]);
    let task = f.ctx.store.get(&task.id).unwrap();
    assert!(task.launch.is_none());
    assert!(!task.queued);
    assert!(task.dispatch_hold);
    assert_eq!(f.state()["panes"].as_array().unwrap().len(), 0);
    assert_eq!(f.state()["submits"], 0);
}

#[test]
fn reservation_precedes_worktree_and_recovers_after_git_process_crash() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};
    for after in [false, true] {
        let f = Fixture::new();
        let task = f.task("app", Status::Ready);
        let real_git = Command::new("which")
            .arg("git")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(real_git.status.success());
        let real_git = String::from_utf8(real_git.stdout).unwrap();
        let bin = f.temp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let wrapper = bin.join("git");
        std::fs::write(&wrapper, format!(
            "#!/usr/bin/env python3\nimport os,signal,subprocess,sys\nargs=sys.argv[1:]\ncrash='worktree' in args and 'add' in args\nif crash and not {after}: os.kill(os.getppid(),signal.SIGKILL); sys.exit(0)\nr=subprocess.run([{git:?}]+args)\nif crash: os.kill(os.getppid(),signal.SIGKILL)\nsys.exit(r.returncode)\n",
            after = if after { "True" } else { "False" }, git = real_git.trim()
        )).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let out = f
            .command(&["dispatch", &task.id])
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(!out.status.success());
        let claimed = f.ctx.store.get(&task.id).unwrap();
        assert!(claimed.launch.as_ref().unwrap().reserved);
        assert_eq!(corral_ops::slots(&f.ctx).unwrap(), (1, 1));

        f.ok(&["sync"]);
        assert_eq!(f.ctx.store.get(&task.id).unwrap().runs.len(), 1);
        assert_eq!(f.state()["creates"], 1);
        assert_eq!(f.state()["submits"], 1);
    }
}
