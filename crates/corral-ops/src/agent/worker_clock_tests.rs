use super::*;

#[test]
fn task_status_receipt_waits_for_registration_clock_after_rollback() {
    let w = worker(false);
    let dependency = w
        .ctx()
        .store
        .create("dependency", Some("corral"), Status::Ready, "")
        .unwrap();
    let registered = Timestamp::now();
    let earlier = registered
        .checked_sub(jiff::SignedDuration::from_secs(1))
        .unwrap();
    let request = corral_core::WaitRequest {
        version: 1,
        id: "clock".into(),
        run: "R1".into(),
        condition: "dependency done".into(),
        owner: "worker".into(),
        source: corral_core::ObligationSource::Worker,
        deadline: None,
        handle: corral_core::WaitHandle::TaskStatus {
            task: dependency.id.clone(),
            status: corral_core::TaskWaitStatus::Done,
        },
    };
    crate::register_wait(w.ctx(), &w.id, request, registered).unwrap();
    crate::move_task(w.ctx(), &dependency.id, Status::Done, true).unwrap();
    crate::obligations_sync::reconcile_one(w.ctx(), &w.id, earlier, None).unwrap();
    assert!(w.task().runs[0].obligations.waits[0].receipt.is_none());
    crate::obligations_sync::reconcile_one(w.ctx(), &w.id, registered, None).unwrap();
    let receipt = w.task().runs[0].obligations.waits[0]
        .receipt
        .clone()
        .unwrap();
    assert_eq!(receipt.observed_at, registered);
}
