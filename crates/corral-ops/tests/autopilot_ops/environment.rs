use super::*;
use std::process::Stdio;

fn child_verdict(stale: bool) -> bool {
    let Some(root) = std::env::var_os("CORRAL_ENV_TEST_ROOT") else {
        return false;
    };
    let root = PathBuf::from(root);
    let ctx = Ctx {
        store: Store::open(&root).unwrap(),
        herdr: Herdr::with_bin(root.join("fake-herdr/herdr"), None),
    };
    let task = ctx.store.get("T-001").unwrap();
    let run = task.current_run().unwrap();
    let attempt = &run
        .review
        .as_ref()
        .unwrap()
        .identity
        .as_ref()
        .unwrap()
        .attempt;
    let result = corral_ops::verdict_bound(
        &ctx,
        &task.id,
        Verdict::Approve,
        "Reviewed from a different process environment.",
        Some(attempt),
    );
    if stale {
        let error = result.unwrap_err().to_string();
        assert!(error.contains("stale review"), "{error}");
        assert!(error.contains("inputs_digest"), "{error}");
        println!("CONFIG_CHANGE_REFUSED {error}");
    } else {
        assert_eq!(result.unwrap().verdict, Some(Verdict::Approve));
        assert_eq!(
            corral_ops::gate_collect(&ctx, &task.id)
                .unwrap()
                .unwrap()
                .result,
            GateResult::Pass
        );
        println!("CROSS_ENV_VERDICT_ACCEPTED");
    }
    true
}

fn reviewing() -> Fx {
    let fx = Fx::new(|r| {
        r.gate = gate(&["printf '%s' \"$PATH\" > gate-path.txt"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
        r.remove_worktree = Some(false);
    });
    fx.change(".gitignore", "gate-path.txt\n");
    fx.change("widget.txt", "spins\n");
    fx.done();
    fx.sync();
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    let identity = fx.run().gate.unwrap().identity.unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(
        &std::fs::read(corral_ops::gate_paths(&fx.ctx, &fx.id, "R1").result).unwrap(),
    )
    .unwrap();
    assert_eq!(
        receipt["identity"],
        serde_json::to_value(&identity).unwrap()
    );
    let captured = identity
        .environment
        .iter()
        .find(|(key, _)| key == "PATH")
        .unwrap()
        .1
        .as_deref();
    assert_eq!(
        std::fs::read_to_string(fx.run().worktree.join("gate-path.txt")).unwrap(),
        captured.unwrap()
    );
    fx
}

fn different_environment(fx: &Fx, test: &str) {
    let mut paths = vec![fx.ctx.store.root().join("reviewer-only-bin")];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("CORRAL_ENV_TEST_ROOT", fx.ctx.store.root())
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("RUSTFLAGS", "--cfg reviewer_only")
        .env_remove("CORRAL_GATE_ATTEMPT")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    print!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn verdict_accepts_different_process_environment() {
    // Given a gate and review created with environment A.
    if child_verdict(false) {
        return;
    }
    let fx = reviewing();
    // When a separate process submits the bound verdict with environment B.
    different_environment(
        &fx,
        "environment::verdict_accepts_different_process_environment",
    );
    // Then the same receipts authorize landing from the original process too.
    assert_eq!(fx.sync().status, Status::Done);
    assert!(fx.app.join("widget.txt").is_file());
}

#[test]
fn verdict_refuses_config_change_across_process_environments() {
    // Given environment A's review, followed by a real config edit.
    if child_verdict(true) {
        return;
    }
    let fx = reviewing();
    let mut store = Store::open(fx.ctx.store.root()).unwrap();
    store.config.agents.get_mut("rev").unwrap().command = "false".into();
    store.save_config().unwrap();
    // When environment B submits the original bound verdict.
    different_environment(
        &fx,
        "environment::verdict_refuses_config_change_across_process_environments",
    );
    // Then no verdict or landing was authorized.
    assert_eq!(fx.run().review.unwrap().verdict, None);
    assert!(!fx.app.join("widget.txt").exists());
}

#[test]
fn verdict_refuses_changed_verification_files() {
    for relative in [
        "AGENTS.md",
        ".cargo/config.toml",
        "rust-toolchain.toml",
        "scripts/check-candidate.py",
        "review.md",
        "run.md",
    ] {
        // Given clean candidates with ignored tool inputs, or external prompt inputs.
        let fx = Fx::new(|r| {
            r.gate = gate(&["true"]);
            r.reviewer = Some("rev".into());
        });
        fx.change(
            ".gitignore",
            "AGENTS.md\n.cargo/\nrust-toolchain.toml\nscripts/\n",
        );
        let path = if matches!(relative, "review.md" | "run.md") {
            fx.ctx.store.prompts_dir().join(relative)
        } else {
            fx.run().worktree.join(relative)
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "original input\n").unwrap();
        fx.done();
        fx.sync();
        // When the input changes without moving HEAD or dirtying the worktree.
        std::fs::write(&path, "changed input\n").unwrap();
        assert!(corral_git::is_clean(&fx.run().worktree).unwrap());
        // Then digest freshness, not merely the clean-tree guard, refuses review.
        let error = corral_ops::verdict(&fx.ctx, &fx.id, Verdict::Approve, "Old input.")
            .unwrap_err()
            .to_string();
        assert!(error.contains("stale review"), "{relative}: {error}");
        assert!(error.contains("inputs_digest"), "{relative}: {error}");
        println!("FILE_CHANGE_REFUSED {relative}");
    }
}

#[test]
fn legacy_environmentless_receipt_requires_a_new_gate() {
    // Given a receipt from before captured environments were stored.
    let fx = reviewing();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            let identity = task
                .current_run_mut()
                .unwrap()
                .gate
                .as_mut()
                .unwrap()
                .identity
                .as_mut()
                .unwrap();
            identity.version = 1;
            identity.environment.clear();
            Ok(())
        })
        .unwrap();
    // When the new collector considers the old pass.
    let gate = corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();
    // Then an environmentless legacy receipt cannot silently authorize landing.
    assert_eq!(gate.result, GateResult::Fail);
    assert!(gate.failed_cmd.unwrap().starts_with("stale_result:"));
}
