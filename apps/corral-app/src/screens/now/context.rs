//! The lower panels of the dock: what changed since this device's last visit past its seen
//! cursor, the board's held and parked tasks, the selected task's captured log tail with the
//! running agents' tiles, and the selected task's plan graph. Also the terminal tail itself
//! (`.term`), which the Session inspector and the phone's task page show too.

use nsh_core::prelude::*;

use crate::model::daemon::Link;
use crate::model::work::{self, LiveTile, Notice, Tail, TailTone};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::{dim, mono};
use crate::ui::term::{blank, cells};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::feed;
use super::parts::{press, press_id, quiet};

const RING: f32 = 2.0;
const CURSOR_BLINK_MS: f32 = 1100.0;

/// The terminal's ground: dark in both appearances (`--term-bg`). The light theme's terminal is
/// the dark theme's sunk ground; the dark theme's is its shell.
fn term_ground() -> AdaptiveColor {
    AdaptiveColor::new(color(Token::BgSunk).dark, color(Token::Shell).dark)
}

/// A tail line's ink on that dark ground: the dark theme's ink, worker cyan for commands, green
/// for passes (`.term`, `.term .p`, `.term .ok`).
fn term_ink(tone: TailTone) -> AdaptiveColor {
    let token = match tone {
        TailTone::Plain => Token::Ink,
        TailTone::Command => Token::Worker,
        TailTone::Ok => Token::Ok,
    };
    color(token).dark.into()
}

/// The same tones as a terminal shows them: its own accents, ink for plain output.
fn cell_ink(tone: TailTone) -> AdaptiveColor {
    match tone {
        TailTone::Plain => color(Token::Ink),
        TailTone::Command => hue_ink(Hue::Worker),
        TailTone::Ok => hue_ink(Hue::Ok),
    }
}

/// The live or capture tag of a tail: a capture reads as held, a stream as passing.
fn tag(app: AppState, tail: &Tail) -> AnyElement {
    chip(
        app,
        tail.label.clone(),
        if tail.snapshot { Hue::Held } else { Hue::Ok },
    )
}

/// A terminal tail in the mock's terminal look: its pane, runtime and live or capture tag over
/// the lines, with any illustration tag at the trailing edge; the lines mono on the dark
/// terminal ground, the cursor blinking after a live stream. A terminal shows the lines in its
/// own cells. With a `test_id`, the lines are pressable and open the session.
pub(super) fn tail(app: AppState, tail: &Tail, test_id: Option<String>) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let head = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(mono(app, tail.pane.clone()).text_color(color(Token::Ink3)))
        .when(!tail.runtime.is_empty(), |head| {
            head.child(dim(app, tail.runtime.clone()))
        })
        .child(tag(app, tail))
        .child(view().flex_1().min_w(px(0.0)))
        .children(tail.illustrative.map(|note| illus(app, note)));
    let size = if form == Form::Phone { 11.0 } else { 11.5 };
    let lines = tail.lines.iter().map(|line| {
        let words = text(line.text.clone()).number_of_lines(1u32);
        if terminal {
            words.text_color(cell_ink(line.tone)).into_element()
        } else {
            words
                .font_family(FONT_MONO)
                .text_size(px(size))
                .line_height(px((size * 1.55).round()))
                .text_color(term_ink(line.tone))
                .into_element()
        }
    });
    let mut screen = view()
        .flex_col()
        .overflow_hidden()
        .children(lines.collect::<Vec<_>>());
    if !terminal {
        screen = screen
            .bg(term_ground())
            .rounded(px(6.0))
            .px(px(12.0))
            .py(px(10.0));
        if tail.cursor {
            let blink = motion().entering(
                MotionSpec::new(KeyframeTiming::new(CURSOR_BLINK_MS).infinite())
                    .values(MotionChannel::Opacity, [1.0, 0.0, 1.0]),
            );
            screen = screen.child(
                view()
                    .w(px(7.0))
                    .h(px(13.0))
                    .mt(px(3.0))
                    .bg(term_ink(TailTone::Plain))
                    .accessibility_elements_hidden(true)
                    .bind_motion(&blink),
            );
        }
    }
    let screen = match (test_id, tail.action.clone()) {
        (Some(id), Some(action)) => screen
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .label(format!("{} {}", tail.id, tail.label))
            .test_id(&id)
            .on_press(move || press(app, action.clone()))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(RING))
            }),
        _ => screen,
    };
    view()
        .gap(px(if terminal { 0.0 } else { 6.0 }))
        .child(head)
        .child(screen)
        .into_element()
}

/// The Live panel's frame: padded, the selected tail over the tiles.
fn live_frame(app: AppState, selected: AnyElement, tiles: Vec<AnyElement>) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    view()
        .px(if terminal { cells(1.0) } else { px(12.0) })
        .py(px(if terminal { 0.0 } else { 10.0 }))
        .gap(px(if terminal { 0.0 } else { 14.0 }))
        .child(selected)
        .when(terminal, |panel| panel.child(blank()))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(if terminal { 8.0 } else { 12.0 }))
                .children(tiles),
        )
        .into_element()
}

/// The Live panel: the selected task's tail, then a tile for each running agent (PM, workers,
/// gates), each opening its session.
pub(super) fn live(app: AppState) -> AnyElement {
    let form = app.form;
    let live = computed(move || {
        let selection = app.selection();
        app.feed.with(|feed| {
            app.work
                .with(|state| work::daemon_live(state, feed, &selection, form))
        })
    });
    dynamic(move || {
        live.with(|live| {
            let selected = match (&live.selected, &live.selected_note) {
                (Some(selected), _) => tail(app, selected, Some("btn-live-session".into())),
                (None, Some(note)) => dim(app, note.text().to_owned()).into_element(),
                (None, None) => view().into_element(),
            };
            let mut tiles: Vec<AnyElement> =
                live.tiles.iter().map(|tile| tile_view(app, tile)).collect();
            if let Some(note) = live.tiles_note {
                tiles.push(dim(app, note).into_element());
            }
            live_frame(app, selected, tiles)
        })
    })
}

/// A running agent (`.tile`): its state chip, id and role; what it is on; the last lines of its
/// tail; its profile, runtime, pane and since at the foot. Pressing it opens the session.
fn tile_view(app: AppState, tile: &LiveTile) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let action = tile.action.clone();
    let small = |content: String, ink: Token| {
        let words = text(content)
            .number_of_lines(1u32)
            .truncate()
            .text_color(color(ink));
        if terminal {
            words
        } else {
            words.font_family(FONT_TEXT).text_size(px(11.0))
        }
    };
    let doing_ink = if tile.doing_hue == Hue::You {
        hue_ink(Hue::You)
    } else {
        color(if terminal { Token::Ink } else { Token::Ink2 })
    };
    let title = text(tile.title.clone())
        .number_of_lines(1u32)
        .truncate()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink));
    let doing = text(tile.doing.clone())
        .number_of_lines(1u32)
        .truncate()
        .text_color(doing_ink);
    let (title, doing) = if terminal {
        (title, doing)
    } else {
        (
            title.font_family(FONT_TEXT).text_size(px(13.5)),
            doing.font_family(FONT_TEXT).text_size(px(12.0)),
        )
    };
    let pad = |v: View| if terminal { v } else { v.px(px(12.0)) };
    let lines = tile.tail.lines.iter().map(|line| {
        let words = text(line.text.clone()).number_of_lines(1u32);
        if terminal {
            words.text_color(cell_ink(line.tone)).into_element()
        } else {
            words
                .font_family(FONT_MONO)
                .text_size(px(11.0))
                .line_height(px(17.0))
                .text_color(term_ink(line.tone))
                .into_element()
        }
    });
    let screen = view()
        .flex_col()
        .overflow_hidden()
        .gap(px(if terminal { 0.0 } else { 4.0 }))
        .child(view().flex_row().child(tag(app, &tile.tail)))
        .children(lines.collect::<Vec<_>>());
    let screen = if terminal {
        screen
    } else {
        screen
            .bg(term_ground())
            .min_h(px(92.0))
            .px(px(12.0))
            .py(px(8.0))
    };
    let foot = pad(view())
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .py(px(if terminal { 0.0 } else { 6.0 }))
        .when(!terminal, |foot| {
            foot.border_t_1().border_t_color(color(Token::Rule))
        })
        .when(!tile.profile.is_empty(), |foot| {
            foot.child(small(tile.profile.clone(), Token::Ink3))
        })
        .when(!tile.runtime.is_empty(), |foot| {
            foot.child(small(tile.runtime.clone(), Token::Ink3))
        })
        .child(mono(app, tile.pane.clone()).text_color(color(Token::Ink3)))
        .child(view().flex_1().min_w(px(0.0)))
        .child(small(tile.since.clone(), Token::Ink3));
    let head = pad(view())
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .pt(px(if terminal { 0.0 } else { 9.0 }))
        .pb(px(if terminal { 0.0 } else { 2.0 }))
        .child(chip(app, tile.chip, tile.chip_hue))
        .child(mono(app, tile.id.clone()).text_color(color(Token::Ink3)))
        .child(view().flex_1().min_w(px(0.0)))
        .child(dim(app, tile.role.clone()));
    let card = view()
        .flex_col()
        .w(if terminal { cells(36.0) } else { px(240.0) })
        .flex_grow(1.0)
        .min_w(px(0.0))
        .overflow_hidden()
        .label(format!("{} {} {}", tile.id, tile.chip, tile.doing))
        .test_id(&format!("agent-{}", tile.id))
        .child(head)
        .child(pad(view()).child(title))
        .child(
            pad(view())
                .pb(px(if terminal { 0.0 } else { 8.0 }))
                .child(doing),
        )
        .child(screen)
        .child(foot);
    let card = match action {
        Some(action) => card
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .on_press(move || press(app, action.clone())),
        None => card,
    };
    if terminal {
        return card.child(blank()).into_element();
    }
    card.rounded(px(8.0))
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule))
        .hover(|s| s.border_color(color(Token::Rule2)))
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(RING))
        })
        .into_element()
}

fn rows_frame(app: AppState, rows: Vec<AnyElement>) -> AnyElement {
    view()
        .py(px(if app.form == Form::Terminal { 0.0 } else { 4.0 }))
        .children(rows)
        .into_element()
}

/// "Since your last visit": the changes past this device's seen cursor, newest first; each
/// event opens its task.
pub(super) fn since(app: AppState) -> AnyElement {
    let since = computed(move || app.feed.with(work::daemon_since));
    dynamic(move || {
        since.with(|since| {
            if let Some(notice) = &since.notice {
                return quiet(app, notice.text().to_owned());
            }
            rows_frame(
                app,
                since
                    .rows
                    .iter()
                    .enumerate()
                    .map(|(index, row)| feed::row(app, row, format!("since-{index}")))
                    .collect(),
            )
        })
    })
}

/// "Parked": the board's held and parked tasks.
pub(super) fn parked(app: AppState) -> AnyElement {
    dynamic(move || {
        app.feed.with(|feed| {
            if let Some(line) = feed.device.board_notice() {
                return quiet(app, line);
            }
            if !feed.board.complete {
                // Nothing to say until the first board arrives or the link fails.
                return if feed.link == Link::Down {
                    quiet(app, work::NO_BOARD)
                } else {
                    view().into_element()
                };
            }
            let rows = work::daemon_holds(feed);
            if rows.is_empty() {
                return quiet(app, work::NOTHING_HELD);
            }
            rows_view(app, &rows)
        })
    })
}

fn rows_view(app: AppState, rows: &[work::HoldRow]) -> AnyElement {
    rows_frame(app, rows.iter().map(|row| feed::hold(app, row)).collect())
}

/// The graph's heading, opening the plan when it has an action.
fn graph_heading(app: AppState, words: String, action: Option<work::Press>) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let heading = text(words.clone())
        .number_of_lines(1u32)
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    let heading = if terminal {
        heading
    } else {
        heading.font_family(FONT_TEXT).text_size(px(12.5))
    };
    match action {
        Some(action) => view()
            .flex_row()
            .cursor_pointer()
            .role(AccessibilityRole::Link)
            .label(words)
            .test_id(&press_id(&action))
            .on_press(move || press(app, action.clone()))
            .child(heading)
            .into_element(),
        None => heading.into_element(),
    }
}

fn graph_frame(app: AppState) -> View {
    let terminal = app.form == Form::Terminal;
    view()
        .px(if terminal { cells(1.0) } else { px(14.0) })
        .py(px(if terminal { 0.0 } else { 10.0 }))
        .gap(px(if terminal { 0.0 } else { 6.0 }))
}

/// The plan graph: the selected task's plan with the task highlighted, from the Plans screen;
/// its heading opens the plan. A task in no plan says so.
pub(super) fn graph(app: AppState) -> AnyElement {
    let form = app.form;
    let context = computed(move || {
        let selection = app.selection();
        app.feed.with(|feed| {
            app.work
                .with(|state| work::daemon_plan(state, feed, &selection, form))
        })
    });
    dynamic(move || context.with(|context| live_graph(app, context)))
}

/// A live plan hand-off: the heading and the plan's body, or why there is none.
pub(super) fn live_graph(app: AppState, context: &work::LivePlan) -> AnyElement {
    graph_frame(app)
        .children(
            context
                .plan
                .is_some()
                .then(|| graph_heading(app, context.heading.clone(), context.action.clone())),
        )
        .child(plan_body(app, context))
        .into_element()
}

/// What a plan section holds under its heading: the Plans screen's live graph, or why there is
/// none.
pub(super) fn plan_body(app: AppState, context: &work::LivePlan) -> AnyElement {
    match &context.plan {
        Some(plan) => crate::screens::plans::live_graph(app, plan.clone(), context.member.clone()),
        None => view()
            .children(
                context
                    .empty
                    .as_ref()
                    .map(|empty: &Notice| dim(app, empty.text().to_owned())),
            )
            .into_element(),
    }
}
