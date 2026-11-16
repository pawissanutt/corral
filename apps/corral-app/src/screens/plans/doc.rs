//! A plan's document at reading measure (`.doc`): each section's caption over its paragraph,
//! list, proposed members, verdicts or owner decisions. And the side column's parts: its title
//! (`.phd`), its blocks (`.sec`), and beside a revision under review what accepting publishes
//! and what the reviewers said.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::plans::{
    AwaitingBrief, DocumentSection, ProposedRow, Publishes, ReviewRow, ReviewerBrief, SectionBody,
};
use crate::state::AppState;
use crate::ui::kit::hint::{hint_on, hinted};
use crate::ui::kit::marks::mark;
use crate::ui::kit::text::{rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::decide;

/// The reading measure of a document (`.doc` 76ch at 13.5 pt).
const MEASURE: f32 = 620.0;

fn element_view() -> View {
    view().min_w(px(0.0))
}

/// Text at `size` in `ink`, wrapping; a terminal keeps its one size.
fn words(app: AppState, content: impl Into<SharedString>, size: f32, ink: Token) -> Text {
    let line = text(content.into()).min_w(px(0.0)).text_color(color(ink));
    if app.form == Form::Terminal {
        return line;
    }
    line.font_family(FONT_TEXT)
        .text_size(px(size))
        .line_height(px((size * 1.45).round()))
}

/// The document's room (`.doc`): 6 20 24 on desktop, 4 18 18 on the phone, a Cell each side in
/// a terminal.
pub(super) fn doc_inset(app: AppState) -> View {
    match app.form {
        Form::Desktop => element_view().px(px(20.0)).pt(px(6.0)).pb(px(24.0)),
        Form::Phone => element_view().px(px(18.0)).pt(px(4.0)).pb(px(18.0)),
        Form::Terminal => element_view().px(term::cells(1.0)),
    }
}

/// The sections of a document.
pub(super) fn sections(app: AppState, sections: &[DocumentSection]) -> AnyElement {
    element_view()
        .when(app.form == Form::Desktop, |doc| doc.max_w(px(MEASURE)))
        .children(
            sections
                .iter()
                .map(|section| doc_section(app, section))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// A section caption (`.doc h3`): 12 pt semibold in ink-3, 18 pt above and 6 below; a terminal
/// leaves a blank row and writes it bold.
fn doc_heading(app: AppState, heading: &'static str) -> AnyElement {
    let caption = text(heading)
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32);
    if app.form == Form::Terminal {
        return element_view()
            .child(term::blank())
            .child(
                caption
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink)),
            )
            .into_element();
    }
    caption
        .mt(px(18.0))
        .mb(px(6.0))
        .font_family(FONT_TEXT)
        .text_size(px(if app.form == Form::Phone { 13.0 } else { 12.0 }))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink3))
        .into_element()
}

fn doc_section(app: AppState, section: &DocumentSection) -> AnyElement {
    let gap = |points: f32| {
        px(if app.form == Form::Terminal {
            0.0
        } else {
            points
        })
    };
    let body = match &section.body {
        SectionBody::Paragraph(words) => rich(app, words),
        SectionBody::Bullets(items) => bullets(app, items),
        SectionBody::Proposed(rows) => element_view()
            .gap(gap(4.0))
            .children(
                rows.iter()
                    .map(|row| proposed(app, row))
                    .collect::<Vec<_>>(),
            )
            .into_element(),
        SectionBody::Reviews(rows) => element_view()
            .gap(gap(6.0))
            .children(rows.iter().map(|row| review(app, row)).collect::<Vec<_>>())
            .into_element(),
        SectionBody::Decisions(rows) => element_view()
            .gap(gap(10.0))
            .children(
                rows.iter()
                    .map(|row| decide::decision(app, row))
                    .collect::<Vec<_>>(),
            )
            .into_element(),
    };
    element_view()
        .child(doc_heading(app, section.heading))
        .child(body)
        .into_element()
}

/// A plain bulleted list (`.doc ul`): items 5 pt apart, 14 pt in.
fn bullets(app: AppState, items: &[String]) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let size = text_size(app.form);
    element_view()
        .role(AccessibilityRole::List)
        .gap(px(if terminal { 0.0 } else { 5.0 }))
        .children(
            items
                .iter()
                .map(|item| {
                    element_view()
                        .flex_row()
                        .items_start()
                        .child(
                            element_view()
                                .w(if terminal { term::cells(2.0) } else { px(14.0) })
                                .flex_shrink_0()
                                .child(words(app, "•", size, Token::Ink4)),
                        )
                        .child(words(app, item.clone(), size, Token::Ink).flex_1())
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// A proposed member: its untouched mark, the name in mono, "can start now" or "after …" at
/// the trailing edge.
fn proposed(app: AppState, row: &ProposedRow) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let name = text(row.name.clone())
        .flex_1()
        .min_w(px(0.0))
        .truncate()
        .text_color(color(Token::Ink));
    let sub = text(row.sub.clone())
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let (name, sub) = if terminal {
        (name, sub)
    } else {
        (
            name.font_family(FONT_MONO).text_size(px(12.0)),
            sub.font_family(FONT_TEXT).text_size(px(12.0)),
        )
    };
    element_view()
        .flex_row()
        .items_center()
        .gap(if terminal { term::cells(1.0) } else { px(8.0) })
        .child(mark(app, row.mark, Hue::Neutral))
        .child(name)
        .child(sub)
        .into_element()
}

/// A verdict line of a document outside review: revision, mark, verdict, then who and note; a
/// superseded revision at 75 % opacity.
fn review(app: AppState, row: &ReviewRow) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let rev = text(row.rev.clone())
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(color(Token::Ink));
    let rev = if terminal {
        rev
    } else {
        rev.font_family(FONT_MONO).text_size(px(11.5))
    };
    element_view()
        .flex_row()
        .items_start()
        .gap(if terminal { term::cells(1.0) } else { px(8.0) })
        .when(row.superseded && !terminal, |v| v.opacity(0.75))
        .child(rev)
        .child(mark(app, row.mark, Hue::Neutral))
        .child(
            element_view()
                .flex_1()
                .child(rich_small(app, &row.verdict))
                .child(rich_small(app, &row.note)),
        )
        .into_element()
}

/// A side column's title (`.phd h2`): 16 pt semibold over a rule, padding 14 18 12, its hint on
/// the title. Phone 17 pt, padding 16 18 10. Terminal: bold, a joined rule under it.
pub(super) fn side_title(
    app: AppState,
    title: impl Into<SharedString>,
    hint: &'static str,
) -> AnyElement {
    let form = app.form;
    let words = text(title.into())
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink));
    match form {
        Form::Terminal => element_view()
            .child(element_view().flex_row().px(term::cells(1.0)).child(hinted(
                app,
                words.font_weight(FontWeight::BOLD),
                hint,
            )))
            .child(term::hrule())
            .into_element(),
        Form::Desktop | Form::Phone => {
            let phone = form == Form::Phone;
            element_view()
                .flex_row()
                .px(px(18.0))
                .pt(px(if phone { 16.0 } else { 14.0 }))
                .pb(px(if phone { 10.0 } else { 12.0 }))
                .border_b(px(1.0))
                .border_color(color(Token::Rule))
                .child(hinted(
                    app,
                    words
                        .font_family(FONT_TEXT)
                        .text_size(px(if phone { 17.0 } else { 16.0 })),
                    hint,
                ))
                .into_element()
        }
    }
}

/// A side block (`.sec`): padding 14 18 with a rule under it; a terminal a Cell in, a blank row
/// under it.
pub(super) fn side_block(app: AppState) -> View {
    match app.form {
        Form::Terminal => element_view().px(term::cells(1.0)).pb(term::rows(1.0)),
        Form::Desktop | Form::Phone => element_view()
            .px(px(18.0))
            .py(px(14.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule)),
    }
}

/// A side block's caption (`.sec h3`): 12 pt semibold in ink-3, 8 pt above its content, its
/// hint on the caption; a terminal bold.
fn side_caption(app: AppState, label: &'static str, hint: &'static str) -> AnyElement {
    let caption = text(label)
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32)
        .font_weight(FontWeight::SEMIBOLD);
    let caption = match app.form {
        Form::Terminal => caption
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::Ink)),
        form => caption
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 13.0 } else { 12.0 }))
            .text_color(color(Token::Ink3)),
    };
    element_view()
        .flex_row()
        .when(app.form != Form::Terminal, |v| v.mb(px(8.0)))
        .child(hinted(app, caption, hint))
        .into_element()
}

/// Beside a revision under review: what accepting publishes, then what the reviewers said.
pub(super) fn awaiting_side(app: AppState, brief: &AwaitingBrief) -> AnyElement {
    element_view()
        .test_id("plan-brief")
        .children(
            brief
                .publishes
                .as_ref()
                .map(|publishes| published(app, publishes)),
        )
        .when(!brief.reviewers.is_empty(), |side| {
            side.child(
                side_block(app)
                    .child(side_caption(
                        app,
                        brief.reviewers_heading,
                        brief.reviewers_hint,
                    ))
                    .child(
                        element_view().role(AccessibilityRole::List).children(
                            brief
                                .reviewers
                                .iter()
                                .enumerate()
                                .map(|(at, reviewer)| verdict(app, reviewer, at > 0))
                                .collect::<Vec<_>>(),
                        ),
                    ),
            )
        })
        .into_element()
}

/// What accepting publishes (`.chainl`): the count sentence as the title, then each proposed
/// member's mark and name in mono, its dependencies in its hint (the phone's next line).
fn published(app: AppState, publishes: &Publishes) -> AnyElement {
    let form = app.form;
    let list = element_view()
        .role(AccessibilityRole::List)
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .children(
            publishes
                .members
                .iter()
                .map(|member| {
                    let name = text(member.name.clone())
                        .min_w(px(0.0))
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(Token::Ink));
                    let name = if form == Form::Terminal {
                        name.into_element()
                    } else {
                        name.font_family(FONT_MONO)
                            .text_size(px(if form == Form::Phone { 13.5 } else { 12.5 }))
                            .into_element()
                    };
                    let line = element_view()
                        .flex_row()
                        .items_center()
                        .gap(if form == Form::Terminal {
                            term::cells(1.0)
                        } else {
                            px(8.0)
                        })
                        .accessible(true)
                        .label(member.name.clone())
                        .child(mark(app, Mark::Next, Hue::Neutral))
                        .child(name);
                    match form {
                        Form::Phone => element_view()
                            .child(line)
                            .child(element_view().pl(px(21.0)).child(words(
                                app,
                                member.hint.clone(),
                                13.0,
                                Token::Ink3,
                            )))
                            .into_element(),
                        Form::Desktop | Form::Terminal => {
                            hint_on(app, line.focusable(true), member.hint.clone()).into_element()
                        }
                    }
                })
                .collect::<Vec<_>>(),
        );
    element_view()
        .test_id("plan-publishes")
        .child(side_title(app, publishes.sentence.clone(), publishes.hint))
        .child(side_block(app).child(list))
        .into_element()
}

/// One verdict (`.vd li`): its mark and `P2 · approve` in semibold, reviewer and time quiet at
/// the trailing edge, the reviewer's note under it; a superseded one dimmed, its note left to
/// its hint. Verdicts after the first are ruled off above.
fn verdict(app: AppState, reviewer: &ReviewerBrief, ruled: bool) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let size = if form == Form::Phone { 13.5 } else { 13.0 };
    let head = element_view()
        .flex_row()
        .items_center()
        .gap(if terminal { term::cells(1.0) } else { px(6.0) })
        .accessible(true)
        .label(reviewer.hint.clone())
        .child(mark(
            app,
            reviewer.mark,
            if reviewer.old {
                Hue::Held
            } else {
                Hue::Neutral
            },
        ))
        .child(
            words(app, reviewer.label.clone(), size, Token::Ink)
                .number_of_lines(1u32)
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(element_view().flex_1())
        .children(
            reviewer
                .byline
                .clone()
                .map(|byline| words(app, byline, 12.0, Token::Ink3).number_of_lines(1u32)),
        );
    let head = if form == Form::Phone {
        head.into_element()
    } else {
        hint_on(app, head.focusable(true), reviewer.hint.clone()).into_element()
    };
    element_view()
        .when(!terminal, |v| v.py(px(8.0)))
        .when(ruled && !terminal, |v| {
            v.border_t(px(1.0)).border_color(color(Token::Rule))
        })
        .when(reviewer.old && !terminal, |v| v.opacity(0.75))
        .child(head)
        .children(reviewer.note.clone().filter(|_| !reviewer.old).map(|note| {
            element_view()
                .when(!terminal, |v| v.mt(px(3.0)))
                .child(words(app, note, 12.5, Token::Ink2))
        }))
        .into_element()
}
