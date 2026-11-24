//! The map's form on the phone and in a terminal: the level's nodes as rows in the map's order,
//! each with its overlay figure and a bar, and under the picked one its edges, each a row that
//! picks the edge (docs/plans/system-map.md §3.1 says why a list and not a smaller drawing).
//! Test ids: a node's row is its `node-…` id, the picked node's edges `edge-line-<n>` from 0.

use nsh_core::prelude::*;

use crate::model::system::Pick;
use crate::model::system::scene::{EdgeKind, Scene};
use crate::model::system::tree::{Line, lines};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::rows::{list_row, list_row_tall};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::Host;

/// Cells of a terminal row's bar.
const BAR_CELLS: usize = 8;

pub fn tree(app: AppState, scene: Memo<Option<Scene>>, host: Host) -> AnyElement {
    let rows = computed(move || {
        let state = host.state(app);
        let marks = host.marks();
        scene.with(|scene| {
            scene
                .as_ref()
                .map(|scene| lines(scene, &state, marks.as_ref()))
                .unwrap_or_default()
        })
    });
    view()
        .test_id("system-tree")
        .role(AccessibilityRole::List)
        .label("Map")
        .child(dynamic(move || {
            let rows = rows.get();
            untracked(|| {
                view()
                    // Edge rows count on their own, so an edge keeps its id as nodes come
                    // and go above it.
                    .children(rows.into_iter().scan(0, |edges, line| {
                        let n = *edges;
                        if matches!(line, Line::Edge(_)) {
                            *edges += 1;
                        }
                        Some(row(app, n, line, host))
                    }))
                    .into_element()
            })
        }))
        .into_element()
}

fn bar_cells(share: f32) -> String {
    let filled = (share * BAR_CELLS as f32).round() as usize;
    let filled = filled.min(BAR_CELLS);
    "■".repeat(filled) + &"·".repeat(BAR_CELLS - filled)
}

fn small(app: AppState, words: String, token: Token) -> AnyElement {
    let t = text(words).number_of_lines(1u32).text_color(color(token));
    match app.form {
        Form::Terminal => t.into_element(),
        _ => t
            .font_family(FONT_TEXT)
            .text_size(px(text_size(app.form) - 1.5))
            .into_element(),
    }
}

fn fixed(element: AnyElement) -> AnyElement {
    view().flex_shrink_0().child(element).into_element()
}

fn row(app: AppState, index: usize, line: Line, host: Host) -> AnyElement {
    let form = app.form;
    match line {
        Line::Section(title) => {
            let t = text(title).text_color(color(Token::Ink3));
            let t = if form == Form::Terminal {
                t.font_weight(FontWeight::BOLD)
            } else {
                t.font_family(FONT_TEXT)
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
            };
            view()
                .when(form == Form::Terminal, |v| {
                    v.pt(term::rows(1.0)).px(term::cells(1.0))
                })
                .when(form != Form::Terminal, |v| {
                    v.pt(px(14.0)).pb(px(4.0)).px(px(18.0))
                })
                .child(t)
                .into_element()
        }
        Line::Node(node) => {
            let pick = Pick::Node(node.node.clone());
            let picked = node.picked;
            let label = format!("{}, {}", node.name, node.figure);
            // A terminal row keeps the name whole on one line; the phone wraps a long one.
            let name = text(node.name.clone())
                .when(form == Form::Terminal, |t| t.number_of_lines(1u32))
                .text_color(color(if node.dim { Token::Ink3 } else { Token::Ink }))
                .font_weight(FontWeight::SEMIBOLD);
            let opens = if node.opens { " ›" } else { "" };
            let tag = node.tag.clone().unwrap_or_default();
            let press = move || host.pick(app, pick.clone());
            match form {
                Form::Terminal => list_row(
                    app,
                    label,
                    node.node.test_id(),
                    move || picked,
                    press,
                    // A terminal row is one line: the name keeps its whole width and everything
                    // after it gives way, the tag first, so a long name is never cut.
                    vec![
                        fixed(name.into_element()),
                        view()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(small(
                                app,
                                if node.bar {
                                    format!(" {}  {}  {tag}", node.figure, bar_cells(node.share))
                                } else {
                                    format!(" {}  {tag}", node.figure)
                                },
                                Token::Ink2,
                            ))
                            .into_element(),
                        fixed(small(app, opens.trim().to_owned(), Token::Ink3)),
                    ],
                ),
                Form::Desktop | Form::Phone => {
                    let bar = view()
                        .h(px(4.0))
                        .w_full()
                        .rounded(px(2.0))
                        .bg(color(Token::Rule))
                        .child(
                            view()
                                .h(px(4.0))
                                .w(relative(node.share.max(0.0)))
                                .rounded(px(2.0))
                                .bg(color(Token::Ink3)),
                        );
                    let head = view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(name.font_family(FONT_MONO).flex_1().min_w(px(0.0)))
                        .child(small(app, format!("{tag}{opens}"), Token::Ink3));
                    list_row_tall(
                        app,
                        label,
                        node.node.test_id(),
                        move || picked,
                        press,
                        vec![
                            view()
                                .flex_1()
                                .min_w(px(0.0))
                                .gap(px(3.0))
                                .child(head)
                                .child(small(app, node.figure, Token::Ink2))
                                .children(node.bar.then_some(bar))
                                .into_element(),
                        ],
                    )
                }
            }
        }
        Line::Edge(edge) => {
            let ink = match edge.kind {
                Some(EdgeKind::ReferencedOnly) => hue_ink(Hue::Held),
                Some(EdgeKind::DeclaredOnly) => color(Token::Ink3),
                Some(EdgeKind::Starts) => hue_ink(Hue::Worker),
                _ => color(Token::Ink2),
            };
            let label = format!("{} {}: {}", edge.arrow, edge.name, edge.words);
            let picked = edge.picked;
            let content = vec![
                view()
                    .w(if form == Form::Terminal {
                        term::cells(4.0)
                    } else {
                        px(28.0)
                    })
                    .flex_shrink_0()
                    .child(text(format!("  {}", edge.arrow)).text_color(color(Token::Ink3)))
                    .into_element(),
                // The other end's name stays whole; the words after it give way.
                view()
                    .flex_shrink_0()
                    .child(
                        text(edge.name.clone())
                            .number_of_lines(1u32)
                            .text_color(color(Token::Ink))
                            .when(form != Form::Terminal, |t| {
                                t.font_family(FONT_MONO).text_size(px(12.5))
                            }),
                    )
                    .into_element(),
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .items_end()
                    .child(
                        text(edge.words.clone())
                            .number_of_lines(1u32)
                            .text_color(ink)
                            .when(form != Form::Terminal, |t| {
                                t.font_family(FONT_TEXT).text_size(px(12.0))
                            }),
                    )
                    .into_element(),
            ];
            match edge.pick {
                Some(pick) => list_row(
                    app,
                    label,
                    format!("edge-line-{index}"),
                    move || picked,
                    move || host.pick(app, pick.clone()),
                    content,
                ),
                None => view()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .when(form == Form::Terminal, |v| v.px(term::cells(1.0)))
                    .when(form != Form::Terminal, |v| v.px(px(16.0)).py(px(4.0)))
                    .accessible(true)
                    .label(label)
                    .children(content)
                    .into_element(),
            }
        }
    }
}
