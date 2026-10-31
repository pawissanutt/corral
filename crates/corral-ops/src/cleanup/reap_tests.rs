#![cfg(target_os = "linux")]
use super::*;
use crate::agent::bridge::fixture;
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use serde_json::json;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct OwnedProcess(Child);
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn process(ctx: &Ctx, task: &str) -> OwnedProcess {
    let receipt = crate::launch_exit::receipt(ctx, task, "R1-R1-review1").unwrap();
    // Use the real publication entry to register marker ownership. Run only its command
    // body as one disposable process, so even a failed assertion cannot orphan a shell tree.
    crate::launch_exit::arm(ctx, "/bin/cat", &receipt).unwrap();
    let marker = crate::launch_exit::ownership_marker(&receipt).unwrap();
    let mut child = Command::new("/bin/sh")
        .args(["-c", "printf 'ready\\n'; exec /bin/cat"])
        .env("CORRAL_PROCESS_OWNER", marker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready, "ready\n");
    OwnedProcess(child)
}
fn exact_event_cost(report: &WorkSnapshot, details: &[serde_json::Value]) {
    report.check_identity().unwrap();
    assert_eq!(report.followup_calls("T-661", "cleanup.verdict_event"), 1);
    let bytes: usize = details
        .iter()
        .map(|detail| serde_json::to_vec(detail).unwrap().len())
        .sum();
    for counter in Counter::ALL {
        let expected = match counter {
            Counter::EventDecodes => details.len() as u64,
            Counter::DecodedDocBytes => bytes as u64,
            _ => 0,
        };
        assert_eq!(
            report
                .cells(counter)
                .followup
                .get(&("T-661".into(), "cleanup.verdict_event".into()))
                .copied()
                .unwrap_or(0),
            expected,
            "{counter:?}"
        );
    }
    assert_eq!(report.unattributed_hydrations(), 0);
}

#[test]
fn actual_closed_reviewer_waits_for_event_then_reaps_without_historical_hydration() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let fx = fixture::fixture(false, 1);
        let now: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
        fx.ctx.store.fixture_pin_now(now).unwrap();
        let mut task = fx.task();
        task.created = now;
        task.updated = now;
        task.status_since = Some(now);
        let run = task.current_run_mut().unwrap();
        assert_eq!(run.id, "R1");
        run.review_runs.clear();
        for n in 0..2 * scale {
            run.review_runs.push(
                serde_json::from_value(json!({
                    "id":format!("cold-{n:04}"),"agent":"fixture","branch":"cold","base":"main","base_sha":"base",
                    "worktree":fx.root(),"started":now,"ended":now,"outcome":"done"
                }))
                .unwrap(),
            );
        }
        run.review_runs.push(
            serde_json::from_value(json!({
                "id":"R1-review1","agent":"fixture","branch":"review","base":"main","base_sha":"base",
                "worktree":fx.root(),"started":now,"ended":now,"outcome":"done",
                "review":{"reviewer":"fixture","sha":"head","at":now,"verdict":"approve"}
            }))
            .unwrap(),
        );
        fx.ctx.store.fixture_put_task(&task).unwrap();
        let mut child = process(&fx.ctx, &task.id);
        let mut details = Vec::new();
        for n in 0..2 * scale {
            let detail = json!({"run":"R1","reviewer":"other","n":format!("{n:04}")});
            fx.ctx
                .store
                .log_event(Some(&task.id), "verdict", detail.clone())
                .unwrap();
            details.push(detail);
        }
        let source = fx.ctx.store.head(&task.id).unwrap();
        let hot = HotScope::enter();
        let before = work::snapshot();
        reap_closed_reviews(&fx.ctx, &source);
        let held = work::snapshot().since(&before);
        drop(hot);
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "verdict row alone cannot authorize signals"
        );
        exact_event_cost(&held, &details);
        let detail = json!({"run":"R1","reviewer":"fixture"});
        fx.ctx
            .store
            .log_event(Some(&task.id), "verdict", detail.clone())
            .unwrap();
        details.push(detail);
        let hot = HotScope::enter();
        let before = work::snapshot();
        reap_closed_reviews(&fx.ctx, &source);
        let reaped = work::snapshot().since(&before);
        drop(hot);
        exact_event_cost(&reaped, &details);
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.0.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "owned closed reviewer was not reaped"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        measured.push((held, reaped));
    }
    for counter in Counter::ALL {
        assert_eq!(
            measured[0].0.get(counter),
            measured[1].0.get(counter),
            "held {counter:?}"
        );
        assert_eq!(
            measured[0].1.get(counter),
            measured[1].1.get(counter),
            "reaped {counter:?}"
        );
    }
}

#[test]
fn verdict_gate_preserves_any_matching_timestamp_when_event_clock_moves_backward() {
    let root = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(root.path()).unwrap();
    let task = store
        .create("Clock", None, corral_core::Status::Review, "")
        .unwrap();
    let future: Timestamp = "2026-01-02T00:00:00Z".parse().unwrap();
    let past: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
    let events: Vec<_> = [future, past]
        .into_iter()
        .map(|ts| corral_core::Event {
            seq: 0,
            task: Some(task.id.clone()),
            ts,
            kind: "verdict".into(),
            actor: None,
            via: None,
            detail: json!({"run":"R1","reviewer":"fixture"}),
        })
        .collect();
    store
        .fixture_append_events(
            &events
                .iter()
                .map(|event| format!("{}\n", serde_json::to_string(event).unwrap()))
                .collect::<String>(),
        )
        .unwrap();
    let ctx = Ctx::from_store(store);
    assert!(durable_verdict(&ctx, &task.id, "R1", "fixture", future).unwrap());
    assert!(!durable_verdict(&ctx, &task.id, "R1", "other", past).unwrap());
    assert!(!durable_verdict(&ctx, &task.id, "R2", "fixture", past).unwrap());
}
