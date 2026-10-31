#![cfg(unix)] // Dispatch fixtures execute the symlinked POSIX fake engine.

use super::*;
use crate::{DispatchOpts, resources::FixedResourceProbe};
use corral_core::{AgentCmd, Run};
use corral_herdr::Herdr;
use std::path::Path;
use std::process::Stdio;

#[path = "dispatch_policy_held_tests.rs"]
mod held;
#[path = "dispatch_policy_plan_target_tests.rs"]
mod plan_targets;
#[path = "dispatch_policy_soft_tests.rs"]
mod soft_target;

struct Fixture {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.name", "Corral Test"],
            vec!["config", "user.email", "test@example.invalid"],
            vec![
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "init",
            ],
        ] {
            let result = crate::test_support::command("git", tmp.path())
                .arg("-C")
                .arg(&repo)
                .args(args)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.agents.insert(
            "fake".into(),
            AgentCmd {
                command: "true".into(),
            },
        );
        store
            .config
            .projects
            .get_mut("app")
            .unwrap()
            .default_profile = Some("fake".into());
        store.config.routing.rules.clear();
        store.config.limits.running = 5;
        store.config.limits.review = 2;
        store.save_config().unwrap();
        let engine = tmp.path().join("engine");
        std::fs::create_dir(&engine).unwrap();
        let bin = engine.join("herdr");
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dispatch_startup/herdr.sh"),
            &bin,
        )
        .unwrap();
        Self {
            _tmp: tmp,
            ctx: Ctx {
                resource_probe: FixedResourceProbe::arc(),
                herdr: Herdr::with_bin(bin, Some("corral-test".into())),
                store,
            },
        }
    }

    fn task(&self, title: &str, status: Status) -> Task {
        self.ctx
            .store
            .create(
                title,
                Some("app"),
                status,
                "## Acceptance\nWorks deterministically.\n",
            )
            .unwrap()
    }

    fn now() -> Timestamp {
        "2026-10-16T12:00:00Z".parse().unwrap()
    }

    fn save(&self) {
        self.ctx.store.save_config().unwrap();
    }

    fn grant(&mut self, tasks: &[&Task]) {
        let preview = self
            .ctx
            .store
            .preview_operating_policy(&corral_core::OperatingSelection {
                tasks: tasks.iter().map(|task| task.id.clone()).collect(),
                authority: corral_core::OperatingAuthority {
                    dispatch: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        self.ctx.store.config = self
            .ctx
            .store
            .confirm_operating_policy(&preview, "fixture owner", Self::now())
            .unwrap();
    }
}

#[test]
fn automatic_backlog_uses_priority_then_age_and_reserves_review_headroom() {
    // Given four dependency-ready tasks competing for three coding slots.
    let mut fx = Fixture::new();
    let low = fx.task("Low priority", Status::Backlog);
    let older = fx.task("Older high priority", Status::Backlog);
    let newer = fx.task("Newer high priority", Status::Backlog);
    let middle = fx.task("Middle priority", Status::Backlog);
    for (task, priority, age) in [
        (&low, "P3", 0),
        (&older, "P0", 1),
        (&newer, "P0", 2),
        (&middle, "P1", 3),
    ] {
        fx.ctx
            .store
            .update(&task.id, |t| {
                t.priority = priority.into();
                t.created = Fixture::now() + jiff::SignedDuration::from_secs(age);
                Ok(())
            })
            .unwrap();
    }
    fx.grant(&[&low, &older, &newer, &middle]);
    // When automatic admission runs.
    let changes = advance(&fx.ctx, Fixture::now()).unwrap();
    // Then ranking determines the three runs and two slots remain for finishing work.
    assert_eq!(
        changes
            .iter()
            .filter(|c| c.to == Status::Running)
            .map(|c| c.task.as_str())
            .collect::<Vec<_>>(),
        [older.id, newer.id, middle.id]
    );
    assert_eq!(
        changes.iter().map(|c| c.to).collect::<Vec<_>>(),
        [
            Status::Ready,
            Status::Running,
            Status::Ready,
            Status::Running,
            Status::Ready,
            Status::Running
        ]
    );
    assert_eq!(fx.ctx.store.get(&low.id).unwrap().status, Status::Backlog);
    assert_eq!(crate::slot_usage(&fx.ctx).unwrap().total(), 3);
    assert_eq!(free_slots(&fx.ctx).unwrap(), 0);
    let audit = automatic_dispatch_status(&fx.ctx).unwrap();
    assert_eq!(audit.recent.len(), 6);
    assert!(
        audit
            .recent
            .iter()
            .filter(|e| e.detail["action"] == "dispatch")
            .all(|e| e.detail["rank"].is_number()
                && e.detail["slots"].is_number()
                && e.detail["route"]["profile"] == "fake")
    );
}

#[test]
fn dependencies_require_done_and_dequeued_backlog_stays_held() {
    // Given blocked, held, and dependency-complete backlog entries.
    let mut fx = Fixture::new();
    let dependency = fx.task("Dependency", Status::Review);
    let blocked = fx.task("Blocked", Status::Backlog);
    fx.ctx
        .store
        .update(&blocked.id, |t| {
            t.depends_on.push(dependency.id.clone());
            Ok(())
        })
        .unwrap();
    let held = fx.task("Held", Status::Backlog);
    crate::dequeue(&fx.ctx, &held.id).unwrap();
    let done = fx.task("Completed dependency", Status::Done);
    let ready = fx.task("Dependency ready", Status::Backlog);
    fx.ctx
        .store
        .update(&ready.id, |t| {
            t.depends_on.push(done.id.clone());
            Ok(())
        })
        .unwrap();
    fx.grant(&[&blocked, &held, &ready]);
    // When the policy runs.
    let changes = advance(&fx.ctx, Fixture::now()).unwrap();
    // Then only Done satisfies admission; dequeue remains an explicit hold.
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().all(|c| c.task == ready.id));
    assert_eq!(changes[0].to, Status::Ready);
    assert_eq!(changes[1].to, Status::Running);
    assert_eq!(fx.ctx.store.get(&held.id).unwrap().status, Status::Backlog);
    assert_eq!(
        fx.ctx.store.get(&blocked.id).unwrap().status,
        Status::Backlog
    );
}

#[test]
fn never_and_suggest_modes_do_not_promote_during_sync() {
    for mode in [PromoteIdeas::Never, PromoteIdeas::Suggest] {
        // Given a qualified idea and spare capacity.
        let mut fx = Fixture::new();
        fx.ctx.store.config.dispatch.promote_ideas = mode;
        fx.save();
        let idea = fx.task("Eligible idea", Status::Idea);
        fx.grant(&[&idea]);
        // When sync evaluates automatic admission.
        let changes = crate::sync_at(&fx.ctx, Fixture::now()).unwrap();
        // Then only auto mode may change an idea.
        assert!(changes.is_empty());
        assert_eq!(fx.ctx.store.get(&idea.id).unwrap().status, Status::Idea);
        assert!(
            automatic_dispatch_status(&fx.ctx)
                .unwrap()
                .recent
                .is_empty()
        );
    }
}

#[test]
fn auto_promotes_and_dispatches_even_when_backlog_automation_is_disabled() {
    // Given auto ideas but manual-only existing backlog.
    let mut fx = Fixture::new();
    fx.ctx.store.config.dispatch.promote_ideas = PromoteIdeas::Auto;
    fx.ctx.store.config.dispatch.auto_backlog = false;
    fx.save();
    let backlog = fx.task("Manual backlog", Status::Backlog);
    let idea = fx.task("Automatic idea", Status::Idea);
    fx.grant(&[&backlog, &idea]);
    // When the policy runs.
    let changes = advance(&fx.ctx, Fixture::now()).unwrap();
    // Then promotion and dispatch are separately audited, with no manual backlog admission.
    assert_eq!(
        changes.iter().map(|c| c.to).collect::<Vec<_>>(),
        [Status::Backlog, Status::Ready, Status::Running]
    );
    assert_eq!(fx.ctx.store.get(&idea.id).unwrap().status, Status::Running);
    assert_eq!(
        fx.ctx.store.get(&backlog.id).unwrap().status,
        Status::Backlog
    );
    let events = fx.ctx.store.events().unwrap();
    assert_eq!(promotions_today(&events, Fixture::now()), 1);
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "dispatch_policy")
            .count(),
        3
    );
}

#[test]
fn daily_cap_is_durable_across_syncs_and_resets_at_utc_midnight() {
    // Given a one-promotion daily limit and two eligible ideas.
    let mut fx = Fixture::new();
    fx.ctx.store.config.dispatch.promote_ideas = PromoteIdeas::Auto;
    fx.ctx.store.config.dispatch.max_auto_promotions_per_day = 1;
    fx.save();
    let first = fx.task("First idea", Status::Idea);
    let second = fx.task("Second idea", Status::Idea);
    fx.grant(&[&first, &second]);
    advance(&fx.ctx, Fixture::now()).unwrap();
    let reopened = Ctx {
        store: Store::open(fx.ctx.store.root()).unwrap(),
        ..fx.ctx.clone()
    };
    // When another sync occurs the same UTC day, then the following day.
    assert!(advance(&reopened, Fixture::now()).unwrap().is_empty());
    let changes = advance(
        &reopened,
        Fixture::now() + jiff::SignedDuration::from_hours(24),
    )
    .unwrap();
    // Then the cap persists across processes but the next UTC date gets a fresh budget.
    assert_eq!(fx.ctx.store.get(&first.id).unwrap().status, Status::Running);
    assert_eq!(
        changes.iter().map(|c| c.task.as_str()).collect::<Vec<_>>(),
        [&second.id, &second.id, &second.id]
    );
    assert_eq!(
        promotions_today(&fx.ctx.store.events().unwrap(), Fixture::now()),
        1
    );
}

#[test]
fn resource_release_and_completed_runs_refill_capacity_on_sync() {
    // Given a backlog held by machine pressure.
    let mut fx = Fixture::new();
    let task = fx.task("Waiting for machine", Status::Backlog);
    fx.grant(&[&task]);
    let mut busy = FixedResourceProbe::healthy();
    busy.0.load_per_core = 100.0;
    fx.ctx.resource_probe = std::sync::Arc::new(busy);
    assert!(crate::sync_at(&fx.ctx, Fixture::now()).unwrap().is_empty());
    fx.ctx.resource_probe = FixedResourceProbe::arc();
    // When the next periodic sync sees recovered resources.
    let changes = crate::sync_at(&fx.ctx, Fixture::now()).unwrap();
    // Then capacity is filled through the real sync path.
    assert!(
        changes
            .iter()
            .any(|c| c.task == task.id && c.to == Status::Running)
    );
}

#[test]
fn running_gates_use_reserved_headroom_instead_of_double_reserving_it() {
    // Given two coding runs and one live gate within a five-slot shared pool.
    let fx = Fixture::new();
    for n in 0..2 {
        let task = fx.task(&format!("Running {n}"), Status::Running);
        if n == 0 {
            fx.ctx
                .store
                .update(&task.id, |t| {
                    let run: Run = serde_json::from_value(json!({
                        "id":"R1","agent":"fake","mode":"interactive","worktree":"/unused",
                        "branch":"main","base":"main","base_sha":"head","started":Fixture::now(),
                        "gate":{"result":"running","sha":"head","at":Fixture::now()}
                    }))
                    .unwrap();
                    t.runs.push(run);
                    Ok(())
                })
                .unwrap();
        }
    }
    // When automatic capacity is calculated.
    let free = free_slots(&fx.ctx).unwrap();
    // Then the live gate shares its worker's slot, so it consumes neither the pool nor the
    // two reserved finishing slots: 5 - 2 workers - 2 reserved.
    assert_eq!(free, 1);
}

#[test]
fn automatic_claim_rechecks_headroom_even_if_manual_work_takes_the_gap() {
    // Given an earlier free-capacity observation followed by three manual claims.
    let mut fx = Fixture::new();
    let candidate = fx.task("Automatic candidate", Status::Ready);
    fx.grant(&[&candidate]);
    assert_eq!(free_slots(&fx.ctx).unwrap(), 3);
    for n in 0..3 {
        let task = fx.task(&format!("Manual task {n}"), Status::Ready);
        crate::dispatch(&fx.ctx, &task.id, &DispatchOpts::default()).unwrap();
    }
    // When a stale automatic decision attempts its actual claim.
    let result = crate::dispatch::dispatch_automatic(
        &fx.ctx,
        &candidate.id,
        json!({"rank":1, "source":"backlog"}),
        Fixture::now(),
    );
    // Then it refuses atomically without reserving or queueing the candidate.
    assert!(result.is_err());
    let current = fx.ctx.store.get(&candidate.id).unwrap();
    assert!(current.launch.is_none());
    assert!(!current.queued);
}

#[test]
fn malformed_backlog_does_not_block_other_dependency_ready_work() {
    // Given a higher-ranked broken dependency graph and valid backlog work.
    let mut fx = Fixture::new();
    let broken = fx.task("Broken graph", Status::Backlog);
    fx.ctx
        .store
        .update(&broken.id, |t| {
            t.priority = "P0".into();
            t.depends_on.push("T-999".into());
            Ok(())
        })
        .unwrap();
    let good = fx.task("Valid backlog", Status::Backlog);
    fx.grant(&[&broken, &good]);
    // When the policy evaluates both.
    let changes = advance(&fx.ctx, Fixture::now()).unwrap();
    // Then the error is visible and does not leave usable slots idle.
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().all(|c| c.task == good.id));
    assert_eq!(changes[0].to, Status::Ready);
    assert_eq!(changes[1].to, Status::Running);
    assert!(
        fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.task.as_deref() == Some(&broken.id) && e.kind == "dispatch_policy_error")
    );
}

#[test]
fn project_override_can_disable_default_backlog_admission() {
    // Given default-on global policy but an explicit project opt-out.
    let mut fx = Fixture::new();
    fx.ctx
        .store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .dispatch
        .auto_backlog = Some(false);
    fx.save();
    let task = fx.task("Manual project backlog", Status::Backlog);
    fx.grant(&[&task]);
    // When automatic admission runs.
    let changes = advance(&fx.ctx, Fixture::now()).unwrap();
    // Then effective project policy wins without queue authorization.
    assert!(changes.is_empty());
    let task = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(task.status, Status::Backlog);
    assert!(!task.queued);
}

#[test]
fn concurrent_policy_passes_cannot_exceed_the_daily_cap() {
    // Given two eligible Ideas and one shared promotion budget.
    let mut fx = Fixture::new();
    fx.ctx.store.config.dispatch.promote_ideas = PromoteIdeas::Auto;
    fx.ctx.store.config.dispatch.max_auto_promotions_per_day = 1;
    fx.save();
    let first = fx.task("First concurrent idea", Status::Idea);
    let second = fx.task("Second concurrent idea", Status::Idea);
    fx.grant(&[&first, &second]);
    let start = std::sync::Barrier::new(2);
    // When two sync callers attempt admission together.
    std::thread::scope(|scope| {
        let left = scope.spawn(|| {
            start.wait();
            advance(&fx.ctx, Fixture::now()).unwrap()
        });
        let right = scope.spawn(|| {
            start.wait();
            advance(&fx.ctx, Fixture::now()).unwrap()
        });
        left.join().unwrap();
        right.join().unwrap();
    });
    // Then serialized decisions reserve exactly one promotion and one run.
    assert_eq!(
        promotions_today(&fx.ctx.store.events().unwrap(), Fixture::now()),
        1
    );
    assert_eq!(
        fx.ctx
            .store
            .list()
            .unwrap()
            .iter()
            .filter(|t| t.status == Status::Running)
            .count(),
        1
    );
}

#[test]
fn completed_coding_work_is_replaced_on_the_next_sync() {
    // Given one coding slot, an active run, and one waiting backlog task.
    let mut fx = Fixture::new();
    fx.ctx.store.config.limits.running = 3;
    fx.save();
    let active = fx.task("Active task", Status::Ready);
    crate::dispatch(&fx.ctx, &active.id, &DispatchOpts::default()).unwrap();
    let next = fx.task("Next backlog", Status::Backlog);
    fx.grant(&[&next]);
    assert_eq!(free_slots(&fx.ctx).unwrap(), 0);
    fx.ctx
        .store
        .update(&active.id, |task| {
            task.status = Status::Done;
            task.current_run_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    // When sync reconciles the released capacity.
    let changes = crate::sync_at(&fx.ctx, Fixture::now()).unwrap();
    // Then the waiting task starts without a PM action.
    assert!(
        changes
            .iter()
            .any(|c| c.task == next.id && c.to == Status::Running)
    );
}
