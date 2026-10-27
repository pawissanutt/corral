use super::*;
use serde_json::{Value, json};

fn gated(accept: Accept) -> Fx {
    Fx::new(|r| {
        r.gate = gate(&["echo first", "echo final"]);
        r.accept = Some(accept);
        r.remove_worktree = Some(false);
    })
}

fn result_file(fx: &Fx) -> (corral_ops::GatePaths, Value) {
    let paths = corral_ops::gate_paths(&fx.ctx, &fx.id, &fx.run().id);
    let result = serde_json::from_slice(&std::fs::read(&paths.result).unwrap()).unwrap();
    (paths, result)
}

#[test]
fn collection_refuses_incomplete_or_mismatched_command_ranges() {
    for corruption in [
        "missing_final",
        "missing_all",
        "empty_final",
        "overlap",
        "gap",
        "out_of_bounds",
        "wrong_command",
        "wrong_header",
        "footer_in_final_range",
        "failed_final",
    ] {
        // Given a real, successful two-command attempt.
        let fx = gated(Accept::Owner);
        corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
        let (paths, mut result) = result_file(&fx);
        match corruption {
            "missing_final" => {
                result["commands"].as_array_mut().unwrap().pop();
            }
            "missing_all" => result["commands"] = json!([]),
            "empty_final" => {
                result["commands"][1]["log_end"] = result["commands"][1]["log_start"].clone();
            }
            "overlap" => result["commands"][1]["log_start"] = json!(0),
            "gap" => result["commands"][1]["log_start"] = json!(1),
            "out_of_bounds" => result["commands"][1]["log_end"] = json!(u64::MAX),
            "wrong_command" => result["commands"][1]["command"] = json!("true"),
            "wrong_header" => {
                let log = std::fs::read_to_string(&paths.log).unwrap();
                std::fs::write(&paths.log, log.replace("$ echo final", "$ echo bogus")).unwrap();
            }
            "footer_in_final_range" => {
                result["commands"][1]["log_end"] =
                    json!(std::fs::metadata(&paths.log).unwrap().len());
            }
            "failed_final" => result["commands"][1]["exit_code"] = json!(1),
            _ => unreachable!(),
        }
        std::fs::write(&paths.result, serde_json::to_vec(&result).unwrap()).unwrap();

        // When the owned result is collected.
        let record = corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();

        // Then neither the result file nor a marker alone can authorize a pass.
        assert_eq!(record.result, GateResult::Fail, "{corruption}");
        assert_eq!(
            record.failed_cmd.as_deref(),
            Some("invalid_command_receipts")
        );
    }
}

#[test]
fn collection_requires_one_terminal_pass_result() {
    for suffix in [
        "\ncorral gate: pass\n",
        "\nmore output\n",
        "\ncorral gate: fail\n",
    ] {
        // Given an otherwise passing log with an extra result or trailing output.
        let fx = gated(Accept::Owner);
        corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
        let (paths, _) = result_file(&fx);
        let mut log = std::fs::read(&paths.log).unwrap();
        log.extend_from_slice(suffix.as_bytes());
        std::fs::write(&paths.log, log).unwrap();

        // When collected.
        let record = corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();

        // Then the apparent passing result is rejected.
        assert_eq!(record.result, GateResult::Fail, "{suffix:?}");
    }
}

#[test]
fn collection_rejects_a_missing_footer_or_result_in_command_output() {
    for command_result in [true, false] {
        // Given either a command-supplied pass or a log missing the runner's footer.
        let fx = Fx::new(|r| {
            r.gate = if command_result {
                gate(&["printf 'corral gate: pass\\n'", "true"])
            } else {
                gate(&["true", "true"])
            };
        });
        corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
        if !command_result {
            let (paths, _) = result_file(&fx);
            let log = std::fs::read_to_string(&paths.log).unwrap();
            std::fs::write(paths.log, log.replace("corral gate: pass", "")).unwrap();
        }

        // When collected.
        let record = corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();

        // Then only the runner's unique terminal result can authorize a pass.
        assert_eq!(record.result, GateResult::Fail);
    }
}

#[test]
fn collection_accepts_only_blank_lines_after_final_pass() {
    // Given a valid two-step gate with trailing blank lines.
    let fx = gated(Accept::Owner);
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    let (paths, _) = result_file(&fx);
    let mut log = std::fs::read(&paths.log).unwrap();
    log.extend_from_slice(b"\n \t\n");
    std::fs::write(&paths.log, log).unwrap();

    // When collected.
    let record = corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();

    // Then the final nonblank result and both command receipts establish the pass.
    assert_eq!(record.result, GateResult::Pass);
    assert_eq!(record.commands.len(), 2);
}

#[test]
fn missing_log_cannot_complete_a_running_gate() {
    // Given an owned passing result whose per-attempt log is missing.
    let fx = gated(Accept::Owner);
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    let (paths, _) = result_file(&fx);
    std::fs::remove_file(paths.log).unwrap();

    // When collected.
    let result = corral_ops::gate_collect(&fx.ctx, &fx.id);

    // Then no pass is recorded.
    assert!(result.is_err());
    assert_eq!(fx.run().gate.unwrap().result, GateResult::Running);
}

#[test]
fn collected_pass_is_invalidated_when_log_is_missing_or_tampered() {
    for missing in [true, false] {
        // Given a completed pass, then loss or same-length tampering of its log.
        let fx = gated(Accept::Owner);
        corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
        corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap();
        let (paths, _) = result_file(&fx);
        if missing {
            std::fs::remove_file(paths.log).unwrap();
        } else {
            let log = std::fs::read_to_string(&paths.log).unwrap();
            std::fs::write(paths.log, log.replace("\nfirst\n", "\nother\n")).unwrap();
        }

        // When the already-finished record is collected again.
        let record = corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();

        // Then the stored pass cannot survive the missing or changed evidence.
        assert_eq!(record.result, GateResult::Fail);
    }
}

#[test]
fn owner_cannot_approve_missing_or_tampered_pass_logs() {
    for missing in [true, false] {
        for strategy in [ApproveStrategy::FastForward, ApproveStrategy::Rebase] {
            // Given a finished candidate with a collected pass.
            let fx = gated(Accept::Owner);
            fx.change("widget.txt", "widget\n");
            fx.done();
            corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
            corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap();
            let base = git(&fx.app, &["rev-parse", "main"]);
            let (paths, _) = result_file(&fx);
            if missing {
                std::fs::remove_file(paths.log).unwrap();
            } else {
                let log = std::fs::read_to_string(&paths.log).unwrap();
                std::fs::write(paths.log, log.replace("\nfirst\n", "\nother\n")).unwrap();
            }

            // When the owner approves directly, without another collection.
            let result = corral_ops::approve(&fx.ctx, &fx.id, strategy, false, true);

            // Then missing or tampered evidence does not authorize landing.
            assert!(result.is_err());
            assert_eq!(git(&fx.app, &["rev-parse", "main"]), base);
            assert_eq!(fx.task().status, Status::Review);
            assert_eq!(fx.events("approved"), 0);
        }
    }
}

#[test]
fn autopilot_cannot_accept_event_only_or_tampered_pass() {
    for missing in [true, false] {
        // Given fresh gate and semantic approval, before autonomous acceptance.
        let fx = Fx::new(|r| {
            r.gate = gate(&["echo first", "echo final"]);
            r.reviewer = Some("rev".into());
            r.accept = Some(Accept::Auto);
        });
        fx.change("widget.txt", "widget\n");
        fx.done();
        fx.sync();
        fx.verdict(Verdict::Approve, "Reviewed.");
        let base = git(&fx.app, &["rev-parse", "main"]);
        let (paths, _) = result_file(&fx);
        if missing {
            std::fs::remove_file(paths.log).unwrap();
        } else {
            let log = std::fs::read_to_string(&paths.log).unwrap();
            std::fs::write(paths.log, log.replace("\nfirst\n", "\nother\n")).unwrap();
        }

        // When autopilot checks acceptance without collecting the finished gate again.
        fx.sync();

        // Then a stored pass or gate event is insufficient for landing.
        assert_eq!(git(&fx.app, &["rev-parse", "main"]), base);
        assert_eq!(fx.events("approved"), 0);
        assert_ne!(fx.task().status, Status::Done);
    }
}

#[test]
fn owner_can_still_override_a_completed_failed_check() {
    // Given fresh, complete evidence of a failing check.
    let fx = Fx::new(|r| {
        r.gate = gate(&["false", "echo never"]);
        r.accept = Some(Accept::Owner);
    });
    fx.change("widget.txt", "widget\n");
    fx.done();
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    let record = corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();
    assert_eq!(record.result, GateResult::Fail);

    // When the owner explicitly approves that failed outcome.
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true).unwrap();

    // Then owner override remains available.
    assert_eq!(fx.task().status, Status::Done);
    assert!(fx.app.join("widget.txt").is_file());
}

#[test]
fn idea_dispatch_requires_promotion_even_when_queueing() {
    for queue_if_blocked in [false, true] {
        // Given an Idea with no project or profile to dispatch.
        let tmp = tempfile::tempdir().unwrap();
        let ctx = corral_ops::resources::FixedResourceProbe::ctx(Store::init(tmp.path()).unwrap());
        let task = ctx.store.create("Idea", None, Status::Idea, "").unwrap();

        // When dispatch is requested.
        let error = corral_ops::dispatch(
            &ctx,
            &task.id,
            &DispatchOpts {
                queue_if_blocked,
                ..Default::default()
            },
        )
        .unwrap_err();

        // Then promotion is required before any queue or runtime side effect.
        assert!(error.to_string().contains("promote"));
        let task = ctx.store.get(&task.id).unwrap();
        assert_eq!(task.status, Status::Idea);
        assert!(task.runs.is_empty());
        assert!(!task.queued);
        assert_eq!(
            std::fs::read_dir(ctx.store.prompts_dir()).unwrap().count(),
            0
        );
    }
}

fn peer_review(fx: &Fx, project: &str, verdict: Option<Verdict>) -> Task {
    let peer = fx
        .ctx
        .store
        .create("Peer", Some(project), Status::Review, "")
        .unwrap();
    let mut run = fx.run();
    run.herdr = None;
    run.autopilot = None;
    run.outcome = None;
    run.review = Some(corral_core::ReviewRecord {
        route: None,
        identity: run.review.as_ref().and_then(|r| r.identity.clone()),
        evidence_digest: None,
        verdict,
        process_lost: false,
        reviewer: "rev".into(),
        notes: None,
        sha: git(&run.worktree, &["rev-parse", "HEAD"]),
        at: now(),
        pane: run.review.as_ref().and_then(|r| r.pane.clone()),
    });
    fx.ctx
        .store
        .update(&peer.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap()
}

#[test]
fn manual_landing_proceeds_while_same_project_review_is_pending() {
    for strategy in [ApproveStrategy::FastForward, ApproveStrategy::Rebase] {
        // Given another task with a pending review in the same project.
        let fx = Fx::new(|_| {});
        fx.change("widget.txt", "widget\n");
        fx.done();
        let peer = peer_review(&fx, "app", None);

        // When the owner lands.
        corral_ops::approve(&fx.ctx, &fx.id, strategy, false, true).unwrap();

        // Then main advances and the peer's review is left untouched.
        assert_eq!(fx.task().status, Status::Done);
        assert!(fx.app.join("widget.txt").is_file());
        assert_eq!(fx.ctx.store.get(&peer.id).unwrap().status, Status::Review);
    }
}

#[test]
fn autopilot_landing_proceeds_while_same_project_review_is_pending() {
    // Given an approved autonomous candidate and a peer awaiting a verdict.
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "widget\n");
    fx.done();
    fx.sync();
    fx.verdict(Verdict::Approve, "Reviewed.");
    peer_review(&fx, "app", None);

    // When autopilot reaches the common landing boundary.
    fx.sync();

    // Then it lands without waiting for the peer's verdict.
    assert_eq!(fx.task().status, Status::Done);
    assert_eq!(fx.events("approved"), 1);
}

#[test]
fn resolved_or_other_project_review_does_not_block_landing() {
    for (project, verdict) in [("app", Some(Verdict::Changes)), ("elsewhere", None)] {
        // Given a resolved peer review or an unrelated project's pending review.
        let fx = Fx::new(|_| {});
        fx.change("widget.txt", "widget\n");
        fx.done();
        peer_review(&fx, project, verdict);

        // When the owner lands.
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true).unwrap();

        // Then neither case unnecessarily blocks the project.
        assert_eq!(fx.task().status, Status::Done);
    }
}

#[test]
fn reworking_or_obsolete_peer_review_does_not_block_landing() {
    for strategy in [ApproveStrategy::FastForward, ApproveStrategy::Rebase] {
        for status in [Status::Running, Status::Review] {
            // Given a previous-round pending review, including legacy Running records.
            let fx = Fx::new(|_| {});
            fx.change("widget.txt", "widget\n");
            fx.done();
            let peer = peer_review(&fx, "app", None);
            fx.ctx
                .store
                .update(&peer.id, |task| {
                    task.status = status;
                    if status == Status::Review {
                        task.current_run_mut().unwrap().review.as_mut().unwrap().sha =
                            git(&fx.app, &["rev-parse", "main"]);
                    }
                    Ok(())
                })
                .unwrap();

            // When another task lands through the common landing boundary.
            corral_ops::approve(&fx.ctx, &fx.id, strategy, false, true).unwrap();

            // Then inactive or old-candidate review records cannot deadlock the project.
            assert_eq!(fx.task().status, Status::Done);
            assert!(fx.app.join("widget.txt").is_file());
            assert_eq!(fx.ctx.store.get(&peer.id).unwrap().status, status);
        }
    }
}

#[test]
fn changing_peer_does_not_block_landing() {
    // Given a peer being updated by another operation.
    let fx = Fx::new(|_| {});
    fx.change("widget.txt", "widget\n");
    fx.done();
    let peer = peer_review(&fx, "app", Some(Verdict::Approve));
    // Concurrent forks can transiently inherit the preceding update's descriptor.
    // Establish the precondition with a blocking lock, not a timing-dependent try_lock.
    let _lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(
            fx.ctx
                .store
                .dir()
                .join("lock")
                .join(format!("{}.lock", peer.id)),
        )
        .unwrap();
    _lock.lock().unwrap();

    // When another task lands while that peer's lock is held.
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true).unwrap();

    // Then landing never takes peer locks, so it neither waits nor refuses.
    assert_eq!(fx.task().status, Status::Done);
}
