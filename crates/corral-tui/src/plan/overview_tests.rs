use corral_core::PlanDocument;
use ratatui::crossterm::event::KeyCode;
use ratatui::{Terminal, backend::TestBackend};

use super::overview::{ReviewField, changes};
use super::tests::{PLAN, awaiting, planning, reviewing_text};
use crate::board::tests::{Fixture, buffer_text, code, key, seeded};
use crate::detail::DetailView;
use crate::{Hit, HitMap, View};

pub(crate) fn human_plan() -> String {
    PLAN.replace(
        "goal:",
        "human_review:\n  problem: Operators cannot rotate a widget explicitly.\n  changes: [Add one explicit clockwise spin operation]\n  retained: [Existing automatic rotations keep their schedule]\ngoal:",
    ).replace("\ndecisions: []\n", "\ndecisions: [Reuse the existing widget contract]\n")
}

fn render(view: &mut DetailView, fx: &Fixture, width: u16, height: u16) -> (String, HitMap) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut hits = HitMap::default();
    terminal
        .draw(|f| view.draw(f, f.area(), &fx.ctx, &mut hits))
        .unwrap();
    (buffer_text(terminal.backend().buffer()), hits)
}

#[test]
fn human_overview_precedes_execution_and_navigation_resets_scroll() {
    // Given a human-first plan awaiting owner review.
    let mut fx = seeded();
    let id = planning(&mut fx);
    let text = human_plan();
    let document = PlanDocument::parse(&text).unwrap();
    reviewing_text(&fx, &id, &text);
    awaiting(&fx, &id);
    let mut view = DetailView::plan(&fx.ctx, &id).unwrap();
    let (initial, _) = render(&mut view, &fx, 120, 65);
    let overview = document.human_review.as_ref().unwrap();
    for content in [
        &overview.problem,
        &document.goal,
        &overview.changes[0],
        &overview.retained[0],
        &document.non_goals[0],
        &document.decisions[0],
        &document.acceptance[0].scenario,
    ] {
        assert!(
            initial.contains(content),
            "missing canonical content {content}"
        );
    }
    assert!(!initial.contains(&document.affected_paths[0]));
    assert!(!initial.contains(&document.dependencies[0].task));
    assert!(initial.find(&document.goal) < initial.find(&document.acceptance[0].scenario));
    // When switching after scrolling to the end of the overview.
    view.on_key(code(KeyCode::PageDown), &fx.ctx);
    render(&mut view, &fx, 80, 30);
    view.on_key(key('i'), &fx.ctx);
    // Then implementation starts at its top and the overview button is a real keyboard hit.
    let (details, hits) = render(&mut view, &fx, 120, 90);
    assert!(details.contains(&document.affected_paths[0]), "{details}");
    assert!(details.contains(&document.dependencies[0].task));
    let overview_key = key('o');
    assert!(hits.iter().any(|(_, hit)| *hit == Hit::Key(overview_key)));
    view.on_key(overview_key, &fx.ctx);
    let (returned, _) = render(&mut view, &fx, 120, 65);
    assert_eq!(initial, returned);
}

#[test]
fn compact_human_review_keeps_detail_navigation_clickable() {
    // Given an 80-column plan review.
    let mut fx = seeded();
    let id = planning(&mut fx);
    reviewing_text(&fx, &id, &human_plan());
    awaiting(&fx, &id);
    let mut view = DetailView::plan(&fx.ctx, &id).unwrap();
    // When drawing the toolbar at the smallest supported terminal size.
    let (text, hits) = render(&mut view, &fx, 80, 30);
    // Then details and the existing approval/change actions all have visible click targets.
    for c in ['i', 'a', 'c'] {
        assert!(
            hits.iter().any(|(_, hit)| *hit == Hit::Key(key(c))),
            "{text}"
        );
    }
}

#[test]
fn refreshed_revision_returns_from_scrolled_details_to_overview_top() {
    // Given a review whose implementation is open and scrolled.
    let mut fx = seeded();
    let id = planning(&mut fx);
    let text = human_plan();
    let document = PlanDocument::parse(&text).unwrap();
    reviewing_text(&fx, &id, &text);
    let mut view = DetailView::plan(&fx.ctx, &id).unwrap();
    view.on_key(key('i'), &fx.ctx);
    view.on_key(code(KeyCode::PageDown), &fx.ctx);
    render(&mut view, &fx, 80, 30);
    corral_ops::plan_verdict(
        &fx.ctx,
        &id,
        &corral_ops::PlanVerdict {
            plan: "P1",
            attempt: Some("PR1"),
            verdict: corral_core::Verdict::Changes,
            notes: "Revise the implementation",
        },
    )
    .unwrap();
    let path = fx.ctx.store.root().join("revision.md");
    std::fs::write(&path, text).unwrap();
    corral_ops::plan_submit(&fx.ctx, &id, &path).unwrap();
    // When the open view receives the new revision.
    view.refresh(&fx.ctx);
    let (rendered, _) = render(&mut view, &fx, 80, 30);
    // Then its first review content is visible, rather than the previous scroll position.
    let mut fresh = DetailView::plan(&fx.ctx, &id).unwrap();
    let (expected, _) = render(&mut fresh, &fx, 80, 30);
    assert_eq!(rendered, expected);
    assert!(!rendered.contains(&document.affected_paths[0]));
}

#[test]
fn notification_addition_is_a_structured_included_scope_change() {
    // Given an original scope and a revision that also includes notifications.
    let original = PlanDocument::parse(&human_plan()).unwrap();
    let mut revised = original.clone();
    let notification = "Notify the operator after a spin".to_string();
    revised
        .human_review
        .as_mut()
        .unwrap()
        .changes
        .push(notification.clone());
    // When comparing review content.
    let delta = changes(&original, &revised);
    // Then the added behavior is isolated from the unchanged scope fields.
    assert_eq!(delta.len(), 1);
    assert_eq!(delta[0].field, ReviewField::Included);
    assert_eq!(delta[0].added, [notification]);
    assert!(delta[0].removed.is_empty());
}

#[test]
fn compact_revision_foregrounds_the_added_behavior_before_implementation() {
    // Given a submitted replacement adding notifications to the included work.
    let mut fx = seeded();
    let id = planning(&mut fx);
    let original = human_plan();
    reviewing_text(&fx, &id, &original);
    corral_ops::plan_verdict(
        &fx.ctx,
        &id,
        &corral_ops::PlanVerdict {
            plan: "P1",
            attempt: Some("PR1"),
            verdict: corral_core::Verdict::Changes,
            notes: "Include the requested notification",
        },
    )
    .unwrap();
    let notification = "Notify operator after each spin";
    let replacement = original.replace(
        "changes: [Add one explicit clockwise spin operation]",
        &format!("changes: [Add one explicit clockwise spin operation, {notification}]"),
    );
    let path = fx.ctx.store.root().join("replacement.md");
    std::fs::write(&path, replacement).unwrap();
    corral_ops::plan_submit(&fx.ctx, &id, &path).unwrap();
    let mut view = DetailView::plan(&fx.ctx, &id).unwrap();
    // When opening the revised plan on a compact terminal.
    let (text, _) = render(&mut view, &fx, 80, 30);
    // Then the added canonical behavior is visible without opening the checklist.
    assert!(text.contains(notification), "{text}");
    assert!(!text.contains("Spin core"));
}

#[test]
fn technical_edits_do_not_report_product_scope_changes() {
    // Given unchanged review content with different paths, commands and task dependencies.
    let original = PlanDocument::parse(&human_plan()).unwrap();
    let mut revised = original.clone();
    revised.affected_paths = vec!["src/rotation.rs".into()];
    revised.verification = vec!["cargo test -p rotation".into()];
    revised.dependencies.clear();
    revised.acceptance[0].criterion = Some("C2".into());
    // When comparing review content.
    let delta = changes(&original, &revised);
    // Then no product-scope delta is manufactured from implementation metadata.
    assert!(delta.is_empty());
}

#[test]
fn adding_legacy_review_context_still_exposes_changed_known_boundaries() {
    // Given a historical plan acquiring context and a changed exclusion.
    let original = PlanDocument::parse(PLAN).unwrap();
    let mut revised = PlanDocument::parse(&human_plan()).unwrap();
    revised.non_goals = vec!["No scheduling changes".into()];
    // When comparing the known review fields across formats.
    let fields = changes(&original, &revised)
        .into_iter()
        .map(|c| c.field)
        .collect::<Vec<_>>();
    // Then new context is not called added scope, but existing boundary edits remain visible.
    assert_eq!(fields, [ReviewField::Exclusions, ReviewField::Choices]);
}

#[test]
fn goal_exclusions_and_acceptance_changes_are_independently_visible() {
    // Given a revision changing three owner-review boundaries.
    let original = PlanDocument::parse(&human_plan()).unwrap();
    let mut revised = original.clone();
    revised.goal = "Rotate twice".into();
    revised.non_goals.clear();
    revised.acceptance[0].expected = "two rotations".into();
    // When comparing canonical review values.
    let fields = changes(&original, &revised)
        .into_iter()
        .map(|c| c.field)
        .collect::<Vec<_>>();
    // Then each boundary is flagged independently.
    assert_eq!(
        fields,
        [
            ReviewField::Goal,
            ReviewField::Exclusions,
            ReviewField::Acceptance
        ]
    );
}
