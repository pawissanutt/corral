use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use corral_core::{PlanRunState, Verdict};
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};

use super::App;
use super::mouse_tests::{FakeClock, find_any, mouse, render};
use crate::board::tests::{Fixture, code, key, seeded};
use crate::plan::tests::{awaiting, planning, reviewing};
use crate::{Action, Mode};

const LEFT: MouseButton = MouseButton::Left;

fn click(app: &mut App, (x, y): (u16, u16)) {
    app.handle_mouse(mouse(MouseEventKind::Down(LEFT), x, y));
    app.handle_mouse(mouse(MouseEventKind::Up(LEFT), x, y));
    render(app);
}

fn awaiting_app(fx: &mut Fixture) -> (App, Rc<Cell<Instant>>, String) {
    let id = planning(fx);
    reviewing(fx, &id);
    awaiting(fx, &id);
    let mut app = fx.app(Mode::Board);
    let now = Rc::new(Cell::new(Instant::now()));
    app.clock = Box::new(FakeClock(now.clone()));
    render(&mut app);
    (app, now, id)
}

/// Double-click the card showing `needle`, like ⏎.
fn open_card(app: &mut App, now: &Rc<Cell<Instant>>, needle: &str) {
    let text = render(app);
    let at = find_any(&text, needle);
    now.set(now.get() + Duration::from_secs(2));
    click(app, at);
    now.set(now.get() + Duration::from_millis(200));
    click(app, at);
}

#[test]
fn mouse_alone_opens_and_approves_an_awaiting_plan_then_opens_a_child() {
    // Given a plan awaiting the owner, counted in the header.
    let mut fx = seeded();
    let (mut app, now, id) = awaiting_app(&mut fx);
    let text = render(&mut app);
    assert!(text.contains("▲ 3 blocked"), "{text}");

    // When the card overview is opened, its Plan link leads to the owner actions.
    open_card(&mut app, &now, "awaiting your");
    let overview = render(&mut app);
    click(&mut app, find_any(&overview, "[8]"));
    assert_eq!(app.stack.len(), 2);
    let text = render(&mut app);
    assert!(
        text.contains("Revisions") && text.contains("[Request changes]"),
        "{text}"
    );

    // When Approve plan and the sheet's Approve are clicked, then the owner decision is recorded.
    click(&mut app, find_any(&text, "[Approve plan]"));
    let text = render(&mut app);
    assert!(text.contains("approve plan P1 of"), "{text}");
    click(&mut app, find_any(&text, "[Approve]"));
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.plans[0].approval.as_ref().unwrap().by, "owner");
    assert_eq!(task.children.len(), 2);

    // And a click on the selected child row opens that child task.
    let text = render(&mut app);
    assert!(text.contains("⊞ decomposed into 2 tasks"), "{text}");
    click(
        &mut app,
        find_any(&text, &format!("▸ {}", task.children[0])),
    );
    assert_eq!(app.stack.len(), 3);
    let text = render(&mut app);
    assert!(text.contains(&format!("{} ", task.children[0])) && text.contains("Spin core"));
}

#[test]
fn mouse_request_changes_and_strip_reach_the_plan_from_any_tab() {
    let mut fx = seeded();
    let (mut app, now, id) = awaiting_app(&mut fx);
    // Given the card menu of an awaiting plan, then it opens the plan and offers no reply or jump.
    let text = render(&mut app);
    click(&mut app, find_any(&text, "awaiting your"));
    app.handle_key(key('.'));
    let text = render(&mut app);
    assert!(text.contains("plan…") && !text.contains("reply…"), "{text}");
    assert!(!text.contains("jump to agent"), "{text}");
    app.handle_key(code(KeyCode::Esc));

    open_card(&mut app, &now, "awaiting your");
    // When another tab is open, then a click on a plan cell of the strip returns to the Plan tab.
    app.handle_key(key('1'));
    let text = render(&mut app);
    assert!(!text.contains("Revisions"), "{text}");
    click(&mut app, find_any(&text, "Your approval >"));
    let text = render(&mut app);
    assert!(text.contains("Revisions"), "{text}");

    // When Request changes is clicked, typed and sent, then an owner changes verdict is recorded.
    click(&mut app, find_any(&text, "[Request changes]"));
    for c in "Split docs".chars() {
        app.handle_key(key(c));
    }
    let text = render(&mut app);
    click(&mut app, find_any(&text, "[Send]"));
    let task = fx.ctx.store.get(&id).unwrap();
    let last = task.plans[0].reviews.last().unwrap();
    assert_eq!(
        (last.reviewer.as_str(), last.verdict, last.notes.as_deref()),
        ("owner", Some(Verdict::Changes), Some("Split docs"))
    );
    assert_eq!(task.plan_runs[0].state, PlanRunState::Changes);
    assert_eq!(app.stack.len(), 2, "the Plan tab stays open");

    // And a review request for a task the planning flow owns opens its plan.
    app.apply(Action::OpenReview(id));
    assert_eq!(app.stack.len(), 3);
    assert!(render(&mut app).contains("Revisions"));
}

#[test]
fn mouse_alone_switches_between_human_overview_and_implementation() {
    // Given a human-first plan open in the real application view stack.
    let mut fx = seeded();
    let id = planning(&mut fx);
    let text = crate::plan::overview_tests::human_plan();
    let document = corral_core::PlanDocument::parse(&text).unwrap();
    crate::plan::tests::reviewing_text(&fx, &id, &text);
    awaiting(&fx, &id);
    let mut app = fx.app(Mode::Board);
    app.apply(Action::OpenPlan(id));
    let initial = render(&mut app);
    assert!(initial.contains(&document.human_review.as_ref().unwrap().problem));
    assert!(!initial.contains(&document.affected_paths[0]));
    // When the implementation button is clicked.
    click(&mut app, find_any(&initial, "[i Implementation details]"));
    let details = render(&mut app);
    // Then the detail mode has a clickable return control, which restores the overview.
    assert!(details.contains("[o Review overview]"));
    click(&mut app, find_any(&details, "[o Review overview]"));
    let returned = render(&mut app);
    assert!(returned.contains(&document.human_review.as_ref().unwrap().problem));
    assert!(!returned.contains(&document.affected_paths[0]));
}
