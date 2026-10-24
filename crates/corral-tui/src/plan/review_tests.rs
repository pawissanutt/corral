use corral_core::{PlanReview, PlanRunState, Verdict};
use corral_ops::{PlanReviewOffer, plan_review_offer, plan_review_wait, plan_submit};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;

use super::tests::{PLAN, planning, reviewing};
use crate::board::tests::{Fixture, buffer_text, key, seeded};
use crate::detail::DetailView;
use crate::{Action, Hit, HitMap, Mode, Tone, View};

fn submitted(fx: &mut Fixture) -> String {
    let id = planning(fx);
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    plan_submit(&fx.ctx, &id, &path).unwrap();
    id
}

fn offer(fx: &Fixture, id: &str) -> Option<PlanReviewOffer> {
    plan_review_offer(&fx.ctx, &fx.ctx.store.get(id).unwrap())
}

fn draw(view: &mut DetailView, fx: &Fixture) -> (String, HitMap) {
    let mut term = Terminal::new(TestBackend::new(160, 60)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| view.draw(f, f.area(), &fx.ctx, &mut hits))
        .unwrap();
    (buffer_text(term.backend().buffer()), hits)
}

fn escalate_review(fx: &Fixture, id: &str) {
    let at = fx.now.get();
    fx.ctx
        .store
        .update(id, |t| {
            t.plan_runs[0].state = PlanRunState::Escalated;
            t.plans[0].reviews.push(PlanReview {
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                route: None,
                id: "PR1".into(),
                reviewer: "reviewer".into(),
                policy_digest: String::new(),
                verdict: Some(Verdict::Escalate),
                notes: Some("plan reviewer disappeared before recording a verdict".into()),
                at,
                herdr: None,
                pending_reply: None,
            });
            Ok(())
        })
        .unwrap();
}

fn r_hits(hits: &HitMap) -> Vec<ratatui::layout::Rect> {
    hits.regions()
        .iter()
        .filter(|(_, hit)| matches!(hit, Hit::Key(k) if k.code == KeyCode::Char('R')))
        .map(|(rect, _)| *rect)
        .collect()
}

#[test]
fn submitted_plan_offers_run_review_on_the_panel_and_the_stage_cell() {
    let mut fx = seeded();
    let id = submitted(&mut fx);
    assert_eq!(offer(&fx, &id), Some(PlanReviewOffer::Run));
    let mut view = DetailView::plan(&fx.ctx, &id).unwrap();
    let (text, hits) = draw(&mut view, &fx);

    assert!(text.contains("Run plan review"), "{text}");
    assert!(text.contains("R run review"), "{text}");
    assert!(view.hints().contains(&("R", "Run plan review")));
    let cell_row = text
        .lines()
        .position(|line| line.contains("Plan review >"))
        .unwrap();
    let rects = r_hits(&hits);
    assert!(
        rects.iter().any(|r| usize::from(r.y) <= cell_row + 2
            && usize::from(r.y) >= cell_row
            && r.width > 10),
        "no mouse hit on the Plan review cell: {rects:?}"
    );
    assert!(
        rects.iter().any(|r| usize::from(r.y) > cell_row + 2),
        "no mouse hit on the panel button: {rects:?}"
    );
}

#[test]
fn a_transient_refusal_is_reported_changes_nothing_and_keeps_the_action() {
    let mut fx = seeded();
    let id = submitted(&mut fx);
    let before = fx.ctx.store.get(&id).unwrap();
    let held = fx.ctx.store.try_lock("session-ownership").unwrap().unwrap();
    let mut view = DetailView::plan(&fx.ctx, &id).unwrap();

    let Action::Toast(toast) = view.on_key(key('R'), &fx.ctx) else {
        panic!("R must answer with a toast");
    };
    assert_eq!(toast.tone, Tone::Warn, "{toast:?}");
    assert!(toast.body.contains("pane inventory"), "{toast:?}");
    assert!(toast.body.contains("retry"), "{toast:?}");
    drop(held);

    let after = fx.ctx.store.get(&id).unwrap();
    assert_eq!(after.plans, before.plans);
    assert_eq!(after.plan_runs, before.plan_runs);
    assert_eq!(offer(&fx, &id), Some(PlanReviewOffer::Run));
}

#[test]
fn an_escalated_review_offers_retry_but_a_pending_one_does_not() {
    let mut fx = seeded();
    let id = submitted(&mut fx);
    escalate_review(&fx, &id);
    assert_eq!(offer(&fx, &id), Some(PlanReviewOffer::Retry));
    let mut view = DetailView::plan(&fx.ctx, &id).unwrap();
    let (text, hits) = draw(&mut view, &fx);
    assert!(text.contains("Retry plan review"), "{text}");
    assert!(text.contains("R retry review"), "{text}");
    assert!(!r_hits(&hits).is_empty());

    let mut other = seeded();
    let pending = planning(&mut other);
    reviewing(&other, &pending);
    assert_eq!(offer(&other, &pending), None);
    let mut view = DetailView::plan(&other.ctx, &pending).unwrap();
    let (text, hits) = draw(&mut view, &other);
    assert!(!text.contains("Run plan review"), "{text}");
    assert!(r_hits(&hits).is_empty());
}

#[test]
fn the_board_marks_a_submitted_plan_awaiting_review_without_a_grant() {
    let mut fx = seeded();
    let id = submitted(&mut fx);
    let task = fx.ctx.store.get(&id).unwrap();
    assert!(plan_review_wait(&fx.ctx, &task).is_some());
    let mut app = fx.app(Mode::Board);
    let text = crate::app::mouse_tests::render(&mut app);
    assert!(text.contains("awaiting"), "{text}");
    assert!(!text.contains("waiting for a reviewer"), "{text}");

    escalate_review(&fx, &id);
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(plan_review_wait(&fx.ctx, &task), None);
}
