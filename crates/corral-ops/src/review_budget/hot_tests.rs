//! Reviewer liveness and finishing on ordinary sync ticks read none of the review child's
//! nested history (T-330 review R1-2, R2-1): the idle update, a watchdog escalation and a lost
//! reviewer's release edit the child's projection, and the next usage refresh, the authorized
//! own-record pass, collects the finished child's usage exactly as finishing used to. With 10x
//! usage entries, checkpoints and their payloads, switches, obligations, applied effects and
//! cursors in the current child, every unattributed cell is unchanged and no own-record work
//! happens on these ticks.
use corral_core::work::{self, Counter, HotScope};
use corral_herdr::{AgentStatus, PaneInfo};

use super::*;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 + secs).unwrap()
}

fn receipt(run: &str, id: &str) -> serde_json::Value {
    json!({"version": 1, "id": format!("rc-{id}"), "run": run, "outcome": "answered",
           "delivery": "acknowledged", "observed_at": at(1), "answer": "yes"})
}

fn decision(run: &str, i: usize) -> serde_json::Value {
    json!({
        "request": {"version": 1, "id": format!("D{i:04}"), "run": run, "question": "Ship?",
                    "requested_action": "answer", "owner": "owner", "source": "worker",
                    "deadline": null},
        "registered_at": at(1),
        "observation": {"at": at(1), "state": "ready", "detail": "asked"},
        "receipt": receipt(run, &format!("D{i:04}"))
    })
}

fn wait(run: &str, i: usize) -> serde_json::Value {
    json!({
        "request": {"version": 1, "id": format!("W{i:04}"), "run": run, "condition": "check exited",
                    "owner": "worker", "source": "worker", "deadline": null,
                    "handle": {"kind": "gate", "sha": "a1b2c3d", "pane": "p-7"}},
        "registered_at": at(1),
        "observation": {"at": at(1), "state": "ready", "detail": "observed"},
        "receipt": receipt(run, &format!("W{i:04}"))
    })
}

fn checkpoint(task: &str, run: &str, i: usize) -> serde_json::Value {
    json!({
        "artifact": format!("artifacts/{task}/{run}/checkpoint-{i:04}.json"),
        "checkpoint": {
            "version": 1, "revision": i + 1,
            "input": {"task": task, "run": run, "spec_revision": format!("spec-{i:04}"),
                      "expected_revision": i, "progress_revision": i, "head": "1".repeat(40),
                      "base": "0".repeat(40), "tree": format!("{i:064}"),
                      "criterion_ids": [], "evidence_refs": [], "next_action": "continue",
                      "decisions": [], "waits": [], "resources": []},
            "progress": null, "completed_steps": [], "current_step": null,
            "run_obligations": {"decisions": [decision(run, i)]},
            "captured": at(1)
        }
    })
}

/// `n` of each nested collection in the current review child: zero-cost usage entries (so its
/// usage totals are equal at every size), checkpoints with payloads, switches, waits and
/// decisions, applied effects and observe cursors. Created, updated, the review start and the
/// run's idle start are fixed so equal states store equal bytes.
fn grow_child(ctx: &Ctx, id: &str, n: usize) {
    let mut task = ctx.store.get(id).unwrap();
    let run = task.current_run_mut().unwrap();
    run.review.as_mut().unwrap().at = at(0);
    run.idle_since = Some(at(0));
    let child = run.review_runs.last_mut().unwrap();
    child.started = at(0);
    let mut value = serde_json::to_value(&*child).unwrap();
    let child_id = child.id.clone();
    value["usage"] = serde_json::to_value(corral_core::Usage::from_entries(
        (0..n)
            .map(|_| {
                serde_json::from_value(json!({"at": null, "latency_ms": null, "model": "m",
                    "input": 0, "output": 0, "cache_read": 0, "cache_write": 0,
                    "reported_usd": null, "estimated_usd": null, "unpriced_tokens": 0}))
                .unwrap()
            })
            .collect(),
    ))
    .unwrap();
    value["switches"] = (0..n)
        .map(|i| json!({"from": format!("p{i:04}"), "to": format!("q{i:04}"), "reason": "switch"}))
        .collect();
    value["checkpoints"] = (0..n).map(|i| checkpoint(id, &child_id, i)).collect();
    value["obligations"] = json!({
        "waits": (0..n).map(|i| wait(&child_id, i)).collect::<Vec<_>>(),
        "decisions": (0..n).map(|i| decision(&child_id, i)).collect::<Vec<_>>(),
    });
    value["inbox"]["next_seq"] = json!(10_000);
    value["inbox"]["effects_applied"] = json!((1..=n).collect::<Vec<_>>());
    value["inbox"]["observe_cursors"] = (0..n)
        .map(|c| {
            json!({"adapter": "pi", "binding": {"kind": "pi", "session": format!("s-{c}")},
                   "cursor": {"offset": c, "last_complete_line": c, "coverage": "complete"}})
        })
        .collect();
    *child = serde_json::from_value(value).unwrap();
    // Every stamp the launch wrote is one fixed instant, so equal states store equal bytes.
    let mut value = serde_json::to_value(&task).unwrap();
    fix_stamps(&mut value);
    let task: corral_core::Task = serde_json::from_value(value).unwrap();
    ctx.store.fixture_put_task(&task).unwrap();
}

fn fix_stamps(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) if text.parse::<Timestamp>().is_ok() => {
            *text = at(0).to_string();
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(fix_stamps),
        serde_json::Value::Object(fields) => fields.values_mut().for_each(fix_stamps),
        _ => {}
    }
}

fn reviewing(n: usize) -> (Fixture, String, String) {
    let fx = Fixture::new(1);
    let id = fx.task();
    let review = crate::reviewer::review_run(&fx.ctx, &id, Some("rev")).unwrap();
    // Stamps written from here on, the finish's included, are this instant.
    fx.ctx.store.fixture_pin_now(at(900)).unwrap();
    grow_child(&fx.ctx, &id, n);
    (fx, id, review.pane.unwrap())
}

/// Every unattributed cell of `f`; it hydrates nothing and does no own-record work.
fn measured(f: impl FnOnce()) -> Vec<(&'static str, u64)> {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    f();
    let delta = work::snapshot().since(&before);
    assert_eq!(delta.unattributed_hydrations(), 0, "{:?}", delta.sites);
    assert_eq!(
        (delta.own_record_rows(), delta.own_record_bytes()),
        (0, 0),
        "no own-record work outside the usage refresh"
    );
    delta.check_identity().unwrap();
    Counter::ALL
        .iter()
        .map(|c| (c.name(), delta.cells(*c).unattributed))
        .collect()
}

fn panes(pane: &str, status: AgentStatus) -> std::collections::HashMap<String, PaneInfo> {
    let info = PaneInfo {
        pane_id: pane.into(),
        workspace_id: "w1".into(),
        tab_id: "w1:t1".into(),
        agent: None,
        status,
        title: None,
        cwd: None,
    };
    std::collections::HashMap::from([(pane.to_owned(), info)])
}

/// The run and its child not yet idle, so the next idle sample writes their idle start.
fn not_idle(fx: &Fixture, id: &str) {
    fx.ctx
        .store
        .update(id, |t| {
            let run = t.current_run_mut().unwrap();
            run.idle_since = None;
            let child = run.review_runs.last_mut().unwrap();
            child.idle_since = None;
            child.agent_state = None;
            Ok(())
        })
        .unwrap();
}

fn idle_tick(fx: &Fixture, id: &str, pane: &str) -> Vec<(&'static str, u64)> {
    not_idle(fx, id);
    let panes = panes(pane, AgentStatus::Idle);
    let snapshot = crate::pane_liveness::Snapshot::injected(&panes);
    let cells = measured(|| {
        let change = crate::reviewer::reconcile_review_sampled_at(&fx.ctx, id, at(1), snapshot);
        assert!(change.unwrap().is_none());
    });
    let run = fx.ctx.store.head(id).unwrap().current_run.unwrap();
    assert_eq!(run.idle_since, Some(at(1)));
    let child = run.current_child.unwrap();
    assert_eq!(
        (child.idle_since, child.agent_state.as_deref(), child.ended),
        (Some(at(1)), Some("idle"), None)
    );
    cells
}

#[test]
fn an_idle_reviewer_update_does_not_read_the_child_history() {
    let (fx, id, pane) = reviewing(1);
    let base = idle_tick(&fx, &id, &pane);
    // The same idle transition again, now with 10x child history.
    grow_child(&fx.ctx, &id, 10);
    assert_eq!(base, idle_tick(&fx, &id, &pane));
}

/// The child as finishing used to leave it: ended at `ended`, its usage collected then.
fn legacy_finish(fx: &Fixture, id: &str, ended: Timestamp) -> corral_core::Run {
    let task = fx.ctx.store.get(id).unwrap();
    let mut child = task
        .current_run()
        .unwrap()
        .review_runs
        .last()
        .unwrap()
        .clone();
    child.ended = Some(ended);
    crate::usage::collect(&mut child, &fx.ctx.store);
    child
}

/// After a finish, the usage refresh collects the child's usage once, as finishing did.
fn assert_refresh_collects_as_finish(fx: &Fixture, id: &str) {
    let finished = fx.ctx.store.get(id).unwrap();
    let child = finished
        .current_run()
        .unwrap()
        .review_runs
        .last()
        .unwrap()
        .clone();
    assert!(
        child.usage_pending,
        "the finish leaves the usage to the refresh"
    );
    let expected = legacy_finish(fx, id, child.ended.unwrap());
    crate::usage::refresh_active_usage(&fx.ctx.store);
    let refreshed = fx.ctx.store.get(id).unwrap();
    let child = refreshed.current_run().unwrap().review_runs.last().unwrap();
    assert!(!child.usage_pending);
    assert_eq!(
        (&child.usage, &child.usage_note),
        (&expected.usage, &expected.usage_note)
    );
}

/// The watchdog's escalation write itself, the production `commit_observation` that
/// `reconcile_review_sampled_at` makes: the run escalates and the current child finishes.
fn escalation_finish(n: usize) -> Vec<(&'static str, u64)> {
    let (fx, id, _) = reviewing(n);
    let shell = fx.ctx.store.head(&id).unwrap().to_shell_task().unwrap();
    let run = shell.current_run().unwrap();
    let observation = crate::reviewer::Observation {
        run,
        review: run.review.as_ref().unwrap(),
        since: Some(at(0)),
        state: "idle",
        reason: Some("the reviewer (rev) has been idle for 60 seconds without a verdict"),
        stalled: false,
        notes: std::path::Path::new("notes.md"),
        now: at(600),
    };
    let cells = measured(|| {
        let saved = crate::reviewer::commit_observation(&fx.ctx, &id, &observation).unwrap();
        assert!(saved.is_some());
    });
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    let child = task.current_run().unwrap().review_runs.last().unwrap();
    assert_eq!(child.ended, Some(at(600)));
    assert_refresh_collects_as_finish(&fx, &id);
    cells
}

#[test]
fn a_watchdog_escalation_finishes_the_child_without_its_history() {
    assert_eq!(escalation_finish(1), escalation_finish(10));
}

#[test]
fn a_watchdog_escalation_tick_hydrates_only_in_its_round_cleanup() {
    // Given a reviewer idle past the review window, with 10x child history.
    let (mut fx, id, pane) = reviewing(10);
    fx.ctx.store.config.sync.review_idle_after_secs = 60;
    let panes = panes(&pane, AgentStatus::Idle);
    let snapshot = crate::pane_liveness::Snapshot::injected(&panes);
    // When the watchdog samples it.
    let _hot = HotScope::enter();
    let before = work::snapshot();
    let change = crate::reviewer::reconcile_review_sampled_at(&fx.ctx, &id, at(600), snapshot);
    let delta = work::snapshot().since(&before);
    // Then it escalates, and only the round's cleanup (the full-task pane cleanup and screen
    // capture that later conversions own) reads the task; the finish itself is measured above.
    assert!(change.unwrap().is_some());
    let files: std::collections::BTreeSet<&str> = delta
        .sites
        .iter()
        .map(|site| site.location.file())
        .collect();
    assert_eq!(
        files,
        std::collections::BTreeSet::from([
            "crates/corral-ops/src/artifacts.rs",
            "crates/corral-ops/src/cleanup.rs",
            "crates/corral-ops/src/reviewer/watchdog.rs",
        ])
    );
    assert_eq!((delta.own_record_rows(), delta.own_record_bytes()), (0, 0));
    drop(_hot);
    assert_eq!(fx.ctx.store.get(&id).unwrap().status, Status::NeedsMe);
    assert_refresh_collects_as_finish(&fx, &id);
}

/// The ticks that release a reviewer whose pane is gone, through its finish.
fn release_tick(n: usize) -> Vec<(&'static str, u64)> {
    let (fx, id, _) = reviewing(n);
    // An inventory without the reviewer's pane (an empty one is not a sample).
    let other = panes("w1:p99", AgentStatus::Working);
    let cells = measured(|| {
        reconcile(&fx.ctx, &other, at(600)).unwrap();
        reconcile(&fx.ctx, &other, at(1200)).unwrap();
    });
    let task = fx.ctx.store.get(&id).unwrap();
    assert!(
        task.current_run()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .process_lost
    );
    assert!(
        task.current_run()
            .unwrap()
            .review_runs
            .last()
            .unwrap()
            .ended
            .is_some()
    );
    assert_refresh_collects_as_finish(&fx, &id);
    cells
}

#[test]
fn a_lost_reviewer_finish_reads_none_of_the_child_history() {
    assert_eq!(release_tick(1), release_tick(10));
}
