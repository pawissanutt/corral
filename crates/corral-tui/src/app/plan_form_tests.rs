use corral_core::Verdict;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};

use super::App;
use super::mouse_tests::{find_any, mouse};
use crate::board::tests::{buffer_text, code, key, seeded};
use crate::hit::Hit;
use crate::plan::decision::Z_FIELD;
use crate::plan::tests::awaiting_decisions;
use crate::{Action, Mode};

fn render(app: &mut App) -> String {
    render_height(app, 30)
}

fn render_height(app: &mut App, height: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(80, height)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    buffer_text(term.backend().buffer())
}

fn click(app: &mut App, (x, y): (u16, u16)) {
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x, y));
    render(app);
}

#[test]
fn tall_owner_form_reveals_each_keyboard_focused_input() {
    let mut fx = seeded();
    let id = awaiting_decisions(&mut fx, 12);
    let mut app = fx.app(Mode::Board);
    app.apply(Action::OpenPlan(id));
    app.handle_key(key('a'));
    for i in 0..=12 {
        let answer = format!("answer{i}");
        for c in answer.chars() {
            app.handle_key(key(c));
        }
        let text = render(&mut app);
        assert!(
            text.contains(&answer),
            "focused input {i} is hidden\n{text}"
        );
        assert!(
            app.hits
                .iter()
                .any(|(_, hit)| hit == &Hit::Item(Z_FIELD, i))
        );
        app.handle_key(code(KeyCode::Tab));
    }
    // Reverse focus must reveal the field as well.
    app.handle_key(code(KeyCode::BackTab));
    assert!(render(&mut app).contains("answer12"));
    let resized = render_height(&mut app, 20);
    assert!(
        resized.contains("answer12"),
        "resize hid the focused note\n{resized}"
    );
    assert!(resized.contains("[Approve]") && resized.contains("[Cancel]"));
}

#[test]
fn tall_owner_form_allows_mouse_resolution_and_submission() {
    let mut fx = seeded();
    let id = awaiting_decisions(&mut fx, 12);
    let mut app = fx.app(Mode::Board);
    app.apply(Action::OpenPlan(id.clone()));
    let text = render(&mut app);
    click(&mut app, find_any(&text, "[Approve plan]"));
    // Both submission and cancellation remain reachable even before scrolling.
    let text = render(&mut app);
    assert!(
        text.contains("[Approve]") && text.contains("[Cancel]"),
        "{text}"
    );
    click(&mut app, find_any(&text, "[Cancel]"));
    assert!(!render(&mut app).contains("approve plan P1 of"));
    let text = render(&mut app);
    click(&mut app, find_any(&text, "[Approve plan]"));
    for i in 0..=12 {
        let target = (0..20)
            .find_map(|_| {
                render(&mut app);
                let target = app.hits.iter().find_map(|(rect, hit)| {
                    (hit == &Hit::Item(Z_FIELD, i)).then_some((rect.x + 1, rect.y))
                });
                if target.is_none() {
                    app.handle_mouse(mouse(MouseEventKind::ScrollDown, 40, 15));
                }
                target
            })
            .unwrap_or_else(|| panic!("input {i} is unreachable by mouse\n{}", render(&mut app)));
        click(&mut app, target);
        if i < 12 {
            for c in format!("resolve{}", i + 1).chars() {
                app.handle_key(key(c));
            }
        }
    }
    let text = render(&mut app);
    assert!(text.contains("note ›"), "{text}");
    click(&mut app, find_any(&text, "[Approve]"));
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.plans[0].approval.as_ref().unwrap().by, "owner");
    let last = task.plans[0].reviews.last().unwrap();
    assert_eq!(last.verdict, Some(Verdict::Approve));
    for i in 1..=12 {
        assert!(
            last.notes
                .as_ref()
                .unwrap()
                .contains(&format!("Resolved: Owner decision {i} => resolve{i}"))
        );
    }
}
