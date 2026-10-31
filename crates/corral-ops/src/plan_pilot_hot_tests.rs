//! Plan work on ordinary sync ticks reads heads, not tasks (T-330 review R1-1). For a plan under
//! review, one awaiting the owner, and one approved and attached, the head-only decision agrees
//! with the full-task path, which writes nothing; a tick and the planner-state status change
//! hydrate nothing and do the same work with 10x earlier plans, plan runs and reviews.
use std::collections::HashMap;

use corral_core::work::{self, Counter, HotScope};
use corral_core::{HerdrRef, PlanApproval};
use corral_herdr::{AgentStatus, PaneInfo};

use super::*;

#[derive(Debug, Clone, Copy)]
enum State {
    Reviewing,
    AwaitingOwner,
    Attached,
}

fn at_state(state: State) -> (tempfile::TempDir, Ctx, String) {
    let (tmp, ctx, id, _) = fixture();
    ctx.store.fixture_pin_now(ctx.store.now()).unwrap();
    match state {
        State::Reviewing => advance(&ctx, &id, Timestamp::now()).unwrap(),
        State::AwaitingOwner | State::Attached => {
            review(&ctx, &id, Verdict::Approve);
            advance(&ctx, &id, Timestamp::now()).unwrap();
        }
    }
    if let State::Attached = state {
        ctx.store
            .update(&id, |t| {
                let plan = t.plans.last_mut().unwrap();
                plan.approval = Some(PlanApproval {
                    by: "owner".into(),
                    at: Timestamp::now(),
                    policy_digest: plan.reviews.last().unwrap().policy_digest.clone(),
                    authorization: None,
                });
                t.plan = Some(format!("{}@{}", t.id, plan.id));
                Ok(())
            })
            .unwrap();
    }
    (tmp, ctx, id)
}

/// Adds `n` earlier plans (each with its reviews), earlier plan runs, and earlier reviews of
/// the current plan; the current plan, its current review and the last plan run are unchanged.
fn grow(ctx: &Ctx, id: &str, n: usize) {
    ctx.store
        .update(id, |t| {
            let current = t.plans.last().unwrap().clone();
            let run = t.plan_runs.last().unwrap().clone();
            for k in 0..n {
                let mut plan = current.clone();
                plan.id = format!("H{k:04}");
                plan.approval = None;
                t.plans.insert(0, plan);
                let mut earlier = run.clone();
                earlier.id = format!("E{k:04}");
                earlier.state = PlanRunState::Failed;
                earlier.ended = Some(earlier.started);
                // Each earlier planner had its own pane on this socket, at every size.
                earlier.herdr = Some(HerdrRef {
                    workspace_id: "w0".into(),
                    pane_id: format!("w0:p{k}"),
                    socket: Some(ctx.herdr.socket_path()?),
                });
                t.plan_runs.insert(0, earlier);
                let reviews = &mut t.plans.last_mut().unwrap().reviews;
                if let Some(last) = reviews.last() {
                    let mut review = last.clone();
                    review.id = format!("{}-old{k:04}", review.id);
                    review.verdict = Some(Verdict::Changes);
                    let at = reviews.len() - 1;
                    reviews.insert(at, review);
                }
            }
            Ok(())
        })
        .unwrap();
}

fn unattributed(f: impl FnOnce()) -> Vec<(&'static str, u64)> {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    f();
    let delta = work::snapshot().since(&before);
    assert_eq!(delta.unattributed_hydrations(), 0, "{:?}", delta.sites);
    Counter::ALL
        .iter()
        .map(|c| (c.name(), delta.cells(*c).unattributed))
        .collect()
}

fn steady_tick(ctx: &Ctx, id: &str, state: State) -> Vec<(&'static str, u64)> {
    let now = Timestamp::now();
    let shell = ctx.store.head(id).unwrap().to_shell_task().unwrap();
    assert!(
        idle(ctx, &shell, now).unwrap(),
        "{state:?}: no action decided"
    );
    // The full-task path agrees: it writes nothing for this state.
    let seq = ctx.store.view().unwrap().seq();
    act(ctx, id, now).unwrap();
    assert_eq!(ctx.store.view().unwrap().seq(), seq, "{state:?}: act wrote");
    unattributed(|| advance(ctx, id, now).unwrap())
}

#[test]
fn plan_states_without_an_action_decide_from_the_head_and_do_not_grow() {
    for state in [State::Reviewing, State::AwaitingOwner, State::Attached] {
        let (_tmp, ctx, id) = at_state(state);
        grow(&ctx, &id, 1);
        let base = steady_tick(&ctx, &id, state);
        grow(&ctx, &id, 9);
        assert_eq!(base, steady_tick(&ctx, &id, state), "{state:?}");
    }
}

fn blocked_planner(ctx: &Ctx, id: &str) {
    ctx.store
        .update(id, |t| {
            let run = t.plan_runs.last_mut().unwrap();
            run.state = PlanRunState::Planning;
            run.ended = None;
            run.herdr = Some(HerdrRef {
                workspace_id: "w1".into(),
                pane_id: "w1:p1".into(),
                socket: Some(ctx.herdr.socket_path()?),
            });
            t.status = Status::Running;
            Ok(())
        })
        .unwrap();
}

fn status_change(ctx: &Ctx, id: &str) -> Vec<(&'static str, u64)> {
    let pane = PaneInfo {
        pane_id: "w1:p1".into(),
        workspace_id: "w1".into(),
        tab_id: "t1".into(),
        agent: None,
        status: AgentStatus::Blocked,
        title: None,
        cwd: None,
    };
    let panes = HashMap::from([("w1:p1".to_owned(), pane)]);
    let shell = ctx.store.head(id).unwrap().to_shell_task().unwrap();
    let now = shell.plan_runs.last().unwrap().started + jiff::SignedDuration::from_secs(1);
    let cells = unattributed(|| reconcile(ctx, &shell, &panes, now).unwrap());
    assert_eq!(ctx.store.head(id).unwrap().status, Status::NeedsMe);
    cells
}

#[test]
fn the_planner_state_change_is_a_head_write_that_does_not_grow() {
    let (_tmp, ctx, id, _) = fixture();
    // Keep timestamp precision identical; only retained history differs between variants.
    ctx.store.fixture_pin_now(ctx.store.now()).unwrap();
    grow(&ctx, &id, 1);
    blocked_planner(&ctx, &id);
    let base = status_change(&ctx, &id);
    grow(&ctx, &id, 9);
    blocked_planner(&ctx, &id);
    assert_eq!(base, status_change(&ctx, &id));
}
