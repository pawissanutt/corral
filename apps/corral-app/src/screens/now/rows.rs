//! Now's rows: an owner move (`.ask`) and a task (`.row`), each one line on desktop and in a
//! terminal and two lines on the phone, plus the quiet `+ N more` line under a collapsed group.
//!
//! Desktop: 38 pt lines, 20 pt in, a hairline under each, the columns of the mock: an owner
//! move is its kind's icon in the needs-you hue, the mono reference, the project, the title in
//! semibold, the age, then its one button; a task is the changed mark, the actor dot, the mono
//! id, the project, the title, the named rail, age and cost (held ink above the cost warning).
//! Pressing a row selects it; its hint (what the move holds up, what the rail's stage is doing)
//! shows on pointer rest and keyboard focus. The list measures its width once ([`Fit`]): the
//! project, then age, then cost give way before the title does.
//!
//! Terminal: the same columns in whole Cells led by the selection bar; NSH bands the selected
//! row. With room to spare the hint follows the row dimmed; otherwise it is the status line.
//!
//! Phone: no hover, so each row keeps its explanatory line: an owner move's consequence with its
//! button beneath, a task's now-phrase beside its rail.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::glance::StatIcon;
use crate::model::hue::Hue;
use crate::model::work::{self, NowAskKind, NowAskRow, NowTaskRow, Press};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::glance::stage_rail;
use crate::ui::kit::hint::{hint_on, hinted};
use crate::ui::kit::marks::{actor, delta, hue_ink};
use crate::ui::term::{cells, space};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, row_height, text_size};

use super::parts::{model_button, press, press_id};
use super::verbs::{row_record, with_menu};

const RING: f32 = 2.0;
/// The desktop row (`.row`, `.askmain`): 38 pt high, 20 pt in, 10 pt between columns.
const DESK_ROW: f32 = 38.0;
pub(crate) const DESK_PAD: f32 = 20.0;
pub(super) const DESK_GAP: f32 = 10.0;
/// What an empty age or cost cell shows: the fact is unknown, not zero.
const UNKNOWN: &str = "–";

/// Column widths: (desktop points, terminal Cells).
pub(super) const DELTA: (f32, f32) = (10.0, 1.0);
pub(super) const ACTOR: (f32, f32) = (12.0, 1.0);
pub(crate) const ID: (f32, f32) = (52.0, 6.0);
const REFERENCE: (f32, f32) = (50.0, 8.0);
pub(super) const PROJECT: (f32, f32) = (60.0, 10.0);
pub(super) const RAIL: (f32, f32) = (150.0, 20.0);
const HINT: (f32, f32) = (0.0, 24.0);
pub(super) const AGE: (f32, f32) = (36.0, 4.0);
pub(super) const COST: (f32, f32) = (50.0, 6.0);
/// The least the title keeps before an optional column takes room from it.
const TITLE: (f32, f32) = (120.0, 12.0);

/// The phone's two-line rows: 18 pt in, the second line under the title past the 12 pt mark
/// and its 10 pt gap.
pub(crate) const PHONE_PAD: f32 = 18.0;
pub(super) const PHONE_INDENT: f32 = 22.0;

/// Which optional columns the list's width holds: the hint (terminal only) goes first, then the
/// project, then age, then cost. A width of 0 (not measured yet) holds them all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fit {
    pub hint: bool,
    pub project: bool,
    pub age: bool,
    pub cost: bool,
}

/// The fit of a list `width` points wide.
pub(crate) fn fit(form: Form, width: f32) -> Fit {
    if width <= 0.0 || form == Form::Phone {
        return Fit {
            hint: form == Form::Terminal,
            project: true,
            age: true,
            cost: true,
        };
    }
    let terminal = form == Form::Terminal;
    let (units, pick, pad, gap) = if terminal {
        ((width / cells(1.0).0).floor(), 1, 1.0, 1.0)
    } else {
        (width, 0, 2.0 * DESK_PAD, DESK_GAP)
    };
    let size = |column: (f32, f32)| if pick == 1 { column.1 } else { column.0 };
    let base = pad + size(DELTA) + size(ACTOR) + size(ID) + size(TITLE) + size(RAIL) + 4.0 * gap;
    let cost = units >= base + size(COST) + gap;
    let age = cost && units >= base + size(COST) + size(AGE) + 2.0 * gap;
    let project = age && units >= base + size(COST) + size(AGE) + size(PROJECT) + 3.0 * gap;
    let hint = terminal
        && project
        && units >= base + size(COST) + size(AGE) + size(PROJECT) + size(HINT) + 4.0 * gap;
    Fit {
        hint,
        project,
        age,
        cost,
    }
}

/// A fixed column `column` wide; `right` aligns its content to the trailing edge.
pub(crate) fn column(form: Form, column: (f32, f32), right: bool, child: impl IntoElement) -> View {
    view()
        .w(if form == Form::Terminal {
            cells(column.1)
        } else {
            px(column.0)
        })
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .overflow_hidden()
        .when(right, |cell| cell.justify_end())
        .child(child)
}

/// An empty column `column` wide, keeping its room on every row. A terminal sizes a box holding
/// a blank text by that text, which is no Cells wide, so there it is an empty box of the width.
pub(crate) fn blank(form: Form, column: (f32, f32)) -> AnyElement {
    match form {
        Form::Terminal => space(column.1),
        Form::Desktop | Form::Phone => view().w(px(column.0)).flex_shrink_0().into_element(),
    }
}

/// [`column`] shown while `show` holds: out of the row, its gap with it, and hidden from screen
/// readers otherwise (the row's label still says it).
pub(super) fn optional(
    form: Form,
    width: (f32, f32),
    right: bool,
    show: impl Fn() -> bool + Copy + 'static,
    child: impl IntoElement,
) -> AnyElement {
    column(form, width, right, child)
        .style_when(move || !show(), |s| s.hidden())
        .accessibility_elements_hidden(move || !show())
        .into_element()
}

/// One line of mono figures (ids, ages, costs).
pub(crate) fn figures(form: Form, content: impl Into<SharedString>, size: f32, ink: Token) -> Text {
    let words = text(content.into())
        .number_of_lines(1u32)
        .truncate()
        .text_color(color(ink));
    if form == Form::Terminal {
        words
    } else {
        words
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(size))
    }
}

/// One line of words at `size`, cut to fit.
pub(crate) fn words(
    form: Form,
    content: impl Into<SharedString>,
    size: f32,
    ink: AdaptiveColor,
) -> Text {
    let words = text(content.into())
        .number_of_lines(1u32)
        .truncate()
        .text_color(ink);
    if form == Form::Terminal {
        words
    } else {
        words.font_family(FONT_TEXT).text_size(px(size))
    }
}

/// An age or cost, or the dash for one not recorded.
fn known(value: &str) -> String {
    if value.is_empty() {
        UNKNOWN.to_owned()
    } else {
        value.to_owned()
    }
}

/// The kind of an owner move as its icon: a question's speech bubble, a plan's acceptance, a
/// verdict's review mark.
fn kind_icon(kind: NowAskKind) -> Icon {
    match kind {
        NowAskKind::Question => Icon::Ask,
        NowAskKind::Plan => Icon::Plans,
        NowAskKind::Verdict => Icon::Review,
    }
}

/// The kind's mark: the icon in the needs-you hue, or in a terminal the needs-you glyph.
pub(super) fn kind_mark(form: Form, kind: NowAskKind, size: f32) -> AnyElement {
    if form == Form::Terminal {
        return text(StatIcon::NeedsYou.glyph().to_string())
            .text_color(hue_ink(Hue::You))
            .accessibility_elements_hidden(true)
            .into_element();
    }
    view()
        .accessibility_elements_hidden(true)
        .child(icon(kind_icon(kind), size, hue_ink(Hue::You)))
        .into_element()
}

/// A pressable line of the list in the form's look, with its selected state: desktop 38 pt with
/// the selection fill and a hover fill (unless `fill` is off: a row around the line draws them,
/// as an owner move's does around its line and its button); terminal one row of Cells after the
/// selection bar; phone a tall block of lines with a pressed fill. Screen readers hear one
/// button named `label`.
pub(crate) fn pressable_line(
    app: AppState,
    label: String,
    test_id: String,
    selected: Memo<bool>,
    action: Press,
    content: Vec<AnyElement>,
    fill: bool,
) -> View {
    let form = app.form;
    let line = view()
        .relative()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .min_w(px(0.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .accessibility_state(move || AccessibilityState {
            selected: selected.get(),
            ..AccessibilityState::default()
        })
        .test_id(&test_id)
        .on_press(move || press(app, action.clone()))
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        });
    match form {
        Form::Desktop => line
            .h(px(DESK_ROW))
            .gap(px(DESK_GAP))
            .when(fill, |line| {
                line.style_when(selected, |s| s.bg(color(Token::Sel)))
                    .hover(|s| s.bg(color(Token::Hover)))
            })
            .children(content),
        Form::Phone => line
            .items_start()
            .style_when(selected, |s| s.bg(color(Token::Sel)))
            .active(|s| s.bg(color(Token::Hover)))
            .children(content),
        Form::Terminal => line
            .h(px(row_height(form)))
            .gap(cells(1.0))
            .child(
                view()
                    .w(cells(1.0))
                    .flex_shrink_0()
                    .accessibility_elements_hidden(true)
                    .child(
                        text(move || if selected.get() { "▌" } else { " " })
                            .text_color(color(Token::Ink)),
                    ),
            )
            .children(content),
    }
}

/// An owner move (`.ask`): its pressable line, then its one button; selected while it holds the
/// side panel. test ids: `inbox-<key>` on the line, `btn-ask-<key>` on the button.
pub(crate) fn ask_row(
    app: AppState,
    row: &NowAskRow,
    selected: Memo<bool>,
    fit: Memo<Fit>,
) -> AnyElement {
    let form = app.form;
    let label = [row.reference.as_str(), row.title.as_str()]
        .into_iter()
        .chain(row.project_chip.as_deref())
        .chain(Some(row.age.as_str()).filter(|age| !age.is_empty()))
        .collect::<Vec<_>>()
        .join(", ");
    let button = ask_button(app, row);
    let test_id = format!("inbox-{}", row.key);
    match form {
        Form::Phone => {
            let line = pressable_line(
                app,
                label,
                test_id,
                selected,
                row.select.clone(),
                vec![phone_ask(app, row)],
                true,
            )
            .px(px(PHONE_PAD))
            .pt(px(10.0))
            .pb(px(8.0));
            view()
                .border_b_1()
                .border_b_color(color(Token::Rule))
                .child(hint_on(app, line, row.consequence.clone()))
                .child(
                    view()
                        .flex_row()
                        .pl(px(PHONE_PAD + PHONE_INDENT))
                        .pb(px(12.0))
                        .child(button),
                )
                .into_element()
        }
        Form::Desktop | Form::Terminal => {
            let terminal = form == Form::Terminal;
            let project = row.project_chip.clone();
            let mut parts = vec![
                column(form, (14.0, 1.0), false, kind_mark(form, row.kind, 14.0)).into_element(),
                column(
                    form,
                    REFERENCE,
                    false,
                    figures(form, row.reference.clone(), 12.0, Token::Ink3),
                )
                .into_element(),
            ];
            if let Some(project) = project {
                parts.push(optional(
                    form,
                    PROJECT,
                    false,
                    move || fit.get().project,
                    words(form, project, 12.0, color(Token::Ink3)),
                ));
            }
            parts.push(
                words(form, row.title.clone(), text_size(form), color(Token::Ink))
                    .font_weight(FontWeight::SEMIBOLD)
                    .flex_1()
                    .min_w(px(0.0))
                    .into_element(),
            );
            if terminal {
                // An empty consequence keeps its Cells (an empty box keeps its width in a
                // terminal; one holding a blank text does not), so titles and ages stay aligned.
                parts.push(if row.consequence.is_empty() {
                    view()
                        .w(cells(HINT.1))
                        .h(px(row_height(form)))
                        .flex_shrink_0()
                        .style_when(move || !fit.get().hint, |s| s.hidden())
                        .into_element()
                } else {
                    optional(
                        form,
                        HINT,
                        false,
                        move || fit.get().hint,
                        words(form, row.consequence.clone(), 12.0, color(Token::Ink3)),
                    )
                });
            }
            parts.push(optional(
                form,
                AGE,
                true,
                move || fit.get().age,
                figures(form, known(&row.age), 12.0, Token::Ink3),
            ));
            let line = pressable_line(
                app,
                label,
                test_id,
                selected,
                row.select.clone(),
                parts,
                false,
            )
            .flex_1()
            .when(!terminal, |line| line.pl(px(DESK_PAD)));
            let line = hint_on(app, line, row.consequence.clone());
            let outer = view()
                .flex_row()
                .items_center()
                .flex_shrink_0()
                .gap(if terminal { cells(1.0) } else { px(12.0) })
                .child(line)
                .child(button);
            if terminal {
                return outer.into_element();
            }
            outer
                .pr(px(DESK_PAD))
                .border_b_1()
                .border_b_color(color(Token::Rule))
                .style_when(selected, |s| s.bg(color(Token::Sel)))
                .hover(|s| s.bg(color(Token::Hover)))
                .into_element()
        }
    }
}

/// The owner move's one button; a button the daemon cannot route says why on its hint.
fn ask_button(app: AppState, row: &NowAskRow) -> AnyElement {
    let button = model_button(app, &row.button, format!("btn-ask-{}", row.key), false);
    match &row.button.reason {
        Some(reason) => hinted(app, button, reason.clone()),
        None => button,
    }
}

/// The phone's owner move: icon, reference, project and age; the title; then what it holds up.
fn phone_ask(app: AppState, row: &NowAskRow) -> AnyElement {
    let form = app.form;
    let first = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(column(
            form,
            (14.0, 1.0),
            false,
            kind_mark(form, row.kind, 14.0),
        ))
        .child(figures(form, row.reference.clone(), 13.0, Token::Ink3).flex_shrink_0())
        .children(row.project_chip.clone().map(|project| {
            words(form, project, 13.0, color(Token::Ink3))
                .flex_shrink(1.0)
                .min_w(px(0.0))
        }))
        .child(view().flex_1().min_w(px(0.0)))
        .child(figures(form, known(&row.age), 12.5, Token::Ink3).flex_shrink_0());
    let title = text(row.title.clone())
        .number_of_lines(2u32)
        .truncate()
        .font_family(FONT_TEXT)
        .text_size(px(text_size(form)))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink));
    // An empty consequence only repeated the title: no line, and no gap for one.
    let consequence = (!row.consequence.is_empty()).then(|| {
        text(row.consequence.clone())
            .number_of_lines(2u32)
            .truncate()
            .font_family(FONT_TEXT)
            .text_size(px(13.5))
            .text_color(color(Token::Ink2))
    });
    view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(3.0))
        .child(first)
        .child(
            view()
                .pl(px(PHONE_INDENT))
                .gap(px(3.0))
                .child(title)
                .children(consequence),
        )
        .into_element()
}

/// A task (`.row`): selected while it holds the side panel; its verb menu on the context
/// press (desktop, terminal) or a held finger (phone). test id `task-<id>`.
pub(crate) fn task_row(
    app: AppState,
    row: &NowTaskRow,
    selected: Memo<bool>,
    fit: Memo<Fit>,
) -> AnyElement {
    let form = app.form;
    let label = match &row.project_chip {
        Some(project) => format!("{}, {project}", row.line.label()),
        None => row.line.label(),
    };
    let test_id = format!("task-{}", row.line.id);
    let content = match form {
        Form::Phone => vec![phone_task(app, row)],
        Form::Desktop | Form::Terminal => task_cells(app, row, fit),
    };
    let line = pressable_line(
        app,
        label,
        test_id,
        selected,
        row.action.clone(),
        content,
        true,
    );
    let line = match form {
        Form::Desktop => line
            .px(px(DESK_PAD))
            .border_b_1()
            .border_b_color(color(Token::Rule)),
        Form::Phone => line
            .px(px(PHONE_PAD))
            .py(px(10.0))
            .border_b_1()
            .border_b_color(color(Token::Rule)),
        Form::Terminal => line,
    };
    let line = hint_on(app, line, row.rail.hint.clone()).into_element();
    match row_record(&row.action) {
        Some(record) => with_menu(app, record, line),
        None => line,
    }
}

/// The task row's columns on desktop and in a terminal.
fn task_cells(app: AppState, row: &NowTaskRow, fit: Memo<Fit>) -> Vec<AnyElement> {
    let form = app.form;
    let line = &row.line;
    let changed = if line.changed {
        column(form, DELTA, false, delta(app)).into_element()
    } else {
        blank(form, DELTA)
    };
    let title_ink = color(if line.quiet() {
        Token::Ink2
    } else {
        Token::Ink
    });
    let cost_ink = if line.cost_high {
        Token::Held
    } else {
        Token::Ink3
    };
    let mut out = vec![
        changed,
        column(form, ACTOR, false, actor(app, line.actor)).into_element(),
        column(
            form,
            ID,
            false,
            figures(form, line.id.clone(), 12.0, Token::Ink3),
        )
        .into_element(),
    ];
    if let Some(project) = row.project_chip.clone() {
        out.push(optional(
            form,
            PROJECT,
            false,
            move || fit.get().project,
            words(form, project, 12.0, color(Token::Ink3)),
        ));
    }
    out.push(
        words(form, line.title.clone(), text_size(form), title_ink)
            .font_weight(FontWeight::SEMIBOLD)
            .flex_1()
            .min_w(px(0.0))
            .into_element(),
    );
    out.push(column(form, RAIL, false, stage_rail(app, &row.rail)).into_element());
    if form == Form::Terminal {
        out.push(optional(
            form,
            HINT,
            false,
            move || fit.get().hint,
            words(form, row.rail.hint.clone(), 12.0, color(Token::Ink3)),
        ));
    }
    out.push(optional(
        form,
        AGE,
        true,
        move || fit.get().age,
        figures(form, known(&line.age), 12.0, Token::Ink3),
    ));
    out.push(optional(
        form,
        COST,
        true,
        move || fit.get().cost,
        figures(form, known(&line.cost), 12.0, cost_ink),
    ));
    out
}

/// The phone's task: actor, id, project, title and age; then the rail, the now-phrase and cost.
fn phone_task(app: AppState, row: &NowTaskRow) -> AnyElement {
    let form = app.form;
    let line = &row.line;
    let quieter = line.quiet();
    let now_ink = match line.now_hue {
        Hue::Neutral => color(Token::Ink2),
        hue => hue_ink(hue),
    };
    let first = view()
        .flex_row()
        .items_baseline()
        .gap(px(8.0))
        .child(
            view()
                .w(px(12.0))
                .flex_shrink_0()
                .child(actor(app, line.actor)),
        )
        .child(figures(form, line.id.clone(), 13.0, Token::Ink3).flex_shrink_0())
        .children(
            row.project_chip
                .clone()
                .map(|project| words(form, project, 13.0, color(Token::Ink3)).flex_shrink_0()),
        )
        .child(
            text(line.title.clone())
                .flex_1()
                .min_w(px(0.0))
                .number_of_lines(2u32)
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(text_size(form)))
                .font_weight(if quieter {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::SEMIBOLD
                })
                .text_color(color(if quieter { Token::Ink2 } else { Token::Ink })),
        )
        .child(figures(form, known(&line.age), 12.5, Token::Ink3).flex_shrink_0());
    let second = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .pl(px(PHONE_INDENT - 2.0))
        .when(line.changed, |second| second.child(delta(app)))
        .child(stage_rail(app, &row.rail))
        .child(
            words(form, row.explanation().to_owned(), 13.5, now_ink)
                .flex_1()
                .min_w(px(0.0)),
        )
        .child(
            figures(
                form,
                known(&line.cost),
                12.5,
                if line.cost_high {
                    Token::Held
                } else {
                    Token::Ink3
                },
            )
            .flex_shrink_0(),
        );
    view()
        .flex_1()
        .min_w(px(0.0))
        .gap(px(4.0))
        .child(first)
        .child(second)
        .into_element()
}

/// The quiet `+ N more` line under a collapsed group: desktop 26 pt under the titles with a
/// chevron after the words; phone at least 48 pt with a hairline; terminal one row of Cells.
pub(super) fn more_row(app: AppState, button: &work::Button) -> AnyElement {
    let form = app.form;
    let label = button.label.to_string();
    let action = button.action.clone();
    let row = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .flex_shrink_0()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label.clone())
        .when_some(action.as_ref().map(press_id), |row, id| row.test_id(&id))
        .on_press(move || {
            if let Some(action) = &action {
                press(app, action.clone());
            }
        })
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        });
    let ink = color(if form == Form::Terminal {
        Token::Ink
    } else {
        Token::Ink3
    });
    match form {
        // Under the titles: past the padding, the changed, actor and id columns and their gaps.
        Form::Desktop => row
            .h(px(28.0))
            .pl(px(DESK_PAD + DELTA.0 + ACTOR.0 + ID.0 + 3.0 * DESK_GAP))
            .pr(px(DESK_PAD))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .hover(|s| s.bg(color(Token::Hover)))
            .child(words(form, label, 12.0, ink))
            .child(icon(Icon::Chevron, 12.0, color(Token::Ink3))),
        Form::Phone => row
            .min_h(px(48.0))
            .px(px(PHONE_PAD))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .active(|s| s.bg(color(Token::Hover)))
            .child(words(form, label, 14.0, ink).flex_1().min_w(px(0.0))),
        Form::Terminal => row
            .h(px(row_height(form)))
            .pl(cells(1.0 + 1.0 + DELTA.1 + ACTOR.1 + ID.1 + 4.0))
            .child(words(form, label, 0.0, ink)),
    }
    .into_element()
}
