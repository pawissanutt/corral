//! The agents wall (`.wall`, `.tile`, `.machine` on desktop; `.ph-sec` and `.prow` on the phone;
//! one line per agent in a terminal), between the assistant's quick thread and its blocks.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Action, Mode, Tile, Wall};
use crate::model::hue::Hue;
use crate::screens::assistant;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::seg;
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::{dot, hue_ink};
use crate::ui::kit::page::{body, group_head, head, pane, search};
use crate::ui::kit::rows::list_row;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::console::{self, TILE};
use super::{run, wall_view};

/// Tiles per row of the desktop grid (`repeat(3, minmax(0, 1fr))`).
const COLUMNS: usize = 3;
/// The gap between tiles, and the wall's paddings (`.wall`: 14 18 20).
const GAP: f32 = 12.0;
/// Cells the terminal row keeps for an agent's title: "supervisor · generation 14" whole.
const TERM_TITLE: f32 = 26.0;

/// The whole page for the form.
pub fn screen(app: AppState) -> AnyElement {
    let wall = computed(move || wall_view(app));
    let (title, subtitle, labels) =
        untracked(move || wall.with(|w| (w.title, w.subtitle, w.mode_labels)));
    let mode_switch = move || {
        seg(
            app,
            &[(Mode::Wall, labels[0]), (Mode::List, labels[1])],
            move || wall.with(|w| w.mode),
            move |mode| run(app, Action::SetMode(mode)),
            "seg-agents",
        )
    };
    let content = dynamic(move || wall.with(|w| content(app, w)));
    match app.form {
        Form::Desktop => view()
            .size_full()
            .flex_col()
            .bg(color(Token::Bg))
            .child(head(
                app,
                title,
                Some(subtitle.to_owned()),
                vec![mode_switch()],
            ))
            .child(pane(app, "agents-wall", content))
            .into_element(),
        Form::Phone => view()
            .flex_1()
            .min_h(px(0.0))
            .flex_col()
            .bg(color(Token::Bg))
            .child(head(app, title, None, vec![search(app)]))
            .child(pane(app, "agents-wall", content))
            .into_element(),
        Form::Terminal => view()
            .size_full()
            .flex_col()
            .child(head(app, title, None, vec![mode_switch()]))
            .child(body(app, "agents-wall", content))
            .into_element(),
    }
}

/// What scrolls under the head: the blocks and the agents in the mockup's order.
fn content(app: AppState, wall: &Wall) -> AnyElement {
    let first = untracked(move || assistant::agents_head(app));
    let last = untracked(move || assistant::agents_tail(app));
    match app.form {
        Form::Desktop => view()
            .child(machine_line(app, wall))
            .child(match wall.mode {
                Mode::Wall => grid(app, first, &wall.tiles),
                Mode::List => view()
                    .pt(px(14.0))
                    .pb(px(20.0))
                    .child(first)
                    .child(view().h(px(GAP)))
                    .children(wall.tiles.iter().map(|tile| list_line(app, tile)))
                    .into_element(),
            })
            .child(last)
            .child(
                view()
                    .px(px(18.0))
                    .pb(px(16.0))
                    .child(illus(app, wall.footer)),
            )
            .into_element(),
        Form::Phone => view()
            .child(first)
            .child(group_head(
                app,
                wall.machine,
                Some(wall.count.to_string()),
                Hue::Neutral,
                Some(wall.phone_machine_detail.to_owned()),
                Some(illus(app, wall.phone_machine_note)),
            ))
            .children(wall.tiles.iter().map(|tile| phone_row(app, tile)))
            .child(last)
            .into_element(),
        Form::Terminal => view()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .h(term::rows(1.0))
                    .child(
                        text(wall.machine)
                            .flex_shrink_0()
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::Ink)),
                    )
                    .child(term::space(2.0))
                    .child(
                        term::quiet(wall.machine_detail)
                            .flex_shrink(1.0)
                            .min_w(px(0.0)),
                    )
                    .child(term::space(2.0))
                    .child(illus(app, wall.machine_note)),
            )
            .child(term::blank())
            .child(first)
            .child(term::hrule())
            .children(wall.tiles.iter().map(|tile| list_line(app, tile)))
            .child(term::hrule())
            .child(last)
            .child(term::blank())
            .child(illus(app, wall.footer))
            .into_element(),
    }
}

/// `.machine`: the terminal icon, the machine in bold, what runs there, the illustrative tag,
/// and at the trailing edge where other machines will come from.
fn machine_line(app: AppState, wall: &Wall) -> AnyElement {
    let quiet = |content: &'static str| {
        text(content)
            .number_of_lines(1u32)
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .font_family(FONT_TEXT)
            .text_size(px(12.5))
            .text_color(color(Token::Ink3))
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(18.0))
        .pt(px(14.0))
        .child(icon(Icon::Term, 15.0, color(Token::Ink2)))
        .child(
            text(wall.machine)
                .flex_shrink_0()
                .font_family(FONT_TEXT)
                .text_size(px(12.5))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .child(quiet(wall.machine_detail))
        .child(illus(app, wall.machine_note))
        .child(view().flex_1().min_w(px(GAP)))
        .child(quiet(wall.other_machines))
        .into_element()
}

/// The tiles in rows of three, the quick thread first; the last row keeps its column widths.
fn grid(app: AppState, first: AnyElement, tiles: &[Tile]) -> AnyElement {
    let mut cells: Vec<AnyElement> = Vec::with_capacity(tiles.len() + 1);
    cells.push(first);
    cells.extend(tiles.iter().map(|t| tile(app, t)));
    let mut rows = Vec::new();
    let mut cells = cells.into_iter().peekable();
    while cells.peek().is_some() {
        let mut row = Vec::with_capacity(COLUMNS);
        for _ in 0..COLUMNS {
            let cell = view().flex_1().min_w(px(0.0)).flex_col();
            row.push(match cells.next() {
                Some(content) => cell.child(content).into_element(),
                None => cell.into_element(),
            });
        }
        rows.push(
            view()
                .flex_row()
                .items_stretch()
                .gap(px(GAP))
                .children(row)
                .into_element(),
        );
    }
    view()
        .gap(px(GAP))
        .px(px(18.0))
        .pt(px(14.0))
        .pb(px(20.0))
        .children(rows)
        .into_element()
}

/// What a screen reader hears for an agent: chip, id, title, what it does, feed, since.
fn spoken(tile: &Tile) -> String {
    let a = &tile.agent;
    [
        tile.chip,
        a.id,
        a.title,
        a.doing,
        a.feed.label(),
        tile.since.as_str(),
    ]
    .join(", ")
}

fn test_id(tile: &Tile) -> String {
    format!("agent-{}", tile.agent.id)
}

/// The colour of what the agent is doing: needs you in its colour, else the secondary ink.
fn doing_ink(tile: &Tile) -> AdaptiveColor {
    match tile.doing_hue {
        Hue::Neutral => color(Token::Ink2),
        hue => hue_ink(hue),
    }
}

/// One desktop tile (`.tile`): chip, id and role; the title; what it does now; the tail in the
/// terminal look under its live or snapshot tag; profile, runtime, pane and since. Pressing it
/// opens the agent's session.
fn tile(app: AppState, tile: &Tile) -> AnyElement {
    let action = tile.action;
    let a = tile.agent;
    let small = |content: &'static str, ink: Token| {
        text(content)
            .number_of_lines(1u32)
            .flex_shrink_0()
            .font_family(FONT_TEXT)
            .text_size(px(11.0))
            .text_color(color(ink))
    };
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule))
        .rounded(px(8.0))
        .overflow_hidden()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(spoken(tile))
        .test_id(&test_id(tile))
        .hover(|s| s.border_color(color(Token::Rule2)))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(2.0))
        })
        .on_press(move || run(app, action))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .px(px(12.0))
                .pt(px(9.0))
                .pb(px(2.0))
                .child(chip(app, tile.chip, tile.hue))
                .child(
                    text(a.id)
                        .number_of_lines(1u32)
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .font_family(FONT_MONO)
                        .text_size(px(11.5))
                        .text_color(color(Token::Ink3)),
                )
                .child(view().flex_1().min_w(px(0.0)))
                .child(small(a.role, Token::Ink3).text_size(px(12.0))),
        )
        .child(
            text(a.title)
                .truncate()
                .px(px(12.0))
                .font_family(FONT_TEXT)
                .text_size(px(13.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink)),
        )
        .child(
            text(a.doing)
                .truncate()
                .px(px(12.0))
                .pt(px(2.0))
                .pb(px(8.0))
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .text_color(doing_ink(tile)),
        )
        .child(
            view()
                .flex_1()
                .min_h(px(92.0))
                .gap(px(4.0))
                .px(px(12.0))
                .py(px(8.0))
                .bg(console::ground())
                .child(console::tag(a.feed, a.feed.wall_label()))
                .child(console::lines(&tile.tail, TILE, false)),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .px(px(12.0))
                .py(px(6.0))
                .border_t_1()
                .border_t_color(color(Token::Rule))
                .child(small(a.profile, Token::Ink3))
                .child(
                    small(a.runtime, Token::Ink3)
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .truncate(),
                )
                .child(small(a.pane, Token::Ink3).font_family(FONT_MONO))
                .child(view().flex_1().min_w(px(0.0)))
                .child(
                    text(tile.since.clone())
                        .number_of_lines(1u32)
                        .flex_shrink_0()
                        .font_family(FONT_TEXT)
                        .text_size(px(11.0))
                        .text_color(color(Token::Ink3)),
                ),
        )
        .into_element()
}

/// A one-line row per agent: the desktop List mode and the terminal wall. Chip, id, title,
/// what it does (left out on a narrow terminal grid), the feed and since.
fn list_line(app: AppState, tile: &Tile) -> AnyElement {
    let action = tile.action;
    let a = tile.agent;
    let form = app.form;
    let terminal = form == Form::Terminal;
    let fixed = |width: f32, child: AnyElement| {
        let slot = view().flex_row().items_center().flex_shrink_0();
        let slot = if terminal {
            slot.w(term::cells(width))
        } else {
            slot.w(px(width * 8.0))
        };
        slot.child(child).into_element()
    };
    let words = |content: SharedString, ink: AdaptiveColor, mono: bool| {
        let t = text(content).truncate().min_w(px(0.0)).text_color(ink);
        if terminal {
            t
        } else {
            t.font_family(if mono { FONT_MONO } else { FONT_TEXT })
                .text_size(px(if mono { 11.5 } else { 12.5 }))
        }
    };
    let quiet = color(Token::Ink3);
    // In a terminal the title keeps its room and what the agent does gives way first; on a
    // narrow grid it is left out and the title takes the rest.
    let doing = view()
        .flex_row()
        .min_w(px(0.0))
        .map(|slot| {
            if terminal {
                slot.flex_1().style_when(|| !term::wide(), |s| s.hidden())
            } else {
                slot.flex_shrink_0().w(px(320.0))
            }
        })
        .child(words(a.doing.into(), doing_ink(tile), false))
        .into_element();
    let title = view()
        .min_w(px(0.0))
        .map(|slot| {
            if terminal {
                slot.w(term::cells(TERM_TITLE))
                    .flex_shrink_0()
                    .style_when(|| !term::wide(), |s| s.flex_1())
            } else {
                slot.flex_1()
            }
        })
        .child(
            words(a.title.into(), color(Token::Ink), false)
                .when(!terminal, |t| t.font_weight(FontWeight::SEMIBOLD)),
        )
        .into_element();
    list_row(
        app,
        spoken(tile),
        test_id(tile),
        || false,
        move || run(app, action),
        vec![
            fixed(11.0, chip(app, tile.chip, tile.hue)),
            fixed(11.0, words(a.id.into(), quiet, true).into_element()),
            title,
            doing,
            fixed(
                9.0,
                words(a.feed.label().into(), quiet, false).into_element(),
            ),
            fixed(
                if terminal { 5.0 } else { 6.0 },
                words(a.since.into(), quiet, true).into_element(),
            ),
        ],
    )
}

/// The phone row (`.prow`): the state dot, the id, the bold title and since; under them what it
/// does (needs you in its colour) and live or snapshot.
fn phone_row(app: AppState, tile: &Tile) -> AnyElement {
    let action = tile.action;
    let a = tile.agent;
    let first = view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(
            view()
                .w(px(12.0))
                .flex_shrink_0()
                .items_center()
                .child(dot(app, tile.hue)),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_baseline()
                .gap(px(8.0))
                .child(
                    text(a.id)
                        .flex_shrink_0()
                        .number_of_lines(1u32)
                        .font_family(FONT_MONO)
                        .text_size(px(13.0))
                        .text_color(color(Token::Ink3)),
                )
                .child(
                    text(a.title)
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .font_family(FONT_TEXT)
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(Token::Ink)),
                ),
        )
        .child(
            text(a.since)
                .flex_shrink_0()
                .number_of_lines(1u32)
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(12.0))
                .text_color(color(Token::Ink3)),
        );
    let second = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .pl(px(22.0))
        .child(
            text(a.doing)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(13.5))
                .text_color(doing_ink(tile)),
        )
        .child(
            text(a.feed.label())
                .flex_shrink_0()
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .text_color(color(Token::Ink3)),
        );
    list_row(
        app,
        spoken(tile),
        test_id(tile),
        || false,
        move || run(app, action),
        vec![
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(first)
                .child(second)
                .into_element(),
        ],
    )
}
