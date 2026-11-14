//! The candidate list: the desktop and terminal column beside the detail, and the served phone
//! destination whose rows push one candidate's page.

use nsh_core::prelude::*;

use crate::model::review::{self, CandidateGroup, CandidateRow};
use crate::model::{Form, flat};
use crate::state::AppState;
use crate::ui::kit::marks::dot;
use crate::ui::kit::notice::empty;
use crate::ui::kit::page::pane;
use crate::ui::kit::rows::group_header;
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::{ANY_WIDTH, Ctx, act, page_view, rich_at};

/// The served phone destination: the candidates, each opening its own page.
pub(super) fn phone_list(cx: Ctx) -> AnyElement {
    let app = cx.app;
    let list = computed(move || {
        let selection = app.selection();
        app.feed
            .with(|feed| review::candidate_list(feed, &selection))
    });
    let content = dynamic(move || {
        let (groups, empty_line) = list.get();
        view()
            .children(groups.iter().map(|group| candidate_group(&cx, group)))
            .children(empty_line.map(|line| empty(app, "Nothing to review", line, None)))
            .into_element()
    });
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
    dynamic(move || {
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
            .when(!terminal, |list| list.pt(px(6.0)).pb(px(14.0)))
            .children(groups.iter().map(|group| candidate_group(&cx, group)))
            .children(note.map(|note| place(note_text(app, note))))
            .children(
                groups
                    .is_empty()
                    .then(|| empty_line.map(|line| place(dim(app, line).into_element())))
                    .flatten(),
            )
            .into_element()
    })
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
        .label(format!(
            "{}, {}, {}",
            flat(&row.identity),
            row.title,
            row.state
        ))
        .accessibility_state(move || AccessibilityState {
            selected,
            ..AccessibilityState::default()
        })
        .test_id(&format!("task-{}", row.task))
        .on_press(move || press());
    match app.form {
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
                    .child(rich_at(app, &row.identity, 0.0, Token::Ink3, true).truncate())
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
                    .child(rich_at(app, &row.identity, 11.5, Token::Ink3, true).truncate())
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
    }
}
