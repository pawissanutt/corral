//! Home on a wide terminal (the desktop draws the owner's board, `board`): a dashboard, not a second Tasks list. A row of six numbers, then two
//! halves: what waits on the owner as cards on the left, what Corral is doing on the right.
//! Ask's composer is docked under it. The halves sit side by side while both keep their least
//! width, else one above the other; on the right Working spans the half and the other panels
//! share rows. A desktop keeps the page to [`MEASURE_PAGE`], centred, so a wide window gives
//! two panels a row and no tile grows into a card.

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::home::{self, Tile};
use crate::model::projects::Scope;
use crate::model::{Form, Overlay};
use crate::screens::agents::ask_view;
use crate::state::AppState;
use crate::ui::composer::docked_composer;
use crate::ui::kit::board_status::{board_status, dim_stale};
use crate::ui::kit::page;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, MEASURE_PAGE, Token, color};

use super::parts::ask_project;
use super::{cards, panels};

/// A half's least width: desktop points, terminal Cells.
const HALF: f32 = 440.0;
const HALF_CELLS: f32 = 48.0;
/// A right-hand panel's least width beside another.
const PANEL: f32 = 270.0;
/// A tile's width: it grows from the least to share the row, never past the most.
const TILE: f32 = 150.0;
const TILE_MOST: f32 = 240.0;

/// The dashboard with its head and the docked composer.
pub(super) fn dashboard(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let form = app.form;
    let said = computed(move || {
        let scope = scope.get();
        (
            home::title(&scope),
            app.feed.with(|feed| home::dash_summary(feed, &scope)),
            scope,
        )
    });
    let head = dynamic(move || {
        let (title, summary, scope) = said.get();
        untracked(move || {
            let actions = ask_project(app, &scope).into_iter().collect();
            page::measured_head(app, MEASURE_PAGE, title, Some(summary), actions)
        })
    });
    let half = |child: AnyElement| {
        view()
            .flex_grow(1.0)
            .w(match form {
                Form::Terminal => term::cells(HALF_CELLS),
                Form::Desktop | Form::Phone => px(HALF),
            })
            .min_w(px(0.0))
            .child(child)
            .into_element()
    };
    let gap = px(if form == Form::Terminal { 0.0 } else { 12.0 });
    let side_panels = [
        panels::by_status(app, scope),
        panels::projects(app, scope),
        panels::cost(app),
        panels::events(app, scope),
        panels::finished(app, scope),
        panels::burnup(app, scope),
    ];
    let right = view()
        .gap(gap)
        .min_w(px(0.0))
        .child(panels::working(app, scope))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap(gap)
                .min_w(px(0.0))
                .children(side_panels.into_iter().map(|panel| {
                    view()
                        .flex_grow(1.0)
                        .w(match form {
                            // A terminal stacks them: each takes the half.
                            Form::Terminal => relative(1.0),
                            Form::Desktop | Form::Phone => px(PANEL).into(),
                        })
                        .min_w(px(0.0))
                        .child(panel)
                        .into_element()
                })),
        );
    let dash = view()
        .test_id("home-dashboard")
        .gap(gap)
        .min_w(px(0.0))
        .child(tiles(app, scope))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_start()
                .gap(gap)
                .min_w(px(0.0))
                .child(half(cards::needs_you(app, scope)))
                .child(half(right.into_element())),
        );
    let dash = match form {
        Form::Terminal => dash.px(term::cells(1.0)),
        Form::Desktop | Form::Phone => dash.px(px(20.0)).pt(px(14.0)).pb(px(20.0)),
    };
    view()
        .size_full()
        .flex_grow(1.0)
        .flex_col()
        .min_h(px(0.0))
        .bg(color(Token::Bg))
        .child(head)
        .child(page::measure(app, MEASURE_PAGE, board_status(app)))
        // The scroll spans the window, so its bar stays at the edge; what it scrolls is measured.
        .child(scroll().flex_1().min_h(px(0.0)).child(page::measure(
            app,
            MEASURE_PAGE,
            dim_stale(app, dash).into_element(),
        )))
        .child(composer(app))
        .into_element()
}

/// The six numbers, each opening the page that holds its detail.
fn tiles(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let form = app.form;
    let shown = computed(move || {
        let scope = scope.get();
        app.feed.with(|feed| {
            let since = home::since(feed, &scope);
            home::tiles(feed, &scope, &since, &home::cost(feed))
        })
    });
    dynamic(move || {
        let tiles = shown.get();
        untracked(move || {
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
                .min_w(px(0.0))
                .children(tiles.into_iter().map(|t| tile(app, t)).collect::<Vec<_>>())
                .into_element()
        })
    })
    .into_element()
}

/// One number with its label and the line under it; Usage draws its tiles with it too.
pub(crate) fn tile(app: AppState, tile: Tile) -> AnyElement {
    tile_frame(app, tile, false)
}

/// A number as a board widget: it fills the columns and the row height it is given.
pub(super) fn filled_tile(app: AppState, tile: Tile) -> AnyElement {
    tile_frame(app, tile, true)
}

fn tile_frame(app: AppState, tile: Tile, fill: bool) -> AnyElement {
    let form = app.form;
    let spoken = format!(
        "{}: {}, {}. {}",
        tile.label, tile.value, tile.sub, tile.hint
    );
    let ink = if tile.you { Token::You } else { Token::Ink };
    let value = text(tile.value)
        .number_of_lines(1u32)
        .flex_shrink_0()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(ink));
    let sub = text(tile.sub)
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .text_color(color(Token::Ink3));
    let frame = match form {
        // Two rows, no frame: a terminal draws no border around one row of words.
        Form::Terminal => view()
            .w(term::cells(17.0))
            .pr(term::cells(1.0))
            .child(
                text(tile.label)
                    .number_of_lines(1u32)
                    .truncate()
                    .min_w(px(0.0))
                    .text_color(color(Token::Ink3)),
            )
            .child(
                view()
                    .flex_row()
                    .gap(term::cells(1.0))
                    .min_w(px(0.0))
                    .child(value.font_weight(FontWeight::BOLD))
                    .child(sub),
            ),
        Form::Desktop | Form::Phone => view()
            .when(!fill, |frame| frame.w(px(TILE)).max_w(px(TILE_MOST)))
            .gap(px(1.0))
            .px(px(12.0))
            .pt(px(9.0))
            .pb(px(10.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(color(if tile.you { Token::You } else { Token::Rule }))
            .bg(color(Token::Surface))
            .child(
                text(tile.label.to_uppercase())
                    .number_of_lines(1u32)
                    .truncate()
                    .font_family(FONT_TEXT)
                    .text_size(px(10.5))
                    .letter_spacing(px(0.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color(Token::Ink4)),
            )
            .child(
                value
                    .font_family(FONT_MONO)
                    .tabular_nums()
                    .text_size(px(22.0))
                    .line_height(px(27.0)),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .min_w(px(0.0))
                    .children(tile.meter.map(meter))
                    .children((!tile.spark.is_empty()).then(|| spark(&tile.spark)))
                    .child(sub.font_family(FONT_TEXT).text_size(px(12.0))),
            ),
    };
    let frame = frame
        .test_id(&format!("stat-{}", tile.key))
        .flex_grow(1.0)
        .min_w(px(0.0))
        .label(spoken);
    match tile.go {
        Some(to) => frame
            .role(AccessibilityRole::Link)
            .cursor_pointer()
            .when(form != Form::Terminal, |frame| {
                frame.hover(|s| {
                    s.bg(color(Token::Surface2))
                        .border_color(color(Token::Rule2))
                })
            })
            .on_press(move || app.go(to))
            .into_element(),
        None => frame.into_element(),
    }
}

/// The running slots as one cell each, the used ones filled.
fn meter((used, limit): (u64, u64)) -> AnyElement {
    view()
        .flex_row()
        .flex_shrink_0()
        .gap(px(2.0))
        .accessibility_elements_hidden(true)
        .children((0..limit.min(12)).map(|at| {
            view()
                .w(px(6.0))
                .h(px(8.0))
                .rounded(px(1.0))
                .bg(color(if at < used {
                    Token::Worker
                } else {
                    Token::Rule2
                }))
                .into_element()
        }))
        .into_element()
}

/// The last days as small bars, against the costliest.
fn spark(days: &[f32]) -> AnyElement {
    view()
        .flex_row()
        .items_end()
        .flex_shrink_0()
        .gap(px(2.0))
        .h(px(14.0))
        .accessibility_elements_hidden(true)
        .children(days.iter().map(|&share| {
            view()
                .w(px(4.0))
                .h(relative(share.clamp(0.08, 1.0)))
                .rounded(px(1.0))
                .bg(color(Token::Ink4))
                .into_element()
        }))
        .into_element()
}

/// Ask's composer docked at the foot: the one chat session, with the project in scope as its
/// context. It takes the keyboard focus only when pressed or on `/`, so the cards keep the keys.
/// While Ask itself is open it steps out: Ask is the same composer, and one field holds the
/// draft.
///
/// Not on the desktop, where Ask is the side pane's first tab (`/`, Ctrl+J, the top bar).
pub(super) fn composer(app: AppState) -> AnyElement {
    if crate::model::dock::supported(app.form) {
        return view().into_element();
    }
    let keyed = computed(move || (!app.is_open(Overlay::Ask)).then(|| app.ask_id()));
    dynamic(move || {
        let id = keyed.get()?;
        Some(untracked(move || {
            let reference = DetailRef::Session { id: id.to_string() };
            if app.daemon() {
                app.load(reference);
            }
            let page = ask_view(app, id.clone());
            let composer = computed(move || page.with(|p| p.composer.clone()));
            // The rule runs edge to edge; the field keeps to the dashboard's measure.
            view()
                .test_id("home-composer")
                .flex_none()
                .when(app.form == Form::Desktop, |dock| {
                    dock.border_t_1().border_color(color(Token::Rule))
                })
                .child(page::measure(
                    app,
                    MEASURE_PAGE,
                    docked_composer(app, id, composer, computed(|| None)),
                ))
                .into_element()
        }))
    })
    .into_element()
}
