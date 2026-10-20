use super::*;

#[test]
fn configured_review_without_gate_escalates_once_without_running_commands() {
    for mode in ["auto", "reviewer", "owner", "explicit_empty"] {
        // Given a configured review policy without executable gate commands.
        let fx = Fx::new(|r| match mode {
            "auto" => r.accept = Some(Accept::Auto),
            "reviewer" => r.reviewer = Some("rev".into()),
            "owner" => r.accept = Some(Accept::Owner),
            "explicit_empty" => r.gate = gate(&[]),
            _ => unreachable!(),
        });
        fx.change("widget.txt", "widget\n");
        fx.done();
        let main = git(&fx.app, &["rev-parse", "main"]);

        // When the autopilot observes the completed candidate, including repeated sync.
        fx.sync();
        fx.sync();

        // Then missing verification cannot launch a reviewer or land the candidate.
        assert_eq!(fx.task().status, Status::NeedsMe, "{mode}");
        assert_eq!(fx.stage(), Some(AutoStage::Escalated));
        assert_eq!(fx.reason(), "project app has no gate configured");
        assert_eq!(fx.transitions(), ["escalated"]);
        assert_eq!(fx.events("gate_started"), 0);
        assert_eq!(fx.events("review_started"), 0);
        assert_eq!(git(&fx.app, &["rev-parse", "main"]), main);
    }
}

#[test]
fn approval_refuses_missing_gate_before_rebasing_candidate() {
    for strategy in [ApproveStrategy::Rebase, ApproveStrategy::FastForward] {
        // Given owner review without a gate and a candidate behind its base.
        let fx = Fx::new(|r| r.reviewer = Some("rev".into()));
        fx.change("widget.txt", "widget\n");
        fx.done();
        let head = git(&fx.run().worktree, &["rev-parse", "HEAD"]);
        fx.commit_on_main("other.txt", "main moved\n", "advance main");
        let main = git(&fx.app, &["rev-parse", "main"]);

        // When approval would otherwise prepare an integrated re-gate.
        let error = corral_ops::approve(&fx.ctx, &fx.id, strategy, false, true).unwrap_err();

        // Then the exact configuration error precedes any git mutation.
        assert_eq!(error.to_string(), "project app has no gate configured");
        assert_eq!(git(&fx.run().worktree, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&fx.app, &["rev-parse", "main"]), main);
        assert_eq!(fx.events("approved"), 0);
    }
}

#[test]
fn deprecated_max_diff_lines_alone_keeps_the_unconfigured_manual_approval_path() {
    for project_layer in [false, true] {
        // Given a project whose only review setting is the removed, ignored key.
        let fx = Fx::new(|r| {
            if !project_layer {
                r.max_diff_lines = Some(1500);
            }
        });
        if project_layer {
            let mut store = Store::open(fx.ctx.store.root()).unwrap();
            store
                .config
                .projects
                .get_mut("app")
                .unwrap()
                .review
                .max_diff_lines = Some(1500);
            store.save_config().unwrap();
        }
        fx.change("widget.txt", "widget\n");
        fx.done();

        // When the owner approves manually, as for any unconfigured project.
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();

        // Then the key introduced no gate requirement and the candidate lands.
        assert_eq!(
            fx.task().status,
            Status::Done,
            "project_layer={project_layer}"
        );
        assert_eq!(fx.events("approved"), 1);
        assert!(fx.app.join("widget.txt").exists());
    }
}

#[test]
fn project_empty_gate_overrides_inherited_commands_and_refuses_launch() {
    // Given a global gate explicitly disabled by the project's empty list.
    let fx = Fx::new(|r| r.gate = gate(&["true"]));
    let mut store = Store::open(fx.ctx.store.root()).unwrap();
    store.config.projects.get_mut("app").unwrap().review.gate = gate(&[]);
    store.save_config().unwrap();
    fx.done();

    // When the reloaded project policy is used by gate and autopilot.
    let error = corral_ops::gate_start(&fx.ctx, &fx.id).unwrap_err();
    fx.sync();

    // Then no inherited command silently restores a disabled gate.
    assert_eq!(error.to_string(), "project app has no gate configured");
    assert_eq!(fx.task().status, Status::NeedsMe);
    assert_eq!(fx.reason(), "project app has no gate configured");
    assert_eq!(fx.events("gate_started"), 0);
}
