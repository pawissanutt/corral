//! The candidate outline (`.plist`): each group's head in its hue with its count, then one row
//! per candidate, the destination's keyboard list (`AppState::list_keys`). A row is the id and
//! the title over the project and the short status in its hue; its hint is the rail as a
//! sentence. Desktop and phone draw the mock's two lines; a terminal one line of Cells.

use nsh_core::prelude::*;

use crate::model::review::{OutlineGroup, OutlineRow};
use crate::model::work::Record;
use crate::model::{Form, Hue, Id};
use crate::screens::now::with_menu;
use crate::state::AppState;
use crate::ui::kit::hint::hint_on;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::rows::{group_header, measured};
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::{Ctx, act};

/// The id column (`.pr { grid-template-columns: 60px … }`).
const ID_WIDTH: f32 = 60.0;
/// A terminal row's id column: `T-1234` and a Cell.
const TERM_ID: f32 = 7.0;
/// The least of a terminal row's title kept before its status is cut further.
const TERM_TITLE: f32 = 8.0;
/// The least touch target on a phone.
const TARGET: f32 = 44.0;
const RING: f32 = 2.0;

/// The outline, rebuilt when its rows change. `opened` is the narrow desktop's: picking a row
/// there also opens the candidate in the outline's place.
pub(super) fn outline(cx: &Ctx, opened: Option<Signal<bool>>) -> AnyElement {
    let app = cx.app;
    let page = cx.page;
    let groups = computed(move || page.with(|page| page.outline.clone()));
    let empty = computed(move || page.with(|page| page.empty.clone()));
    let title = page.with_untracked(|page| page.title);
    // The candidate last scrolled into view: a refresh that keeps the selection leaves the
    // scroll where the reader put it.
    let revealed = stored(None::<Id>);
    // A terminal row's width, which decides what of its status and project it keeps.
    let room = signal(0.0_f32);
    let rows = {
        let cx = cx.clone();
        dynamic(move || {
            let groups = groups.get();
            if groups.is_empty() {
                return empty.get().map(|line| empty_line(app, line));
            }
            Some(
                view()
                    .children(
                        groups
                            .iter()
                            .map(|group| outline_group(&cx, group, opened, revealed, room))
                            .collect::<Vec<_>>(),
                    )
                    .into_element(),
            )
        })
    };
    measured(app, view(), room)
        .element_ref(&app.list_keys)
        .role(AccessibilityRole::List)
        .label(title)
        .when(app.form != Form::Terminal, |list| list.pb(px(14.0)))
        .child(rows)
        .into_element()
}

/// Why no candidate is listed, as the model says it.
pub(super) fn empty_line(app: AppState, line: String) -> AnyElement {
    let place = view();
    let place = match app.form {
        Form::Terminal => place.px(term::cells(1.0)).pt(term::rows(1.0)),
        Form::Desktop => place.px(px(20.0)).py(px(16.0)),
        Form::Phone => place.px(px(18.0)).py(px(16.0)),
    };
    place.child(dim(app, line)).into_element()
}

fn outline_group(
    cx: &Ctx,
    group: &OutlineGroup,
    opened: Option<Signal<bool>>,
    revealed: Stored<Option<Id>>,
    room: Signal<f32>,
) -> AnyElement {
    view()
        .child(group_header(
            cx.app,
            group.label,
            Some(group.count),
            group.hue,
            None,
        ))
        .children(
            group
                .rows
                .iter()
                .map(|row| outline_row(cx, row, opened, revealed, room))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// What a screen reader hears for a row: id, title, status, project.
fn row_label(row: &OutlineRow) -> String {
    let mut parts = vec![format!("{} {}", row.id, row.title), row.status.clone()];
    parts.extend(row.project_chip.clone());
    parts.join(", ")
}

/// A candidate's row with the commands of its context menu (`.`, a held finger on the phone),
/// scrolled into view when the keyboard has just selected it.
fn outline_row(
    cx: &Ctx,
    row: &OutlineRow,
    opened: Option<Signal<bool>>,
    revealed: Stored<Option<Id>>,
    room: Signal<f32>,
) -> AnyElement {
    let app = cx.app;
    let selected = row.selected;
    let press = act(cx, row.action.clone());
    let base = view()
        .relative()
        .flex_row()
        .flex_shrink_0()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(row_label(row))
        .accessibility_state(move || AccessibilityState {
            selected,
            ..AccessibilityState::default()
        })
        .test_id(&row.test_id)
        .on_press(move || {
            press();
            if let Some(opened) = opened {
                opened.set(true);
            }
        });
    let drawn = match app.form {
        Form::Terminal => terminal_row(row, base, room),
        Form::Desktop | Form::Phone => two_lines(app, row, base),
    };
    let drawn = hint_on(app, drawn, row.hint.clone()).into_element();
    let drawn = with_menu(app, Record::Review(Id::new(&row.id)), drawn);
    if selected {
        revealing(Id::new(&row.id), revealed, drawn)
    } else {
        drawn
    }
}

/// The mock's `.pr`: the mono id in a 60 pt column beside the semibold title, then the project
/// under the id and the status in its hue under the title. The selected row is the sel fill.
fn two_lines(app: AppState, row: &OutlineRow, base: View) -> View {
    let phone = app.form == Form::Phone;
    let (id_size, title_size, line_size) = if phone {
        (13.0, 15.0, 13.5)
    } else {
        (12.0, 13.0, 12.0)
    };
    let small = |words: String, ink: AdaptiveColor| {
        text(words)
            .truncate()
            .font_family(FONT_TEXT)
            .text_size(px(line_size))
            .text_color(ink)
    };
    let id_column = view()
        .w(px(ID_WIDTH))
        .flex_shrink_0()
        .gap(px(2.0))
        .child(
            text(row.id.clone())
                .number_of_lines(1u32)
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(id_size))
                .text_color(color(Token::Ink3)),
        )
        .children(
            row.project_chip
                .clone()
                .map(|project| small(project, color(Token::Ink3))),
        );
    let status_ink = match row.hue {
        Hue::Neutral => color(Token::Ink3),
        hue => hue_ink(hue),
    };
    let main_column = view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(2.0))
        .child(
            text(row.title.clone())
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(title_size))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink)),
        )
        .child(small(row.status.clone(), status_ink));
    base.items_start()
        .gap(px(8.0))
        .px(px(if phone { 18.0 } else { 16.0 }))
        .py(px(if phone { 10.0 } else { 9.0 }))
        .when(phone, |r| r.min_h(px(TARGET)))
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .when(row.selected, |r| r.bg(color(Token::Sel)))
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        })
        .child(id_column)
        .child(main_column)
}

/// What a terminal row keeps beside its title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TermFit {
    /// The status and the project.
    All,
    /// The status; the project went first.
    Status,
    /// The status's first word.
    Word,
}

impl TermFit {
    /// What fits in a row `cells` wide beside the bar, the id and the title's least (all of a
    /// short title, else `TERM_TITLE`); everything before the row is measured.
    fn of(cells: f32, title: f32, status: f32, project: Option<f32>) -> TermFit {
        if cells <= 0.0 {
            return TermFit::All;
        }
        let left = cells - 1.0 - TERM_ID - title.min(TERM_TITLE);
        let status = status + 1.0;
        match project {
            Some(project) if left >= status + project + 1.0 => TermFit::All,
            None if left >= status => TermFit::All,
            _ if left >= status => TermFit::Status,
            _ => TermFit::Word,
        }
    }
}

/// One line of Cells: the selection bar, the id (never cut), the title taking whatever room is
/// left, then the status in its hue and the project quiet at the trailing edge. As the row
/// narrows the project goes first, then the status shortens to its first word, and only then
/// is the title cut, down to `TERM_TITLE` Cells.
fn terminal_row(row: &OutlineRow, base: View, room: Signal<f32>) -> View {
    let status_ink = match row.hue {
        Hue::Neutral => color(Token::Ink),
        hue => hue_ink(hue),
    };
    let length = |words: &str| words.chars().count() as f32;
    let title = length(&row.title);
    let fit = {
        let status = length(&row.status);
        let project = row.project_chip.as_deref().map(length);
        computed(move || TermFit::of(room.get() / term::cells(1.0).0, title, status, project))
    };
    let after = |words: Text, shown: fn(TermFit) -> bool| {
        view()
            .flex_row()
            .flex_shrink_0()
            .style_when(move || !shown(fit.get()), |s| s.hidden())
            .child(term::space(1.0))
            .child(words.number_of_lines(1u32))
    };
    base.items_center()
        .h(term::rows(1.0))
        .child(
            view()
                .w(term::cells(1.0))
                .flex_shrink_0()
                .accessibility_elements_hidden(true)
                .child(text(if row.selected { "▌" } else { " " }).text_color(color(Token::Ink))),
        )
        .child(
            view()
                .w(term::cells(TERM_ID))
                .flex_shrink_0()
                .child(term::quiet(row.id.clone()).number_of_lines(1u32)),
        )
        .child(
            text(row.title.clone())
                .truncate()
                .flex_1()
                .min_w(term::cells(title.min(TERM_TITLE)))
                .text_color(color(Token::Ink)),
        )
        .child(after(
            text(row.status.clone()).text_color(status_ink),
            |fit| fit != TermFit::Word,
        ))
        .child(
            after(
                text(row.status_word.clone()).text_color(status_ink),
                |fit| fit == TermFit::Word,
            )
            // Narrower still, the word gives way rather than the title's least.
            .flex_shrink(1.0)
            .min_w(px(0.0)),
        )
        .children(
            row.project_chip
                .clone()
                .map(|project| after(term::quiet(project), |fit| fit == TermFit::All)),
        )
}

/// The selected row, scrolled into the list's view when the selection has just moved to it, so
/// a key press never leaves it off screen; a row already in view stays put. The candidate
/// counts as revealed only once the scroll has run: a rebuild of the rows before then drops the
/// pending scroll with the row it was for, and the rebuilt row asks again.
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
