use std::sync::{Arc, RwLock};

use corral_core::{DiskSpace, ResourceSnapshot, ReviewWork};
use corral_ops::resources::{FixedResourceProbe, ResourceProbe};

use super::*;

#[path = "resource_attention.rs"]
mod attention;

#[derive(Debug)]
struct Probe(RwLock<ResourceSnapshot>);

impl ResourceProbe for Probe {
    fn sample(&self, _: &Store) -> anyhow::Result<ResourceSnapshot> {
        Ok(self.0.read().unwrap().clone())
    }
}

impl Probe {
    fn install(fx: &mut Fx) -> Arc<Self> {
        let probe = Arc::new(Self(RwLock::new(FixedResourceProbe::healthy().0)));
        fx.ctx.resource_probe = probe.clone();
        probe
    }

    fn load(&self, per_core: f64) {
        let mut sample = self.0.write().unwrap();
        sample.load_per_core = per_core;
        sample.load_one = per_core * f64::from(sample.cores);
    }
}

#[test]
fn queued_dispatch_releases_automatically_after_boundary_recovery() {
    // Given an authorized queued task and an overloaded, injectable host.
    let mut fx = Fx::new(|_| {});
    // One existing agent, one queued agent, and two reserved finishing slots.
    fx.ctx.store.config.limits.running = 4;
    fx.ctx.store.save_config().unwrap();
    let probe = Probe::install(&mut fx);
    probe.load(1.2);
    let task = fx
        .ctx
        .store
        .create("Next", Some("app"), Status::Ready, "")
        .unwrap();
    let opts = DispatchOpts {
        profile: Some("fake".into()),
        queue_if_blocked: true,
        ..Default::default()
    };
    // When dispatch and sync observe overload followed by a boundary sample.
    let result = corral_ops::dispatch(&fx.ctx, &task.id, &opts).unwrap();
    assert!(matches!(result, DispatchOutcome::Queued { .. }));
    probe.load(0.99);
    fx.sync();
    // Then no run or failed dispatch has been recorded.
    assert!(fx.ctx.store.get(&task.id).unwrap().runs.is_empty());
    assert_eq!(fx.events("dispatch_failed"), 0);
    let entries = corral_ops::dispatch_queue(&fx.ctx).unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.id == task.id && entry.reason.starts_with("waiting: machine busy")),
        "{entries:?}"
    );

    // When a recovery sample arrives, the next sync drains the same authorization.
    probe.load(0.5);
    fx.sync();
    let task = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.runs.len(), 1);
}

#[test]
fn autopilot_gate_wait_preserves_rounds_and_starts_when_host_recovers() {
    // Given a completed run that requires a gate.
    let mut fx = Fx::new(|policy| {
        policy.gate = gate(&["git merge-base --is-ancestor main HEAD"]);
        policy.accept = Some(Accept::Owner);
    });
    fx.change("widget.txt", "widget\n");
    fx.done();
    let probe = Probe::install(&mut fx);
    probe.load(2.0);
    // When sync tries to start the gate on a busy host.
    let task = fx.sync();
    // Then it records waiting, not a gate attempt or failed round.
    assert_eq!(task.review_wait, Some(ReviewWork::Gate));
    assert!(fx.run().gate.is_none());
    assert_eq!(task.rounds, 0);
    assert_eq!(fx.run().failures, 0);
    assert_eq!(fx.events("autopilot_error"), 0);
    assert!(
        corral_ops::dispatch_queue(&fx.ctx)
            .unwrap()
            .iter()
            .any(|entry| entry.id == fx.id),
        "pending gate must remain in the dispatch queue"
    );

    // When main moves during the wait and pressure clears, integrate before admission.
    fx.commit_on_main("other.txt", "landed\n", "main moved during gate wait");
    let base = git(&fx.app, &["rev-parse", "main"]);
    probe.load(0.5);
    let task = fx.sync();
    assert_eq!(task.review_wait, None);
    let gate = fx.run().gate.unwrap();
    assert_eq!(gate.result, GateResult::Pass);
    assert_eq!(gate.identity.unwrap().integration_base, base);
    assert_eq!(fx.events("gate_started"), 1);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewed));
    assert_eq!(task.rounds, 0);
}

#[test]
fn reviewer_wait_does_not_consume_a_review_attempt() {
    // Given a completed run with a passing gate but no reviewer yet.
    let mut fx = Fx::new(|policy| {
        policy.gate = gate(&["true"]);
        policy.reviewer = Some("rev".into());
    });
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap();
    fx.done();
    let probe = Probe::install(&mut fx);
    probe.load(1.5);
    // When the gate is reused while the host is busy.
    let task = fx.sync();
    // Then review waits without creating a reviewer or changing rounds.
    assert_eq!(task.review_wait, Some(ReviewWork::Reviewer));
    assert!(fx.run().review.is_none());
    assert!(fx.run().review_runs.is_empty());
    assert_eq!(task.rounds, 0);

    // When pressure clears, the pending reviewer starts automatically.
    probe.load(0.5);
    fx.sync();
    assert_eq!(fx.run().review.unwrap().reviewer, "rev");
    assert_eq!(fx.run().review_runs.len(), 1);
}

#[test]
fn manual_resource_overrides_do_not_bypass_other_admission_limits() {
    // Given host overload and an otherwise eligible task.
    let mut fx = Fx::new(|policy| policy.gate = gate(&["true"]));
    let probe = Probe::install(&mut fx);
    probe.load(2.0);
    let task = fx
        .ctx
        .store
        .create("Manual", Some("app"), Status::Ready, "")
        .unwrap();
    let opts = DispatchOpts {
        profile: Some("fake".into()),
        ..Default::default()
    };
    // When manual operations omit the override.
    assert!(corral_ops::dispatch(&fx.ctx, &task.id, &opts).is_err());
    assert!(corral_ops::gate_start(&fx.ctx, &fx.id).is_err());
    // Then neither operation has created a run/gate.
    assert!(fx.ctx.store.get(&task.id).unwrap().runs.is_empty());
    assert!(fx.run().gate.is_none());

    // When the owner explicitly forces machine admission, both can proceed.
    let forced = DispatchOpts {
        force_resources: true,
        ..opts
    };
    assert!(matches!(
        corral_ops::dispatch(&fx.ctx, &task.id, &forced).unwrap(),
        DispatchOutcome::Started { .. }
    ));
    corral_ops::gate_start_with_force(&fx.ctx, &fx.id, true).unwrap();
    assert_eq!(fx.run().gate.unwrap().result, GateResult::Running);
    // The host override still cannot create another gate in exhausted capacity.
    fx.ctx.store.config.limits.running = 2;
    fx.ctx.store.save_config().unwrap();
    assert!(corral_ops::gate_start_with_force(&fx.ctx, &task.id, true).is_err());
}

#[test]
fn memory_pressure_escalates_once_until_it_clears() {
    // Given a running task with measurable worktree and target files.
    let mut fx = Fx::new(|_| {});
    let target = fx.run().worktree.join("target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("build"), [0_u8; 512]).unwrap();
    let probe = Probe::install(&mut fx);
    probe.0.write().unwrap().mem_available = 1_000_000_000;
    // When successive syncs see the same pressure episode.
    fx.sync();
    let task = fx.sync();
    // Then attention and one escalation persist even while the agent is still present.
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(fx.events("escalated"), 1);
    let events = fx.ctx.store.events().unwrap();
    let event = events
        .iter()
        .find(|event| event.kind == "escalated")
        .unwrap();
    assert_eq!(event.detail["source"], "resources");
    assert_eq!(event.detail["snapshot"]["mem_available"], 1_000_000_000_u64);
    assert_eq!(
        event.detail["largest_worktrees"][0]["target_bytes"],
        512_u64
    );
    assert!(
        event.detail["largest_worktrees"][0]["worktree_bytes"]
            .as_u64()
            .unwrap()
            >= 512
    );

    // When memory recovers, attention clears without resending a notification.
    probe.0.write().unwrap().mem_available = u64::MAX;
    assert_eq!(fx.sync().status, Status::Running);
    assert_eq!(fx.events("resources_cleared"), 1);
    // A later episode gets a fresh escalation.
    probe.0.write().unwrap().mem_available = 1_000_000_000;
    fx.sync();
    assert_eq!(fx.events("escalated"), 2);
}

#[test]
fn disk_pressure_escalates_but_cpu_pressure_only_holds_admission() {
    // Given a running task and CPU overload alone.
    let mut fx = Fx::new(|_| {});
    let probe = Probe::install(&mut fx);
    probe.load(2.0);
    // When sync sees CPU pressure.
    assert_eq!(fx.sync().status, Status::Running);
    // Then no owner escalation occurs.
    assert_eq!(fx.events("escalated"), 0);

    // When a worktree filesystem also drops below the disk floor.
    probe.0.write().unwrap().disks = vec![DiskSpace {
        path: fx.run().worktree,
        free_bytes: 1,
    }];
    // Then the task needs owner attention, exactly once.
    assert_eq!(fx.sync().status, Status::NeedsMe);
    fx.sync();
    assert_eq!(fx.events("escalated"), 1);
}
