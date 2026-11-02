use super::*;

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
