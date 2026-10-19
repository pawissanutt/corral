use corral_core::{Status, Store};
use corral_ops::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use serde_json::json;

use super::timeline::Z_TIMELINE;
use super::{DetailView, PLAN, TIMELINE};
use crate::artifacts::panels_fixture::{key, render_at};
use crate::{Action, Hit, MouseCtx, View};

fn fixture() -> (tempfile::TempDir, Ctx, String) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store
        .create("Task history log", None, Status::Idea, "")
        .unwrap();
    let events = include_str!("../../../corral-ops/tests/fixtures/task-history.jsonl")
        .replace("T-123", &task.id);
    std::fs::write(store.dir().join("events.jsonl"), events).unwrap();
    (dir, crate::board::tests::fixture_ctx(store), task.id)
}

fn timeline(view: &mut DetailView, ctx: &Ctx) -> String {
    view.on_key(key(KeyCode::Char('7')), ctx);
    render_at(view, ctx, 120, 35).0
}

#[test]
fn timeline_number_and_mouse_tabs_preserve_the_existing_order() {
    // Given a task with and without the conditional Plan tab.
    let (_dir, ctx, id) = fixture();
    for design in [false, true] {
        ctx.store
            .update(&id, |task| {
                task.design = design;
                Ok(())
            })
            .unwrap();
        let mut view = DetailView::new(&ctx, &id).unwrap();
        view.on_key(key(KeyCode::Char('1')), &ctx);

        // When each rendered tab's mouse hit is dispatched as its number key.
        let (screen, hits) = render_at(&mut view, &ctx, 80, 35);
        let names = [
            "Spec",
            "Runs",
            "Artifacts",
            "Diff",
            "Acceptance",
            "Comments",
            "Timeline",
            "Plan",
        ];
        for (i, name) in names.iter().enumerate().take(if design { 8 } else { 7 }) {
            let number = char::from_digit(u32::try_from(i + 1).unwrap(), 10).unwrap();
            let (rect, hit) = hits
                .iter()
                .find(|(_, h)| *h == Hit::Key(key(KeyCode::Char(number))))
                .unwrap();
            assert_eq!(hits.at(rect.x, rect.y), Some(hit));
            let Hit::Key(pressed) = hit else {
                unreachable!()
            };
            view.on_key(*pressed, &ctx);

            // Then Spec..Comments retain keys 1..6, Timeline is 7, and Plan is last.
            assert!(screen.contains(&format!("{number} {name}")), "{screen}");
            assert_eq!(view.tab, i);
        }
        view.on_key(key(KeyCode::Char('7')), &ctx);
        assert_eq!(view.tab, TIMELINE);
        view.on_key(key(KeyCode::Tab), &ctx);
        assert_eq!(view.tab, if design { PLAN } else { 0 });
        view.on_key(key(KeyCode::BackTab), &ctx);
        assert_eq!(view.tab, TIMELINE);
    }
}

#[test]
fn timeline_loads_on_first_visit_and_invalidates_on_refresh() {
    // Given an unopened timeline and an event written after detail construction.
    let (_dir, ctx, id) = fixture();
    let mut view = DetailView::new(&ctx, &id).unwrap();
    ctx.store
        .log_event(Some(&id), "replied", json!({"text":"FIRST-VISIT"}))
        .unwrap();

    // When first visited, revisited before refresh, then refreshed on and off the tab.
    assert!(timeline(&mut view, &ctx).contains("FIRST-VISIT"));
    ctx.store
        .log_event(Some(&id), "replied", json!({"text":"AFTER-LOAD"}))
        .unwrap();
    assert!(
        !timeline(&mut view, &ctx).contains("AFTER-LOAD"),
        "cached on revisit"
    );
    view.refresh(&ctx);

    // Then active refresh reloads immediately; inactive refresh leaves the next visit lazy.
    assert!(render_at(&mut view, &ctx, 120, 35).0.contains("AFTER-LOAD"));
    view.on_key(key(KeyCode::Char('1')), &ctx);
    view.refresh(&ctx);
    ctx.store
        .log_event(Some(&id), "replied", json!({"text":"AFTER-REFRESH"}))
        .unwrap();
    assert!(timeline(&mut view, &ctx).contains("AFTER-REFRESH"));
}

#[test]
fn timeline_noise_toggle_is_local_and_reversible() {
    // Given the scenario's two Noise rows hidden on first visit.
    let (_dir, ctx, id) = fixture();
    // Close the operation once so advancing open ages cannot change screen equality.
    ctx.store
        .log_event(Some(&id), "unstuck", json!({"operation":"install"}))
        .unwrap();
    let mut view = DetailView::new(&ctx, &id).unwrap();
    let relevant = timeline(&mut view, &ctx);
    assert!(!relevant.contains("future_kind") && !relevant.contains("reply_delivery"));

    // When a is pressed on Timeline.
    assert_eq!(view.on_key(key(KeyCode::Char('a')), &ctx), Action::None);
    let all = render_at(&mut view, &ctx, 120, 35).0;

    // Then Noise appears, can be hidden again, and a still jumps from Comments.
    assert!(
        all.contains("future_kind") && all.contains("reply_delivery"),
        "{all}"
    );
    view.on_key(key(KeyCode::Char('a')), &ctx);
    assert_eq!(render_at(&mut view, &ctx, 120, 35).0, relevant);
    view.on_key(key(KeyCode::Char('6')), &ctx);
    assert_eq!(view.on_key(key(KeyCode::Char('a')), &ctx), Action::Jump(id));
}

#[test]
fn timeline_scenario_rows_render_once_without_wrapping_at_80_and_120() {
    // Given the shared scenario fixture, including unknown Noise and another task.
    let (_dir, ctx, id) = fixture();
    let entries = corral_ops::timeline(&ctx.store, &id, corral_ops::TimelineFilter::All).unwrap();
    for width in [80, 120] {
        let mut view = DetailView::new(&ctx, &id).unwrap();
        view.on_key(key(KeyCode::Char('7')), &ctx);
        view.on_key(key(KeyCode::Char('a')), &ctx);

        // When the real detail is rendered at each required terminal width.
        let (screen, hits) = render_at(&mut view, &ctx, width, 35);
        let rows: Vec<_> = hits
            .iter()
            .filter_map(|(rect, hit)| match hit {
                Hit::Item(Z_TIMELINE, i) => Some((rect, *i)),
                _ => None,
            })
            .collect();

        // Then every entry occupies one consecutive clickable row, never a continuation.
        assert_eq!(rows.len(), 15, "{screen}");
        for pair in rows.windows(2) {
            assert_eq!(pair[0].0.y + 1, pair[1].0.y, "{screen}");
        }
        for (rect, index) in &rows {
            let row = screen.lines().nth(usize::from(rect.y)).unwrap();
            let entry = &entries[*index];
            let time = crate::artifacts::ui::local_date(entry.at, "%Y-%m-%d %H:%M");
            assert!(row.contains(&time), "{row}");
            assert!(
                row.contains(&crate::artifacts::list::truncate(&entry.kind, 13)),
                "{row}"
            );
            assert!(row.contains(entry.actor.as_deref().unwrap_or("?")), "{row}");
            assert!(row.contains(entry.via.as_deref().unwrap_or("?")), "{row}");
            assert!(row.contains(entry.run.as_deref().unwrap_or("-")), "{row}");
        }
        assert!(!screen.contains("OTHER TASK") && !screen.contains("GLOBAL MUST"));
        assert!(screen.contains("1h 05m"), "{screen}");
        if width == 120 {
            assert!(screen.contains("jsonl or sqlite?"), "{screen}");
        }
        println!("Timeline scenario at {width} columns:\n{screen}");
    }
}

#[test]
fn timeline_mouse_selection_opens_raw_json_and_modal_scrolls() {
    // Given Timeline's actual rendered row hit for the edited event.
    let (_dir, ctx, id) = fixture();
    let mut view = DetailView::new(&ctx, &id).unwrap();
    timeline(&mut view, &ctx);
    let (_, hits) = render_at(&mut view, &ctx, 80, 24);
    let (rect, hit) = hits
        .iter()
        .find(|(_, h)| *h == Hit::Item(Z_TIMELINE, 1))
        .unwrap();
    let mouse = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };

    // When that row is clicked, then Enter opens its raw event.
    view.on_mouse(
        mouse,
        &MouseCtx {
            ctx: &ctx,
            hit: Some(hit.clone()),
            double: false,
        },
    );
    assert!(!view.timeline.overlay_open());
    view.on_key(key(KeyCode::Enter), &ctx);
    let (before, modal_hits) = render_at(&mut view, &ctx, 80, 24);

    // Then JSON preserves the payload, scrolls modally, and Esc returns to Timeline.
    assert!(
        before.contains("Raw event · edited") && before.contains("\"kind\": \"edited\""),
        "{before}"
    );
    assert!(
        !modal_hits
            .iter()
            .any(|(_, h)| matches!(h, Hit::Item(Z_TIMELINE, _)))
    );
    view.on_key(key(KeyCode::PageDown), &ctx);
    let after = render_at(&mut view, &ctx, 80, 24).0;
    assert_ne!(before, after);
    assert!(after.contains("\"sha_to\": \"9a0e\""), "{after}");
    view.on_key(key(KeyCode::Char('1')), &ctx);
    assert_eq!(view.tab, TIMELINE, "overlay owns navigation keys");
    view.on_key(key(KeyCode::Esc), &ctx);
    assert!(!view.timeline.overlay_open());
    assert_eq!(view.tab, TIMELINE);
}

#[test]
fn timeline_keyboard_selection_stays_visible_in_short_terminals() {
    // Given more rows than a short terminal can display.
    let (_dir, ctx, id) = fixture();
    let mut view = DetailView::new(&ctx, &id).unwrap();
    timeline(&mut view, &ctx);

    // When selecting the last relevant row with the keyboard.
    for _ in 0..12 {
        view.on_key(key(KeyCode::Down), &ctx);
    }
    let (screen, hits) = render_at(&mut view, &ctx, 80, 14);

    // Then the selected row is visible and Enter opens that row's raw event.
    assert!(
        hits.iter()
            .any(|(_, hit)| *hit == Hit::Item(Z_TIMELINE, 12)),
        "{screen}"
    );
    view.on_key(key(KeyCode::Enter), &ctx);
    assert!(
        render_at(&mut view, &ctx, 80, 24)
            .0
            .contains("\"kind\": \"plan_verdict\"")
    );
}
