use super::*;

#[test]
fn ordinary_job_named_channel_becomes_pm_attention_after_reconciliation() {
    let fx = Fx::new("fake");
    let mut request = wait(&fx);
    request.handle = WaitHandle::Channel {
        channel_id: "job:123".into(),
        capability: Some("wait_v1".into()),
    };
    request.deadline = Some(fx.at(3600));
    let registered = register_wait(&fx.ctx, "T-001", request, fx.at(0)).unwrap();
    assert_eq!(state(&registered), WaitState::Pending);
    let board = corral_ops::attention::board(&fx.ctx, fx.at(1800)).unwrap();
    let entry = board.tasks.iter().find(|t| t.task == "T-001").unwrap();
    assert!(
        entry
            .assessment
            .conditions
            .iter()
            .any(|c| c.reason_code == "manual_wait_overdue"
                && c.responsible == corral_core::attention::Responsible::Pm)
    );
}

#[test]
fn cli_status_wait_wakes_generic_worker_once() {
    let fx = Fx::new("fake");
    let dependency = fx
        .ctx
        .store
        .create("dependency", None, Status::Ready, "")
        .unwrap();
    let mut request = wait(&fx);
    request.handle = WaitHandle::TaskStatus {
        task: dependency.id.clone(),
        status: corral_core::TaskWaitStatus::Done,
    };
    request.deadline = None;
    register_wait(&fx.ctx, "T-001", request, fx.at(0)).unwrap();
    corral_ops::move_task(&fx.ctx, &dependency.id, Status::Done, true).unwrap();
    fx.sync(10);
    fx.sync(20);
    let replies = std::fs::read_to_string(fx.bin_dir.join("replies.log")).unwrap();
    assert_eq!(replies.lines().count(), 1, "{replies}");
    assert!(
        replies.contains("wait `event` resolved: Ready"),
        "{replies}"
    );
}
