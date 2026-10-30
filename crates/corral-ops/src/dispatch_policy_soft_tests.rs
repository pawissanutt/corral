//! T-537: automatic admission fills to `limits.running_target`; owner work, P0 and red-main
//! tasks may use the headroom up to the hard `limits.running`.
use super::*;

/// Hard limit 5, soft target 4 and two reserved finishing slots: automatic work gets two
/// slots, exempt work three.
fn soft_fixture() -> Fixture {
    let mut fx = Fixture::new();
    fx.ctx.store.config.limits.running_target = Some(4);
    fx.save();
    fx
}

fn status(fx: &Fixture, id: &str) -> Status {
    fx.ctx.store.get(id).unwrap().status
}

fn used(fx: &Fixture) -> usize {
    crate::slot_usage(&fx.ctx).unwrap().total()
}

fn set(fx: &Fixture, id: &str, priority: &str, label: Option<&str>) {
    fx.ctx
        .store
        .update(id, |t| {
            t.priority = priority.into();
            t.labels.extend(label.map(String::from));
            Ok(())
        })
        .unwrap();
}

fn manual(fx: &Fixture, title: &str) -> Task {
    let task = fx.task(title, Status::Ready);
    crate::dispatch(&fx.ctx, &task.id, &DispatchOpts::default()).unwrap();
    task
}

fn finish(fx: &Fixture, id: &str) {
    fx.ctx
        .store
        .update(id, |task| {
            task.status = Status::Done;
            task.current_run_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
            Ok(())
        })
        .unwrap();
}

fn reason(fx: &Fixture, id: &str) -> String {
    automatic_capacity_reason(&fx.ctx.store, &fx.ctx.store.get(id).unwrap())
        .unwrap()
        .unwrap_or_default()
}

#[test]
fn automatic_backlog_stops_at_soft_target_while_p0_and_manual_use_headroom_up_to_hard() {
    // Given three ordinary Backlog tasks.
    let mut fx = soft_fixture();
    let first = fx.task("First", Status::Backlog);
    let second = fx.task("Second", Status::Backlog);
    let third = fx.task("Third", Status::Backlog);
    fx.grant(&[&first, &second, &third]);
    // When automatic admission runs, it fills only to the soft target and explains the wait.
    advance(&fx.ctx, Fixture::now()).unwrap();
    assert_eq!(status(&fx, &first.id), Status::Running);
    assert_eq!(status(&fx, &second.id), Status::Running);
    assert_eq!(status(&fx, &third.id), Status::Backlog);
    assert_eq!(free_slots(&fx.ctx).unwrap(), 0);
    assert!(reason(&fx, &third.id).contains("at soft target"));
    // When P0 work arrives, the first takes the headroom; the second is refused at hard.
    let urgent = fx.task("Urgent", Status::Backlog);
    let urgent2 = fx.task("Urgent two", Status::Backlog);
    set(&fx, &urgent.id, "P0", None);
    set(&fx, &urgent2.id, "P0", None);
    fx.grant(&[&third, &urgent, &urgent2]);
    advance(&fx.ctx, Fixture::now()).unwrap();
    assert_eq!(status(&fx, &urgent.id), Status::Running);
    assert_eq!(status(&fx, &urgent2.id), Status::Backlog);
    assert_eq!(status(&fx, &third.id), Status::Backlog);
    assert!(
        reason(&fx, &urgent2.id).contains("waiting for automatic coding capacity"),
        "{}",
        reason(&fx, &urgent2.id)
    );
    // Then manual dispatch may fill to the hard limit, but not past it.
    manual(&fx, "Manual one");
    manual(&fx, "Manual two");
    assert_eq!(used(&fx), 5);
    let over = fx.task("Manual three", Status::Ready);
    assert!(crate::dispatch(&fx.ctx, &over.id, &DispatchOpts::default()).is_err());
    assert_eq!(status(&fx, &over.id), Status::Ready);
    assert_eq!(used(&fx), 5);
}

#[test]
fn p0_and_red_main_ideas_use_headroom_while_ordinary_ideas_wait() {
    // Given auto promotion and two manual runs filling the soft target (one hard slot left).
    let mut fx = soft_fixture();
    fx.ctx.store.config.dispatch.promote_ideas = PromoteIdeas::Auto;
    fx.save();
    let ordinary = fx.task("Ordinary idea", Status::Idea);
    let urgent = fx.task("Urgent idea", Status::Idea);
    set(&fx, &urgent.id, "P0", None);
    let first = manual(&fx, "Manual one");
    manual(&fx, "Manual two");
    fx.grant(&[&ordinary, &urgent]);
    assert_eq!(free_slots(&fx.ctx).unwrap(), 0);
    // When the PM observes and automatic admission runs.
    let cards: Vec<_> = crate::pm_next::observe::observations(&fx.ctx)
        .unwrap()
        .into_iter()
        .filter_map(|o| match o {
            crate::PmObservation::IdleIdeas { ideas, .. } => Some(ideas),
            _ => None,
        })
        .collect();
    advance(&fx.ctx, Fixture::now()).unwrap();
    // Then only the P0 Idea is offered and promoted into the headroom.
    assert_eq!(cards, [vec![urgent.id.clone()]]);
    assert_eq!(status(&fx, &urgent.id), Status::Running);
    assert_eq!(status(&fx, &ordinary.id), Status::Idea);
    // When a manual slot frees, a red-main Idea takes headroom; the ordinary one still waits.
    finish(&fx, &first.id);
    let red = fx.task("Red main idea", Status::Idea);
    set(&fx, &red.id, "P2", Some("red_main"));
    fx.grant(&[&ordinary, &red]);
    advance(&fx.ctx, Fixture::now()).unwrap();
    assert_eq!(status(&fx, &red.id), Status::Running);
    assert_eq!(status(&fx, &ordinary.id), Status::Idea);
    assert_eq!(used(&fx), 3);
}

#[test]
fn automatic_ready_waits_at_soft_target_while_owner_queue_uses_headroom() {
    // Given two manual runs filling the soft target and an automatically admitted Ready task.
    let mut fx = soft_fixture();
    let automatic = fx.task("Automatic ready", Status::Ready);
    fx.grant(&[&automatic]);
    let first = manual(&fx, "Manual one");
    let second = manual(&fx, "Manual two");
    // When the Ready queue drains.
    crate::queue_drain::start_queued(&fx.ctx, Fixture::now()).unwrap();
    // Then automatic Ready work stays unlaunched with the soft-target reason.
    assert_eq!(status(&fx, &automatic.id), Status::Ready);
    assert!(fx.ctx.store.get(&automatic.id).unwrap().launch.is_none());
    let entry = crate::queue::dispatch_queue(&fx.ctx)
        .unwrap()
        .into_iter()
        .find(|e| e.id == automatic.id)
        .unwrap();
    assert!(entry.reason.contains("at soft target"), "{}", entry.reason);
    // When the owner queues work, it launches in the same headroom.
    let owner = fx.task("Owner queued", Status::Ready);
    crate::queue::enqueue(&fx.ctx, &owner.id, &DispatchOpts::default()).unwrap();
    crate::queue_drain::start_queued(&fx.ctx, Fixture::now()).unwrap();
    assert_eq!(status(&fx, &owner.id), Status::Running);
    assert_eq!(status(&fx, &automatic.id), Status::Ready);
    assert_eq!(used(&fx), 3);
    // When capacity is released below the soft target, the automatic task is admitted.
    finish(&fx, &first.id);
    finish(&fx, &second.id);
    crate::queue_drain::start_queued(&fx.ctx, Fixture::now()).unwrap();
    assert_eq!(status(&fx, &automatic.id), Status::Running);
    assert_eq!(used(&fx), 2);
}

#[test]
fn a_blocked_ready_queue_resolves_the_operating_policy_once_per_drain() {
    // Given automatic Ready tasks that one drain queued behind a full soft target.
    let mut fx = soft_fixture();
    let waiting: Vec<Task> = (0..4)
        .map(|i| fx.task(&format!("Waiting {i}"), Status::Ready))
        .collect();
    fx.grant(&waiting.iter().collect::<Vec<_>>());
    manual(&fx, "Manual one");
    manual(&fx, "Manual two");
    crate::queue_drain::start_queued(&fx.ctx, Fixture::now()).unwrap();
    // When the queue drains again with every task still blocked (T-547).
    let resolves = || corral_core::work::count(corral_core::work::Work::PolicyResolve);
    let before = resolves();
    crate::queue_drain::start_queued(&fx.ctx, Fixture::now()).unwrap();
    // Then each was checked and skipped against one policy resolution, not one per check.
    for task in &waiting {
        let current = fx.ctx.store.get(&task.id).unwrap();
        assert!(current.queued && current.dispatch_plan.is_some());
        assert_eq!(current.status, Status::Ready);
    }
    assert_eq!(used(&fx), 2);
    assert_eq!(resolves() - before, 1);
}

#[test]
fn interrupted_automatic_launch_recovery_respects_soft_target_without_leaking_a_slot() {
    // Given an engine that refuses workspace creation once a marker file exists.
    let mut fx = soft_fixture();
    let engine = fx._tmp.path().join("engine");
    let real = engine.join("real-herdr");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dispatch_startup/herdr.sh"),
        &real,
    )
    .unwrap();
    std::fs::remove_file(engine.join("herdr")).unwrap();
    let wrapper = format!(
        "#!/usr/bin/env bash\nif [ -f '{0}/fail-create' ]; then case \"$*\" in *'workspace create'*) \
         echo '{{\"error\":{{\"code\":\"injected\",\"message\":\"injected\"}}}}'; exit 1;; esac; fi\n\
         exec bash '{1}' \"$@\"\n",
        engine.display(),
        real.display()
    );
    std::fs::write(engine.join("herdr"), wrapper).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(engine.join("herdr"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(engine.join("fail-create"), "").unwrap();
    let automatic = fx.task("Automatic launch", Status::Ready);
    fx.grant(&[&automatic]);
    // When its automatic launch is interrupted after the claim.
    let launched = crate::dispatch::dispatch_automatic(
        &fx.ctx,
        &automatic.id,
        json!({"rank":1, "source":"backlog"}),
        Fixture::now(),
    );
    assert!(launched.is_err());
    let interrupted = fx.ctx.store.get(&automatic.id).unwrap();
    let claim = interrupted.launch.as_ref().unwrap();
    assert!(!claim.reserved, "a failed launch must release its slot");
    assert_eq!(used(&fx), 0);
    // When the soft target fills before recovery runs.
    std::fs::remove_file(engine.join("fail-create")).unwrap();
    let first = manual(&fx, "Manual one");
    manual(&fx, "Manual two");
    crate::queue_drain::start_queued(&fx.ctx, Fixture::now()).unwrap();
    // Then recovery does not bypass the soft target or hold a reservation.
    let held = fx.ctx.store.get(&automatic.id).unwrap();
    assert_ne!(held.status, Status::Running);
    assert!(!held.launch.as_ref().unwrap().reserved);
    assert_eq!(used(&fx), 2);
    assert!(
        reason(&fx, &automatic.id).contains("at soft target"),
        "{}",
        reason(&fx, &automatic.id)
    );
    // When capacity is released, recovery reuses the same run.
    finish(&fx, &first.id);
    crate::queue_drain::start_queued(&fx.ctx, Fixture::now()).unwrap();
    let recovered = fx.ctx.store.get(&automatic.id).unwrap();
    assert_eq!(recovered.status, Status::Running);
    assert_eq!(recovered.runs.len(), 1);
    assert!(recovered.launch.is_none());
    assert_eq!(used(&fx), 2);
}
