use super::*;

#[test]
fn unresolved_vendor_escalates_once_instead_of_retrying_sync() {
    for stage in [None, Some(AutoStage::Reviewing)] {
        // Given a finished historical run whose profile cannot identify a vendor.
        let fx = Fx::new(|review| {
            review.gate = gate(&["true"]);
            review.reviewer = Some("cross-vendor".into());
        });
        fx.done();
        fx.ctx
            .store
            .update(&fx.id, |task| {
                let run = task.current_run_mut().unwrap();
                run.vendor = None;
                run.model = None;
                run.agent = "missing-profile".into();
                run.autopilot = stage.map(|stage| corral_core::AutopilotState {
                    stage,
                    reason: None,
                    sha: None,
                    at: now(),
                });
                Ok(())
            })
            .unwrap();
        // When sync repeats, even through fresh operation contexts.
        for _ in 0..3 {
            fx.sync();
        }
        // Then the closed run stays in Needs me with one escalation and notification.
        assert_eq!(fx.task().status, Status::NeedsMe);
        assert_eq!(fx.stage(), Some(AutoStage::Escalated));
        assert!(fx.reason().contains("missing-profile"));
        assert_eq!(fx.transitions(), ["escalated"]);
        assert_eq!(fx.herdr_log("notify.log").lines().count(), 1);
        assert_eq!(fx.events("autopilot_error"), 0);
        assert!(fx.run().review_runs.is_empty());
    }
}

#[test]
fn transient_autopilot_errors_are_logged_once_until_the_reason_changes() {
    // Given a finished run whose engine rejects reviewer tab creation.
    let fx = Fx::new(|review| review.reviewer = Some("rev".into()));
    fx.done();
    let failure = fx.bin_dir.join("tab-error");
    let fail = |reason: &str| {
        std::fs::write(&failure, format!("{reason}\n")).unwrap();
    };
    fail("first failure");
    let (tx, rx) = std::sync::mpsc::channel();
    let _capture = corral_ops::capture_messages(tx);
    // When the same failure occurs repeatedly, then changes and recurs.
    fx.sync();
    fx.sync();
    assert_eq!(fx.events("autopilot_error"), 1);
    // Model a parallel fork retaining the writer: mutable fixture data must not block exec.
    let _inherited_writer = std::fs::OpenOptions::new()
        .write(true)
        .open(&failure)
        .unwrap();
    fail("different failure");
    fx.sync();
    fail("first failure");
    fx.sync();
    // Then retryable failures remain in Review, while event history records each change once.
    let messages: Vec<_> = rx.try_iter().collect();
    assert_eq!(fx.events("autopilot_error"), 3, "{messages:#?}");
    assert_eq!(fx.task().status, Status::Review);
    assert_eq!(messages.len(), 4);
    for (message, reason) in messages.iter().zip([
        "first failure",
        "first failure",
        "different failure",
        "first failure",
    ]) {
        assert_eq!(message.task.as_deref(), Some(fx.id.as_str()));
        assert_eq!(message.source, "autopilot");
        assert!(message.text.ends_with(reason), "{message:?}");
    }
}
