//! Rows of lists: the group header over a run of task rows, and a generic selectable row other
//! lists fill. A terminal row is one line of whole Cells led by a one-Cell selection bar; NSH
//! bands the selected, hovered or focused row itself, so the row draws no fill of its own there.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term::{cells, hrule};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, row_height};

const RING: f32 = 2.0;
/// The phone row's least height (`.prow`).
const PHONE_ROW: f32 = 60.0;
/// The desktop two-line row's least height (`.row` of a two-line list).
const DESK_TALL_ROW: f32 = 40.0;

/// How a row stands on desktop: one line, or tall enough for two or three lines.
enum Height {
    Line,
    Tall,
}

/// The terminal title's least room beside a project chip: the chip shows only while it leaves
/// this many Cells.
const TERM_TITLE_WITH_CHIP: f32 = 16.0;

/// A row's title cell: the title filling it, or, with a project chip, the title cut first and
/// the quiet chip right after it. Any list row whose model carries a `project_chip` uses it. In a
/// terminal the chip yields to the title ([`project_chip`]).
pub fn titled(app: AppState, title: Text, project: Option<String>) -> AnyElement {
    match project {
        None => title.flex_1().min_w(px(0.0)).into_element(),
        Some(project) => {
            let room = signal(0.0_f32);
            let face = project_chip(app, project, room, 0.0);
            measured(
                app,
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_row()
                    .items_center()
                    .gap(if app.form == Form::Terminal {
                        cells(1.0)
                    } else {
                        px(6.0)
                    }),
                room,
            )
            .child(title.flex_shrink(1.0).min_w(px(0.0)))
            .child(face)
            .into_element()
        }
    }
}

/// Records the width of `container` in `room` (a terminal only), for [`project_chip`]. The
/// container must take its width from its parent, not from its content, so the chip showing or
/// hiding never changes it.
pub fn measured(app: AppState, container: View, room: Signal<f32>) -> View {
    if app.form == Form::Terminal {
        container.on_layout(move |frame: &Bounds<Points>| room.set(frame.size.width.0))
    } else {
        container
    }
}

/// Whether a terminal row's project chip fits: the title keeps at least [`TERM_TITLE_WITH_CHIP`]
/// Cells beside it. `room` is the width of the [`measured`] container holding title and chip,
/// `reserved` the Cells its other content takes. Before the first measure it holds.
pub fn project_fits(project: &str, room: Signal<f32>, reserved: f32) -> Memo<bool> {
    // The label, a Cell of padding either side and the Cell before it.
    let need = project.chars().count() as f32 + 3.0 + reserved + TERM_TITLE_WITH_CHIP;
    computed(move || {
        let width = room.get();
        width <= 0.0 || (width / cells(1.0).0).floor() >= need
    })
}

/// A row's quiet project chip. In a terminal it shows only while it [`project_fits`]; when it
/// does not it is out of the row, its gap with it, and hidden from screen readers (the row's
/// label still names the project). Elsewhere it always shows.
pub fn project_chip(
    app: AppState,
    project: String,
    room: Signal<f32>,
    reserved: f32,
) -> AnyElement {
    if app.form != Form::Terminal {
        return chip(app, project, Hue::Neutral);
    }
    let fits = project_fits(&project, room, reserved);
    view()
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .style_when(move || !fits.get(), |s| s.hidden())
        .accessibility_elements_hidden(move || !fits.get())
        .child(chip(app, project, Hue::Neutral))
        .into_element()
}

/// The header over a run of task rows (`.grp` on desktop, `.ph-sec` on a phone): the label in
/// the hue, the count in mono ink-3, then on desktop a hairline to the trailing edge and the
/// optional note in ink-4 ("gating · reviewing · landing"). Weights follow the mock: needs you
/// bold, corral and worker semibold, held semibold, [`Hue::Neutral`] (queued, parked) medium in
/// ink-2. Desktop 12 pt with 12 pt above and 4 pt below, 16 pt in from the edge; phone 13 pt bold
/// with 18 pt above, the note right-aligned in 12.5 pt ink-3 and no line; terminal the label bold
/// in its hue, the count, a joined `─` rule and the note. The label and count never shrink; a
/// narrow row cuts the note with an ellipsis first. A header for screen readers.
pub fn group_header(
    app: AppState,
    label: impl Into<SharedString>,
    count: Option<u32>,
    hue: Hue,
    note: Option<&'static str>,
) -> AnyElement {
    let form = app.form;
    let ink = match hue {
        Hue::Neutral => color(if form == Form::Terminal {
            Token::Ink
        } else {
            Token::Ink2
        }),
        _ => hue_ink(hue),
    };
    let weight = match (form, hue) {
        (Form::Phone, _) | (_, Hue::You) => FontWeight::BOLD,
        (_, Hue::Neutral) => FontWeight::MEDIUM,
        _ => FontWeight::SEMIBOLD,
    };
    let caption = text(label.into())
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(ink)
        .font_weight(if form == Form::Terminal {
            FontWeight::BOLD
        } else {
            weight
        });
    let figures = count.map(|n| text(n.to_string()).number_of_lines(1u32).flex_shrink_0());
    let row = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .role(AccessibilityRole::Header);
    match form {
        Form::Terminal => row
            .child(caption)
            .children(figures.map(|t| t.text_color(color(Token::Ink3))))
            .child(view().flex_1().min_w(px(0.0)).child(hrule()))
            .children(note.map(|note| {
                text(note)
                    .truncate()
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .text_color(color(Token::Ink3))
            }))
            .into_element(),
        Form::Desktop => row
            .pt(px(12.0))
            .pb(px(4.0))
            // The task rows' own edges, so the label stands over the delta column.
            .pl(px(16.0))
            .pr(px(14.0))
            .child(caption.font_family(FONT_TEXT).text_size(px(12.0)))
            .children(figures.map(|t| {
                t.font_family(FONT_MONO)
                    .text_size(px(11.0))
                    .text_color(color(Token::Ink3))
            }))
            .child(
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(1.0))
                    .bg(color(Token::Rule)),
            )
            .children(note.map(|note| {
                text(note)
                    .truncate()
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .font_family(FONT_TEXT)
                    .text_size(px(11.0))
                    .text_color(color(Token::Ink4))
            }))
            .into_element(),
        Form::Phone => row
            .pt(px(18.0))
            .pb(px(6.0))
            .px(px(18.0))
            .child(caption.font_family(FONT_TEXT).text_size(px(13.0)))
            .children(figures.map(|t| {
                t.font_family(FONT_MONO)
                    .text_size(px(12.0))
                    .text_color(color(Token::Ink3))
            }))
            .child(view().flex_1().min_w(px(0.0)))
            .children(note.map(|note| {
                text(note)
                    .truncate()
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .font_family(FONT_TEXT)
                    .text_size(px(12.5))
                    .text_color(color(Token::Ink3))
            }))
            .into_element(),
    }
}

/// A selectable, pressable list row the caller fills with `content`, laid out in a row: a button
/// for screen readers named `label`, with its selected state. Desktop: 30 pt high, 16 pt in on the
/// leading side and 14 pt on the trailing, children 8 pt apart, hover fill, selected the sel fill
/// with a 1 pt ink bar at the leading edge. Phone: at least 60 pt high, 10 by 18 pt padding,
/// 10 pt apart, a hairline under it, a pressed fill. Terminal: one row of Cells led by a
/// one-Cell bar on the selected row (`▌`), children a Cell apart; NSH bands it. `content` sizes
/// its own children (give the one that should give way `flex_1().min_w(0)`).
pub fn list_row(
    app: AppState,
    label: impl Into<SharedString>,
    test_id: impl Into<String>,
    selected: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
    content: Vec<AnyElement>,
) -> AnyElement {
    shell(
        app,
        label.into(),
        test_id.into(),
        selected,
        on_press,
        content,
        Height::Line,
    )
}

/// [`list_row`] for content of two or three lines (a title over a facts line): on desktop at
/// least 40 pt high with 6 pt above and below, so the lines are never clipped to the one-line
/// row's 30 pt, the content centred on the row; phone and terminal rows as [`list_row`].
pub fn list_row_tall(
    app: AppState,
    label: impl Into<SharedString>,
    test_id: impl Into<String>,
    selected: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
    content: Vec<AnyElement>,
) -> AnyElement {
    shell(
        app,
        label.into(),
        test_id.into(),
        selected,
        on_press,
        content,
        Height::Tall,
    )
}

/// The body of [`list_row`] and [`list_row_tall`].
fn shell(
    app: AppState,
    label: SharedString,
    test_id: String,
    selected: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
    content: Vec<AnyElement>,
    height: Height,
) -> AnyElement {
    let form = app.form;
    let selected = computed(selected);
    let tall = matches!(height, Height::Tall);
    let row = view()
        .relative()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .accessibility_state(move || AccessibilityState {
            selected: selected.get(),
            ..AccessibilityState::default()
        })
        .test_id(&test_id)
        .on_press(move || on_press());
    let row = match form {
        Form::Desktop => row
            .when(!tall, |row| row.h(px(row_height(form))))
            .when(tall, |row| row.min_h(px(DESK_TALL_ROW)).py(px(6.0)))
            .pl(px(16.0))
            .pr(px(14.0))
            .gap(px(8.0))
            .style_when(selected, |s| s.bg(color(Token::Sel)))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-RING))
            })
            .child(
                view()
                    .absolute()
                    .left(px(0.0))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(1.0))
                    .bg(color(Token::Ink))
                    .opacity(move || if selected.get() { 1.0_f32 } else { 0.0_f32 }),
            ),
        Form::Phone => row
            .min_h(px(PHONE_ROW.max(row_height(form))))
            .py(px(10.0))
            .px(px(18.0))
            .gap(px(10.0))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .style_when(selected, |s| s.bg(color(Token::Sel)))
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-RING))
            }),
        // A column of its own, so the rows line up whether one is selected or not.
        Form::Terminal => row.h(px(row_height(form))).gap(px(8.0)).child(
            view()
                .w(cells(1.0))
                .flex_shrink_0()
                .accessibility_elements_hidden(true)
                .child(
                    text(move || if selected.get() { "▌" } else { " " })
                        .text_color(color(Token::Ink)),
                ),
        ),
    };
    row.children(content).into_element()
}
