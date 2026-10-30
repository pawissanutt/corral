//! Default pending-question reads preserve full views without loading terminal task/native tails.
use super::*;
use crate::agent::native_test_support::{data, host};
use corral_core::agent::{AgentRole, AgentRunState};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{PlanRunState, Status};
use serde_json::json;

pub(super) fn questions(container: &ContainerRef, role: AgentRole, n: usize) -> Box<AgentRunState> {
    let mut state = host(role, n);
    for i in 1..=n + 1 {
        state.questions.push(serde_json::from_value(json!({"version":1,"key":format!("{}/Q{}",container.key(),if i==n+1 {9999}else {i}),"revision":1,"kind":"clarification",
        "asked":{"request_id":"ask","payload_sha256":"00","session_id":"fixed-session","generation":999,"at":data::at(0)},
        "blocking":true,"text":"question","state":if i==n+1 {"open"}else {"cancelled"},"wake":{"route":"none","key":"wake","state":"pending"}})).unwrap());
    }
    Box::new(state)
}

#[cfg(unix)]
#[test]
fn owner_attention_commits_one_head_notice_and_preserves_exact_growth_work_on_replay() {
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
        task.status = Status::Running;
        let run = task.runs.last_mut().unwrap();
        run.outcome = None;
        run.review = None;
        let container = ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        let mut state = questions(&container, AgentRole::Worker, 2 * scale);
        state.questions.last_mut().unwrap().state = QuestionState::Escalated;
        run.agent_host = Some(state);
        store.fixture_put_task(&task).unwrap();
        let transport = crate::agent::bridge::fixture::fixture(false, 5);
        let mut ctx = Ctx::from_store(store);
        ctx.herdr = transport.ctx.herdr.clone();
        let hot = HotScope::enter();
        let before = work::snapshot();
        assert!(hold_owner_attention(&ctx, &task.id).unwrap());
        assert!(!hold_owner_attention(&ctx, &task.id).unwrap());
        let head = ctx.store.head(&task.id).unwrap();
        assert_eq!(head.status, Status::NeedsMe);
        assert!(
            head.current_run
                .as_ref()
                .unwrap()
                .blocked_excerpt
                .as_ref()
                .unwrap()
                .starts_with(corral_core::agent::QUESTION_HOLD_PREFIX)
        );
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        drop(hot);
        let events = ctx.store.events().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == "needs_me")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == "notification_claimed")
                .count(),
            1
        );
    }
    assert_eq!(measured[0], measured[1]);
}

#[test]
fn pending_views_and_default_lists_match_full_order_closure_and_constant_history_work() {
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
        task.status = Status::Review;
        let revision = corral_core::plan_contract_revision(&task).unwrap();
        let id = task.id.clone();
        let run = task.runs.last_mut().unwrap();
        run.outcome = Some(corral_core::RunOutcome::Done);
        run.review = None;
        let worker = ContainerRef::Worker {
            task: id.clone(),
            run: run.id.clone(),
        };
        run.agent_host = Some(questions(&worker, AgentRole::Worker, 2 * scale));
        let child = run.review_runs.last_mut().unwrap();
        child.outcome = None;
        child.review = None;
        let review = ContainerRef::Reviewer {
            task: id.clone(),
            run: run.id.clone(),
            review: child.id.clone(),
        };
        child.agent_host = Some(questions(&review, AgentRole::Reviewer, 2 * scale));
        let run = task.plan_runs.last_mut().unwrap();
        run.ended = None;
        run.state = PlanRunState::Planning;
        let planner = ContainerRef::Planner {
            task: id.clone(),
            run: run.id.clone(),
        };
        run.agent_host = Some(questions(&planner, AgentRole::Planner, 2 * scale));
        let planning_run = run.id.clone();
        let plan = task.plans.last_mut().unwrap();
        plan.spec_revision = revision;
        plan.run = Some(planning_run);
        let child = plan.reviews.last_mut().unwrap();
        child.verdict = None;
        let review = ContainerRef::PlanReviewer {
            task: id.clone(),
            plan: plan.id.clone(),
            review: child.id.clone(),
        };
        child.agent_host = Some(questions(&review, AgentRole::PlanReviewer, 2 * scale));
        let old = &mut task.runs[0];
        let child = &mut old.review_runs[0];
        let old_container = ContainerRef::Reviewer {
            task: id.clone(),
            run: old.id.clone(),
            review: child.id.clone(),
        };
        child.agent_host = Some(questions(&old_container, AgentRole::Reviewer, 2 * scale));
        let all = views(&task);
        let expected: Vec<_> = all
            .iter()
            .filter(|view| view.question.recorded_pending())
            .cloned()
            .collect();
        assert_eq!(expected.len(), 5);
        let pending: Vec<_> = expected
            .iter()
            .filter(|view| view.pending)
            .cloned()
            .collect();
        assert_eq!(pending.len(), 4);
        assert!(matches!(
            QuestionKey::parse(&expected[0].question.key)
                .unwrap()
                .container,
            ContainerRef::Planner { .. }
        ));
        assert_eq!(
            expected
                .iter()
                .find(|view| view.container == old_container.key())
                .unwrap()
                .effective_state,
            QuestionState::Cancelled
        );
        store.fixture_put_task(&task).unwrap();
        for _ in 0..2 * scale {
            store.create("Terminal", None, Status::Done, "").unwrap();
        }
        let ctx = Ctx::from_store(store);
        let hot = HotScope::enter();
        let before = work::snapshot();
        let head = ctx.store.head(&id).unwrap();
        assert_eq!(recorded_pending_views_ref(&head).unwrap(), expected);
        assert_eq!(list(&ctx, Some(&id), false).unwrap(), pending);
        assert_eq!(list(&ctx, None, false).unwrap(), pending);
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        drop(hot);
        assert_eq!(
            list(&ctx, Some(&id), true).unwrap(),
            all,
            "explicit complete output remains intact"
        );
    }
    assert_eq!(measured[0], measured[1]);
}
