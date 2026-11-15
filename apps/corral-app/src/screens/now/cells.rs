//! The list's own column head in the task rows' columns, which give way as the kit's do
//! (`rows::task_columns`): the list measures its width once and its column head hides the
//! now-phrase, then age, then cost with the task rows beside it. A terminal lays them in whole
//! Cells: the selection bar, Δ, the actor mark, the full id, the title (the one cut), the rail,
//! then the optional columns.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::work::GroupedList;
use crate::ui::kit::rows::{TaskColumns, task_columns};
use crate::ui::term::cells;
use crate::ui::tokens::{Token, color};

/// Terminal column widths in Cells, as the kit's task row lays them.
const ID: f32 = 6.0;
const RAIL: f32 = 7.0;
const NOW: f32 = 22.0;
const AGE: f32 = 3.0;
const COST: f32 = 5.0;

/// The list's measured width in points, and the optional columns it holds (tracked).
pub(super) fn measured(form: Form) -> (Signal<f32>, Memo<TaskColumns>) {
    let width = signal(0.0_f32);
    let columns = computed(move || {
        let points = width.get();
        let units = match form {
            Form::Terminal => (points / cells(1.0).0).floor(),
            Form::Desktop | Form::Phone => points,
        };
        task_columns(form, units)
    });
    (width, columns)
}

/// Hides an optional column whole while `show` does not hold.
pub(super) fn optional<S: Styled>(s: S, show: impl Fn() -> bool + 'static) -> S {
    s.style_when(move || !show(), |s| s.hidden())
}

/// A terminal column `width` Cells wide.
fn column(width: f32, right: bool, child: AnyElement) -> View {
    view()
        .w(cells(width))
        .flex_shrink_0()
        .flex_row()
        .when(right, |cell| cell.justify_end())
        .child(child)
}

/// A terminal column shown while `show` holds.
fn shown(
    width: f32,
    right: bool,
    show: impl Fn() -> bool + Copy + 'static,
    child: AnyElement,
) -> AnyElement {
    optional(column(width, right, child), show)
        .accessibility_elements_hidden(move || !show())
        .into_element()
}

fn words(content: impl Into<SharedString>, ink: AdaptiveColor) -> Text {
    text(content.into()).number_of_lines(1u32).text_color(ink)
}

fn space() -> AnyElement {
    text(" ").into_element()
}

/// The terminal column head: the model's column names over the rows' columns.
pub(super) fn head(list: &GroupedList, columns: Memo<TaskColumns>) -> AnyElement {
    let name = |index: usize| {
        words(
            list.columns.get(index).copied().unwrap_or_default(),
            color(Token::Ink3),
        )
    };
    view()
        .flex_row()
        .items_center()
        .h(px(16.0))
        .gap(px(8.0))
        .accessibility_elements_hidden(true)
        // The selection bar, Δ and the actor mark come first.
        .child(column(1.0, false, space()))
        .child(column(1.0, false, space()))
        .child(column(1.0, false, space()))
        .child(column(ID, false, name(0).into_element()))
        .child(name(1).flex_1().min_w(px(0.0)).truncate())
        .child(column(RAIL, false, name(2).into_element()))
        .child(shown(
            NOW,
            false,
            move || columns.get().now,
            name(3).into_element(),
        ))
        .child(shown(
            AGE,
            true,
            move || columns.get().age,
            name(4).into_element(),
        ))
        .child(shown(
            COST,
            true,
            move || columns.get().cost,
            name(5).into_element(),
        ))
        .into_element()
}
