//! The scaffolds every screen sits in: page heads, columns of panes, scrolling bodies, the
//! context column, the pinned decision bar and panels. The pieces the phone mockup introduced
//! (the pushed page's head, group heads, page sections, card groups) live in `page_phone` and
//! are re-exported here, so every scaffold is `kit::page::*`.
//!
//! Each piece takes the app first and draws the look of its form. Desktop metrics are the
//! mockup's (`css/app.css`: `.pagehead`, `.qhead`, `.cols`, `.pane`, `.side-col`, `.decide`,
//! `.pairflow`); the phone's are its `.plarge`, `.pbody` and `.pdecide`; a terminal draws whole
//! Cells, ruled with the joined lines of `ui::term`, and uses colour for state only.
//!
//! Scrolling is explicit: [`pane`], [`body`], [`context`] and [`bar_pane`] each scroll their
//! content; [`columns`] only lays out what it is given, so pass it those.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

pub use crate::ui::kit::page_phone::{card, group_head, pushed_head, search, section};

/// Room the phone leaves under scrolling content for the floating Ask button (`.askfab`), so the
/// last row stays pressable above it.
const ASK_CLEARANCE: f32 = 64.0;
/// Desktop points per terminal Cell for a fixed column: a 400 pt list keeps 33 Cells, which
/// leaves the detail room beside it on a 120-column grid.
const POINTS_PER_COLUMN_CELL: f32 = 12.0;

/// How wide a column is: the rest of the row, or fixed (desktop points, as the mockup's grid
/// columns: `400`, `340`, `300`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Width {
    Fill,
    Fixed(f32),
}

/// The page head (`.pagehead`): the title, an optional sub line, the actions at the trailing
/// edge. Desktop: a 46 pt row under a rule, title 15 pt bold, sub 12 pt quiet. Phone: the large
/// title (`.plarge`, 30 pt) with the actions at its trailing edge, on its last line (pass
/// [`search`] there on the tab roots); the sub line is left out, as the mockup's phone heads
/// have none. Terminal: one row, the title bold and never cut, the actions at the end, the sub
/// line quiet between them taking what room is left: cut with an ellipsis, gone below six Cells;
/// a joined rule under it. The title carries test_id "page-title" and the header role.
pub fn head(
    app: AppState,
    title: impl Into<SharedString>,
    sub: Option<String>,
    actions: Vec<AnyElement>,
) -> AnyElement {
    let form = app.form;
    let title = title_text(form, title.into(), "page-title", Title::Page);
    match form {
        Form::Desktop => view()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(12.0))
            .h(px(46.0))
            .px(px(18.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(title)
            .when_some(sub, |row, sub| row.child(quiet(form, sub, 12.0)))
            .child(spacer())
            .child(actions_row(form, actions, 8.0))
            .into_element(),
        Form::Phone => view()
            .flex_row()
            .flex_none()
            .items_end()
            .gap(px(10.0))
            .px(px(18.0))
            .pt(px(6.0))
            .pb(px(8.0))
            .bg(color(Token::Bg))
            .child(title)
            .child(spacer())
            .child(actions_row(form, actions, 2.0))
            .into_element(),
        Form::Terminal => view()
            .flex_none()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .h(term::rows(1.0))
                    .px(term::cells(1.0))
                    .child(title)
                    .when_some(sub, |row, sub| row.child(yielding_sub(sub)))
                    .child(spacer())
                    .child(actions_row(form, actions, 2.0)),
            )
            .child(term::hrule())
            .into_element(),
    }
}

/// The handover head (`.qhead`): a meta row of chips and short texts, the title, an optional
/// note under it, the actions beside the title. Desktop: padding 14 24 11 under a rule, meta
/// 12 pt wrapping (6 pt rows, 10 pt apart), title 19 pt bold at line height 1.3, wrapping.
/// Phone: a page band (`.pband`): meta chips wrapping 6 pt apart, title 20 pt bold at line
/// height 1.25, wrapping, the note 13 pt quiet (the mockup moves "from worker … ago" there),
/// actions in a row under it. Terminal: the meta row, the title in bold, wrapping, the note
/// quiet, a joined rule under them. The title has test_id "qhead-title".
pub fn qhead(
    app: AppState,
    meta: Vec<AnyElement>,
    title: impl Into<SharedString>,
    note: Option<String>,
    actions: Vec<AnyElement>,
) -> AnyElement {
    let form = app.form;
    let title = title_text(form, title.into(), "qhead-title", Title::Handover);
    match form {
        Form::Desktop => view()
            .flex_none()
            .px(px(24.0))
            .pt(px(14.0))
            .pb(px(11.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(meta_row(form, meta))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .mt(px(6.0))
                    .child(title)
                    .child(spacer())
                    .child(actions_row(form, actions, 8.0)),
            )
            .when_some(note, |head, note| {
                head.child(view().mt(px(4.0)).child(quiet(form, note, 12.0)))
            })
            .into_element(),
        Form::Phone => view()
            .flex_none()
            .px(px(18.0))
            .pt(px(14.0))
            .pb(px(16.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(view().mb(px(8.0)).child(meta_row(form, meta)))
            .child(title)
            .when_some(note, |head, note| {
                head.child(view().mt(px(4.0)).child(quiet(form, note, 13.0)))
            })
            .when(!actions.is_empty(), |head| {
                head.child(view().mt(px(12.0)).child(actions_row(form, actions, 8.0)))
            })
            .into_element(),
        Form::Terminal => view()
            .flex_none()
            .child(
                view()
                    .px(term::cells(1.0))
                    .child(meta_row(form, meta))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .child(title)
                            .child(spacer())
                            .child(actions_row(form, actions, 2.0)),
                    )
                    .when_some(note, |head, note| head.child(term::quiet(note))),
            )
            .child(term::hrule())
            .into_element(),
    }
}

/// Columns of panes side by side (`.cols`): each `(width, pane)` in order, a rule between
/// neighbours. Pass panes ([`pane`], [`body`], [`context`], [`bar_pane`]): columns do not
/// scroll themselves. Desktop: fixed columns keep their points, `Fill` takes the rest, a 1 pt
/// rule on the left of every column after the first. Phone: the panes stacked in order with a
/// rule between (phone screens usually push the detail instead). Terminal: fixed columns get a
/// Cell for every 12 points, joined `│` lines between them; on a grid narrower than
/// `term::wide()` only the column at index `keep` shows, across the whole width.
pub fn columns(app: AppState, cols: Vec<(Width, AnyElement)>, keep: usize) -> AnyElement {
    match app.form {
        Form::Desktop => view()
            .flex_1()
            .flex_row()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .children(cols.into_iter().enumerate().map(|(at, (width, col))| {
                let cell = view().flex_col().min_w(px(0.0)).min_h(px(0.0));
                let cell = match width {
                    Width::Fill => cell.flex_1(),
                    Width::Fixed(points) => cell.w(px(points)).flex_shrink_0(),
                };
                cell.when(at > 0, |cell| {
                    cell.border_l(px(1.0)).border_l_color(color(Token::Rule))
                })
                .child(col)
                .into_element()
            }))
            .into_element(),
        Form::Phone => view()
            .children(cols.into_iter().enumerate().map(|(at, (_, col))| {
                view()
                    .when(at > 0, |cell| {
                        cell.border_t(px(1.0)).border_color(color(Token::Rule))
                    })
                    .child(col)
                    .into_element()
            }))
            .into_element(),
        Form::Terminal => {
            let mut parts = Vec::new();
            for (at, (width, col)) in cols.into_iter().enumerate() {
                let kept = at == keep;
                if at > 0 {
                    parts.push(
                        view()
                            .flex_row()
                            .self_stretch()
                            .style_when(move || !term::wide(), |s| s.hidden())
                            .child(term::vrule())
                            .into_element(),
                    );
                }
                let cell = view().flex_col().min_w(px(0.0)).min_h(px(0.0));
                let cell = match width {
                    Width::Fill => cell.flex_1(),
                    Width::Fixed(points) => cell
                        .w(term::cells((points / POINTS_PER_COLUMN_CELL).round()))
                        .flex_shrink_0()
                        // Alone on a narrow grid, the kept column takes the whole width.
                        .style_when(move || kept && !term::wide(), |s| s.flex_1()),
                };
                parts.push(
                    cell.style_when(move || !kept && !term::wide(), |s| s.hidden())
                        .child(col)
                        .into_element(),
                );
            }
            view()
                .flex_1()
                .flex_row()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .children(parts)
                .into_element()
        }
    }
}

/// A scrolling pane with no padding of its own, for lists whose rows carry their paddings
/// (`.pane`). Phone: the page body (`.pbody`), with the 64 pt clearance under the content for
/// the floating Ask button; it is the page's only scroll, so the shell must not wrap it in
/// another. `test_id` goes on the scroll view.
pub fn pane(app: AppState, test_id: &str, content: AnyElement) -> AnyElement {
    let inner = view().child(content);
    let inner = if app.form == Form::Phone {
        inner.pb(px(ASK_CLEARANCE))
    } else {
        inner
    };
    scroll()
        .test_id(test_id)
        .flex_1()
        .min_h(px(0.0))
        .child(inner)
        .into_element()
}

/// A scrolling page body with the mockup's paddings: desktop 18 24 30 (`.set .pane`), phone the
/// same as [`pane`] (its sections carry their own 18 pt sides), terminal a Cell each side.
pub fn body(app: AppState, test_id: &str, content: AnyElement) -> AnyElement {
    let inner = view().child(content);
    let inner = match app.form {
        Form::Desktop => inner.px(px(24.0)).pt(px(18.0)).pb(px(30.0)),
        Form::Phone => inner.pb(px(ASK_CLEARANCE)),
        Form::Terminal => inner.px(term::cells(1.0)),
    };
    scroll()
        .test_id(test_id)
        .flex_1()
        .min_h(px(0.0))
        .child(inner)
        .into_element()
}

/// The context column (`.side-col`): a scrolling pane on the sunken ground, padding 16 18 24.
/// Phone: a padded block (14 18) on the sunken ground, for when a screen stacks it under the
/// main content. Terminal: a scrolling pane a Cell in from its lines, no fill.
pub fn context(app: AppState, test_id: &str, content: AnyElement) -> AnyElement {
    match app.form {
        Form::Desktop => scroll()
            .test_id(test_id)
            .flex_1()
            .min_h(px(0.0))
            .bg(color(Token::BgSunk))
            .child(view().px(px(18.0)).pt(px(16.0)).pb(px(24.0)).child(content))
            .into_element(),
        Form::Phone => view()
            .test_id(test_id)
            .px(px(18.0))
            .py(px(14.0))
            .bg(color(Token::BgSunk))
            .child(content)
            .into_element(),
        Form::Terminal => scroll()
            .test_id(test_id)
            .flex_1()
            .min_h(px(0.0))
            .child(view().px(term::cells(1.0)).child(content))
            .into_element(),
    }
}

/// A scrolling pane with a pinned bar under it: `content` scrolls, `bar` (usually [`bar`])
/// stays at the bottom. On the phone the pair fills the page and the bar replaces the tab bar,
/// so the shell hides its tabs while such a page is on top.
pub fn bar_pane(app: AppState, test_id: &str, content: AnyElement, bar: AnyElement) -> AnyElement {
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(pane(app, test_id, content))
        .child(bar)
        .into_element()
}

/// The decision bar (`.decide`, `.pdecide`): the actions in a row and, on desktop and in a
/// terminal, an attribution at the trailing edge. Desktop: padding 12 24, 10 pt apart, a rule
/// above, the attribution right-aligned in 11.5 pt quiet text at most 430 pt wide. Phone: the
/// actions only (pass them `wide`), padding 10 14 and the home indicator's inset under them.
/// Terminal: a joined rule, then one row of actions two Cells apart, never cut; the attribution
/// under them, a Cell in, clipped to two rows on a wide grid and one on a narrow one.
/// test_id "decide-bar", role toolbar.
pub fn bar(app: AppState, actions: Vec<AnyElement>, attribution: Option<AnyElement>) -> AnyElement {
    let form = app.form;
    let root = view()
        .test_id("decide-bar")
        .role(AccessibilityRole::Toolbar)
        .flex_none();
    match form {
        Form::Desktop => root
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .px(px(24.0))
            .py(px(12.0))
            .bg(color(Token::Bg))
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
            .children(actions)
            .child(spacer())
            .when_some(attribution, |row, attribution| {
                row.child(
                    view()
                        .flex_shrink(1.0)
                        .min_w(px(0.0))
                        .max_w(px(430.0))
                        .items_end()
                        .child(attribution),
                )
            })
            .into_element(),
        Form::Phone => root
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(14.0))
            .pt(px(10.0))
            .pb_safe_area_plus(px(12.0))
            .bg(color(Token::Bg))
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
            .children(actions)
            .into_element(),
        Form::Terminal => root
            .child(term::hrule())
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .px(term::cells(1.0))
                    .children(spaced(
                        actions
                            .into_iter()
                            .map(|action| view().flex_shrink_0().child(action).into_element())
                            .collect(),
                        2.0,
                    )),
            )
            .when_some(attribution, |bar, attribution| {
                bar.child(
                    view()
                        .px(term::cells(1.0))
                        .max_h(move || term::rows(if term::wide() { 2.0 } else { 1.0 }))
                        .overflow_hidden()
                        .child(attribution),
                )
            })
            .into_element(),
    }
}

/// A raised panel around related content (`.pairflow` on desktop, `.pcard-q` on the phone),
/// with an optional title. Desktop: surface, 1 pt `rule-2` line, radius 10, padding 16, the
/// title 13 pt bold 10 pt above the content. Phone: surface, radius 14, padding 14 16, the
/// title 16 pt bold. Terminal: a framed region with the title set into its top line.
pub fn panel(app: AppState, title: Option<String>, content: AnyElement) -> AnyElement {
    let form = app.form;
    match form {
        Form::Desktop | Form::Phone => {
            let phone = form == Form::Phone;
            view()
                .bg(color(Token::Surface))
                .border_1()
                .border_color(color(Token::Rule2))
                .rounded(px(if phone { 14.0 } else { 10.0 }))
                .px(px(16.0))
                .py(px(if phone { 14.0 } else { 16.0 }))
                .when_some(title, |panel, title| {
                    panel.child(
                        text(title)
                            .role(AccessibilityRole::Header)
                            .mb(px(10.0))
                            .font_family(FONT_TEXT)
                            .text_size(px(if phone { 16.0 } else { 13.0 }))
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::Ink)),
                    )
                })
                .child(content)
                .into_element()
        }
        Form::Terminal => term::framed(view().px(term::cells(1.0)).child(content))
            .when_some(title, |frame, title| {
                frame.child(term::top_title(
                    text(title)
                        .role(AccessibilityRole::Header)
                        .number_of_lines(1u32)
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink)),
                ))
            })
            .into_element(),
    }
}

/// Which head a title leads: a page's, on one line, or a handover's, which wraps.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Title {
    Page,
    Handover,
}

/// The title of a head: bold, the header role. Desktop `.pagehead h1` (15 pt, one line) or
/// `.qhead h1` (19 pt, line height 1.3); phone `.plarge h1` (30 pt) or the `.pband` title
/// (20 pt, line height 1.25); terminal one line for a page, wrapping for a handover.
pub(super) fn title_text(
    form: Form,
    title: SharedString,
    test_id: &str,
    kind: Title,
) -> AnyElement {
    let handover = kind == Title::Handover;
    let t = text(title)
        .test_id(test_id)
        .role(AccessibilityRole::Header)
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    let (size, line, tracking) = match (form, handover) {
        (Form::Desktop, false) => (15.0, 20.0, -0.08),
        (Form::Desktop, true) => (19.0, 24.7, -0.23),
        (Form::Phone, false) => (30.0, 33.0, -0.6),
        (Form::Phone, true) => (20.0, 25.0, -0.2),
        // A page title is never cut in a terminal: its sub line gives way instead.
        (Form::Terminal, _) => {
            return t
                .when(!handover, |t| t.flex_shrink_0().number_of_lines(1u32))
                .into_element();
        }
    };
    t.font_family(FONT_TEXT)
        .text_size(px(size))
        .line_height(px(line))
        .letter_spacing(px(tracking))
        .when(form == Form::Desktop && !handover, |t| t.truncate())
        .into_element()
}

/// Quiet secondary text at `size` (desktop and phone), muted in a terminal.
fn quiet(form: Form, content: String, size: f32) -> AnyElement {
    if form == Form::Terminal {
        return term::quiet(content).into_element();
    }
    text(content)
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .line_height(px(size * 1.4))
        .text_color(color(Token::Ink3))
        .into_element()
}

/// The meta row of a handover head: chips and short texts wrapping onto further rows.
fn meta_row(form: Form, meta: Vec<AnyElement>) -> AnyElement {
    match form {
        Form::Terminal => view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .children(spaced(meta, 1.0))
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_y(px(6.0))
            .gap_x(px(if form == Form::Phone { 6.0 } else { 10.0 }))
            .children(meta)
            .into_element(),
    }
}

/// Actions in a row at the trailing edge; in a terminal `gap` Cells apart.
fn actions_row(form: Form, actions: Vec<AnyElement>, gap: f32) -> AnyElement {
    if form == Form::Terminal {
        return view()
            .flex_row()
            .items_center()
            .flex_shrink_0()
            .children(spaced(actions, gap))
            .into_element();
    }
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(gap))
        .children(actions)
        .into_element()
}

/// Terminal items `n` blank Cells apart: a gap there collapses to whole Cells unevenly.
fn spaced(items: Vec<AnyElement>, n: f32) -> Vec<AnyElement> {
    let mut out = Vec::with_capacity(items.len() * 2);
    for (at, item) in items.into_iter().enumerate() {
        if at > 0 {
            out.push(term::space(n));
        }
        out.push(item);
    }
    out
}

/// The terminal head's sub line, two Cells after the title: it gives way to the title and the
/// actions, cut with an ellipsis, and goes once fewer than six Cells are left for it, where an
/// ellipsis alone would say nothing.
fn yielding_sub(content: String) -> AnyElement {
    let cramped = signal(false);
    view()
        .flex_row()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .overflow_hidden()
        .on_layout(move |frame: &Bounds<Points>| {
            let short = frame.size.width.0 < term::cells(6.0).0;
            if cramped.get_untracked() != short {
                cramped.set(short);
            }
        })
        .style_when(move || cramped.get(), |s| s.invisible())
        .child(term::space(2.0))
        .child(
            term::quiet(content)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .truncate(),
        )
        .into_element()
}

pub(super) fn spacer() -> AnyElement {
    view().flex_1().min_w(px(0.0)).into_element()
}
