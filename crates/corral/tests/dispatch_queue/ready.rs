use std::sync::Arc;

use corral_core::{Limits, RunMode, Status};
use corral_herdr::Herdr;
use corral_ops::resources::FixedResourceProbe;
use corral_ops::{DispatchOpts, DispatchOutcome};

use crate::{queue_fixture, queue_support::Fixture};

#[cfg(target_os = "linux")]
#[test]
fn concurrent_terminal_move_is_not_reopened_by_stale_dequeue() {
    // Given real CLI processes synchronized at their blocking board/task lock syscalls.
    let output = std::process::Command::new("python3")
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/queue_support/dequeue_race.py"),
        )
        .arg(env!("CARGO_BIN_EXE_corral"))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    // When move saves Dropped before the stale dequeue acquires its mutation locks,
    // then dequeue fails without reopening the task or writing a dequeued event.
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn suggested_idea_promotion_command_remains_eligible_for_automatic_dispatch() {
    // Given the default Suggest policy, free coding capacity, and an eligible Idea.
    let mut f = queue_fixture();
    f.ctx.store.config.dispatch.auto_backlog = true;
    f.ctx.store.save_config().unwrap();
    let idea = f
        .ctx
        .store
        .create(
            "Eligible promotion",
            Some("app"),
            Status::Idea,
            "## Acceptance\nThe fixture completes.",
        )
        .unwrap();
    let card: serde_json::Value =
        serde_json::from_slice(&f.ok(&["pm", "next", "--json"]).stdout).unwrap();
    assert_eq!(card["kind"], "promote");
    assert_eq!(card["task"], idea.id);

    // When the owner executes the actual recommended command, then syncs.
    let mut command = std::process::Command::new("sh");
    command
        .arg("-c")
        .arg(card["command"].as_str().unwrap())
        .current_dir(f.temp.path())
        .stdin(std::process::Stdio::null());
    for (name, value) in f.command(&[]).get_envs() {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    f.ok(&["sync"]);

    // Then the suggested promotion starts without another authorization.
    let started = f.ctx.store.get(&idea.id).unwrap();
    assert_eq!(started.status, Status::Running);
    assert_eq!(started.runs.len(), 1);
    assert_eq!(f.state()["creates"], 1);
}

#[test]
fn queue_remove_rejects_review_and_terminal_tasks_without_side_effects() {
    for status in [Status::Review, Status::Done, Status::Dropped] {
        // Given a nonqueue task with a completed run, plus a live gate in Review.
        let mut f = queue_fixture();
        f.ctx.store.config.review.gate = Some(vec!["true".into()]);
        f.ctx.store.save_config().unwrap();
        let task = f.task("app", Status::Ready);
        f.ok(&["dispatch", &task.id]);
        f.ok(&[
            "done",
            &task.id,
            "--summary",
            "Verified: fixture\nNot verified: none",
        ]);
        if status == Status::Review {
            f.ok(&["gate", &task.id]);
        } else {
            f.ctx
                .store
                .update(&task.id, |t| {
                    t.status = status;
                    Ok(())
                })
                .unwrap();
        }
        let before = f.ctx.store.get(&task.id).unwrap();
        let events = serde_json::to_value(f.ctx.store.events().unwrap()).unwrap();
        let engine = f.state();
        assert!(!before.queued && before.launch.is_none());
        if status == Status::Review {
            assert!(before.current_run().unwrap().gate.is_some());
        }

        // When queue removal is requested without a queue authorization.
        let output = f.cli(&["queue", "remove", &task.id]);

        // Then lifecycle, finishing metadata, events, and engine state are unchanged.
        assert!(!output.status.success(), "{status:?}: {output:?}");
        assert_eq!(f.ctx.store.get(&task.id).unwrap(), before);
        assert_eq!(
            serde_json::to_value(f.ctx.store.events().unwrap()).unwrap(),
            events
        );
        assert_eq!(f.state(), engine);
    }
}

#[test]
fn enqueue_and_cli_queue_dispatch_promote_without_git_or_engine_calls() {
    for status in [Status::Idea, Status::Backlog, Status::Ready] {
        for dispatch in [false, true] {
            // Given free capacity and a project that Git cannot open.
            let f = queue_fixture();
            let task = f.task("app", status);
            std::fs::rename(
                f.temp.path().join("app/.git"),
                f.temp.path().join("app/git-unavailable"),
            )
            .unwrap();
            let mut args = if dispatch {
                vec!["dispatch", task.id.as_str(), "--queue"]
            } else {
                vec!["queue", "add", task.id.as_str()]
            };
            args.extend([
                "--profile",
                "generic",
                "--base",
                "pinned-base",
                "--headless",
            ]);

            // When the real CLI enqueues the task, including dispatch --queue.
            f.ok(&args);

            // Then Ready is durable, its route is pinned, and nothing was launched.
            let queued = f.ctx.store.get(&task.id).unwrap();
            assert_eq!(queued.status, Status::Ready);
            assert!(queued.queued);
            assert!(!queued.dispatch_hold);
            assert!(queued.runs.is_empty());
            let plan = queued.dispatch_plan.unwrap();
            assert_eq!(plan.route.profile, "generic");
            assert_eq!(plan.base.as_deref(), Some("pinned-base"));
            assert_eq!(plan.mode, RunMode::Headless);
            assert!(!f.temp.path().join("protocol.json").exists());
            assert!(!f.temp.path().join("app/.worktrees").exists());
        }
    }
}

#[test]
fn ops_queue_if_blocked_still_starts_immediately_when_free() {
    let f = queue_fixture();
    let task = f.task("app", Status::Backlog);
    let outcome = corral_ops::dispatch(
        &f.ctx,
        &task.id,
        &DispatchOpts {
            queue_if_blocked: true,
            ..DispatchOpts::default()
        },
    )
    .unwrap();
    assert!(matches!(outcome, DispatchOutcome::Started { .. }));
    assert_eq!(f.ctx.store.get(&task.id).unwrap().status, Status::Running);
    assert_eq!(f.state()["creates"], 1);
    assert_eq!(f.state()["submits"], 1);
}

#[test]
fn ops_blocked_backlog_becomes_ready_with_pinned_plan() {
    let f = queue_fixture();
    let dep = f.task("app", Status::Backlog);
    let task = f.task("other", Status::Backlog);
    crate::queue_support::git(&f.temp.path().join("other"), &["branch", "pinned-base"]);
    f.ctx
        .store
        .update(&task.id, |t| {
            t.depends_on.push(dep.id.clone());
            Ok(())
        })
        .unwrap();
    let outcome = corral_ops::dispatch(
        &f.ctx,
        &task.id,
        &DispatchOpts {
            queue_if_blocked: true,
            base: Some("pinned-base".into()),
            ..DispatchOpts::default()
        },
    )
    .unwrap();
    let DispatchOutcome::Queued { task, .. } = outcome else {
        panic!("blocked task started");
    };
    assert_eq!(task.status, Status::Ready);
    assert!(task.queued);
    assert!(task.runs.is_empty());
    assert_eq!(
        task.dispatch_plan.unwrap().base.as_deref(),
        Some("pinned-base")
    );
    assert!(!f.temp.path().join("app/.worktrees").exists());
    assert!(!f.temp.path().join("other/.worktrees").exists());
    assert!(!f.temp.path().join("protocol.json").exists());
}

#[test]
fn default_ready_admission_includes_every_configured_project() {
    let mut f = queue_fixture();
    f.ctx.store.config.limits.running = 4;
    f.ctx.store.config.limits.review = 2;
    f.ctx.store.save_config().unwrap();
    assert!(f.ctx.store.config.dispatch.ready_projects.is_empty());
    let app = f.task("app", Status::Ready);
    let other = f.task("other", Status::Ready);
    let backlog = f.task("app", Status::Backlog);
    let idea = f.task("other", Status::Idea);

    f.ok(&["sync"]);

    for task in [app, other] {
        let started = f.ctx.store.get(&task.id).unwrap();
        assert_eq!(started.status, Status::Running);
        assert_eq!(started.runs.len(), 1);
    }
    for task in [backlog, idea] {
        assert_eq!(f.ctx.store.get(&task.id).unwrap(), task);
    }
    assert_eq!(f.state()["creates"], 2);
    assert_eq!(f.state()["submits"], 2);
}

#[test]
fn project_restriction_applies_to_queued_and_unqueued_ready() {
    let mut f = queue_fixture();
    f.ctx.store.config.dispatch.ready_projects = vec!["app".into()];
    f.ctx.store.save_config().unwrap();
    let admitted = f.task("app", Status::Ready);
    let unqueued = f.task("other", Status::Ready);
    let queued = f.task("other", Status::Backlog);
    f.ok(&["queue", "add", &queued.id]);
    let pinned = f.ctx.store.get(&queued.id).unwrap().dispatch_plan;

    f.ok(&["sync"]);

    assert_eq!(
        f.ctx.store.get(&admitted.id).unwrap().status,
        Status::Running
    );
    for id in [&unqueued.id, &queued.id] {
        let waiting = f.ctx.store.get(id).unwrap();
        assert_eq!(waiting.status, Status::Ready);
        assert!(waiting.runs.is_empty());
    }
    assert!(!f.ctx.store.get(&unqueued.id).unwrap().queued);
    let waiting = f.ctx.store.get(&queued.id).unwrap();
    assert!(waiting.queued);
    assert_eq!(waiting.dispatch_plan, pinned);
    assert_eq!(f.state()["creates"], 1);
}

#[test]
fn dequeue_and_backlog_move_hold_against_auto_backlog_until_ready_move() {
    for dequeue in [false, true] {
        let mut f = queue_fixture();
        f.ctx.store.config.dispatch.auto_backlog = true;
        f.ctx.store.save_config().unwrap();
        let task = f.task("app", Status::Idea);
        f.ok(&["queue", "add", &task.id]);
        if dequeue {
            f.ok(&[
                "comment",
                &task.id,
                "owner-ok: defer Withdraw from dispatch",
            ]);
            f.ok(&["queue", "remove", &task.id]);
        } else {
            f.ok(&["move", &task.id, "backlog", "--owner-ok"]);
        }
        f.ok(&["sync"]);
        let held = f.ctx.store.get(&task.id).unwrap();
        assert_eq!(held.status, Status::Backlog);
        assert!(!held.queued);
        assert!(held.dispatch_hold);
        assert!(held.runs.is_empty());
        assert_eq!(f.state()["creates"], 0);

        f.ok(&["move", &task.id, "ready"]);
        let authorized = f.ctx.store.get(&task.id).unwrap();
        assert_eq!(authorized.status, Status::Ready);
        assert!(!authorized.dispatch_hold);
        f.ok(&["sync"]);
        assert_eq!(f.ctx.store.get(&task.id).unwrap().status, Status::Running);
        assert_eq!(f.state()["creates"], 1);
    }
}

#[test]
fn default_headroom_limits_all_ready_admissions_but_not_manual_dispatch() {
    for queued in [false, true] {
        let mut f = queue_fixture();
        f.ctx.store.config.limits = Limits::default();
        f.ctx.store.save_config().unwrap();
        assert_eq!(f.ctx.store.config.limits.running, 3);
        assert_eq!(f.ctx.store.config.limits.review, 2);
        let tasks: Vec<_> = (0..3).map(|_| f.task("app", Status::Ready)).collect();
        if queued {
            for task in &tasks {
                f.ok(&["queue", "add", &task.id]);
            }
        }

        f.ok(&["sync"]);

        let current = f.ctx.store.list().unwrap();
        assert_eq!(
            current
                .iter()
                .filter(|t| t.status == Status::Running)
                .count(),
            1
        );
        assert_eq!(
            current.iter().filter(|t| t.status == Status::Ready).count(),
            2
        );
        assert_eq!(f.state()["creates"], 1);
        assert_eq!(corral_ops::slots(&f.ctx).unwrap(), (1, 3));
        for task in current.iter().filter(|t| t.status == Status::Ready) {
            f.ok(&["dispatch", &task.id]);
        }
        assert_eq!(corral_ops::slots(&f.ctx).unwrap(), (3, 3));
        assert_eq!(f.state()["creates"], 3);
        assert_eq!(f.state()["submits"], 3);
    }
}

#[test]
fn pause_blocks_queued_and_unqueued_ready_until_resumed() {
    for queued in [false, true] {
        let f = queue_fixture();
        let task = f.task("app", Status::Ready);
        if queued {
            f.ok(&["queue", "add", &task.id]);
        }
        f.ok(&["queue", "pause"]);

        f.ok(&["sync"]);

        let waiting = f.ctx.store.get(&task.id).unwrap();
        assert_eq!(waiting.status, Status::Ready);
        assert!(waiting.runs.is_empty());
        assert_eq!(f.state()["creates"], 0);
        f.ok(&["queue", "resume"]);
        f.ok(&["sync"]);
        assert_eq!(f.ctx.store.get(&task.id).unwrap().status, Status::Running);
        assert_eq!(f.state()["creates"], 1);
    }
}

#[test]
fn unqueued_ready_waits_for_done_dependency_not_review() {
    let f = queue_fixture();
    let dependency = f.task("app", Status::Review);
    let task = f.task("other", Status::Ready);
    f.ctx
        .store
        .update(&task.id, |t| {
            t.depends_on.push(dependency.id.clone());
            Ok(())
        })
        .unwrap();

    f.ok(&["sync"]);

    let waiting = f.ctx.store.get(&task.id).unwrap();
    assert_eq!(waiting.status, Status::Ready);
    assert!(waiting.runs.is_empty());
    assert_eq!(f.state()["creates"], 0);
    f.ctx
        .store
        .update(&dependency.id, |t| {
            t.status = Status::Done;
            Ok(())
        })
        .unwrap();
    f.ok(&["sync"]);
    assert_eq!(f.ctx.store.get(&task.id).unwrap().status, Status::Running);
    assert_eq!(f.state()["creates"], 1);
}

#[test]
fn resource_pressure_blocks_queued_and_unqueued_ready_without_launching() {
    for queued in [false, true] {
        let mut f = queue_fixture();
        let task = f.task("app", Status::Ready);
        if queued {
            f.ok(&["queue", "add", &task.id]);
        }
        let mut probe = FixedResourceProbe::healthy();
        probe.0.mem_available = 0;
        f.ctx.resource_probe = Arc::new(probe);

        corral_ops::sync(&f.ctx).unwrap();

        let waiting = f.ctx.store.get(&task.id).unwrap();
        assert_eq!(waiting.status, Status::Ready);
        assert!(waiting.runs.is_empty());
        assert!(corral_ops::resources::status(&f.ctx).unwrap().held);
        assert_eq!(f.state()["creates"], 0);
    }
}

#[test]
fn legacy_backlog_migrates_once_with_pinned_route_even_without_herdr() {
    for offline in [false, true] {
        let mut f = queue_fixture();
        f.ctx.store.config.dispatch.paused = true;
        f.ctx.store.save_config().unwrap();
        let task = f.task("app", Status::Backlog);
        f.ok(&[
            "queue",
            "add",
            &task.id,
            "--profile",
            "generic",
            "--base",
            "pinned-base",
            "--headless",
        ]);
        let legacy = f
            .ctx
            .store
            .update(&task.id, |t| {
                t.status = Status::Backlog;
                t.dispatch_hold = true;
                Ok(())
            })
            .unwrap();
        if offline {
            f.ctx.herdr = Herdr::with_bin(
                f.temp.path().join("missing-herdr"),
                Some("corral-test".into()),
            );
        }

        let changes = corral_ops::sync(&f.ctx).unwrap();

        let migrated = f.ctx.store.get(&task.id).unwrap();
        assert_eq!(migrated.status, Status::Ready);
        assert!(migrated.queued);
        assert!(!migrated.dispatch_hold);
        assert_eq!(migrated.dispatch_plan, legacy.dispatch_plan);
        assert!(migrated.runs.is_empty());
        assert_eq!(
            changes
                .iter()
                .filter(|c| c.task == task.id && c.from == Status::Backlog && c.to == Status::Ready)
                .count(),
            1
        );
        corral_ops::sync(&f.ctx).unwrap();
        assert_eq!(f.ctx.store.get(&task.id).unwrap(), migrated);
        let events = f.ctx.store.events().unwrap();
        let migrations: Vec<_> = events
            .iter()
            .filter(|e| e.task.as_deref() == Some(task.id.as_str()) && e.kind == "queue_migrated")
            .collect();
        assert_eq!(migrations.len(), 1);
        assert_eq!(migrations[0].detail["from"], "backlog");
        assert_eq!(migrations[0].detail["to"], "ready");
        assert!(!f.temp.path().join("app/.worktrees").exists());
        if !offline {
            assert_eq!(f.state()["creates"], 0);
        }
    }
}

#[test]
fn foreign_sync_cannot_migrate_legacy_queue_owned_by_another_session() {
    let f = Fixture::new();
    let owner = f.task("app", Status::Ready);
    f.ok(&["dispatch", &owner.id]);
    let legacy = f.task("other", Status::Backlog);
    let legacy = f
        .ctx
        .store
        .update(&legacy.id, |t| {
            t.queued = true;
            Ok(())
        })
        .unwrap();
    let events = serde_json::to_value(f.ctx.store.events().unwrap()).unwrap();
    let inventory = f.state();

    let output = f
        .command(&["sync"])
        .env("CORRAL_HERDR_SESSION", "foreign-session")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(f.ctx.store.get(&legacy.id).unwrap(), legacy);
    assert_eq!(
        serde_json::to_value(f.ctx.store.events().unwrap()).unwrap(),
        events
    );
    assert_eq!(f.state(), inventory);
    f.ok(&["sync"]);
    assert_eq!(f.ctx.store.get(&legacy.id).unwrap().status, Status::Ready);
    assert_eq!(
        f.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.task.as_deref() == Some(legacy.id.as_str()) && e.kind == "queue_migrated")
            .count(),
        1
    );
}
