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
    assert!(toast.title.contains("R retries"), "{toast:?}");
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
fn substantive_escalation_requires_resolution_then_a_new_snapshot() {
    for verdict in [Verdict::Escalate, Verdict::Changes] {
        let mut fx = seeded();
        let id = planning(&mut fx);
        let path = fx.ctx.store.root().join("initial.md");
        std::fs::write(
            &path,
            PLAN.replace(
                "owner_decisions: []",
                "owner_decisions: ['OD-1: choose storage']",
            ),
        )
        .unwrap();
        plan_submit(&fx.ctx, &id, &path).unwrap();
        escalate_review(&fx, &id);
        fx.ctx
            .store
            .update(&id, |t| {
                t.plans[0].reviews[0].verdict = Some(verdict);
                t.plans[0].reviews[0].notes =
                    Some("OD-1: choose storage; E2: revise verification".into());
                Ok(())
            })
            .unwrap();
        assert_eq!(offer(&fx, &id), None);
        let mut view = DetailView::plan(&fx.ctx, &id).unwrap();
        let (text, hits) = draw(&mut view, &fx);
        assert!(r_hits(&hits).is_empty(), "{text}");
        assert!(!text.contains("Retry plan review"), "{text}");
        assert!(
            text.contains("Pending decision: OD-1: choose storage"),
            "{text}"
        );
        assert!(text.contains("Resolve / revise plan"), "{text}");
        assert!(
            text.contains("Resolved: <decision> => <resolution>"),
            "{text}"
        );
        let before = fx.ctx.store.get(&id).unwrap();
        view.on_key(key('R'), &fx.ctx);
        assert_eq!(fx.ctx.store.get(&id).unwrap().plans, before.plans);
        // The owner changes verdict resolves OD-1 and requests the revision.
        view.on_key(key('c'), &fx.ctx);
        assert!(
            draw(&mut view, &fx)
                .0
                .contains("request plan changes P1 of")
        );
        for c in "SQLite".chars() {
            view.on_key(key(c), &fx.ctx);
        }
        view.on_key(crate::board::tests::code(KeyCode::Tab), &fx.ctx);
        for c in "Revise E2".chars() {
            view.on_key(key(c), &fx.ctx);
        }
        let action = view.on_key(crate::board::tests::code(KeyCode::Enter), &fx.ctx);
        assert!(
            matches!(action, Action::Toast(ref t) if t.tone == Tone::Info),
            "{action:?}"
        );
        let resolved = fx.ctx.store.get(&id).unwrap();
        let review = resolved.plans[0].reviews.last().unwrap();
        assert_eq!(review.verdict, Some(Verdict::Changes));
        assert!(
            review
                .notes
                .as_deref()
                .unwrap()
                .contains("Resolved: OD-1: choose storage => SQLite")
        );
        assert_eq!(resolved.plan_runs[0].state, PlanRunState::Changes);
        // Revision launch may be blocked by the fixture's absent project; put its
        // planner at the submission seam. A new snapshot has its own review history.
        fx.ctx
            .store
            .update(&id, |t| {
                t.plan_runs[0].state = PlanRunState::Planning;
                Ok(())
            })
            .unwrap();
        let path = fx.ctx.store.root().join("revised.md");
        std::fs::write(&path, PLAN).unwrap();
        plan_submit(&fx.ctx, &id, &path).unwrap();
        assert_eq!(offer(&fx, &id), Some(PlanReviewOffer::Run));
    }
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

#[test]
fn a_paused_grant_marks_the_plan_but_an_active_grant_leaves_it_to_automation() {
    let mut fx = seeded();
    let id = submitted(&mut fx);
    let root = fx.ctx.store.root().to_path_buf();
    std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&root)
        .status()
        .unwrap();
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("projects.corral.path", "."))
        .unwrap();
    fx.ctx.store.reload_config_if_changed().unwrap();
    fx.grant(&[&id]);
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(plan_review_wait(&fx.ctx, &task), None);

    fx.ctx.store.pause_operating_policy(true).unwrap();
    let reason = plan_review_wait(&fx.ctx, &task).expect("paused automation does not review");
    assert!(reason.contains("paused"), "{reason}");
    let mut app = fx.app(Mode::Board);
    let text = crate::app::mouse_tests::render(&mut app);
    assert!(text.contains("awaiting"), "{text}");
}

#[test]
fn compact_cards_reserve_the_awaiting_marker_at_every_width_in_both_compact_modes() {
    let mut fx = seeded();
    let id = submitted(&mut fx);
    let task = fx.ctx.store.get(&id).unwrap();
    let all = fx.ctx.store.list().unwrap();
    let reason =
        "automatic PlanReview is not authorized for T-008: no matching confirmed workspace grant";
    for by_density in [false, true] {
        let mut config = fx.ctx.store.config.clone();
        if by_density {
            config.board.card.density = corral_core::Density::Compact;
        } else {
            config.ui.compact_cards = true;
        }
        for (width, word) in [
            (8, "\u{26a0}"),
            (11, "plan"),
            (14, "plan"),
            (17, "plan"),
            (24, "plan"),
            (26, "plan review"),
            (40, "awaiting plan review"),
            (70, "awaiting plan review"),
        ] {
            let extra = crate::board::CardExtra {
                plan_review_wait: Some(reason),
                ..Default::default()
            };
            let lines =
                crate::board::card_lines(&task, &all, &config, width, false, fx.now.get(), extra);
            let line = lines[0].to_string();
            assert_eq!(lines.len(), 1, "width {width}: one compact line");
            assert!(
                line.contains('\u{26a0}') && line.contains(word),
                "density mode {by_density}, width {width}: {line:?}"
            );
        }
    }
}
