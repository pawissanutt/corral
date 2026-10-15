use corral_core::{Density, Usage};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::BoardView;
use super::tests::{Fixture, buffer_text, seeded};
use crate::{HitMap, View};

fn render(b: &mut BoardView, fx: &Fixture, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| b.draw(f, f.area(), &fx.ctx, &mut hits))
        .unwrap();
    buffer_text(term.backend().buffer())
}

fn with_usage(fx: &Fixture) {
    let usage = |id: &str, u: Usage| {
        fx.ctx
            .store
            .update(id, |t| {
                t.current_run_mut().unwrap().usage = Some(u);
                Ok(())
            })
            .unwrap();
    };
    usage(
        "T-004",
        Usage {
            input: 1000,
            reported_usd: Some(1.5),
            cost_usd: Some(1.5),
            ..Usage::default()
        },
    );
    usage(
        "T-006",
        Usage {
            input: 2000,
            estimated_usd: Some(2.25),
            ..Usage::default()
        },
    );
}

fn line_with<'a>(text: &'a str, needles: &[&str]) -> &'a str {
    text.lines()
        .find(|l| needles.iter().all(|n| l.contains(n)))
        .unwrap_or_else(|| panic!("no line with {needles:?}\n{text}"))
}

#[test]
fn cost_is_on_every_card_by_default_with_reported_and_estimated_labels() {
    let fx = seeded();
    with_usage(&fx);
    let mut b = fx.board();
    let wide = render(&mut b, &fx, 160, 45);
    assert!(wide.contains("$1.50"), "reported spend\n{wide}");
    assert!(
        wide.contains("sub ~$2.25"),
        "API-equivalent estimate\n{wide}"
    );
    assert!(wide.contains("$0.00"), "a card that never ran\n{wide}");
    let narrow = render(&mut b, &fx, 80, 24);
    assert!(narrow.contains("$1.50"), "cost survives 80x24\n{narrow}");
}

#[test]
fn lowest_priority_fields_drop_first_at_narrow_widths() {
    let mut fx = seeded();
    with_usage(&fx);
    fx.ctx.store.config.board.card.fields = vec!["cost".into(), "tokens".into()];
    let mut b = fx.board();
    let wide = render(&mut b, &fx, 160, 45);
    assert!(wide.contains("1.0k tok"), "{wide}");
    let narrow = render(&mut b, &fx, 80, 24);
    assert!(
        narrow.contains("$1.50") && !narrow.contains("1.0k tok"),
        "{narrow}"
    );
}

#[test]
fn compact_density_is_one_line_per_card() {
    let mut fx = seeded();
    with_usage(&fx);
    fx.ctx.store.config.board.card.density = Density::Compact;
    let mut b = fx.board();
    let text = render(&mut b, &fx, 160, 45);
    let line = line_with(&text, &["T-004", "$1.50"]);
    assert!(!line.contains("working"), "{line}");
}

#[test]
fn column_header_can_show_the_cost_sum() {
    let mut fx = seeded();
    with_usage(&fx);
    fx.ctx.store.config.board.card.column_cost = true;
    let mut b = fx.board();
    let text = render(&mut b, &fx, 160, 45);
    line_with(&text, &["Running", "$1.50"]);
    line_with(&text, &["Review", "$2.25"]);
    assert!(
        !text.contains("-0.00"),
        "an empty column sums to $0.00\n{text}"
    );
}

#[test]
fn per_column_override_replaces_the_field_list() {
    let mut fx = seeded();
    with_usage(&fx);
    fx.ctx
        .store
        .config
        .board
        .card
        .columns
        .insert("review".into(), vec!["tokens".into()]);
    let mut b = fx.board();
    let text = render(&mut b, &fx, 160, 45);
    assert!(
        text.contains("2.0k tok") && !text.contains("sub ~$2.25"),
        "{text}"
    );
    assert!(
        text.contains("$1.50"),
        "other columns keep the default\n{text}"
    );
}
