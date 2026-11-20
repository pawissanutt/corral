//! Lists of facts and steps: the key and value list, the quiet table, the device and usage
//! tables with a header row, and the marked and numbered step lists. Cells and values are
//! elements, so any screen fills them with text, chips, marks or buttons from the kit.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::kit::page::Width;
use crate::ui::kit::text::label_value;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

/// Desktop points per terminal Cell for a fixed table column: a 120 pt label column keeps 15.
const POINTS_PER_TABLE_CELL: f32 = 8.0;

/// A headed table's test id and the paddings (x, y) of its header cells and body cells.
struct Headed {
    test_id: &'static str,
    head: (f32, f32),
    cell: (f32, f32),
}

/// `.utable th` 8 12, `td` 7 12.
const UTABLE: Headed = Headed {
    test_id: "utable",
    head: (12.0, 8.0),
    cell: (12.0, 7.0),
};

/// Where a numbered step stands (`.steps4 li`): done, the current one, or still to come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Done,
    On,
    Next,
}

/// The key and value list (`.kv`, a `<dl>`): one `text::label_value` row per `(label, value)`.
/// Desktop: rows 5 pt apart (a 76 pt label column). Phone: 8 pt apart, each label over its
/// value. Terminal: one row each, no space between. test_id "kv".
pub fn kv(app: AppState, rows: Vec<(String, AnyElement)>) -> AnyElement {
    let gap = match app.form {
        Form::Desktop => 5.0,
        Form::Phone => 8.0,
        Form::Terminal => 0.0,
    };
    view()
        .test_id("kv")
        .gap(px(gap))
        .children(
            rows.into_iter()
                .map(|(label, value)| label_value(app, label, value))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// The quiet table (`.qtable`): rows of cells under no header, a rule under each row. `widths`
/// gives each column's width (missing ones fill); cells are laid out in that order. Desktop
/// and phone: cells padded 6 10, centred on the row. Terminal: one line per row, fixed columns
/// a Cell for every 8 points, two Cells between columns, no rules. Roles table, row and cell;
/// test_id "qtable".
pub fn qtable(app: AppState, widths: &[Width], rows: Vec<Vec<AnyElement>>) -> AnyElement {
    view()
        .test_id("qtable")
        .role(AccessibilityRole::Table)
        .children(
            rows.into_iter()
                .map(|cells| table_row(app, widths, &[], cells, (10.0, 6.0)))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// The usage table (`.utable`): [`devtable_aligned`] at its tighter paddings, header cells 8 12
/// and body cells 7 12; columns where `align_end` holds sit at the trailing edge, as its
/// `.n` figure columns. Phone and terminal as [`devtable`]. test_id "utable".
pub fn utable(
    app: AppState,
    widths: &[Width],
    header: &[&str],
    align_end: &[bool],
    rows: Vec<Vec<AnyElement>>,
) -> AnyElement {
    headed(app, &UTABLE, widths, header, align_end, rows)
}

/// A table under a header row, in the paddings of `look`.
fn headed(
    app: AppState,
    look: &Headed,
    widths: &[Width],
    header: &[&str],
    align_end: &[bool],
    rows: Vec<Vec<AnyElement>>,
) -> AnyElement {
    let form = app.form;
    let root = view().test_id(look.test_id).role(AccessibilityRole::Table);
    if form == Form::Phone {
        return root
            .children(
                rows.into_iter()
                    .map(|cells| phone_record(app, header, cells))
                    .collect::<Vec<_>>(),
            )
            .into_element();
    }
    let names = header
        .iter()
        .map(|name| column_name(form, name))
        .collect::<Vec<_>>();
    let head = table_row(app, widths, align_end, names, look.head);
    let root = root.child(head);
    let root = if form == Form::Terminal {
        root.child(term::hrule())
    } else {
        root
    };
    root.children(
        rows.into_iter()
            .map(|cells| table_row(app, widths, align_end, cells, look.cell))
            .collect::<Vec<_>>(),
    )
    .into_element()
}

/// The numbered step list (`.steps4`): a numbered disc, the step's title in bold and a quiet
/// detail under it. Desktop: 20 pt discs, 10 pt between items and after the disc, title 13 pt,
/// detail 12 pt; done is a filled disc with `✓`, the current one an ink ring, the rest a faint
/// ring. Phone: 24 pt discs, title 16 pt, detail 14 pt. Terminal: `✓`, the number in bold for the
/// current step or quiet for the rest, the title (bold when current), the detail quiet under
/// it. Each item reads "Step <n>, done | current | to come: <title>"; test_id "steps4".
pub fn steps4(app: AppState, items: Vec<(StepState, String, String)>) -> AnyElement {
    let form = app.form;
    view()
        .test_id("steps4")
        .role(AccessibilityRole::List)
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .children(
            items
                .into_iter()
                .enumerate()
                .map(|(at, (state, title, detail))| numbered(form, at + 1, state, title, detail))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// One row of a table: each cell at its column's width, at its trailing edge where `align_end`
/// holds, padded `(x, y)` on desktop and phone. A cell never draws past its column: a text that
/// does not shrink (pass `.flex_shrink(1.0).min_w(px(0.0))` to let it wrap) is clipped at the
/// column's edge rather than written over its neighbour's figures. Desktop and phone centre the
/// cells on the row, as the mockup; a terminal sets them on the row's first line, so a label
/// stays beside the first line of a value that runs to several.
fn table_row(
    app: AppState,
    widths: &[Width],
    align_end: &[bool],
    cells: Vec<AnyElement>,
    (pad_x, pad_y): (f32, f32),
) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let mut parts = Vec::with_capacity(cells.len() * 2);
    for (at, cell) in cells.into_iter().enumerate() {
        let width = widths.get(at).copied().unwrap_or(Width::Fill);
        let slot = view()
            .role(AccessibilityRole::Cell)
            .flex_row()
            .when(terminal, |slot| slot.items_start())
            .when(!terminal, |slot| slot.items_center())
            .min_w(px(0.0))
            .overflow_hidden()
            .when(align_end.get(at).copied().unwrap_or(false), |slot| {
                slot.justify_end()
            });
        let slot = match (form, width) {
            (_, Width::Fill) => slot.flex_1(),
            (Form::Terminal, Width::Fixed(points) | Width::Between { min: points, .. }) => slot
                .w(term::cells((points / POINTS_PER_TABLE_CELL).round()))
                .flex_shrink_0(),
            (_, Width::Fixed(points)) => slot.w(px(points)).flex_shrink_0(),
            (_, Width::Between { share, min, max }) => slot
                .w(relative(share))
                .min_w(px(min))
                .max_w(px(max))
                .flex_shrink_0(),
        };
        if form == Form::Terminal {
            if at > 0 {
                parts.push(term::space(2.0));
            }
            parts.push(slot.child(cell).into_element());
        } else {
            parts.push(slot.px(px(pad_x)).py(px(pad_y)).child(cell).into_element());
        }
    }
    let row = view()
        .role(AccessibilityRole::Row)
        .flex_row()
        .when(terminal, |row| row.items_start())
        .when(!terminal, |row| row.items_center())
        .children(parts);
    match form {
        Form::Terminal => row.px(term::cells(1.0)).into_element(),
        Form::Desktop | Form::Phone => row
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .into_element(),
    }
}

/// A column's name in the header row.
fn column_name(form: Form, name: &str) -> AnyElement {
    let name = text(SharedString::from(name.to_owned()))
        .role(AccessibilityRole::ColumnHeader)
        .number_of_lines(1u32)
        .uppercase();
    match form {
        Form::Terminal => name
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::Ink))
            .into_element(),
        Form::Desktop | Form::Phone => name
            .font_family(FONT_TEXT)
            .text_size(px(11.0))
            .letter_spacing(px(0.66))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color(Token::Ink3))
            .into_element(),
    }
}

/// A device table's row on the phone: its first cell, then the others named by their column.
fn phone_record(app: AppState, header: &[&str], cells: Vec<AnyElement>) -> AnyElement {
    let mut cells = cells.into_iter();
    let first = cells.next();
    let rest = cells
        .enumerate()
        .map(|(at, cell)| {
            let name = header.get(at + 1).copied().unwrap_or_default();
            label_value(app, SharedString::from(name.to_owned()), cell)
        })
        .collect::<Vec<_>>();
    view()
        .role(AccessibilityRole::Row)
        .gap(px(6.0))
        .px(px(18.0))
        .py(px(10.0))
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .children(first)
        .children(rest)
        .into_element()
}

/// One numbered step: its disc (or terminal marker), title and detail.
fn numbered(form: Form, n: usize, state: StepState, title: String, detail: String) -> AnyElement {
    let spoken = format!(
        "Step {n}, {}: {title}",
        match state {
            StepState::Done => "done",
            StepState::On => "current",
            StepState::Next => "to come",
        }
    );
    // One element for screen readers, read as the label, not as the disc and texts in turn.
    let item = view()
        .flex_row()
        .items_start()
        .accessible(true)
        .label(spoken);
    if form == Form::Terminal {
        let marker = match state {
            StepState::Done => text("✓").text_color(color(Token::Ink)),
            StepState::On => text(n.to_string())
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
            StepState::Next => term::quiet(n.to_string()),
        };
        return item
            .child(view().w(term::cells(2.0)).flex_shrink_0().child(marker))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        text(title)
                            .text_color(color(Token::Ink))
                            .when(state == StepState::On, |t| t.font_weight(FontWeight::BOLD)),
                    )
                    .child(text(detail).text_color(color(Token::Ink3))),
            )
            .into_element();
    }
    let phone = form == Form::Phone;
    let (disc, title_size, detail_size) = if phone {
        (24.0, 16.0, 14.0)
    } else {
        (20.0, text_size(form), 12.0)
    };
    let (fill, ring, ink) = match state {
        StepState::Done => (
            color(Token::Ink2),
            AdaptiveColor::TRANSPARENT,
            color(Token::Bg),
        ),
        StepState::On => (
            AdaptiveColor::TRANSPARENT,
            color(Token::Ink),
            color(Token::Ink),
        ),
        StepState::Next => (
            AdaptiveColor::TRANSPARENT,
            color(Token::Rule2),
            color(Token::Ink3),
        ),
    };
    let figure = if state == StepState::Done {
        String::from("✓")
    } else {
        n.to_string()
    };
    item.gap(px(10.0))
        .child(
            view()
                .size(px(disc))
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(fill)
                .border(px(1.5))
                .border_color(ring)
                .accessibility_elements_hidden(true)
                .child(
                    text(figure)
                        .font_family(FONT_TEXT)
                        .text_size(px(if phone { 12.0 } else { 11.0 }))
                        .font_weight(FontWeight::BOLD)
                        .text_color(ink),
                ),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(
                    text(title)
                        .font_family(FONT_TEXT)
                        .text_size(px(title_size))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink)),
                )
                .child(
                    text(detail)
                        .font_family(FONT_TEXT)
                        .text_size(px(detail_size))
                        .line_height(px(detail_size * 1.45))
                        .text_color(color(Token::Ink3)),
                ),
        )
        .into_element()
}
