//! A terminal retired worker's one orphan is reaped without decoding retained job/wait history.
use super::*;
use crate::agent::native_test_support::{data, host};
use corral_core::Status;
use corral_core::agent::{AgentRole, JobMonitor, MonitorOn};
use corral_core::work::{self, Counter, HotScope};

#[test]
fn retired_worker_job_and_named_monitor_settle_once_with_constant_history_work() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let (_dir, store) = data::store();
        store.fixture_pin_now(data::at(100_000)).unwrap();
        let mut task = data::rich(
            &store,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale + 1,
            2 * scale,
        );
        task.status = Status::Done;
        let run = &mut task.runs[0];
        run.inbox.settled = Some(data::at(0));
        let run_id = run.id.clone();
        let mut state = host(AgentRole::Worker, 2 * scale);
        for n in 0..2 * scale {
            state.jobs.push(serde_json::from_value(json!({"id":format!("old-{n:04}"),"generation":1,"command":"true","state":"exited","prepared_at":data::at(0),"ended_at":data::at(1),"exit_code":0,"group_clear":true,"swept":true})).unwrap());
        }
        let mut wait = data::wait(&run_id, 999, false);
        wait["request"]["handle"] =
            json!({"kind":"channel","channel_id":"job:job-live","capability":"wait_v1"});
        run.obligations
            .waits
            .push(serde_json::from_value(wait).unwrap());
        let mut job:JobRecord=serde_json::from_value(json!({"id":"job-live","generation":1,"command":"true","state":"prepared","prepared_at":data::at(0)})).unwrap();
        job.monitors.push(JobMonitor {
            wait: "W0999".into(),
            on: MonitorOn::Exit,
            pattern: None,
            request_id: "monitor".into(),
            at: data::at(0),
        });
        state.jobs.push(job);
        run.agent_host = Some(Box::new(state));
        store.fixture_put_task(&task).unwrap();
        let ctx = Ctx::from_store(store);
        let hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(reap(&ctx, &task.id, data::at(100_000)).unwrap(), 1);
        assert_eq!(reap(&ctx, &task.id, data::at(100_000)).unwrap(), 0);
        let head = ctx.store.head(&task.id).unwrap();
        assert!(head.unfinished_native_containers().unwrap().is_empty());
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        drop(hot);
        let wait = head.wait(&run_id, "W0999").unwrap().unwrap();
        assert_eq!(wait.observation.state, WaitState::Lost);
        assert_eq!(wait.receipt.unwrap().outcome, ReceiptOutcome::Lost);
        let container = ContainerRef::Worker {
            task: task.id.clone(),
            run: run_id,
        };
        let job = head.agent_job(&container, "job-live").unwrap().unwrap();
        assert_eq!(job.state, JobState::Lost);
        assert!(job.swept && job.group_clear);
        assert_eq!(job.ended_at, Some(data::at(100_000)));
        assert_eq!(
            ctx.store
                .events()
                .unwrap()
                .iter()
                .filter(|event| event.kind == "agent_job_reaped")
                .count(),
            1
        );
    }
    assert_eq!(measured[0], measured[1]);
}
