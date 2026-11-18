//! The plan list (`.plist`): each section's head in its hue with its count, then its plans, one
//! row each in two terse lines: the id over the project, the title over the members' state (a
//! segment per member, landed of total, the state's icon) or the plan's status phrase. What a
//! line stands for is the row's hint; the phone, which has no hover, writes it out instead.

use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::plans::{
    self, ListNotice, MemberShare, OutlineRow, OutlineSecond, OutlineSection, PlanOutline,
};
use crate::model::text::span;
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::hint::hint_on;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::{empty, error, loading, notice};
use crate::ui::kit::page::group_head;
use crate::ui::kit::rows::{list_row, list_row_tall, titled};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::{press, verbs};

/// The most member glyphs a terminal row shows; a longer plan ends its strip in "+n", so the id
/// and title keep their room.
const TERM_SEGMENTS: usize = 8;

/// The list, the destination's keyboard list (`AppState::list_keys`): a loading, empty, error or
/// stale notice where corrald says so, then each section. `opens`: on a desktop too narrow for
/// the plan beside the list, a row press also opens the plan in the list's place.
pub(super) fn list(
    app: AppState,
    outline: Memo<PlanOutline>,
    opens: Option<Signal<bool>>,
) -> AnyElement {
    // The plan last scrolled into view: a refresh that keeps the selection leaves the scroll
    // where the reader put it.
    let revealed = stored(None::<Id>);
    let terminal = app.form == Form::Terminal;
    let rows = dynamic(move || {
        let content = outline.get();
        untracked(move || {
            let (lead, sections) = match &content.notice {
                Some(ListNotice::Loading) => (
                    Some(
                        view()
                            .px(px(if terminal { 0.0 } else { 16.0 }))
                            .py(px(8.0))
                            .child(loading(
                                app,
                                plans::LIST_LOADING,
                                &[(0.7, 12.0), (0.5, 10.0), (0.6, 12.0)],
                            ))
                            .into_element(),
                    ),
                    &[][..],
                ),
                Some(ListNotice::Empty { heading, body }) => {
                    (Some(empty(app, *heading, *body, None)), &[][..])
                }
                Some(ListNotice::Error(line)) => {
                    (Some(error(app, &vec![span(line.as_str())], None)), &[][..])
                }
                Some(ListNotice::Stale(line)) => (
                    Some(
                        view()
                            .px(px(if terminal { 0.0 } else { 12.0 }))
                            .py(px(6.0))
                            .child(notice(app, Hue::Held, &vec![span(line.as_str())], None))
                            .into_element(),
                    ),
                    &content.sections[..],
                ),
                None => (None, &content.sections[..]),
            };
            view()
                .children(lead)
                .children(
                    sections
                        .iter()
                        .map(|section| group(app, section, revealed, opens))
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    });
    view()
        .element_ref(&app.list_keys)
        .pb(px(if terminal { 0.0 } else { 14.0 }))
        .role(AccessibilityRole::List)
        .label(outline.with_untracked(|outline| outline.title))
        .child(rows)
        .into_element()
}

fn group(
    app: AppState,
    section: &OutlineSection,
    revealed: Stored<Option<Id>>,
    opens: Option<Signal<bool>>,
) -> AnyElement {
    view()
        .child(group_head(
            app,
            section.label,
            Some(section.count.to_string()),
            section.hue,
            None,
            None,
        ))
        .children(
            section
                .rows
                .iter()
                .map(|row| plan_row(app, row, revealed, opens))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// What a screen reader hears for a plan: its id, project and hint (the title and what the
/// second line says).
fn spoken(row: &OutlineRow) -> String {
    match &row.project_chip {
        Some(project) => format!("{}, {project}, {}", row.id_label, row.hint),
        None => format!("{}, {}", row.id_label, row.hint),
    }
}

fn plan_row(
    app: AppState,
    row: &OutlineRow,
    revealed: Stored<Option<Id>>,
    opens: Option<Signal<bool>>,
) -> AnyElement {
    let form = app.form;
    let action = row.action.clone();
    // The phone pushes the plan: its list keeps no selection.
    let selected = row.selected && form != Form::Phone;
    let on_press = move || {
        press(app, action.clone());
        if let Some(opens) = opens {
            opens.set(true);
        }
    };
    let drawn = match form {
        Form::Desktop => list_row_tall(
            app,
            spoken(row),
            row.test_id.clone(),
            move || selected,
            on_press,
            two_lines(app, row),
        ),
        Form::Phone => list_row(
            app,
            spoken(row),
            row.test_id.clone(),
            move || selected,
            on_press,
            two_lines(app, row),
        ),
        Form::Terminal => list_row(
            app,
            spoken(row),
            row.test_id.clone(),
            move || selected,
            on_press,
            terminal_cells(app, row),
        ),
    };
    // The row is the focus stop, so the row carries the hint: the focus landing on it shows it.
    let drawn = hint_on(app, view().child(drawn), row.hint.clone()).into_element();
    // A plan's row carries its commands in its context menu (RC-03).
    let drawn = verbs::row_menu(app, row.id.clone(), drawn);
    if selected {
        revealing(row.id.clone(), revealed, drawn)
    } else {
        drawn
    }
}

/// The selected row, scrolled into the list's view when the selection has just moved to it, so
/// a key press never leaves it off screen; a row already in view stays put. The plan counts as
/// revealed only once the scroll has run: a rebuild of the rows before then (the selected plan's
/// detail arriving, the feed's clock) drops the pending scroll with the row it was for, and the
/// rebuilt row asks again.
fn revealing(id: Id, revealed: Stored<Option<Id>>, row: AnyElement) -> AnyElement {
    let handle = element_ref();
    if revealed.with(|shown| shown.as_ref() != Some(&id)) {
        after_answer(move || {
            revealed.replace(Some(id));
            handle.reveal(RevealOptions::default().vertical(ScrollAlignment::Nearest));
        });
    }
    view().element_ref(&handle).child(row).into_element()
}

/// One line of quiet small text that gives way, cut at its end.
fn quiet_line(content: impl Into<SharedString>, size: f32, mono: bool) -> Text {
    text(content.into())
        .number_of_lines(1u32)
        .truncate()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .font_family(if mono { FONT_MONO } else { FONT_TEXT })
        .text_size(px(size))
        .line_height(px((size * 1.4).round()))
        .text_color(color(Token::Ink3))
}

/// The ink of a status phrase: its hue, or quiet.
fn status_ink(hue: Hue) -> AdaptiveColor {
    match hue {
        Hue::Neutral => color(Token::Ink3),
        hue => hue_ink(hue),
    }
}

/// The member bar's widest: a segment each (12 pt) up to six members, past that the members
/// (or past the model's limit each state's share) share this width.
const BAR: f32 = 84.0;
const SEGMENT: f32 = 12.0;
/// The project's widest at the head of the second line, so the members keep their room.
const PROJECT_MAX: f32 = 96.0;
/// The id column the title lines up after (`.pr` 60 pt; the phone's 64).
const ID_COLUMN: f32 = 60.0;
const PHONE_ID_COLUMN: f32 = 64.0;

/// The fill of a member's segment: its hue; one not yet moving rule-2.
fn segment_fill(hue: Hue) -> AdaptiveColor {
    match hue {
        Hue::Neutral => color(Token::Rule2),
        hue => hue_ink(hue),
    }
}

/// A bar piece 5 pt high (`.segs i`) that takes `grow` parts of the bar.
fn piece(hue: Hue, grow: f32) -> AnyElement {
    view()
        .flex_grow(grow)
        .flex_shrink(1.0)
        .min_w(px(2.0))
        .h(px(5.0))
        .rounded(px(1.0))
        .bg(segment_fill(hue))
        .into_element()
}

/// The members' bar: a segment per member, 12 pt each up to six and sharing `BAR` past that;
/// or, where the model gives each state's share (a long plan), those shares of `BAR`. Hidden
/// from screen readers: the row says it.
fn member_bar(segments: &[(Mark, Hue)], shares: Option<&[MemberShare]>) -> AnyElement {
    let pieces: Vec<AnyElement> = match shares {
        Some(shares) => shares
            .iter()
            .map(|share| piece(share.hue, share.count as f32))
            .collect(),
        None => segments.iter().map(|(_, hue)| piece(*hue, 1.0)).collect(),
    };
    let count = pieces.len() as f32;
    let natural = count * SEGMENT + (count - 1.0).max(0.0) * 2.0;
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .w(px(if shares.is_some() {
            BAR
        } else {
            natural.min(BAR)
        }))
        .gap(px(2.0))
        .accessibility_elements_hidden(true)
        .children(pieces)
        .into_element()
}

/// The second line on desktop and phone: the member bar, the landed figures and the state's
/// icon (the phone adds the state's sentence), or the status phrase.
fn second_line(app: AppState, second: &OutlineSecond, size: f32) -> AnyElement {
    let line = view()
        .flex_row()
        .items_center()
        .flex_1()
        .gap(px(8.0))
        .min_w(px(0.0));
    match second {
        OutlineSecond::Members {
            segments,
            shares,
            progress,
            state,
        } => line
            .when(!segments.is_empty(), |line| {
                line.child(member_bar(segments, shares.as_deref()))
            })
            .child(quiet_line(progress.clone(), size, true).flex_shrink_0())
            .child(
                view()
                    .flex_shrink_0()
                    .accessibility_elements_hidden(true)
                    .child(icon(Icon::stat(state.icon), size, status_ink(state.hue))),
            )
            .when(app.form == Form::Phone, |line| {
                line.child(quiet_line(state.hint.clone(), size, false))
            })
            .into_element(),
        OutlineSecond::Status {
            text: words, hue, ..
        } => line
            .child(quiet_line(words.clone(), size, false).text_color(status_ink(*hue)))
            .into_element(),
    }
}

/// Desktop and phone (`.pr`): the id in its column and the title on the first line; the
/// project at its own width (at most `PROJECT_MAX`, every project in its hint when the chip
/// names the first) then the second line, or the second line under the title without one.
fn two_lines(app: AppState, row: &OutlineRow) -> Vec<AnyElement> {
    let phone = app.form == Form::Phone;
    let (id_column, id_size, title_size, small, line_height) = if phone {
        (PHONE_ID_COLUMN, 13.0, 15.5, 12.5, 21.0)
    } else {
        (ID_COLUMN, 12.0, 13.0, 12.0, 18.0)
    };
    let first =
        view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .min_w(px(0.0))
            .child(view().w(px(id_column)).flex_shrink_0().child(
                quiet_line(row.id_label.clone(), id_size, true).line_height(px(line_height)),
            ))
            .child(
                text(row.title.clone())
                    .flex_1()
                    .number_of_lines(1u32)
                    .truncate()
                    .min_w(px(0.0))
                    .font_family(FONT_TEXT)
                    .text_size(px(title_size))
                    .line_height(px(line_height))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color(Token::Ink)),
            );
    let lead = match &row.project_chip {
        Some(project) => view()
            .flex_shrink_0()
            .min_w(px(id_column))
            .max_w(px(PROJECT_MAX))
            .child(quiet_line(project.clone(), small, false))
            .into_element(),
        None => view().w(px(id_column)).flex_shrink_0().into_element(),
    };
    let second = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .min_w(px(0.0))
        .child(lead)
        .child(second_line(app, &row.second, small));
    vec![
        view()
            .flex_1()
            .min_w(px(0.0))
            .gap(px(if phone { 3.0 } else { 2.0 }))
            .child(first)
            .child(second)
            .into_element(),
    ]
}

/// One line of Cells: the id, the title with its project chip where it fits, then the member
/// glyphs, the landed figures and the state's glyph, or the status phrase. The sentences are
/// the status line's (the row's hint).
fn terminal_cells(app: AppState, row: &OutlineRow) -> Vec<AnyElement> {
    let title = text(row.title.clone())
        .number_of_lines(1u32)
        .truncate()
        .text_color(color(Token::Ink));
    let mut cells = vec![
        view()
            .min_w(term::cells(7.0))
            .flex_shrink_0()
            .child(
                text(row.id_label.clone())
                    .number_of_lines(1u32)
                    .text_color(color(Token::Ink3)),
            )
            .into_element(),
        titled(app, title, row.project_chip.clone()),
    ];
    match &row.second {
        // The terminal folds a long plan into its first glyphs and a count, so it draws no
        // shares.
        OutlineSecond::Members {
            segments,
            progress,
            state,
            ..
        } => {
            let drawn = if segments.len() > TERM_SEGMENTS {
                TERM_SEGMENTS - 2
            } else {
                segments.len()
            };
            let mut line = text("")
                .flex_shrink_0()
                .number_of_lines(1u32)
                .accessibility_elements_hidden(true);
            // Each member as its mark's shape, so the strip reads without colour.
            for (mark, hue) in segments.iter().take(drawn) {
                line = line.child(text(mark.glyph().to_string()).text_color(status_ink(*hue)));
            }
            if drawn < segments.len() {
                line = line.child(
                    text(format!("+{}", segments.len() - drawn)).text_color(color(Token::Ink3)),
                );
            }
            cells.push(line.into_element());
            cells.push(
                text("")
                    .flex_shrink_0()
                    .number_of_lines(1u32)
                    .child(text(progress.clone()).text_color(color(Token::Ink3)))
                    .child(" ")
                    .child(
                        text(state.icon.glyph().to_string()).text_color(match state.hue {
                            Hue::Neutral => color(Token::Ink),
                            hue => hue_ink(hue),
                        }),
                    )
                    .into_element(),
            );
        }
        OutlineSecond::Status {
            text: words, hue, ..
        } => cells.push(
            text(words.clone())
                .flex_shrink_0()
                .number_of_lines(1u32)
                .text_color(status_ink(*hue))
                .into_element(),
        ),
    }
    cells
}
