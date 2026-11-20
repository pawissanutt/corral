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
use crate::ui::kit::{notice, page_phone};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

pub use crate::ui::kit::page_phone::{group_head, pushed_head, search, section};

/// Room the phone leaves under scrolling content for the floating Ask button (`.askfab`), so the
/// last row stays pressable above it.
const ASK_CLEARANCE: f32 = 64.0;
/// Desktop points per terminal Cell for a fixed column: a 400 pt list keeps 33 Cells.
const POINTS_PER_COLUMN_CELL: f32 = 12.0;
/// Stacked in a terminal, a list takes the rows it has up to two fifths of the height and the
/// column it leads to the rest, so the detail keeps the most rows while the list still shows
/// several, and a short list leaves no empty rows.
const STACKED_LIST_MOST: f32 = 0.4;
/// A part that gives way in a head row is shrunk this many times faster than the title, so the
/// title keeps its words until the part is gone.
const GIVE_WAY_SHRINK: f32 = 1000.0;
/// Narrower than this a desktop sub line would show a few letters and an ellipsis, which says
/// nothing: it goes instead.
const SUB_MIN_WIDTH: f32 = 48.0;
/// Scrolled further than this, a phone root page's large title folds to its bar size.
const COLLAPSE_AFTER: f32 = 40.0;
/// On a short window (a phone held sideways) a phone page bar stands 48 pt: its 44 pt buttons,
/// the rule above them, and what is left split above and below.
const SHORT_BAR_PAD: f32 = (48.0 - 44.0 - 1.0) / 2.0;

/// How wide a column is: the rest of the row, fixed (desktop points, as the mockup's grid
/// columns: `400`, `340`, `300`), or a share of the row kept between two widths, so a side
/// column grows with a wide window instead of leaving all the room to the one beside it. A
/// terminal takes `Between` at its least width, as it would a fixed column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Width {
    Fill,
    Fixed(f32),
    Between { share: f32, min: f32, max: f32 },
}

impl Width {
    /// The points a terminal sizes this column from; none for `Fill`.
    pub const fn least(self) -> Option<f32> {
        match self {
            Width::Fill => None,
            Width::Fixed(points) | Width::Between { min: points, .. } => Some(points),
        }
    }
}

/// The page head (`.pagehead`): the title, an optional sub line, the actions at the trailing
/// edge. Desktop: a 46 pt row under a rule, title 15 pt bold, sub 12 pt quiet on one line; as
/// the row runs short the sub line gives way first (cut, then gone), the title last. Phone: the
/// large title (`.plarge`, 30 pt) with the actions at its trailing edge, on its last line (pass
/// [`search`] there on the tab roots), and the connection banner under it, as the mockup puts
/// it; the sub line is left out, as the mockup's phone heads have none. The phone shell draws no
/// banner of its own over a page that leads with this head. Terminal: one row, the title bold
/// and never cut, the actions at the end, the sub line quiet between them taking what room is
/// left: cut with an ellipsis, gone below six Cells; a joined rule under it. The title carries
/// test_id "page-title" and the header role.
pub fn head(
    app: AppState,
    title: impl Into<SharedString>,
    sub: Option<String>,
    actions: Vec<AnyElement>,
) -> AnyElement {
    let form = app.form;
    let title: SharedString = title.into();
    match form {
        Form::Desktop => desktop_head_row(title, sub, actions)
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .into_element(),
        Form::Phone => view()
            .flex_none()
            .child(phone_head(title, actions))
            .child(notice::connection(app))
            .into_element(),
        Form::Terminal => view()
            .flex_none()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .h(term::rows(1.0))
                    .px(term::cells(1.0))
                    .child(title_text(form, title, "page-title", Title::Page))
                    .when_some(sub, |row, sub| row.child(yielding_sub(sub)))
                    .child(spacer())
                    .child(actions_row(form, actions, 2.0)),
            )
            .child(term::hrule())
            .into_element(),
    }
}

/// [`bar`] under a page whose content is kept to `max` points ([`measure`]): on a desktop the
/// actions and attribution sit in the measured column while the bar's ground and rule run edge
/// to edge. Phone and terminal bars are [`bar`]'s.
pub fn measured_bar(
    app: AppState,
    max: f32,
    actions: Vec<AnyElement>,
    attribution: Option<AnyElement>,
) -> AnyElement {
    match app.form {
        Form::Desktop => view()
            .flex_none()
            .bg(color(Token::Bg))
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
            .child(measure(
                app,
                max,
                desktop_bar_row(
                    view()
                        .test_id("decide-bar")
                        .role(AccessibilityRole::Toolbar)
                        .flex_none(),
                    actions,
                    attribution,
                )
                .into_element(),
            ))
            .into_element(),
        Form::Phone | Form::Terminal => bar(app, actions, attribution),
    }
}

/// The desktop bar's row of actions and trailing attribution, without its ground and rule.
fn desktop_bar_row(
    root: View,
    actions: Vec<AnyElement>,
    attribution: Option<AnyElement>,
) -> View {
    root.flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(24.0))
        .py(px(12.0))
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
}

/// [`head`] over a page whose content is kept to `max` points ([`measure`]): on a desktop the
/// title and actions sit in the measured column while the rule under them runs edge to edge, as
/// the bands under a head do. Phone and terminal heads are [`head`]'s.
pub fn measured_head(
    app: AppState,
    max: f32,
    title: impl Into<SharedString>,
    sub: Option<String>,
    actions: Vec<AnyElement>,
) -> AnyElement {
    match app.form {
        Form::Desktop => view()
            .flex_none()
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(measure(
                app,
                max,
                desktop_head_row(title.into(), sub, actions).into_element(),
            ))
            .into_element(),
        Form::Phone | Form::Terminal => head(app, title, sub, actions),
    }
}

/// The desktop head's 46 pt row without its rule.
fn desktop_head_row(title: SharedString, sub: Option<String>, actions: Vec<AnyElement>) -> View {
    view()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(12.0))
        .h(px(46.0))
        .px(px(18.0))
        .child(title_text(Form::Desktop, title, "page-title", Title::Page))
        .when_some(sub, |row, sub| row.child(desktop_sub(sub)))
        .child(spacer())
        .child(actions_row(Form::Desktop, actions, 8.0))
}

/// The phone's page head: the large title (`.plarge`, 30 pt) with the actions at its trailing
/// edge, on its last line. Once the page has scrolled past [`COLLAPSE_AFTER`] the title folds to
/// the 17 pt bold of a navigation bar and the head tightens, giving the rows the room; it opens
/// again back at the top (the shell resets the scroll on every tab or page change).
pub(super) fn phone_head(title: SharedString, actions: Vec<AnyElement>) -> AnyElement {
    let collapsed = computed(|| page_phone::page_scroll().with(|s| s.offset > COLLAPSE_AFTER));
    let title = text(title)
        .test_id("page-title")
        .role(AccessibilityRole::Header)
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink))
        .font_family(FONT_TEXT)
        .text_size(px(30.0))
        .line_height(px(33.0))
        .letter_spacing(px(-0.6))
        .style_when(collapsed, |s| {
            s.text_size(px(17.0))
                .line_height(px(22.0))
                .letter_spacing(px(-0.2))
        });
    view()
        .flex_row()
        .flex_none()
        .items_end()
        .gap(px(10.0))
        .px(px(18.0))
        .pt(px(6.0))
        .pb(px(8.0))
        .style_when(collapsed, |s| s.pt(px(4.0)).pb(px(4.0)))
        .bg(color(Token::Bg))
        .child(title)
        .child(spacer())
        .child(actions_row(Form::Phone, actions, 2.0))
        .into_element()
}

/// Columns of panes side by side (`.cols`): each `(width, pane)` in order, a rule between
/// neighbours. Pass panes ([`pane`], [`body`], [`context`], [`bar_pane`]): columns do not
/// scroll themselves. Desktop: fixed columns keep their points, `Fill` takes the rest, a 1 pt
/// rule on the left of every column after the first. Phone: the panes stacked in order with a
/// rule between (phone screens usually push the detail instead). Terminal: see
/// [`terminal_columns`]; `main` is the page's main column there, the one the columns before it
/// (lists) lead to and the columns after it (side context) stand beside.
pub fn columns(app: AppState, cols: Vec<(Width, AnyElement)>, main: usize) -> AnyElement {
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
                    Width::Between { share, min, max } => cell
                        .w(relative(share))
                        .min_w(px(min))
                        .max_w(px(max))
                        .flex_shrink_0(),
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
        Form::Terminal => terminal_columns(app, cols, main),
    }
}

/// Terminal columns. Side by side while the page holds every fixed column with the detail keeping
/// its least width (`term::beside`): fixed columns get a Cell for every 12 points, a lone list
/// growing with a wide page (`term::list_columns`), joined `│` lines between. Short of that they
/// stack rather than leave the list out: the columns before `main` above it, a joined `─` line
/// between, a list taking its own rows up to [`STACKED_LIST_MOST`] of the height and `main`
/// the rest; the columns after `main` step out until the page has room for them again.
fn terminal_columns(app: AppState, cols: Vec<(Width, AnyElement)>, main: usize) -> AnyElement {
    let fixed_count = cols
        .iter()
        .filter(|(width, _)| width.least().is_some())
        .count();
    let fixed: f32 = cols
        .iter()
        .map(|(width, _)| width.least().map_or(0.0, fixed_cells))
        .sum();
    // `term::beside` counts the one line beside a list; any further line is the columns' too.
    let lines = cols.len().saturating_sub(2) as f32;
    let stacked = move || !term::beside(app, fixed + lines);
    let mut parts = Vec::with_capacity(cols.len() * 3);
    for (at, (width, col)) in cols.into_iter().enumerate() {
        let side = at > main;
        if at > 0 {
            parts.push(
                view()
                    .flex_row()
                    .self_stretch()
                    .style_when(stacked, |s| s.hidden())
                    .child(term::vrule())
                    .into_element(),
            );
            if !side {
                parts.push(
                    view()
                        .flex_shrink_0()
                        .style_when(move || !stacked(), |s| s.hidden())
                        .child(term::hrule())
                        .into_element(),
                );
            }
        }
        let cell = view().flex_col().min_w(px(0.0)).min_h(px(0.0));
        let cell = match width.least() {
            None => cell.flex_1(),
            Some(points) => {
                let natural = fixed_cells(points);
                // Only a list alone beside its detail grows: next to another fixed column its
                // growth would come out of the detail's least width.
                let grows = at < main && fixed_count == 1;
                cell.w(move || {
                    term::cells(if grows {
                        term::list_columns(app, natural)
                    } else {
                        natural
                    })
                })
                .flex_shrink_0()
            }
        };
        let cell = if side {
            cell.style_when(stacked, |s| s.hidden())
        } else if at < main {
            cell.style_when(stacked, |s| {
                s.flex_shrink(1.0)
                    .max_h(relative(STACKED_LIST_MOST))
                    .w(relative(1.0))
            })
        } else {
            cell.style_when(stacked, |s| s.flex_1().w(relative(1.0)))
        };
        parts.push(cell.child(col).into_element());
    }
    view()
        .flex_1()
        .flex_row()
        .style_when(stacked, |s| s.flex_col())
        .min_w(px(0.0))
        .min_h(px(0.0))
        .children(parts)
        .into_element()
}

/// A fixed column's width in Cells: one for every [`POINTS_PER_COLUMN_CELL`] desktop points.
fn fixed_cells(points: f32) -> f32 {
    (points / POINTS_PER_COLUMN_CELL).round()
}

/// Desktop content kept to `max` points and centred in a wider row (the `MEASURE_*` tokens), so a
/// wide window does not stretch it edge to edge; a narrower row gives it the whole width. For
/// content that flows down a scrolling body; [`measure_fill`] is the form for content that
/// fills the height it is given. Phone and terminal content is returned as is.
pub fn measure(app: AppState, max: f32, content: AnyElement) -> AnyElement {
    match app.form {
        Form::Desktop => view()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .child(
                view()
                    .flex_col()
                    .w_full()
                    .min_w(px(0.0))
                    .max_w(px(max))
                    .child(content),
            )
            .into_element(),
        Form::Phone | Form::Terminal => content,
    }
}

/// [`measure`] for content that fills its column's height (a pane, columns of panes, a
/// conversation over its composer): the measured box grows to the height the page leaves.
pub fn measure_fill(app: AppState, max: f32, content: AnyElement) -> AnyElement {
    match app.form {
        Form::Desktop => view()
            .flex_1()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .items_center()
            .child(
                view()
                    .flex_1()
                    .flex_col()
                    .w_full()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .max_w(px(max))
                    .child(content),
            )
            .into_element(),
        Form::Phone | Form::Terminal => content,
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
    page_phone::follow_scroll(app.form, scroll())
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
    page_phone::follow_scroll(app.form, scroll())
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

/// The decision bar (`.decide`, `.pdecide`): the actions in a row and, on desktop and in a
/// terminal, an attribution at the trailing edge. Desktop: padding 12 24, 10 pt apart, a rule
/// above, the attribution right-aligned in 11.5 pt quiet text at most 430 pt wide. Phone: the
/// actions only (pass them `wide`), padding 10 14 12; on a short window (held sideways) 48 pt
/// tall, giving the rows the rest; the shell adds the home indicator's inset.
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
        Form::Desktop => desktop_bar_row(root, actions, attribution)
            .bg(color(Token::Bg))
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
            .into_element(),
        Form::Phone => root
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(14.0))
            .pt(px(10.0))
            .pb(px(12.0))
            .style_when(page_phone::short_window, |s| {
                s.pt(px(SHORT_BAR_PAD)).pb(px(SHORT_BAR_PAD))
            })
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

/// Which head a title leads: a page's, on one line, or a handover's, which wraps.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Title {
    Page,
    Handover,
}

/// The title of a head: bold, the header role. Desktop `.pagehead h1` (15 pt, one line) or
/// `.qhead h1` (19 pt, line height 1.3); phone `.plarge h1` (30 pt) or the `.pband` title
/// (20 pt, line height 1.25); terminal one line for a page, wrapping for a handover. The title
/// may be a fixed text or a memo or closure it follows.
pub(super) fn title_text(
    form: Form,
    title: impl IntoReactive<SharedString>,
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

/// Actions in a row at the trailing edge; in a terminal `gap` Cells apart.
pub(super) fn actions_row(form: Form, actions: Vec<AnyElement>, gap: f32) -> AnyElement {
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

/// The desktop head's sub line: one line that gives way before the title, cut with an ellipsis,
/// and gone once narrower than [`SUB_MIN_WIDTH`].
fn desktop_sub(content: String) -> AnyElement {
    let cramped = signal(false);
    text(content)
        .flex_shrink(GIVE_WAY_SHRINK)
        .min_w(px(0.0))
        .truncate()
        .font_family(FONT_TEXT)
        .text_size(px(12.0))
        .text_color(color(Token::Ink3))
        .on_layout(move |frame: &Bounds<Points>| {
            let short = frame.size.width.0 < SUB_MIN_WIDTH;
            if cramped.get_untracked() != short {
                cramped.set(short);
            }
        })
        .style_when(move || cramped.get(), |s| s.invisible())
        .into_element()
}

pub(super) fn spacer() -> AnyElement {
    view().flex_1().min_w(px(0.0)).into_element()
}
