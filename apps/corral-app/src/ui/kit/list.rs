//! The list page every place with rows shares: grouped sections of table rows beside an
//! inspector panel (`model::list`).
//!
//! Desktop and terminal: the page's head, then a header row naming the columns over a
//! scrolling list of sections, each a header (status mark, name, count) over its rows; beside
//! the list the inspector shows the selected row. A press selects a row and a second press on
//! it, or Enter, opens its page; `j` `k` `↑` `↓` walk the rows (`state::keys`). With the
//! inspector hidden (`i`), or where the page has no room for it, a row opens on one press.
//! Columns keep their widths and line up under the header; as the list narrows the columns
//! named to give way leave whole, in their order. Phone: one column of rows, no header row and
//! no inspector; a row opens its page.

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::list::{self, Column, ColumnWidth, ListKey, Section};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::more::with_menu;
use crate::ui::kit::status::look_mark;
use crate::ui::term::{self, cells};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, row_height};

/// The least desktop page that keeps the inspector beside the list (the mock's 980, with the
/// inspector at its least, `list::inspector_width`).
const BESIDE: f32 = 980.0;
/// The same in a terminal: a list of 44 Cells, the line and the inspector at its least.
const BESIDE_CELLS: f32 = 81.0;
/// A desktop row's paddings and the gap between two of its columns.
const PAD_LEFT: f32 = 16.0;
const PAD_RIGHT: f32 = 14.0;
const GAP: f32 = 8.0;
const RING: f32 = 2.0;
/// The phone row's least height.
const PHONE_ROW: f32 = 60.0;
/// The least a section heading keeps beside a trailing control before the control wraps under
/// it: desktop and phone points, terminal Cells.
const HEAD_LEAST: f32 = 200.0;
const HEAD_LEAST_PHONE: f32 = 170.0;
const HEAD_LEAST_CELLS: f32 = 22.0;

/// One row as the page fills it. Desktop and terminal: a cell per column, in the columns'
/// order. Phone: what the row shows, laid out in a row (a mark, a column of two lines, a
/// control).
pub struct Row {
    /// `task-<id>`, `agent-<id>` ([`list::row_id`]).
    pub test_id: String,
    /// What a screen reader hears for the row.
    pub spoken: String,
    pub cells: Vec<AnyElement>,
    /// The record whose More menu the row carries as its context menu.
    pub menu: Option<crate::model::work::Record>,
}

/// Builds one row from its key.
pub type RowBuilder = Rc<dyn Fn(&Id) -> Row>;
/// Builds the inspector's content for the row with this key.
pub type InspectorBuilder = Rc<dyn Fn(&Id) -> AnyElement>;
pub type SectionAccessory = Rc<dyn Fn(&Section) -> Option<AnyElement>>;

/// What a list page gives the scaffold.
pub struct ListPage {
    /// `tasks`: the list's test id is `list-<key>`.
    pub key: &'static str,
    pub head: AnyElement,
    pub columns: Vec<Column>,
    /// Tracked: the sections and their rows' keys. Rows are built again only when these change.
    pub sections: Memo<Vec<Section>>,
    /// Tracked: the selected row, already settled on a row the list holds.
    pub selected: Memo<Option<Id>>,
    /// One row's cells; each cell follows its own facts.
    pub row: RowBuilder,
    /// The inspector's content for a row; none for a list without one.
    pub inspector: Option<InspectorBuilder>,
    /// A row was selected or opened.
    pub on_key: Rc<dyn Fn(ListKey)>,
    /// A folding section's header was pressed.
    pub on_fold: Rc<dyn Fn(&str)>,
    /// Tracked: what the list says while it has no row (loading, empty).
    pub notice: Memo<Option<String>>,
}

/// Tracked: whether a list page of `width` shows its inspector: asked for (`i`) and the page
/// has room for the list beside it. Never on a phone.
fn inspector_fits(app: AppState, width: f32) -> bool {
    match app.form {
        Form::Phone => false,
        // Before the first layout the page is as wide as its window allows.
        Form::Desktop => width <= 0.0 || width >= BESIDE,
        Form::Terminal => width <= 0.0 || width / cells(1.0).0 >= BESIDE_CELLS,
    }
}

/// The list page.
pub fn list_page(app: AppState, page: ListPage) -> AnyElement {
    list_page_with(app, page, None)
}

/// [`list_page`] with `foot` under the rows, inside the scrolling list: the pager of a paged
/// section.
pub fn list_page_with(app: AppState, page: ListPage, foot: Option<AnyElement>) -> AnyElement {
    list_page_with_sections(app, page, foot, None)
}

/// A list with optional controls at the trailing edge of each section heading (a section's
/// sort), on the heading's line rather than a row of their own.
pub fn list_page_with_sections(
    app: AppState,
    page: ListPage,
    foot: Option<AnyElement>,
    accessory: Option<SectionAccessory>,
) -> AnyElement {
    let form = app.form;
    let ListPage {
        key,
        head,
        columns,
        sections,
        selected,
        row,
        inspector,
        on_key,
        on_fold,
        notice,
    } = page;
    let width = signal(0.0_f32);
    let has_inspector = inspector.is_some();
    let beside =
        computed(move || has_inspector && app.inspector.get() && inspector_fits(app, width.get()));
    let terminal = form == Form::Terminal;
    // The inspector's width in the form's unit: points, or Cells in a terminal.
    let inspector_width = computed(move || {
        let page = width.get();
        list::inspector_width(if terminal { page / cells(1.0).0 } else { page }, terminal)
    });
    // The columns that fit the list: the page less the inspector.
    let columns = Rc::new(columns);
    let shown = {
        let columns = Rc::clone(&columns);
        computed(move || {
            let page = width.get();
            if page <= 0.0 {
                return vec![true; columns.len()];
            }
            let panel = if beside.get() {
                inspector_width.get()
            } else {
                0.0
            };
            let room = if terminal {
                // The line before the inspector, the selection bar's Cell and the Cell after it.
                page / cells(1.0).0 - if beside.get() { panel + 1.0 } else { 0.0 } - 2.0
            } else {
                page - panel - PAD_LEFT - PAD_RIGHT
            };
            list::fitted(&columns, room, if terminal { 1.0 } else { GAP }, terminal)
        })
    };
    let rows = {
        let columns = Rc::clone(&columns);
        dynamic(move || {
            let sections = sections.get();
            let (row, on_key, on_fold) = (Rc::clone(&row), Rc::clone(&on_key), Rc::clone(&on_fold));
            let columns = Rc::clone(&columns);
            let accessory = accessory.clone();
            untracked(move || {
                let mut out = Vec::new();
                for section in &sections {
                    let control = accessory.as_ref().and_then(|build| build(section));
                    out.push(section_head(app, section, Rc::clone(&on_fold), control));
                    for id in &section.rows {
                        out.push(table_row(
                            app,
                            &columns,
                            shown,
                            row(id),
                            id.clone(),
                            selected,
                            beside,
                            Rc::clone(&on_key),
                        ));
                    }
                }
                view().children(out).into_element()
            })
        })
    };
    let empty = dynamic(move || {
        notice.get().map(|words| {
            let words = text(words).text_color(color(Token::Ink3));
            match form {
                Form::Terminal => view().px(cells(2.0)).child(words).into_element(),
                Form::Desktop | Form::Phone => view()
                    .px(px(18.0))
                    .py(px(16.0))
                    .child(words.font_family(FONT_TEXT).text_size(px(13.0)))
                    .into_element(),
            }
        })
    });
    let list = view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .when(form != Form::Phone, |list| {
            list.child(table_head(app, &columns, shown))
        })
        .child(
            scroll()
                .test_id(&format!("list-{key}"))
                .element_ref(&app.list_keys)
                .flex_1()
                .min_h(px(0.0))
                .child(rows)
                .children(foot)
                .child(empty),
        );
    let panel = inspector.map(|inspector| {
        when(
            move || beside.get(),
            move || {
                let inspector = Rc::clone(&inspector);
                let body = dynamic(move || {
                    let id = selected.get();
                    let inspector = Rc::clone(&inspector);
                    untracked(move || id.map(|id| inspector(&id)))
                });
                let panel = view().test_id("inspector").flex_shrink_0().min_h(px(0.0));
                match form {
                    Form::Terminal => view()
                        .flex_row()
                        .flex_shrink_0()
                        .min_h(px(0.0))
                        .child(term::vrule())
                        .child(
                            panel
                                .w(move || cells(inspector_width.get()))
                                .child(scroll().flex_1().min_h(px(0.0)).child(body)),
                        )
                        .into_element(),
                    Form::Desktop | Form::Phone => panel
                        .w(move || px(inspector_width.get()))
                        .border_l(px(1.0))
                        .border_color(color(Token::Rule))
                        .bg(color(Token::Surface))
                        .child(scroll().flex_1().min_h(px(0.0)).child(body))
                        .into_element(),
                }
            },
        )
    });
    view()
        .size_full()
        .flex_grow(1.0)
        .flex_col()
        .min_h(px(0.0))
        .bg(color(Token::Bg))
        .child(head)
        .child(
            view()
                .flex_1()
                .flex_row()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
                .child(list)
                .children(panel),
        )
        .into_element()
}

/// A column's slot: its fixed width, or a share of what the fixed ones leave, never under its
/// least nor over its most; what the bounded columns leave on a wide page stays after the last
/// one, inside the row. Out of the row while the column has given way.
fn slot(form: Form, column: &Column, at: usize, shown: Memo<Vec<bool>>) -> View {
    let slot = view()
        .flex_row()
        .items_center()
        .min_w(px(0.0))
        .overflow_hidden()
        .when(column.end, |slot| slot.justify_end())
        .style_when(
            move || !shown.with(|shown| shown.get(at).copied().unwrap_or(true)),
            |s| s.hidden(),
        );
    let length = |points: f32, cell_count: f32| {
        if form == Form::Terminal {
            cells(cell_count)
        } else {
            px(points)
        }
    };
    match column.width {
        ColumnWidth::Fixed { points, cells } => slot.w(length(points, cells)).flex_shrink_0(),
        ColumnWidth::Flex {
            points,
            cells,
            grow,
            most,
        } => slot
            .w(length(points, cells))
            .flex_grow(grow)
            .flex_shrink(1.0)
            .when_some(most, |slot, (points, cells)| {
                slot.max_w(length(points, cells))
            }),
    }
}

/// The header row over the list: each column's name over its cells, in the quiet capitals of
/// a table head; a rule under it. Desktop and terminal only.
fn table_head(app: AppState, columns: &[Column], shown: Memo<Vec<bool>>) -> AnyElement {
    let form = app.form;
    let names = columns.iter().enumerate().map(|(at, column)| {
        let name = text(column.heading(form == Form::Terminal))
            .role(AccessibilityRole::ColumnHeader)
            .number_of_lines(1u32);
        let name = match form {
            Form::Terminal => name.text_color(color(Token::Ink3)),
            Form::Desktop | Form::Phone => name
                .font_family(FONT_TEXT)
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink3)),
        };
        slot(form, column, at, shown).child(name).into_element()
    });
    let row = view()
        .test_id("list-head")
        .role(AccessibilityRole::Row)
        .flex_row()
        .flex_none()
        .items_center();
    match form {
        Form::Terminal => view()
            .flex_none()
            .child(
                row.h(term::rows(1.0))
                    .gap(px(GAP))
                    .child(term::space(1.0))
                    .children(names),
            )
            .child(term::hrule())
            .into_element(),
        Form::Desktop | Form::Phone => row
            .h(px(28.0))
            .pl(px(PAD_LEFT))
            .pr(px(PAD_RIGHT))
            .gap(px(GAP))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .bg(color(Token::BgSunk))
            .children(names)
            .into_element(),
    }
}

/// A section's header: its mark, name and count, then its note; a folding one is a button
/// that opens and closes it (test id `section-<key>`). `trailing` sits at the line's end,
/// beside the button rather than inside it.
fn section_head(
    app: AppState,
    section: &Section,
    on_fold: Rc<dyn Fn(&str)>,
    trailing: Option<AnyElement>,
) -> AnyElement {
    let form = app.form;
    let label = text(section.label.clone())
        .number_of_lines(1u32)
        .flex_shrink_0()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink));
    let count = text(section.count.to_string())
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(Token::Ink3));
    let note = section.note.clone().map(|note| {
        text(note)
            .truncate()
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .text_color(color(Token::Ink3))
    });
    let mark = section
        .look
        .map(|look| look_mark(app, look, section.label.clone()));
    // A fold's caret: down while open, right while closed.
    let caret = section.fold.map(|open| match form {
        Form::Terminal => text(if open { "▾" } else { "▸" })
            .flex_shrink_0()
            .text_color(color(Token::Ink3))
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .flex_shrink_0()
            .when(open, |caret| caret.rotate(deg(90.0)))
            .child(icon(Icon::Chevron, 12.0, color(Token::Ink3)))
            .into_element(),
    });
    let spoken = match section.fold {
        Some(true) => format!("{}, {}, open", section.label, section.count),
        Some(false) => format!("{}, {}, folded", section.label, section.count),
        None => format!("{}, {}", section.label, section.count),
    };
    let head = view()
        .test_id(&format!("section-{}", section.key))
        .flex_row()
        .items_center()
        .flex_1()
        .min_w(px(0.0))
        .self_stretch()
        .label(spoken);
    let head = match section.fold {
        Some(open) => {
            let key = section.key.clone();
            head.role(AccessibilityRole::Button)
                .accessibility_state(move || AccessibilityState {
                    expanded: Some(open),
                    ..AccessibilityState::default()
                })
                .cursor_pointer()
                .on_press(move || on_fold(&key))
        }
        None => head.role(AccessibilityRole::Header),
    };
    // With a trailing control the line wraps it under the heading once the heading would get
    // less than its least: a narrow phone or a terminal list beside the inspector.
    let wraps = trailing.is_some();
    let least = match form {
        Form::Terminal => cells(HEAD_LEAST_CELLS),
        Form::Desktop => px(HEAD_LEAST),
        Form::Phone => px(HEAD_LEAST_PHONE),
    };
    let head = head.when(wraps, |head| head.min_w(least));
    let line = view()
        .flex_row()
        .items_center()
        .flex_none()
        .when(wraps, |line| line.flex_wrap());
    let trailing = trailing.map(|trailing| {
        view()
            .flex_row()
            .items_center()
            .flex_shrink_0()
            .child(trailing)
    });
    match form {
        Form::Terminal => line
            .min_h(term::rows(1.0))
            .gap_x(px(GAP))
            .child(
                head.h(term::rows(1.0))
                    .gap(px(GAP))
                    .child(term::space(1.0))
                    .children(caret)
                    .children(mark)
                    .child(label)
                    .child(count)
                    .children(note),
            )
            .children(
                trailing.map(|trailing| trailing.h(term::rows(1.0)).pl(cells(1.0)).pr(cells(1.0))),
            )
            .into_element(),
        Form::Desktop => line
            .min_h(px(row_height(form)))
            .pl(px(PAD_LEFT))
            .pr(px(PAD_RIGHT))
            .gap_x(px(GAP))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(
                head.h(px(row_height(form)))
                    .gap(px(GAP))
                    .children(caret)
                    .children(mark)
                    .child(label.font_family(FONT_TEXT).text_size(px(12.0)))
                    .child(count.font_family(FONT_MONO).text_size(px(11.0)))
                    .children(note.map(|note| note.font_family(FONT_TEXT).text_size(px(11.5)))),
            )
            .children(trailing.map(|trailing| trailing.min_h(px(row_height(form)))))
            .into_element(),
        // The heading's height and paddings are the fold button's own, so its whole band
        // takes the press; a trailing control sits beside it at the label's height.
        Form::Phone => line
            .px(px(18.0))
            .gap_x(px(GAP))
            .child(
                head.min_h(px(36.0))
                    .pt(px(14.0))
                    .pb(px(6.0))
                    .gap(px(GAP))
                    .children(caret)
                    .children(mark)
                    .child(label.font_family(FONT_TEXT).text_size(px(13.0)))
                    .child(count.font_family(FONT_MONO).text_size(px(12.0)))
                    .children(note.map(|note| note.font_family(FONT_TEXT).text_size(px(12.5)))),
            )
            .children(trailing.map(|trailing| trailing.pt(px(8.0)).pb(px(6.0))))
            .into_element(),
    }
}

/// One row: its cells in the columns' slots, selected while the list's selection names it,
/// scrolled into view when it becomes so. It tells the app while it holds the keyboard focus
/// (`AppState::focused_row`), so Enter opens the row that has it.
#[allow(clippy::too_many_arguments)]
fn table_row(
    app: AppState,
    columns: &[Column],
    shown: Memo<Vec<bool>>,
    row: Row,
    id: Id,
    selected: Memo<Option<Id>>,
    beside: Memo<bool>,
    on_key: Rc<dyn Fn(ListKey)>,
) -> AnyElement {
    let form = app.form;
    let Row {
        test_id,
        spoken,
        cells: content,
        menu,
    } = row;
    let me = id.clone();
    let is_selected = computed(move || selected.with(|selected| selected.as_ref() == Some(&me)));
    let handle = element_ref();
    effect(move || {
        if is_selected.get() {
            after_answer(move || {
                handle.reveal(RevealOptions::default().vertical(ScrollAlignment::Nearest));
            });
        }
    });
    let focused_row = app.focused_row;
    let (focused, blurred) = (test_id.clone(), test_id.clone());
    let line = view()
        .element_ref(&handle)
        .test_id(&test_id)
        .relative()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(spoken)
        .accessibility_state(move || AccessibilityState {
            selected: is_selected.get(),
            ..AccessibilityState::default()
        })
        .on_focus(move || focused_row.set(Some(focused.clone())))
        .on_blur(move || {
            focused_row.update(|row| {
                if row.as_deref() == Some(blurred.as_str()) {
                    *row = None;
                }
            })
        })
        .on_press(move || {
            let selected = selected.get_untracked();
            on_key(list::press(&id, selected.as_ref(), beside.get_untracked()));
        });
    let ring = |s: nsh_core::element::StyleRefinement| {
        s.outline(px(RING))
            .outline_color(color(Token::Focus))
            .outline_offset(px(-RING))
    };
    let drawn = match form {
        Form::Phone => line
            .min_h(px(PHONE_ROW))
            .py(px(10.0))
            .px(px(18.0))
            .gap(px(10.0))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring)
            .children(content)
            .into_element(),
        Form::Desktop => line
            .h(px(row_height(form)))
            .pl(px(PAD_LEFT))
            .pr(px(PAD_RIGHT))
            .gap(px(GAP))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .style_when(is_selected, |s| s.bg(color(Token::Sel)))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring)
            .child(
                view()
                    .absolute()
                    .left(px(0.0))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(2.0))
                    .bg(color(Token::Ink))
                    .opacity(move || if is_selected.get() { 1.0_f32 } else { 0.0_f32 }),
            )
            .children(slotted(form, columns, shown, content))
            .into_element(),
        // A column of its own, so the rows line up whether one is selected or not: the focus
        // mark while the row has the keyboard focus, else the selection's bar.
        Form::Terminal => {
            let has_focus = term::focus_of(handle);
            line.h(px(row_height(form)))
                .gap(px(GAP))
                .child(
                    view()
                        .w(cells(1.0))
                        .flex_shrink_0()
                        .accessibility_elements_hidden(true)
                        .child(
                            text(move || {
                                if has_focus.get() {
                                    term::FOCUS_MARK
                                } else if is_selected.get() {
                                    "▌"
                                } else {
                                    " "
                                }
                            })
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::Ink)),
                        ),
                )
                .children(slotted(form, columns, shown, content))
                .into_element()
        }
    };
    match menu {
        Some(record) => with_menu(app, record, drawn),
        None => drawn,
    }
}

/// The cells in their columns' slots.
fn slotted(
    form: Form,
    columns: &[Column],
    shown: Memo<Vec<bool>>,
    content: Vec<AnyElement>,
) -> Vec<AnyElement> {
    columns
        .iter()
        .enumerate()
        .zip(content)
        .map(|((at, column), cell)| {
            slot(form, column, at, shown)
                .role(AccessibilityRole::Cell)
                .child(cell)
                .into_element()
        })
        .collect()
}

/// A cell of words on one line, cut with an ellipsis at its column's edge: a title in ink, a
/// phrase or a fact in the quiet ink.
pub fn cell_text(app: AppState, words: impl Into<SharedString>, quiet: bool) -> AnyElement {
    let form = app.form;
    let words = text(words.into())
        .truncate()
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(if quiet && form != Form::Terminal {
            Token::Ink3
        } else if quiet {
            Token::Ink2
        } else {
            Token::Ink
        }));
    match form {
        Form::Terminal => words.into_element(),
        Form::Desktop => words
            .font_family(FONT_TEXT)
            .text_size(px(if quiet { 12.0 } else { 13.0 }))
            .into_element(),
        Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(if quiet { 13.5 } else { 16.0 }))
            .into_element(),
    }
}

/// The least a title cell is wide before its tags show: under it the title has the cell.
const TAGS_FROM: f32 = 260.0;
const TAGS_FROM_CELLS: f32 = 30.0;

/// A title cell: the title, then its tags (a priority, the plan it is in). The title is what
/// tells one row from the next, so in a narrow column the tags step out whole and the title
/// has the cell; they come back once the cell is wide enough for both.
pub fn cell_title(
    app: AppState,
    title: impl Into<SharedString>,
    tags: Vec<AnyElement>,
) -> AnyElement {
    let form = app.form;
    let words = text(title.into())
        .truncate()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(Token::Ink));
    let words = match form {
        Form::Terminal => words,
        Form::Desktop => words.font_family(FONT_TEXT).text_size(px(13.0)),
        Form::Phone => words.font_family(FONT_TEXT).text_size(px(16.0)),
    };
    let width = signal(0.0_f32);
    let cramped = computed(move || {
        let width = width.get();
        match form {
            Form::Terminal => width > 0.0 && width / cells(1.0).0 < TAGS_FROM_CELLS,
            Form::Desktop | Form::Phone => width > 0.0 && width < TAGS_FROM,
        }
    });
    view()
        .flex_1()
        .min_w(px(0.0))
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { GAP } else { 6.0 }))
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .child(words)
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_shrink_0()
                .gap(px(if form == Form::Terminal { GAP } else { 6.0 }))
                .style_when(move || cramped.get(), |s| s.hidden())
                .accessibility_elements_hidden(move || cramped.get())
                .children(tags),
        )
        .into_element()
}

/// A cell holding an id, a figure or an age in the mono font, never cut.
pub fn cell_mono(app: AppState, words: impl Into<SharedString>, quiet: bool) -> AnyElement {
    let form = app.form;
    let words = text(words.into())
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(if quiet { Token::Ink3 } else { Token::Ink2 }));
    match form {
        Form::Terminal => words.into_element(),
        Form::Desktop => words
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(11.5))
            .into_element(),
        Form::Phone => words
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(13.0))
            .into_element(),
    }
}

/// A phone row's two lines: the id and the title on the first, a quiet phrase under them
/// when there is one. Fills the row between its mark and its trailing control.
pub fn phone_lines(app: AppState, id: String, title: String, sub: String) -> AnyElement {
    view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(2.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(cell_mono(app, id, false))
                .child(cell_text(app, title, false)),
        )
        .when(!sub.is_empty(), |lines| {
            lines.child(view().flex_row().child(cell_text(app, sub, true)))
        })
        .into_element()
}
