use corral_core::{Density, Usage, UsageEntry};
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
    line_with(&text, &["Review", "~$2.25"]);
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

fn compact_fixture() -> Fixture {
    let mut fx = seeded();
    with_usage(&fx);
    fx.ctx.store.config.board.card.density = Density::Compact;
    fx
}

#[test]
fn compact_cards_keep_cost_at_both_sizes_with_reported_and_subscription_labels() {
    let fx = compact_fixture();
    let mut b = fx.board();
    let wide = render(&mut b, &fx, 160, 45);
    line_with(&wide, &["T-004", "$1.50"]);
    line_with(&wide, &["T-006", "sub ~$2.25"]);
    let narrow = render(&mut b, &fx, 80, 24);
    line_with(&narrow, &["004 $1.50"]);
    line_with(&narrow, &["06 ~$2.25"]);
}

fn entry(reported: Option<f64>, estimated: Option<f64>, unpriced_tokens: u64) -> UsageEntry {
    UsageEntry {
        input: 100,
        reported_usd: reported,
        estimated_usd: estimated,
        unpriced_tokens,
        ..UsageEntry::default()
    }
}

/// T-004: reported $10 and estimate-only $2 on the run, reported $3 on its review.
fn mixed_accounting(fx: &Fixture, extra: Option<UsageEntry>) {
    fx.ctx
        .store
        .update("T-004", |t| {
            let run = t.current_run_mut().unwrap();
            let mut entries = vec![entry(Some(10.0), None, 0), entry(None, Some(2.0), 0)];
            entries.extend(extra.clone());
            run.usage = Some(Usage::from_entries(entries));
            let mut review = run.clone();
            review.id = "R1-review1".into();
            review.review_runs.clear();
            review.usage = Some(Usage::from_entries(vec![entry(Some(3.0), None, 0)]));
            run.review_runs = vec![review];
            Ok(())
        })
        .unwrap();
}

#[test]
fn mixed_reported_and_estimated_entries_keep_every_known_amount() {
    let fx = seeded();
    mixed_accounting(&fx, None);
    let task = fx.ctx.store.get("T-004").unwrap();
    let cost = corral_core::TaskCost::of(&task);
    assert_eq!((cost.reported, cost.estimated), (Some(13.0), Some(2.0)));
    assert_eq!(cost.total(), 15.0);
    assert_eq!(cost.text(), "$13.00 + sub ~$2.00");
    assert_eq!(cost.short(), "~$15.00");

    let mut fx = fx;
    fx.ctx.store.config.board.card.column_cost = true;
    let mut b = fx.board();
    let text = render(&mut b, &fx, 160, 45);
    line_with(&text, &["$13.00 + sub ~$2.00"]);
    line_with(&text, &["Running", "~$15.00"]);
}

#[test]
fn unpriced_tokens_mark_a_total_as_a_lower_bound() {
    let fx = seeded();
    mixed_accounting(&fx, Some(entry(None, None, 100)));
    let task = fx.ctx.store.get("T-004").unwrap();
    assert_eq!(
        corral_core::TaskCost::of(&task).text(),
        "$13.00 + sub ~$2.00+"
    );
    let reported_only = corral_core::TaskCost {
        reported: Some(5.0),
        unpriced_tokens: 7,
        ..Default::default()
    };
    assert_eq!(reported_only.text(), "$5.00+");
}

fn fg_of(buf: &ratatui::buffer::Buffer, text: &str, needle: &str) -> ratatui::style::Color {
    let (y, line) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains(needle))
        .unwrap_or_else(|| panic!("no {needle:?}\n{text}"));
    let x = line
        .chars()
        .take_while(|_| true)
        .collect::<String>()
        .find(needle)
        .unwrap();
    let col = line[..x].chars().count() as u16;
    buf[(col, y as u16)].fg
}

#[test]
fn cost_warning_colours_the_known_total_not_only_the_reported_part() {
    let render_fg = |warn: Option<f64>| {
        let mut fx = seeded();
        mixed_accounting(&fx, None);
        fx.ctx.store.config.board.card.cost_warn = warn;
        let mut b = fx.board();
        let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
        let mut hits = HitMap::default();
        term.draw(|f| b.draw(f, f.area(), &fx.ctx, &mut hits))
            .unwrap();
        let buf = term.backend().buffer().clone();
        fg_of(&buf, &buffer_text(&buf), "$13.00 + sub")
    };
    assert_eq!(
        render_fg(Some(14.0)),
        crate::theme::YELLOW,
        "$15 known total >= $14"
    );
    assert_ne!(
        render_fg(Some(20.0)),
        crate::theme::YELLOW,
        "below the threshold"
    );
}

fn running_profile(fx: &Fixture, profile: &str, harness: &str) {
    fx.ctx
        .store
        .update("T-004", |t| {
            let run = t.current_run_mut().unwrap();
            run.profile = Some(profile.into());
            run.harness = Some(harness.into());
            Ok(())
        })
        .unwrap();
}

#[test]
fn rendered_cards_drop_the_lowest_priority_field_by_cell_width() {
    let mut fx = seeded();
    with_usage(&fx);
    running_profile(&fx, "界", "omo");
    fx.ctx.store.config.board.card.fields = vec!["profile".into(), "cost".into()];
    let mut b = fx.board();
    let text = render(&mut b, &fx, 80, 24);
    line_with(&text, &["界"]);
    assert!(
        !text.contains("$1.…"),
        "the cost is dropped whole, never clipped\n{text}"
    );

    running_profile(&fx, "e\u{301}e\u{301}", "omo");
    fx.ctx.store.config.board.card.fields = vec!["profile".into(), "harness".into()];
    let mut b = fx.board();
    let text = render(&mut b, &fx, 80, 24);
    line_with(&text, &["e\u{301}e\u{301} · omo"]);
}
