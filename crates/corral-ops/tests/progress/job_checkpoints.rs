use super::{Fixture, json, save_checkpoint};

#[test]
fn settled_job_waits_are_not_recaptured() {
    let f = Fixture::new();
    f.ctx.store.update(&f.id, |task| {
        let run = task.current_run_mut().unwrap();
        for i in 0..10 {
            run.obligations.waits.push(serde_json::from_value(json!({
                "request": {"version": 1, "id": format!("j{i}.m1"), "run": "R1", "condition": format!("job j{i} exits"), "owner": "worker", "source": "worker", "handle": {"kind": "channel", "channel_id": format!("job:j{i}"), "capability": "wait_v1"}},
                "registered_at": "2026-10-15T00:00:00Z", "observation": {"at": "2026-10-15T00:00:00Z", "state": "failed", "detail": "exit 1"}
            })).unwrap());
        }
        Ok(())
    }).unwrap();
    for _ in 0..3 {
        save_checkpoint(&f.ctx, &f.id, &f.checkpoint()).unwrap();
    }
    let task = f.task();
    let run = task.current_run().unwrap();
    assert_eq!(run.obligations.waits.len(), 10);
    assert_eq!(run.checkpoints.len(), 3);
    assert!(
        run.checkpoints
            .iter()
            .all(|c| c.checkpoint.run_obligations.waits.is_empty())
    );
}
