use corral_core::Status;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::HistoryView;
use crate::artifacts::panels_fixture::{key, render};
use crate::board::tests::{Fixture, seeded};
use crate::{Action, View};

/// The seeded board plus T-008 (approved, fast-forward, mitiplay, kind feature) and T-009 (dropped, corral,
/// kind spike), finished in that order after T-007.
fn history_fixture() -> Fixture {
    let fx = seeded();
    let store = &fx.ctx.store;
    let landed = store
        .create("Shard cache landing", Some("mitiplay"), Status::Review, "")
        .unwrap()
        .id;
    let dropped = store
        .create("Abandoned spike", Some("corral"), Status::Ready, "")
        .unwrap()
        .id;
    for (id, status, kind) in [
        (&landed, Status::Done, "feature"),
        (&dropped, Status::Dropped, "spike"),
    ] {
        store
            .update(id, |t| {
                t.status = status;
                t.kind = Some(kind.into());
                Ok(())
            })
            .unwrap();
    }
    store
        .log_event(
            Some(&landed),
            "approved",
            serde_json::json!({"run": "R1", "sha": "abc1234def", "strategy": "fast_forward"}),
        )
        .unwrap();
    store
        .log_event(
            Some(&dropped),
            "discarded",
            serde_json::json!({"run": null}),
        )
        .unwrap();
    fx
}

/// Replaces the filter with `text` (ctrl-u clears it first).
fn set_filter(v: &mut HistoryView, fx: &Fixture, text: &str) {
    v.on_key(
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        &fx.ctx,
    );
    for c in text.chars() {
        v.on_key(key(KeyCode::Char(c)), &fx.ctx);
    }
}

fn result_ids(screen: &str) -> Vec<&str> {
    ["T-007", "T-008", "T-009"]
        .into_iter()
        .filter(|id| screen.contains(id))
        .collect()
}

#[test]
fn history_lists_done_and_dropped_newest_first_with_columns() {
    // Given two finished tasks and a dropped one next to open tasks.
    let fx = history_fixture();

    // When the history view renders.
    let mut v = HistoryView::new(&fx.ctx).unwrap();
    let screen = render(&mut v, &fx.ctx);

    // Then it lists every Done and Dropped task, newest first, with the history columns.
    assert!(screen.contains("history · 2 done · 1 dropped"), "{screen}");
    let header = screen
        .lines()
        .find(|l| l.contains("finished"))
        .unwrap_or_else(|| panic!("{screen}"));
    for col in ["id", "title", "project", "finished", "sha", "art"] {
        assert!(header.contains(col), "missing column {col}: {header}");
    }
    let row = |id: &str| {
        screen
            .lines()
            .position(|l| l.contains(id))
            .unwrap_or_else(|| panic!("{id} missing\n{screen}"))
    };
    assert!(row("T-009") < row("T-008") && row("T-008") < row("T-007"));
    let landed = screen.lines().nth(row("T-008")).unwrap();
    assert!(
        landed.contains("✓") && landed.contains("abc1234 ff") && landed.contains("mitiplay"),
        "{landed}"
    );
    assert!(screen.lines().nth(row("T-009")).unwrap().contains('✗'));
    for open in ["T-001", "T-004", "T-006"] {
        assert!(!screen.contains(open), "{open} is not finished\n{screen}");
    }
}

#[test]
fn history_filters_by_text_project_and_kind_then_opens_detail() {
    let fx = history_fixture();
    let mut v = HistoryView::new(&fx.ctx).unwrap();
    let cases = [
        ("project:mitigraph", vec!["T-007"]),
        ("kind:spike", vec!["T-009"]),
        ("shard", vec!["T-008"]),
        ("dropped", vec!["T-009"]),
        ("abc1234", vec!["T-008"]),
        ("ff", vec!["T-008"]),
        ("project:corral kind:feature", vec![]),
    ];
    for (query, want) in cases {
        // When the filter holds `query`.
        set_filter(&mut v, &fx, query);
        let screen = render(&mut v, &fx.ctx);

        // Then exactly the matching tasks are listed.
        assert_eq!(result_ids(&screen), want, "{query}\n{screen}");
        if want.is_empty() {
            assert!(screen.contains("no tasks match"), "{screen}");
        }
    }

    // When the filter narrows to the dropped task and ⏎ is pressed, its detail opens.
    set_filter(&mut v, &fx, "spike");
    assert_eq!(
        v.on_key(key(KeyCode::Enter), &fx.ctx),
        Action::OpenDetail("T-009".into())
    );
    assert_eq!(v.on_key(key(KeyCode::Esc), &fx.ctx), Action::Back);
}

#[test]
fn history_uses_shared_boolean_queries_pins_and_inline_errors() {
    let fx = history_fixture();
    fx.ctx.store.pin("T-008", None, false).unwrap();
    let mut view = HistoryView::new(&fx.ctx).unwrap();
    set_filter(&mut view, &fx, "status:done ff");
    assert_eq!(view.selected().unwrap().id, "T-008");
    set_filter(&mut view, &fx, "pinned");
    assert_eq!(view.selected().unwrap().id, "T-008");
    set_filter(&mut view, &fx, "\"unterminated");
    let screen = render(&mut view, &fx.ctx);
    assert!(view.error.is_some(), "{screen}");
    assert!(screen.contains(view.error.as_ref().unwrap()), "{screen}");
    assert!(view.selected().is_none());
    set_filter(&mut view, &fx, "project:COR kind:SPI");
    assert_eq!(view.selected().unwrap().id, "T-009");
}

#[test]
fn history_compact_rows_fit_the_results_box() {
    use crate::board::tests::buffer_text;
    use crate::{Hit, HitMap};
    use ratatui::{Terminal, backend::TestBackend};

    let fx = history_fixture();
    let mut view = HistoryView::new(&fx.ctx).unwrap();
    let mut term = Terminal::new(TestBackend::new(40, 15)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| view.draw(f, f.area(), &fx.ctx, &mut hits))
        .unwrap();
    let text = buffer_text(term.backend().buffer());
    for (rect, hit) in hits.iter() {
        if let Hit::Item(super::Z_ROW, n) = hit {
            let task = &view.rows[view.matches[*n]].0;
            assert!(
                text.lines()
                    .nth(usize::from(rect.y))
                    .unwrap()
                    .contains(&task.id),
                "{text}"
            );
            assert!(
                HistoryView::compact_line(task, usize::from(rect.width), false, false).width()
                    <= usize::from(rect.width)
            );
        }
    }
    assert!(
        hits.iter()
            .any(|(_, h)| matches!(h, Hit::Item(super::Z_ROW, _)))
    );
}
