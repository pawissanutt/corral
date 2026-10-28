//! A real runner exits between passes, rather than inside fake herdr's `pane run`.
use super::*;

#[test]
fn next_board_pass_collects_a_gate_that_exited_between_passes_and_starts_review() {
    finished_between_passes(false);
}

#[test]
fn next_board_pass_collects_finished_gate_even_while_inventory_is_busy() {
    finished_between_passes(true);
}

fn finished_between_passes(busy_inventory: bool) {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "spins\n");
    fx.done();
    std::fs::write(fx.bin_dir.join("defer-gate"), "").unwrap();

    // A normal board/autopilot pass launches the gate but cannot collect it yet.
    corral_ops::sync_with_attention(&fx.ctx).unwrap();
    assert_eq!(fx.stage(), Some(AutoStage::Gating));
    assert_eq!(fx.run().gate.unwrap().result, GateResult::Running);
    let paths = corral_ops::gate_paths(&fx.ctx, &fx.id, "R1");
    assert!(!paths.result.exists());
    assert_eq!(fx.events("gate_started"), 1);

    // Execute the generated runner to completion with NO sync in flight. Waiting on
    // child exit is the barrier: no sleeps, polling, fabricated receipt or manual collect.
    let output = support::command("bash", &fx.app)
        .arg(fx.bin_dir.join("deferred-gate.sh"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(paths.result.is_file());
    assert!(
        std::fs::read_to_string(&paths.log)
            .unwrap()
            .contains("corral gate: pass")
    );
    assert_eq!(fx.run().gate.unwrap().result, GateResult::Running);

    // The SAME long-lived context used by the board must discover the receipt,
    // advance autopilot and start the reviewer on its next ordinary pass.
    if busy_inventory {
        // Do not bypass an ownership migration merely because a result exists.
        let ownership = fx.ctx.store.try_lock("session-ownership").unwrap().unwrap();
        corral_ops::sync_with_attention(&fx.ctx).unwrap();
        assert_eq!(fx.run().gate.unwrap().result, GateResult::Running);
        drop(ownership);
    }
    let inventory_lease =
        busy_inventory.then(|| fx.ctx.store.try_lock("pane-liveness").unwrap().unwrap());
    corral_ops::sync_with_attention(&fx.ctx).unwrap();
    assert_eq!(fx.run().gate.unwrap().result, GateResult::Pass);
    if busy_inventory {
        // Receipt collection needs no pane sample. Launching the reviewer does:
        // retain the launch guard, then advance when the contended inventory is free.
        assert_eq!(fx.stage(), Some(AutoStage::Gating));
        drop(inventory_lease);
        corral_ops::sync_with_attention(&fx.ctx).unwrap();
    }
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    assert_eq!(
        fx.events("gate_started"),
        1,
        "must reuse the finished attempt"
    );
    assert!(fx.run().review.is_some());
    assert!(fx.task().artifacts.iter().any(|a| a.title == "Gate: pass"));
}
