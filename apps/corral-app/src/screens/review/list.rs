//! The candidate list: the desktop and terminal column beside the detail, and the served phone
//! destination whose rows push one candidate's page; the Review scope's project chips head both.

use nsh_core::prelude::*;

use crate::model::projects::FilterScreen;
use crate::model::review::{self, CandidateGroup, CandidateRow};
use crate::model::work::Record;
use crate::model::{Form, Id, flat};
use crate::screens::now::with_menu;
use crate::state::AppState;
use crate::ui::kit::marks::dot;
use crate::ui::kit::notice::empty;
use crate::ui::kit::page::pane;
use crate::ui::kit::project_filter::project_filter;
use crate::ui::kit::rows::{group_header, measured, project_chip};
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::{ANY_WIDTH, Ctx, act, page_view, rich_at, scope};

/// The served phone destination: the candidates, each opening its own page.
pub(super) fn phone_list(cx: Ctx) -> AnyElement {
    let app = cx.app;
    let list = computed(move || {
        let selection = app.selection();
        let filter = scope(app);
        app.feed
            .with(|feed| review::candidate_list_filtered(feed, &selection, &filter))
    });
    let rows = dynamic(move || {
        let (groups, empty_line) = list.get();
        view()
            .children(groups.iter().map(|group| candidate_group(&cx, group)))
            .children(empty_line.map(|line| empty(app, "Nothing to review", line, None)))
            .into_element()
    });
    let content = view()
        .child(project_filter(app, FilterScreen::Review))
        .child(rows)
        .into_element();
    pane(app, "review-list", content)
}

/// The candidate list (desktop and terminal) and the fixture's replay footnote.
pub(super) fn candidates(cx: Ctx) -> AnyElement {
    let app = cx.app;
    let terminal = app.form == Form::Terminal;
    let list = {
        let cx = cx.clone();
        computed(move || {
            let page = page_view(&cx, ANY_WIDTH);
            (page.candidates, page.candidate_note, page.empty)
        })
    };
    let rows = dynamic(move || {
        let (groups, note, empty_line) = list.get();
        let place = |content: AnyElement| {
            let place = view();
            let place = if terminal {
                place.px(term::cells(1.0))
            } else {
                place.px(px(16.0)).py(px(8.0))
            };
            place.child(content).into_element()
        };
        view()
            .when(!terminal, |list| list.pb(px(14.0)))
            .children(groups.iter().map(|group| candidate_group(&cx, group)))
            .children(note.map(|note| place(note_text(app, note))))
            .children(
                groups
                    .is_empty()
                    .then(|| empty_line.map(|line| place(dim(app, line).into_element())))
                    .flatten(),
            )
            .into_element()
    });
    view()
        .when(!terminal, |list| list.pt(px(6.0)))
        .child(project_filter(app, FilterScreen::Review))
        .child(rows)
        .into_element()
}

/// A note that wraps (`.illus` in a block): `kit::chip::illus` is one line.
fn note_text(app: AppState, note: String) -> AnyElement {
    let words = text(note).italic();
    match app.form {
        Form::Terminal => words.text_color(color(Token::Ink3)).into_element(),
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(10.5))
            .line_height(px(15.0))
            .letter_spacing(px(0.1))
            .text_color(color(Token::Ink4))
            .into_element(),
    }
}

fn candidate_group(cx: &Ctx, group: &CandidateGroup) -> AnyElement {
    view()
        .child(group_header(
            cx.app,
            group.label,
            Some(group.count),
            group.hue,
            None,
        ))
        .children(group.rows.iter().map(|row| candidate_row(cx, row)))
        .into_element()
}

/// The candidate's identity line, its quiet project chip after it while the list spans projects.
fn identity(app: AppState, row: &CandidateRow, size: f32) -> AnyElement {
    let words = rich_at(app, &row.identity, size, Token::Ink3, true).truncate();
    match row.project_chip.clone() {
        None => words.into_element(),
        Some(project) => {
            let room = signal(0.0_f32);
            let tag = project_chip(app, project, room, 0.0);
            measured(
                app,
                view()
                    .flex_row()
                    .items_center()
                    .min_w(px(0.0))
                    .gap(if app.form == Form::Terminal {
                        term::cells(1.0)
                    } else {
                        px(6.0)
                    })
                    // A terminal row measures this line, so it takes its width from its cell.
                    .when(app.form == Form::Terminal, |line| line.w_full()),
                room,
            )
            .child(words.flex_shrink(1.0).min_w(px(0.0)))
            .child(tag)
            .into_element()
        }
    }
}

/// A candidate: the actor dot, then the mono identity, the title and the state on three lines
/// (`.row` at `height: auto`). `kit::rows::list_row` is one fixed row high, so this keeps its
/// look (selected fill, the 1 pt ink bar, hover, focus ring) at the candidate's own height.
fn candidate_row(cx: &Ctx, row: &CandidateRow) -> AnyElement {
    let app = cx.app;
    let selected = row.selected;
    let press = act(cx, row.action.clone());
    let base = view()
        .relative()
        .flex_row()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label({
            let mut label = format!("{}, {}, {}", flat(&row.identity), row.title, row.state);
            if let Some(project) = &row.project_chip {
                label = format!("{label}, {project}");
            }
            label
        })
        .accessibility_state(move || AccessibilityState {
            selected,
            ..AccessibilityState::default()
        })
        .test_id(&format!("task-{}", row.task))
        .on_press(move || press());
    let shown = match app.form {
        Form::Terminal => base
            .items_start()
            .child(
                view()
                    .w(term::cells(1.0))
                    .flex_shrink_0()
                    .accessibility_elements_hidden(true)
                    .child(text(if selected { "▌" } else { " " }).text_color(color(Token::Ink))),
            )
            .child(term::space(1.0))
            .child(dot(app, row.hue))
            .child(term::space(1.0))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(identity(app, row, 0.0))
                    .child(
                        text(row.title.clone())
                            .truncate()
                            .text_color(color(Token::Ink)),
                    )
                    .child(
                        term::quiet(row.state.clone())
                            .flex_shrink(1.0)
                            .min_w(px(0.0))
                            .truncate(),
                    ),
            )
            .into_element(),
        Form::Desktop | Form::Phone => base
            .items_center()
            .gap(px(8.0))
            .pl(px(16.0))
            .pr(px(14.0))
            .py(px(8.0))
            .when(app.form == Form::Phone, |r| r.min_h(px(44.0)))
            .when(selected, |r| r.bg(color(Token::Sel)))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(2.0))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-2.0))
            })
            .when(selected, |r| {
                r.child(
                    view()
                        .absolute()
                        .left(px(0.0))
                        .top(px(4.0))
                        .bottom(px(4.0))
                        .w(px(1.0))
                        .bg(color(Token::Ink)),
                )
            })
            .child(
                view()
                    .w(px(14.0))
                    .flex_shrink_0()
                    .items_center()
                    .child(dot(app, row.hue)),
            )
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(1.0))
                    .child(identity(app, row, 11.5))
                    .child(
                        text(row.title.clone())
                            .truncate()
                            .font_family(FONT_TEXT)
                            .text_size(px(13.0))
                            .text_color(color(Token::Ink)),
                    )
                    .child(
                        text(row.state.clone())
                            .truncate()
                            .font_family(FONT_TEXT)
                            .text_size(px(12.0))
                            .text_color(color(Token::Ink3)),
                    ),
            )
            .into_element(),
    };
    // A candidate's row carries its commands: its context menu on desktop and terminal, the
    // action sheet a held finger opens on the phone.
    with_menu(app, Record::Review(Id::new(&row.task)), shown)
}
