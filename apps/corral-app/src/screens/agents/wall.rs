//! The agents wall (`.wall`, `.tile`, `.machine` on desktop; `.ph-sec` and `.prow` on the phone;
//! one line per agent in a terminal), between the assistant's quick thread and its blocks.

use nsh_core::prelude::*;

use crate::model::agents::{Action, Fill, Mode, Note, Tile, Wall};
use crate::model::assistant::QUICK;
use crate::model::hue::Hue;
use crate::model::text::span;
use crate::model::{Form, Id};
use crate::screens::assistant;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with, seg};
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::{dot, hue_ink};
use crate::ui::kit::notice::{empty, error, notice, skeleton};
use crate::ui::kit::page::{body, group_head, head, pane, search};
use crate::ui::kit::rows::list_row;
use crate::ui::kit::text::dim;
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

/// The whole page for the form. With the daemon, the page (not the content it rebuilds on every
/// agent update) subscribes the quick thread its tile quotes and settles the assistant blocks'
/// session operations, so neither restarts while the page stays open. Other quick-thread
/// views (the Ask drawer, the thread page) hold their own reference; the subscription closes
/// only with the last of them.
pub fn screen(app: AppState) -> AnyElement {
    if untracked(|| app.daemon()) {
        assistant::open_scope(app, Id::from(QUICK), false);
        assistant::watch_ops(app);
    }
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

/// The footer note: the fixture's illustrative tag, or a quiet sentence about the data.
fn note(app: AppState, note: &Note) -> AnyElement {
    match note {
        Note::Illustrative(words) => illus(app, *words),
        Note::Plain(words) => dim(app, words.clone()).into_element(),
    }
}

/// What stands in for the agents while there are none to show: the skeleton while the first
/// rows load, the failure, or the empty state.
fn fill(app: AppState, fill: &Fill) -> Option<AnyElement> {
    match fill {
        Fill::Ready => None,
        Fill::Loading => Some(
            view()
                .when(app.form != Form::Terminal, |pad| {
                    pad.px(px(18.0)).py(px(8.0))
                })
                .child(skeleton(app, &[(0.4, 10.0), (0.7, 10.0), (0.55, 10.0)]))
                .into_element(),
        ),
        Fill::Failed(words) => Some(error(app, &vec![span(words.clone())], None)),
        Fill::Empty { heading, paragraph } => Some(empty(app, *heading, paragraph.clone(), None)),
    }
}

/// The notice over agents shown from the last update while corrald is not answering.
fn stale(app: AppState, wall: &Wall) -> Option<AnyElement> {
    let words = wall.stale.clone()?;
    let notice = notice(app, Hue::Held, &vec![span(words)], None);
    Some(if app.form == Form::Terminal {
        notice
    } else {
        view()
            .px(px(if app.form == Form::Phone { 16.0 } else { 18.0 }))
            .pt(px(12.0))
            .child(notice)
            .into_element()
    })
}

/// Launch PM while no PM is live: why, the button and, once pressed, the launch's outcome in its
/// tone, with "Check result" when it is unknown.
fn launch(app: AppState, wall: &Wall) -> Option<AnyElement> {
    let launch = wall.launch.as_ref()?;
    let action = launch.action.clone();
    let press = button_with(
        app,
        launch.label,
        ButtonKind::Primary,
        ButtonOptions {
            disabled: !launch.enabled,
            ..ButtonOptions::default()
        },
        "btn-launch-pm",
        move || run(app, action.clone()),
    );
    let recover = launch.recover.clone().map(|action| {
        button(
            app,
            "Check result",
            ButtonKind::Quiet,
            "btn-recover-launch",
            move || run(app, action.clone()),
        )
    });
    let mut words = vec![span(launch.words.clone())];
    if let Some((hue, status)) = &launch.status {
        words.push(span(format!(" {status}.")).hue(*hue));
    }
    let actions = view()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .gap(px(8.0))
        .children(recover)
        .child(press)
        .into_element();
    let notice = notice(app, Hue::Held, &words, Some(actions));
    Some(if app.form == Form::Terminal {
        notice
    } else {
        view()
            .px(px(if app.form == Form::Phone { 16.0 } else { 18.0 }))
            .pt(px(12.0))
            .child(notice)
            .into_element()
    })
}

/// What scrolls under the head: the blocks and the agents in the mockup's order.
fn content(app: AppState, wall: &Wall) -> AnyElement {
    let first = untracked(move || assistant::agents_head(app));
    let last = untracked(move || assistant::agents_tail(app));
    let stale = stale(app, wall);
    let launch = launch(app, wall);
    let fill = fill(app, &wall.fill);
    match app.form {
        Form::Desktop => view()
            .child(machine_line(app, wall))
            .children(stale)
            .children(launch)
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
            .children(fill)
            .child(last)
            .child(
                view()
                    .px(px(18.0))
                    .pb(px(16.0))
                    .child(note(app, &wall.footer)),
            )
            .into_element(),
        Form::Phone => view()
            .child(first)
            .children(stale)
            .children(launch)
            .child(group_head(
                app,
                wall.machine.clone(),
                Some(wall.count.to_string()),
                Hue::Neutral,
                Some(wall.phone_machine_detail.clone()),
                wall.phone_machine_note.map(|words| illus(app, words)),
            ))
            .children(fill)
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
                        text(wall.machine.clone())
                            .flex_shrink_0()
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::Ink)),
                    )
                    .child(term::space(2.0))
                    .child(
                        term::quiet(wall.machine_detail.clone())
                            .flex_shrink(1.0)
                            .min_w(px(0.0)),
                    )
                    .children(wall.machine_note.map(|words| {
                        view()
                            .flex_row()
                            .child(term::space(2.0))
                            .child(illus(app, words))
                    })),
            )
            .child(term::blank())
            .children(stale)
            .children(launch)
            .child(first)
            .child(term::hrule())
            .children(fill)
            .children(wall.tiles.iter().map(|tile| list_line(app, tile)))
            .child(term::hrule())
            .child(last)
            .child(term::blank())
            .child(note(app, &wall.footer))
            .into_element(),
    }
}

/// `.machine`: the terminal icon, the machine in bold, what runs there, the illustrative tag in
/// the preview, and at the trailing edge where other machines will come from.
fn machine_line(app: AppState, wall: &Wall) -> AnyElement {
    let quiet = |content: String| {
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
            text(wall.machine.clone())
                .flex_shrink_0()
                .font_family(FONT_TEXT)
                .text_size(px(12.5))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .child(quiet(wall.machine_detail.clone()))
        .children(wall.machine_note.map(|words| illus(app, words)))
        .child(view().flex_1().min_w(px(GAP)))
        .child(quiet(wall.other_machines.to_owned()))
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
    [
        tile.chip,
        tile.label.as_str(),
        tile.title.as_str(),
        tile.doing.as_str(),
        tile.feed_short.as_str(),
        tile.since.as_str(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(", ")
}

fn test_id(tile: &Tile) -> String {
    format!("agent-{}", tile.id)
}

/// The colour of what the agent is doing: needs you in its colour, else the secondary ink.
fn doing_ink(tile: &Tile) -> AdaptiveColor {
    match tile.doing_hue {
        Hue::Neutral => color(Token::Ink2),
        hue => hue_ink(hue),
    }
}

/// One desktop tile (`.tile`): chip, id and role; the title; what it does now; the tail in the
/// terminal look under its live or snapshot tag; profile, runtime, pane, usage and since.
/// Pressing it opens the agent's session.
fn tile(app: AppState, tile: &Tile) -> AnyElement {
    let action = tile.action.clone();
    let small = |content: String, ink: Token| {
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
        .on_press(move || run(app, action.clone()))
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
                    text(tile.label.clone())
                        .number_of_lines(1u32)
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .font_family(FONT_MONO)
                        .text_size(px(11.5))
                        .text_color(color(Token::Ink3)),
                )
                .child(view().flex_1().min_w(px(0.0)))
                .child(small(tile.role.clone(), Token::Ink3).text_size(px(12.0))),
        )
        .child(
            text(tile.title.clone())
                .truncate()
                .px(px(12.0))
                .font_family(FONT_TEXT)
                .text_size(px(13.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink)),
        )
        .child(
            text(tile.doing.clone())
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
                .child(console::tag(tile.feed, tile.feed_label.clone()))
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
                .child(small(tile.profile.clone(), Token::Ink3))
                .child(
                    small(tile.runtime.clone(), Token::Ink3)
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .truncate(),
                )
                .child(small(tile.pane.clone(), Token::Ink3).font_family(FONT_MONO))
                .children(tile.usage.clone().map(|usage| {
                    small(usage, Token::Ink3)
                        .font_family(FONT_MONO)
                        .tabular_nums()
                }))
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
    let action = tile.action.clone();
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
        .child(words(tile.doing.clone().into(), doing_ink(tile), false))
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
            words(tile.title.clone().into(), color(Token::Ink), false)
                .when(!terminal, |t| t.font_weight(FontWeight::SEMIBOLD)),
        )
        .into_element();
    list_row(
        app,
        spoken(tile),
        test_id(tile),
        || false,
        move || run(app, action.clone()),
        vec![
            fixed(11.0, chip(app, tile.chip, tile.hue)),
            fixed(
                11.0,
                words(tile.label.clone().into(), quiet, true).into_element(),
            ),
            title,
            doing,
            fixed(
                9.0,
                words(tile.feed_short.clone().into(), quiet, false).into_element(),
            ),
            fixed(
                if terminal { 5.0 } else { 6.0 },
                words(tile.since_short.clone().into(), quiet, true).into_element(),
            ),
        ],
    )
}

/// The phone row (`.prow`): the state dot, the id, the bold title and since; under them what it
/// does (needs you in its colour) and the tail's freshness.
fn phone_row(app: AppState, tile: &Tile) -> AnyElement {
    let action = tile.action.clone();
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
                    text(tile.label.clone())
                        .flex_shrink_0()
                        .number_of_lines(1u32)
                        .font_family(FONT_MONO)
                        .text_size(px(13.0))
                        .text_color(color(Token::Ink3)),
                )
                .child(
                    text(tile.title.clone())
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
            text(tile.since_short.clone())
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
            text(tile.doing.clone())
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(13.5))
                .text_color(doing_ink(tile)),
        )
        .child(
            text(tile.feed_short.clone())
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
        move || run(app, action.clone()),
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
