use corral_core::{Density, Usage, UsageEntry};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::tests::{Fixture, buffer_text, seeded};
use super::{BoardView, CardExtra, card_lines};
use crate::theme;
use crate::{Action, Hit, HitMap, MouseCtx, View};
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

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
    line_with(&text, &["e\u{301}e\u{301} ->"]);
    assert!(!line_with(&text, &["e\u{301}e\u{301} ->"]).contains(" · omo"));
}

fn render_hits(b: &mut BoardView, fx: &Fixture, w: u16, h: u16) -> (String, HitMap) {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| b.draw(f, f.area(), &fx.ctx, &mut hits))
        .unwrap();
    (buffer_text(term.backend().buffer()), hits)
}

fn mouse(
    b: &mut BoardView,
    fx: &Fixture,
    hits: &HitMap,
    kind: MouseEventKind,
    (x, y): (u16, u16),
) -> Action {
    let ev = MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    let m = MouseCtx {
        ctx: &fx.ctx,
        hit: hits.at(x, y).cloned(),
        double: false,
    };
    b.on_mouse(ev, &m)
}

fn cell_of(text: &str, row_contains: &str, needle: &str) -> (u16, u16) {
    let (y, line) = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains(row_contains) && l.contains(needle))
        .unwrap_or_else(|| panic!("no row with {row_contains:?} and {needle:?}\n{text}"));
    let byte = line.find(needle).unwrap();
    (
        unicode_width::UnicodeWidthStr::width(&line[..byte]) as u16,
        y as u16,
    )
}

fn is_card(hits: &HitMap, at: (u16, u16), id: &str) -> bool {
    matches!(hits.at(at.0, at.1), Some(Hit::Card(c)) if c == id)
}

#[test]
fn compact_cost_cells_are_card_hits_for_click_and_drag_at_both_sizes() {
    for (w, h) in [(80, 24), (160, 45)] {
        let fx = compact_fixture();
        let mut b = fx.board();
        let (text, hits) = render_hits(&mut b, &fx, w, h);
        let (x0, y) = cell_of(&text, "002", "$0.00");
        for dx in 0..5 {
            assert!(
                is_card(&hits, (x0 + dx, y), "T-002"),
                "{w}x{h}: cost cell {dx} is part of the card\n{text}"
            );
        }

        let last_digit = (x0 + 4, y);
        let action = mouse(
            &mut b,
            &fx,
            &hits,
            MouseEventKind::Down(MouseButton::Left),
            last_digit,
        );
        assert!(matches!(action, Action::None), "{w}x{h}");
        assert!(
            b.menu.is_none(),
            "{w}x{h}: pressing the cost opens no hidden menu"
        );
        assert_eq!(b.selected.as_deref(), Some("T-002"), "{w}x{h}");
        assert!(b.drag.is_some(), "{w}x{h}: the press starts a drag");

        let (rx, ry) = cell_of(&text, "Ready", "╭ Ready");
        let ready = (rx + 2, ry);
        mouse(
            &mut b,
            &fx,
            &hits,
            MouseEventKind::Drag(MouseButton::Left),
            ready,
        );
        assert!(b.dragging(), "{w}x{h}");
        mouse(
            &mut b,
            &fx,
            &hits,
            MouseEventKind::Up(MouseButton::Left),
            ready,
        );
        assert_eq!(
            fx.ctx.store.get("T-002").unwrap().status,
            corral_core::Status::Ready,
            "{w}x{h}: dragging from the cost moved the card"
        );
    }
}

#[test]
fn compact_cards_still_open_the_menu_by_right_click_and_comfortable_keeps_its_glyph_target() {
    for (w, h) in [(80, 24), (160, 45)] {
        let fx = compact_fixture();
        let mut b = fx.board();
        let (text, hits) = render_hits(&mut b, &fx, w, h);
        let (x0, y) = cell_of(&text, "002", "$0.00");
        mouse(
            &mut b,
            &fx,
            &hits,
            MouseEventKind::Down(MouseButton::Right),
            (x0 + 4, y),
        );
        assert!(
            b.menu.is_some(),
            "{w}x{h}: right-click on a compact card opens the menu"
        );
    }
    let mut fx = seeded();
    with_usage(&fx);
    fx.ctx.store.config.board.card.density = Density::Comfortable;
    let mut b = fx.board();
    let (text, hits) = render_hits(&mut b, &fx, 160, 45);
    let (x0, y) = cell_of(&text, "T-002", "⋯");
    assert!(
        matches!(hits.at(x0, y), Some(Hit::CardMenu(c)) if c == "T-002"),
        "the visible glyph stays the menu button\n{text}"
    );
}

/// Configures a seeded project so default Ready admission (empty `ready_projects`) covers it.
pub(crate) fn configure_project(fx: &mut Fixture, name: &str, path: &str) {
    fx.ctx.store.config.projects.insert(
        name.into(),
        corral_core::Project {
            dispatch: Default::default(),
            artifact_retention: None,
            post_land: Vec::new(),
            path: path.into(),
            default_profile: None,
            base: None,
            review: Default::default(),
        },
    );
}

#[test]
fn ready_card_reason_is_weighted_by_owner_attention() {
    // Given the seeded Ready task and each class of shared dispatch-queue reason.
    let fx = seeded();
    let task = fx.ctx.store.get("T-003").unwrap();
    let all = fx.ctx.store.list().unwrap();
    let cases = [
        (
            "eligible",
            "▸ eligible, dispatches on next sync",
            theme::GREEN,
        ),
        (
            "waiting on T-002 (Backlog)",
            "⧗ waiting on T-002",
            theme::DIM,
        ),
        (
            "waiting for a slot (3/3 tasks)",
            "⧗ waiting for a slot",
            theme::BLUE,
        ),
        (
            "waiting for automatic coding capacity (2 review slots reserved)",
            "⧗ waiting for",
            theme::BLUE,
        ),
        (
            "waiting: machine busy (load/core 2.00, max 1.00)",
            "⧗ waiting: machine busy",
            theme::YELLOW,
        ),
        ("dispatch paused", "⧗ dispatch paused", theme::YELLOW),
        (
            "project is not admitted by dispatch.ready_projects",
            "⧗ project is not",
            theme::YELLOW,
        ),
        (
            "pinned queue route changed",
            "⧗ pinned queue route",
            theme::RED,
        ),
    ];
    for (reason, text, color) in cases {
        // When the card renders with that reason.
        let extra = CardExtra {
            wait: Some(reason),
            ..Default::default()
        };
        let lines = card_lines(
            &task,
            &all,
            &fx.ctx.store.config,
            40,
            false,
            task.updated,
            extra,
        );
        // Then the reason line starts with its glyph and carries its attention color.
        let span = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.starts_with(text))
            .unwrap_or_else(|| panic!("no {text:?} in {lines:?}"));
        assert_eq!(span.style.fg, Some(color), "{reason}");
    }
}

#[test]
fn board_ready_cards_show_queue_wait_and_backlog_cards_no_queue_badge() {
    // Given full coding slots, a default-admitted Ready project, and a legacy queued flag on Backlog.
    let mut fx = seeded();
    configure_project(&mut fx, "mitigraph", ".");
    fx.ctx.store.config.limits.running = 2;
    fx.ctx
        .store
        .update("T-002", |t| {
            t.queued = true;
            Ok(())
        })
        .unwrap();
    fx.grant(&["T-003"]);
    let mut b = fx.board();
    for (w, h) in [(160, 45), (100, 30)] {
        // When the board renders at a wide and a narrow size.
        let text = render(&mut b, &fx, w, h);
        // Then the Ready card explains its wait and no card carries a queued badge.
        let ready = line_with(&text, &["⧗ waiting"]);
        assert!(!ready.contains("⧗ waiting on"), "{text}");
        assert!(!text.contains("⏳ queued"), "{text}");
    }
    // When dispatch is paused, the same card names the pause instead.
    fx.ctx.store.config.dispatch.paused = true;
    b.refresh(&fx.ctx);
    let text = render(&mut b, &fx, 160, 45);
    assert!(text.contains("⧗ dispatch paused"), "{text}");
}

/// Each admission class, its compact word, and the glyph that alone tells it apart on the narrowest cards.
const COMPACT_WAITS: [(&str, &str, &str, ratatui::style::Color); 6] = [
    ("eligible", "▸ eligible", "▸", theme::GREEN),
    ("waiting on T-002 (Backlog)", "⛓ T-002", "⛓", theme::DIM),
    (
        "waiting for a slot (3/3 tasks, reviews, gates or reservations)",
        "⧗ slot",
        "⧗",
        theme::BLUE,
    ),
    (
        "waiting for automatic coding capacity (2 review slots reserved)",
        "⧗ coding",
        "⧗",
        theme::BLUE,
    ),
    (
        "waiting: machine busy (load/core 2.00, max 1.00)",
        "⚠ machine",
        "⚠",
        theme::YELLOW,
    ),
    ("dispatch paused", "⏸ paused", "⏸", theme::YELLOW),
];

#[test]
fn compact_ready_cards_keep_the_admission_state_on_their_one_line() {
    // Given compact density and the seeded Ready card, which has never run.
    let mut fx = seeded();
    fx.ctx.store.config.board.card.density = Density::Compact;
    let task = fx.ctx.store.get("T-003").unwrap();
    let all = fx.ctx.store.list().unwrap();
    // 22 cells is a card on a 160-column board, 15 on a 119-column one, and 8 on an 80-column one.
    let sizes = [(40, true), (22, true), (15, false), (10, false), (8, false)];
    for ((width, wide), selected) in sizes.into_iter().flat_map(|s| [(s, false), (s, true)]) {
        for (reason, word, glyph, color) in COMPACT_WAITS {
            // When the card renders with that reason.
            let extra = CardExtra {
                wait: Some(reason),
                ..Default::default()
            };
            let config = &fx.ctx.store.config;
            let lines = card_lines(&task, &all, config, width, selected, task.updated, extra);
            // Then it is still one line of exactly the card width, keeps the configured cost, and shows the
            // state's word where it fits and its glyph alone where it does not, in the attention color.
            assert_eq!(lines.len(), 1, "{width} {reason}: {lines:?}");
            let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(
                unicode_width::UnicodeWidthStr::width(text.as_str()),
                width + 2,
                "{text:?}"
            );
            assert!(text.ends_with("$0.00"), "{width} {reason}: {text:?}");
            assert!(text.contains("03 "), "the id keeps its tail: {text:?}");
            assert_eq!(text.starts_with('▌'), selected, "{text:?}");
            let shown = if wide { word } else { glyph };
            let span = lines[0]
                .spans
                .iter()
                .find(|s| s.content == shown)
                .unwrap_or_else(|| panic!("{width} {reason}: no {shown:?} in {text:?}"));
            assert_eq!(span.style.fg, Some(color), "{width} {reason}");
            if width == 40 {
                assert!(
                    text.contains("Bench seed"),
                    "the title keeps its room: {text:?}"
                );
            }
        }
    }
}

#[test]
fn compact_ready_design_card_keeps_plan_stage_and_admission_state() {
    // Given a design-required Ready task at compact density.
    let mut fx = seeded();
    fx.ctx.store.config.board.card.density = Density::Compact;
    let mut task = fx.ctx.store.get("T-003").unwrap();
    task.design = true;
    let all = fx.ctx.store.list().unwrap();

    // When its paused admission state and plan stage render together.
    let lines = card_lines(
        &task,
        &all,
        &fx.ctx.store.config,
        40,
        false,
        task.updated,
        CardExtra {
            wait: Some("dispatch paused"),
            ..Default::default()
        },
    );

    // Then the one-line card retains both states and its task identity.
    assert_eq!(lines.len(), 1);
    let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
    let expected_stage = crate::plan::PlanStage::Needed.label();
    assert!(text.contains(&expected_stage), "{text:?}");
    assert!(text.contains('⏸'), "{text:?}");
    assert!(text.contains(&task.id), "{text:?}");
}

#[test]
fn compact_board_ready_card_shows_its_live_admission_state_at_both_sizes() {
    // Given compact density and a default-admitted Ready project.
    let mut fx = compact_fixture();
    configure_project(&mut fx, "mitigraph", ".");
    let mut b = fx.board();
    for state in 0..4 {
        // When the queue sees an open dependency, full slots, machine pressure, then a pause.
        match state {
            0 => {
                fx.ctx
                    .store
                    .update("T-003", |t| {
                        t.depends_on = vec!["T-002".into()];
                        Ok(())
                    })
                    .unwrap();
            }
            1 => {
                fx.ctx
                    .store
                    .update("T-003", |t| {
                        t.depends_on.clear();
                        Ok(())
                    })
                    .unwrap();
                fx.ctx.store.config.limits.running = 2;
            }
            2 => {
                let mut busy = corral_ops::resources::FixedResourceProbe::healthy();
                busy.0.load_per_core = 100.0;
                fx.ctx.resource_probe = std::sync::Arc::new(busy);
            }
            _ => fx.ctx.store.config.dispatch.paused = true,
        }
        fx.grant(&["T-003"]);
        b.refresh(&fx.ctx);
        let (word, glyph) = [
            ("⛓ T-002", "⛓"),
            ("⧗ ", "⧗"),
            ("⚠ machine", "⚠"),
            ("⏸ paused", "⏸"),
        ][state];
        for (w, h, needle) in [(160, 45, word), (80, 24, glyph)] {
            // Then the Ready card's single line carries that state at a wide and a narrow size.
            let text = render(&mut b, &fx, w, h);
            let line = line_with(&text, &[needle]);
            assert!(line.contains("$0.00"), "{w}x{h} state {state}\n{text}");
            assert!(
                !text.contains("⧗ waiting"),
                "no normal-density meta line\n{text}"
            );
        }
    }
}

#[test]
fn review_card_preserves_the_complete_automation_blocker_in_both_densities() {
    // Given a Review card and the exact service explanation.
    let fx = seeded();
    let task = fx.ctx.store.get("T-006").unwrap();
    let tasks = fx.ctx.store.list().unwrap();
    let reason = "automatic gate refused: task T-006 is outside the confirmed finite scope";
    for compact in [false, true] {
        let mut config = fx.ctx.store.config.clone();
        config.ui.compact_cards = compact;
        // When a narrow card renders that explanation.
        let lines = card_lines(
            &task,
            &tasks,
            &config,
            24,
            false,
            task.updated,
            CardExtra {
                automation_block: Some(reason),
                ..Default::default()
            },
        );
        // Then wrapping retains every word rather than replacing the reason with an ellipsis.
        let text = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .skip(1)
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
                    .trim()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains(reason), "{text}");
    }
}
