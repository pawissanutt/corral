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
    let fail = |reason: &str| {
        let script = FAKE_HERDR.replace(
            "\"tab create\")",
            &format!("\"tab create\") printf '%s\\n' '{{\"id\":\"fake\",\"error\":{{\"code\":\"failed\",\"message\":\"{reason}\"}}}}'; exit 1 ;;\n\"unused\")"),
        );
        std::fs::write(fx.bin_dir.join("herdr"), script).unwrap();
    };
    fail("first failure");
    let (tx, rx) = std::sync::mpsc::channel();
    let _capture = corral_ops::capture_messages(tx);
    // When the same failure occurs repeatedly, then changes and recurs.
    fx.sync();
    fx.sync();
    assert_eq!(fx.events("autopilot_error"), 1);
    fail("different failure");
    fx.sync();
    fail("first failure");
    fx.sync();
    // Then retryable failures remain in Review, while event history records each change once.
    assert_eq!(fx.events("autopilot_error"), 3);
    assert_eq!(fx.task().status, Status::Review);
    let messages: Vec<_> = rx.try_iter().collect();
    assert_eq!(messages.len(), 4);
    assert!(messages.iter().all(|message| {
        message.task.as_deref() == Some(&fx.id) && message.source == "autopilot"
    }));
}
