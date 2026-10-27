//! S10, S11, S14 and S15: unattended questions, latched resume, queue fences and holds.
use corral_core::Status;
use serde_json::json;

use crate::fixture::{Fx, at};
use crate::operating_fixture;
use crate::support::{covered, invariants, park, stats, sync, unpark};

fn events(fx_ctx: &crate::Ctx, id: &str, kind: &str) -> usize {
    fx_ctx
        .store
        .events()
        .unwrap()
        .iter()
        .filter(|event| event.kind == kind && event.task.as_deref() == Some(id))
        .count()
}

#[test]
fn s10_unanswered_blocking_question_with_absent_pm_reaches_needs_me_without_answers() {
    // Given a blocking question and no PM.
    let fx = Fx::new();
    let task = fx.questions(1);
    fx.ctx
        .store
        .update(&task.id, |task| {
            let host = task.current_run_mut().unwrap().agent_host.as_mut().unwrap();
            host.questions[0].blocking = true;
            Ok(())
        })
        .unwrap();
    // When passes run past the unattended interval and the owner bound.
    for seconds in [0, 300, 600, 900, 1800, 2400] {
        sync(&fx, seconds);
    }
    // Then the unattended rule raised Needs me with one owner send and nothing was answered.
    assert_eq!(fx.ctx.store.get(&task.id).unwrap().status, Status::NeedsMe);
    assert_eq!(events(&fx.ctx, &task.id, "question_unattended"), 1);
    assert_eq!(events(&fx.ctx, &task.id, "question_answered"), 0);
    assert_eq!(fx.calls(), 1);
    let stats = stats(&fx, 2400);
    let entry = &stats.notifications.by_producer["question_needs_me"];
    assert_eq!((entry.batches, entry.success), (1, 1));
    assert_eq!(stats.notifications.batches, 1);
    assert_eq!(stats.actions.attributed, 0);
    covered(&fx.reopen(), 2400);
    invariants(&stats);
}

#[test]
fn s11_latched_resume_never_unparks_or_starts_an_executor() {
    // Given a stalled review parked by the PM on a prerequisite's status.
    let fx = Fx::new();
    let task = fx.stalled(Some("app"));
    let prerequisite = fx
        .ctx
        .store
        .create("Prerequisite", None, Status::Review, "")
        .unwrap();
    sync(&fx, 0);
    let checks = format!("status:{}=done", prerequisite.id);
    park(fx.ctx.store.root(), true, &task.id, &checks, 10);
    sync(&fx, 3600);
    // When the prerequisite releases, and then reopens.
    let set = |status| {
        fx.ctx
            .store
            .update(&prerequisite.id, |task| {
                task.status = status;
                Ok(())
            })
            .unwrap();
    };
    set(Status::Done);
    sync(&fx, 3602);
    set(Status::Review);
    sync(&fx, 3603);
    sync(&fx, 7200);
    // Then one latched resume-ready condition remains, still parked, and nothing started.
    let parked = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(parked.park.as_ref().unwrap().ready_at, Some(at(3602)));
    assert_eq!(parked.status, Status::Review);
    for kind in ["unparked", "dispatched", "gate_started", "review_started"] {
        assert_eq!(events(&fx.ctx, &task.id, kind), 0, "{kind}");
    }
    let stats = stats(&fx, 7200);
    assert_eq!(stats.population.by_kind["park_resume_ready"], 1);
    let stall = stats
        .conditions
        .values()
        .find(|row| row.kind == "unexpected_stall")
        .unwrap();
    assert_eq!(
        (stall.wakes, stall.owner_levels),
        (1, 0),
        "parked stall ladder"
    );
    covered(&fx.reopen(), 7200);
    invariants(&stats);
}

fn stalled_review(fx: &operating_fixture::Fx) -> corral_core::Task {
    fx.ctx
        .store
        .create_initialized("Stalled review", Some("app"), Status::Review, "", |task| {
            task.runs.push(serde_json::from_value(json!({
                "id":"R1","agent":"fixture","worktree":"/missing-fixture-worktree",
                "branch":"corral/T","base":"main","base_sha":"abc","started":operating_fixture::now(),
                "outcome":"done",
                "autopilot":{"stage":"reviewing","at":operating_fixture::now(),"sha":"abc"}
            }))?);
            Ok(())
        })
        .unwrap()
}

#[test]
fn s14_parked_and_stalled_neighbours_do_not_delay_ready_dispatch() {
    // Given one parked, one stalled and one Ready task under the same grant.
    let mut fx = operating_fixture::Fx::new();
    let parked = fx.task("Parked", Status::Ready);
    let stalled = stalled_review(&fx);
    let ready = fx.task("Ready", Status::Ready);
    fx.grant(&[&parked.id, &stalled.id, &ready.id]);
    park(fx.ctx.store.root(), true, &parked.id, "", 0);
    // When one real sync pass drains the grant.
    corral_ops::sync_at(&fx.fresh(), operating_fixture::now()).unwrap();
    // Then the Ready task dispatches in that pass while the parked one does not start.
    assert_eq!(fx.ctx.store.get(&ready.id).unwrap().status, Status::Running);
    let parked = fx.ctx.store.get(&parked.id).unwrap();
    assert_eq!(parked.status, Status::Ready);
    assert!(parked.runs.is_empty());
    assert_eq!(fx.records("submissions.jsonl").len(), 1);
    covered(&fx.fresh(), 0);
}

#[test]
fn s15_park_and_unpark_leave_an_owner_hold_unchanged() {
    // Given a Ready task the owner holds out of automatic admission.
    let fx = Fx::new();
    let task = fx
        .ctx
        .store
        .create("Held", Some("app"), Status::Ready, "")
        .unwrap();
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let conditions = |seconds| {
        corral_ops::attention::board(&fx.reopen(), at(seconds))
            .unwrap()
            .tasks
            .into_iter()
            .find(|row| row.task == task.id)
            .unwrap()
            .assessment
            .conditions
    };
    let held = conditions(200);
    assert!(!held.is_empty());
    // When the owner parks and unparks it, with a pass between.
    park(fx.ctx.store.root(), false, &task.id, "", 0);
    sync(&fx, 60);
    assert_ne!(conditions(200), held, "parked while held");
    let revision = fx.ctx.store.get(&task.id).unwrap().park.unwrap().revision;
    unpark(fx.ctx.store.root(), false, &task.id, revision);
    sync(&fx, 120);
    // Then the hold and its derived wait are unchanged, and nothing was dispatched.
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert!(after.dispatch_hold);
    assert!(after.park.is_none() && after.runs.is_empty());
    assert_eq!(events(&fx.ctx, &task.id, "unparked"), 1);
    assert_eq!(conditions(200), held);
    let stats = stats(&fx, 120);
    assert_eq!(stats.actions.duplicates, 0);
    invariants(&stats);
}
