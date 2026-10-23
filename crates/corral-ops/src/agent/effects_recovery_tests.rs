//! Owed post-commit effects recovered by sync (`reconcile_task`) in a fresh Ctx without
//! replaying the original call: every receipt home, the receipt's named run and review child
//! only, and a review capture that stays owed until it is durably complete.
use std::path::PathBuf;

use super::*;

fn capture_file(fx: &Fx, child: &str) -> PathBuf {
    let dir = fx.ctx.store.state_dir().join("review-watch").join(&fx.id);
    dir.join(format!("{child}.json"))
}

fn captured(fx: &Fx, child: &str) -> bool {
    let bytes = std::fs::read(capture_file(fx, child)).unwrap_or_default();
    serde_json::from_slice::<Value>(&bytes).is_ok_and(|v| v["captured_at"].is_string())
}

fn verdict(fx: &Fx, o: &RequestOrigin) -> anyhow::Result<Receipted> {
    let report = Report {
        origin: o,
        gate_ref: None,
        contract_sha256: None,
    };
    crate::reviewer::verdict_request(&fx.ctx, &fx.id, Verdict::Approve, "ok", None, &report)
}

fn recover(fx: &Fx) {
    effects::reconcile_task(&ctx_at(fx.root()), &fx.id).unwrap();
}

fn owed(fx: &Fx, o: &RequestOrigin) -> Vec<String> {
    fx.receipt(o).unwrap().post_pending
}

fn start(fx: &Fx, id: &str) {
    let next = run(id, &fx.root().join("app"), "w0:p3");
    let update = |t: &mut Task| {
        if id.contains("review") {
            t.runs[0].review_runs.push(next);
        } else {
            t.runs.push(next);
            t.status = Status::Running;
        }
        Ok(())
    };
    fx.ctx.store.update(&fx.id, update).unwrap();
}

#[test]
fn sync_completes_reviewer_and_pm_receipts_on_their_named_child() {
    // Given a reviewer-only binding whose verdict committed just before its effects ran, and a
    // newer review child started before recovery.
    let fx = fixture(false);
    let o = fx.reviewer();
    assert!(crash_at("swept_after_commit", || verdict(&fx, &o)));
    start(&fx, "R1-review2");

    // When sync reconciles the task in a fresh Ctx.
    recover(&fx);

    // Then the historical child's event and capture complete, and the newer child is untouched.
    assert!(owed(&fx, &o).is_empty());
    assert_eq!(fx.events("verdict", &o.request_id), (1, 1));
    assert!(captured(&fx, "R1-review1") && !capture_file(&fx, "R1-review2").exists());

    let target = BindTarget::Pm {
        project: "app".into(),
    };
    let g = open_generation(&fx.ctx.store, &target, "pm-pane")
        .unwrap()
        .generation;
    let c = origin(&fx, ContainerRef::Pm, g, "comment-1", "comment");
    let comment = || crate::comments::comment_request(&fx.ctx, &fx.id, "Use the payload.", &c);
    assert!(crash_at("swept_after_commit", comment));
    recover(&fx);
    assert!(fx.task().agent_pm_receipts[0].post_pending.is_empty());
    assert_eq!(fx.events("commented", &c.request_id), (1, 1));
}

#[test]
fn owed_effects_of_an_older_run_never_land_on_the_newer_one() {
    // Given an audited done of R1 whose effects were owed when a retry started R2.
    let fx = fixture(false);
    let observation = criteria(&fx, true);
    crate::completion::record_evidence(&fx.ctx, &fx.id, &observation).unwrap();
    let o = bound(&fx, "done-1", "done");
    let report = Report {
        origin: &o,
        gate_ref: None,
        contract_sha256: None,
    };
    let done = || crate::review::done_request(&fx.ctx, &fx.id, SUMMARY, None, &report);
    assert!(crash_at("swept_after_commit", done));
    start(&fx, "R2");

    // When sync recovers, then R1's completion audit is attributed to R1 alone.
    recover(&fx);
    let audits: Vec<String> = fx
        .task()
        .artifacts
        .into_iter()
        .filter(|a| a.title == "Completion audit")
        .map(|a| a.run)
        .collect();
    assert_eq!(audits, ["R1"]);
    assert!(owed(&fx, &o).is_empty());

    // Given a wait of R1 whose reconciliation was owed when R2 started, then that effect stays
    // owed rather than reconciling R2, while the event still completes.
    let fx = fixture(false);
    let o = bound(&fx, "wait-1", "wait");
    let now = jiff::Timestamp::now();
    let wait =
        || crate::obligations::register_wait_request(&fx.ctx, &fx.id, wait_request(), now, &o);
    assert!(crash_at("swept_after_commit", wait));
    start(&fx, "R2");
    recover(&fx);
    assert_eq!(owed(&fx, &o), ["effect:reconcile_wait"]);
    assert_eq!(fx.events("wait_registered", &o.request_id), (1, 1));
}

#[test]
fn a_review_capture_stays_owed_while_busy_or_failing_until_it_completes() {
    let fx = fixture(false);
    let o = fx.reviewer();
    assert!(crash_at("swept_after_commit", || verdict(&fx, &o)));

    // Given another capture holding the child's lock, then the capture stays owed.
    let name = format!("review-capture-{}-R1-review1", fx.id);
    let lock = fx.ctx.store.try_lock(&name).unwrap().unwrap();
    recover(&fx);
    assert_eq!(owed(&fx, &o), ["effect:capture_review"]);
    drop(lock);

    // Given a capture whose monitor file cannot be read or written, then it stays owed too.
    let monitor = capture_file(&fx, "R1-review1");
    let blocker = monitor.parent().unwrap().to_path_buf();
    std::fs::create_dir_all(blocker.parent().unwrap()).unwrap();
    std::fs::write(&blocker, "not a directory").unwrap();
    recover(&fx);
    assert_eq!(owed(&fx, &o), ["effect:capture_review"]);

    // When the failure clears, then the retry captures the named child and clears the flag.
    std::fs::remove_file(&blocker).unwrap();
    recover(&fx);
    assert!(owed(&fx, &o).is_empty() && captured(&fx, "R1-review1"));
    assert_eq!(fx.events("verdict", &o.request_id), (1, 1));
}
