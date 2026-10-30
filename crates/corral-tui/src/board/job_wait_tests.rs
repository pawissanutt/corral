use super::tests::seeded;
use corral_core::Status;
use serde_json::json;

#[test]
fn ready_and_expired_job_exits_do_not_block_and_open_waits_remain_visible() {
    let fx = seeded();
    let at = fx.now.get();
    fx.ctx.store.update("T-004", |task| {
        let waits = &mut task.current_run_mut().unwrap().obligations.waits;
        for state in ["ready", "expired"] {
            waits.push(serde_json::from_value(json!({
                "request": {"version":1,"id":state,"run":"R1","condition":format!("job {state} exits"),"owner":"worker","source":"worker","handle":{"kind":"channel","channel_id":format!("job:{state}"),"capability":"wait_v1"}},
                "registered_at":at,"observation":{"at":at,"state":state,"detail":""}
            })).unwrap());
        }
        Ok(())
    }).unwrap();
    let board = fx.board();
    assert!(
        board
            .column_tasks(Status::Running)
            .iter()
            .any(|t| t.id == "T-004")
    );
    assert!(
        !board
            .column_tasks(Status::NeedsMe)
            .iter()
            .any(|t| t.id == "T-004")
    );
    fx.ctx.store.update("T-004", |task| {
        task.current_run_mut().unwrap().obligations.waits.push(serde_json::from_value(json!({
            "request":{"version":1,"id":"external","run":"R1","condition":"CI finishes","owner":"worker","source":"worker","handle":{"kind":"channel","channel_id":"ci","capability":"wait_v1"}},
            "registered_at":at,"observation":{"at":at,"state":"pending","detail":""}
        })).unwrap());
        Ok(())
    }).unwrap();
    let task = fx.ctx.store.get("T-004").unwrap();
    let lines = crate::obligations::card_lines(task.current_run().unwrap(), true);
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().any(|l| l.to_string().contains("2 settled")));
    assert!(lines.iter().any(|l| l.to_string().contains("CI finishes")));
    let board = fx.board();
    assert!(
        board
            .column_tasks(Status::NeedsMe)
            .iter()
            .any(|t| t.id == "T-004")
    );
}
