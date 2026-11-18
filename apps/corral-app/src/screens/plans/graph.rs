//! The plan graph: `plans::layout` drawn as the model places it. Where every layer fits the
//! measured width, member cards (`.mcard`) sit at the model's points over a canvas of cubic
//! dependency curves; otherwise the layers stack under their headings, the cards across the
//! width. In a terminal each member is one row of Cells. Every card carries its task's shared
//! Actions menu as its context menu.

use nsh_core::SvgPaint;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::Id;
use crate::model::plans::{self, Graph, GraphInput, Layer, Node};
use crate::model::work::Record;
use crate::screens::now::with_menu;
use crate::state::AppState;
use crate::ui::kit::glance::stage_rail;
use crate::ui::kit::hint::hint_on;
use crate::ui::kit::rows::list_row;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::list::revealing;
use super::{RING, press};

/// Edge stroke width (`.graph svg.edges path`).
const EDGE: f32 = 1.25;
/// Layer heading size (the stacked layers' heading).
const CAPTION: f32 = 10.5;
/// The phone's touch-target floor.
const TARGET: f32 = 44.0;

/// The graph `source` gives (members, edges and the selected member), measured to the width it
/// is given. A terminal takes no measure: its layout is the stacked one. The source is read
/// tracked, so a member press or a board change redraws the graph alone. Each member card has
/// test id `task-<id>`; a stacked member newly selected (`J`, `K`) scrolls into view.
pub(crate) fn drawing(
    app: AppState,
    source: impl Fn() -> (GraphInput, Option<Id>) + 'static,
) -> AnyElement {
    let revealed = stored(None::<Id>);
    if app.form == Form::Terminal {
        return view()
            .test_id("plan-graph")
            .child(dynamic(move || {
                let (members, selected) = source();
                let graph = plans::layout(&members, selected.as_deref(), 0.0);
                layers(app, &graph, revealed)
            }))
            .into_element();
    }
    let width = signal(None::<f32>);
    view()
        .test_id("plan-graph")
        .w_full()
        .min_w(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| width.set(Some(frame.size.width.0)))
        .child(dynamic(move || {
            width.get().map(|room| {
                let (members, selected) = source();
                let graph = plans::layout(&members, selected.as_deref(), room);
                if graph.fits {
                    placed(app, &graph)
                } else {
                    layers(app, &graph, revealed)
                }
            })
        }))
        .into_element()
}

/// Cards and curves at the model's coordinates (`.graph`).
fn placed(app: AppState, graph: &Graph) -> AnyElement {
    let painter = graph
        .edges
        .iter()
        .fold(
            Painter::new()
                .stroke_style_from_view(SvgPaint::None)
                .line_width(EDGE)
                .begin_path(),
            |p, edge| {
                p.move_to(edge.start.x, edge.start.y).bezier_curve_to(
                    point(px(edge.control1.x), px(edge.control1.y)),
                    point(px(edge.control2.x), px(edge.control2.y)),
                    point(px(edge.end.x), px(edge.end.y)),
                )
            },
        )
        .stroke();
    view()
        .relative()
        .flex_shrink_0()
        .w(px(graph.width))
        .h(px(graph.height))
        .stroke_color(color(Token::Rule2))
        .child(
            canvas(painter)
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .w(px(graph.width))
                .h(px(graph.height))
                .pointer_events(PointerEvents::None)
                .accessibility_elements_hidden(true),
        )
        .children(
            graph
                .nodes
                .iter()
                .map(|node| {
                    view()
                        .absolute()
                        .left(px(node.x))
                        .top(px(node.y))
                        .w(px(node.w))
                        .h(px(node.h))
                        .child(menu(
                            app,
                            node,
                            card(app, node).w(px(node.w)).h(px(node.h)).into_element(),
                        ))
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// A member card (`.mcard`): the id in mono and the rail with its stage named on the first
/// line, the title on the second; surface with a rule-2 edge and 8 pt corners, selected the
/// selection fill inside an ink edge, needs you the you ground inside a you edge. A button named
/// by id and title, with its selected state, whose hint is the member's now-phrase; test id
/// `task-<id>` whatever the press does.
fn card(app: AppState, node: &Node) -> View {
    let phone = app.form == Form::Phone;
    let action = node.action.clone();
    let edge = match (node.needs_you, node.selected) {
        (true, true) => color(Token::You),
        (true, false) => color(Token::You).opacity(0.6),
        (false, true) => color(Token::Ink),
        (false, false) => color(Token::Rule2),
    };
    let fill = color(match (node.needs_you, node.selected) {
        (true, _) => Token::YouBg,
        (false, true) => Token::Sel,
        (false, false) => Token::Surface,
    });
    let card = view()
        .justify_center()
        .gap(px(3.0))
        .min_w(px(0.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(8.0))
        .bg(fill)
        .border_1()
        .border_color(edge)
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(format!("{} {}", node.id, node.title))
        .accessibility_state(AccessibilityState {
            selected: node.selected,
            ..AccessibilityState::default()
        })
        .test_id(&format!("task-{}", node.id))
        .on_press(move || press(app, action.clone()))
        .when(!node.needs_you && !node.selected, |card| {
            card.hover(|s| s.border_color(color(Token::Ink4)))
        })
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(RING))
        })
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .min_w(px(0.0))
                .child(
                    text(node.id.to_string())
                        .flex_shrink_0()
                        .number_of_lines(1u32)
                        .font_family(FONT_MONO)
                        .tabular_nums()
                        .text_size(px(if phone { 13.0 } else { 12.0 }))
                        .line_height(px(18.0))
                        .text_color(color(Token::Ink3)),
                )
                .child(view().flex_1().min_w(px(0.0)))
                .child(stage_rail(app, &node.stage)),
        )
        .child(
            text(node.title.clone())
                .number_of_lines(1u32)
                .truncate()
                .min_w(px(0.0))
                .font_family(FONT_TEXT)
                .text_size(px(if phone { 15.0 } else { 13.0 }))
                .line_height(px(if phone { 20.0 } else { 18.0 }))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink)),
        );
    hint_on(app, card, node.stage.hint.clone())
}

/// Stacked layers (`.graph` at 900 pt and under): each layer's heading, 10 pt above all but the
/// first, then its cards across the width, 12 pt apart. Terminal: the heading quiet, then a row
/// of Cells per member.
fn layers(app: AppState, graph: &Graph, revealed: Stored<Option<Id>>) -> AnyElement {
    view()
        .w_full()
        .children(
            graph
                .layers
                .iter()
                .map(|layer| {
                    let members = graph
                        .nodes
                        .iter()
                        .filter(|node| layer.members.contains(&node.id))
                        .map(|node| stacked(app, node, revealed))
                        .collect::<Vec<_>>();
                    view()
                        .child(layer_heading(app, layer))
                        .child(
                            view()
                                .gap(px(if app.form == Form::Terminal {
                                    0.0
                                } else {
                                    12.0
                                }))
                                .children(members),
                        )
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}

fn layer_heading(app: AppState, layer: &Layer) -> AnyElement {
    if app.form == Form::Terminal {
        return view()
            .flex_row()
            .px(term::cells(1.0))
            .child(term::quiet(layer.label.clone()))
            .into_element();
    }
    text(layer.label.clone())
        .role(AccessibilityRole::Header)
        .mt(px(if layer.index > 0 { 14.0 } else { 0.0 }))
        .mb(px(6.0))
        .number_of_lines(1u32)
        .font_family(FONT_TEXT)
        .text_size(px(CAPTION + 1.5))
        .text_color(color(Token::Ink3))
        .into_element()
}

/// One member of a stacked layer: its card across the width (a phone card never under the
/// 44 pt floor), or in a terminal one row of Cells: id, title, the rail's glyphs with its stage
/// named; the now-phrase is the row's status line. The selected one is scrolled into view.
fn stacked(app: AppState, node: &Node, revealed: Stored<Option<Id>>) -> AnyElement {
    let drawn = stacked_member(app, node);
    if node.selected {
        revealing(node.id.clone(), revealed, drawn)
    } else {
        drawn
    }
}

fn stacked_member(app: AppState, node: &Node) -> AnyElement {
    if app.form != Form::Terminal {
        return menu(app, node, card(app, node).min_h(px(TARGET)).into_element());
    }
    let action = node.action.clone();
    let selected = node.selected;
    let row = list_row(
        app,
        format!("{} {}", node.id, node.title),
        format!("task-{}", node.id),
        move || selected,
        move || press(app, action.clone()),
        vec![
            view()
                .w(term::cells(6.0))
                .flex_shrink_0()
                .child(text(node.id.to_string()).text_color(color(Token::Ink3)))
                .into_element(),
            text(node.title.clone())
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_color(color(Token::Ink))
                .into_element(),
            stage_rail(app, &node.stage),
        ],
    );
    menu(
        app,
        node,
        hint_on(app, view().child(row), node.stage.hint.clone()).into_element(),
    )
}

/// A member's card with its task's Actions menu.
fn menu(app: AppState, node: &Node, card: AnyElement) -> AnyElement {
    with_menu(app, Record::Task(node.id.clone()), card)
}
