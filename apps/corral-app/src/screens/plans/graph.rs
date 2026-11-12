//! The plan graph: `plans::layout` drawn as the model places it. Where every layer fits the
//! measured width, cards sit at the model's points over a canvas of cubic dependency curves;
//! otherwise (and always in a terminal) the members stand in vertical layers, one row each.

use nsh_core::SvgPaint;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::plans::{self, CardGeometry, Graph, Layer, Node};
use crate::state::AppState;
use crate::ui::kit::marks::{dot, hue_ink, mark, rail};
use crate::ui::kit::rows::list_row;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::{RING, press, target_id};

/// The share of a line's size above its baseline: converts the model's text baselines (SVG
/// `y`) to the top of a text box whose line is as tall as its size.
const ASCENT: f32 = 0.8;
/// Edge stroke width (`.graph .edge`).
const EDGE: f32 = 1.3;
/// Layer caption size (`.layer-h`, the vertical layers' heading).
const CAPTION: f32 = 10.5;

/// The graph of `plan` with `selected` ringed, measured to the width it is given. A terminal
/// takes no measure: its layout is the vertical one.
pub(super) fn drawing(
    app: AppState,
    plan: &'static str,
    selected: impl Fn() -> &'static str + 'static,
) -> AnyElement {
    if app.form == Form::Terminal {
        return view()
            .test_id("plan-graph")
            .child(dynamic(move || {
                layers(
                    app,
                    &plans::layout(plans::plan(plan), selected(), 0.0, false),
                )
            }))
            .into_element();
    }
    let phone = app.form == Form::Phone;
    let width = signal(None::<f32>);
    view()
        .test_id("plan-graph")
        .w_full()
        .min_w(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| width.set(Some(frame.size.width.0)))
        .child(dynamic(move || {
            width.get().map(|room| {
                let graph = plans::layout(plans::plan(plan), selected(), room, phone);
                if graph.fits {
                    cards(app, &graph)
                } else {
                    layers(app, &graph)
                }
            })
        }))
        .into_element()
}

/// Cards and curves at the model's coordinates (`svg.graph`).
fn cards(app: AppState, graph: &Graph) -> AnyElement {
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
                .layers
                .iter()
                .map(|layer| layer_caption(layer))
                .collect::<Vec<_>>(),
        )
        .children(
            graph
                .nodes
                .iter()
                .map(|node| card(app, node))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// "Layer N" at its baseline, in faint spaced capitals.
fn layer_caption(layer: &Layer) -> AnyElement {
    text(layer.label.clone())
        .absolute()
        .left(px(layer.position.x))
        .top(px(layer.position.y - CAPTION * ASCENT))
        .number_of_lines(1u32)
        .uppercase()
        .font_family(FONT_TEXT)
        .text_size(px(CAPTION))
        .line_height(px(CAPTION))
        .letter_spacing(px(CAPTION * 0.06))
        .text_color(color(Token::Ink4))
        .accessibility_elements_hidden(true)
        .into_element()
}

/// One line of a card at its baseline, cut where the card or its rail begins.
fn card_line(
    content: String,
    origin: (f32, f32),
    right: f32,
    size: f32,
    mono: bool,
    weight: FontWeight,
    ink: AdaptiveColor,
) -> AnyElement {
    text(content)
        .absolute()
        .left(px(origin.0))
        .right(px(right))
        .top(px(origin.1 - size * ASCENT))
        .truncate()
        .font_family(if mono { FONT_MONO } else { FONT_TEXT })
        .text_size(px(size))
        .line_height(px(size))
        .font_weight(weight)
        .text_color(ink)
        .into_element()
}

/// The rail's four marks at the card's rail points; the running run stage in the worker
/// colour, as `.mk.active.w`.
fn card_rail(app: AppState, node: &Node, geometry: &CardGeometry) -> AnyElement {
    view()
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .accessibility_elements_hidden(true)
        .children(
            node.rail
                .iter()
                .zip(geometry.rail.iter())
                .enumerate()
                .map(|(stage, (state, at))| {
                    let hue = if stage == 0 && *state == Mark::Active {
                        Hue::Worker
                    } else {
                        Hue::Neutral
                    };
                    view()
                        .absolute()
                        .left(px(at.x))
                        .top(px(at.y))
                        .w(px(geometry.rail_size.x))
                        .h(px(geometry.rail_size.y))
                        .items_center()
                        .justify_center()
                        .child(mark(app, *state, hue))
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// A card (`.gnode`): surface with a rule-2 edge, 6 pt corners; selected a 1.6 pt ink ring;
/// needs you the you ground and edge. Id, title and now-phrase on the model's baselines, the
/// rail at its points. A button named by id and title, with its selected state.
fn card(app: AppState, node: &Node) -> AnyElement {
    let Some(geometry) = node.card.as_ref() else {
        return view().into_element();
    };
    let action = node.action;
    let edge = if node.needs_you {
        color(Token::You)
    } else if node.selected {
        color(Token::Ink)
    } else {
        color(Token::Rule2)
    };
    let fill = color(if node.needs_you {
        Token::YouBg
    } else {
        Token::Surface
    });
    let now_ink = match node.now_hue {
        Hue::Neutral => color(Token::Ink2),
        hue => hue_ink(hue),
    };
    let rail_left = geometry.rail.first().map_or(node.w, |at| at.x);
    let [id, title, now] = &node.lines;
    let [id_at, title_at, now_at] = &geometry.text;
    view()
        .absolute()
        .left(px(node.x))
        .top(px(node.y))
        .w(px(node.w))
        .h(px(node.h))
        .rounded(px(6.0))
        .bg(fill)
        .border(px(if node.selected { 1.6 } else { 1.0 }))
        .border_color(edge)
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(format!("{} {}", node.id, node.title))
        .accessibility_state(AccessibilityState {
            selected: node.selected,
            ..AccessibilityState::default()
        })
        .test_id(&target_id(action))
        .on_press(move || press(app, action))
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(RING))
        })
        .child(card_line(
            id.clone(),
            (id_at.x, id_at.y),
            node.w - rail_left + 4.0,
            11.0,
            true,
            FontWeight::NORMAL,
            color(Token::Ink3),
        ))
        .child(card_rail(app, node, geometry))
        .child(card_line(
            title.clone(),
            (title_at.x, title_at.y),
            title_at.x,
            12.5,
            false,
            FontWeight::SEMIBOLD,
            color(Token::Ink),
        ))
        .child(card_line(
            now.clone(),
            (now_at.x, now_at.y),
            now_at.x,
            11.5,
            false,
            if node.needs_you {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            },
            now_ink,
        ))
        .into_element()
}

/// Vertical layers (`.vlayers`): each layer's caption, 10 pt above all but the first, then a
/// row per member at the model's row height.
fn layers(app: AppState, graph: &Graph) -> AnyElement {
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
                        .map(|node| layer_row(app, node))
                        .collect::<Vec<_>>();
                    view()
                        .child(layer_heading(app, layer))
                        .children(members)
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
            .child(term::quiet(layer.label.clone()).uppercase())
            .into_element();
    }
    let phone = app.form == Form::Phone;
    text(layer.label.clone())
        .role(AccessibilityRole::Header)
        .mt(px(if layer.index > 0 { 10.0 } else { 0.0 }))
        .mb(px(4.0))
        .number_of_lines(1u32)
        .uppercase()
        .font_family(FONT_TEXT)
        .text_size(px(CAPTION))
        .line_height(px(CAPTION * if phone { 1.4 } else { 1.45 }))
        .letter_spacing(px(CAPTION * 0.07))
        .text_color(color(Token::Ink3))
        .into_element()
}

/// One member of a vertical layer: dot, id, full title, rail. Terminal: one line of Cells
/// through the kit's list row.
fn layer_row(app: AppState, node: &Node) -> AnyElement {
    let action = node.action;
    let selected = node.selected;
    let label = format!("{} {}", node.id, node.title);
    if app.form == Form::Terminal {
        return list_row(
            app,
            label,
            target_id(action),
            move || selected,
            move || press(app, action),
            vec![
                dot(app, node.hue),
                view()
                    .w(term::cells(6.0))
                    .flex_shrink_0()
                    .child(text(node.id).text_color(color(Token::Ink3)))
                    .into_element(),
                text(node.title)
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(color(Token::Ink))
                    .into_element(),
                rail(app, node.rail),
            ],
        );
    }
    let phone = app.form == Form::Phone;
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .h(px(node.h))
        .px(px(6.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .accessibility_state(AccessibilityState {
            selected,
            ..AccessibilityState::default()
        })
        .test_id(&target_id(action))
        .on_press(move || press(app, action))
        .when(selected, |row| row.bg(color(Token::Sel)))
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        })
        .child(view().w(px(12.0)).flex_shrink_0().child(dot(app, node.hue)))
        .child(
            view().w(px(46.0)).flex_shrink_0().child(
                text(node.id)
                    .number_of_lines(1u32)
                    .font_family(FONT_MONO)
                    .tabular_nums()
                    .text_size(px(if phone { 13.0 } else { 11.5 }))
                    .text_color(color(Token::Ink3)),
            ),
        )
        .child(
            text(node.title)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(if phone { 16.0 } else { 13.0 }))
                .text_color(color(Token::Ink)),
        )
        .child(rail(app, node.rail))
        .into_element()
}
