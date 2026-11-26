//! A workflow recipe artifact drawn as a diagram: on desktop and web a first-party WebView
//! (bundled AntV G6, no network) over a list of every element, which every form shows and the
//! keyboard walks; beside or under it the selected element's inspector. The WebView and the list
//! share one selection, which Rust accepts only when the open recipe holds the ID
//! (`model::artifacts::diagram`). The WebView gets IDs and plain labels; source, facts and
//! navigation stay here.
//!
//! test ids: "diagram", "diagram-webview", "diagram-status", "diagram-el-<id>" (a list row),
//! "diagram-inspector", "diagram-source", "diagram-derived".

use std::time::Duration;

use nsh_core::prelude::*;

use crate::model::artifacts::diagram::{
    self, AUTHORED_NOTE, DERIVED_HEAD, DERIVED_NOTE, Entry, Evidence, Graph, Inspector, LIST_HINT,
    NO_VIEWER, VIEWER_FAILED, VIEWER_OPENING, VIEWER_READY,
};
use crate::model::artifacts::{Action, LOADING_CONTENT};
use crate::model::{Form, Hue};
use crate::screens::system::lenses::source as source_lines;
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::rows::list_row;
use crate::ui::kit::text::{dim, h3, label_value, plain};
use crate::ui::tokens::{Token, color};

const G6: &str = include_str!("diagram/g6.min.js");
const VIEW: &str = include_str!("diagram/view.html");
/// The graph's height on desktop and web.
const GRAPH_HEIGHT: f32 = 360.0;
/// From this page width the list and the inspector stand side by side under the graph.
const SIDE_BY_SIDE: f32 = 760.0;
const LIST_WIDTH: f32 = 300.0;
/// How long a pending element waits before it is asked for again.
const RETRY: Duration = Duration::from_millis(1500);

/// The diagram of the artifact `graph` names; the graph is drawn once per recipe, and only the
/// selection moves after that.
pub(super) fn diagram(app: AppState, graph: Memo<Option<Graph>>) -> AnyElement {
    let structure = computed(move || {
        graph.with(|g| {
            g.as_ref()
                .map(|g| (g.reference.clone(), g.payload.clone(), diagram::ids(g)))
        })
    });
    let selected = computed(move || graph.with(|g| g.as_ref().and_then(|g| g.selected.clone())));
    let marks = computed(move || {
        graph.with(|g| {
            crate::model::artifacts::diagram_compare::marks_message(
                g.as_ref().and_then(|g| g.compare.as_ref()),
            )
        })
    });
    retry_pending(app, graph);
    let picture = dynamic(move || {
        let (reference, payload, ids) = structure.get()?;
        Some(untracked(|| {
            if app.form == Form::Desktop {
                viewer(app, reference, payload, ids, selected, marks)
            } else {
                dim(app, NO_VIEWER).into_element()
            }
        }))
    });
    let list = list(app, graph);
    let inspector = inspector(app, graph);
    let wide = app.form != Form::Phone;
    view()
        .test_id("diagram")
        .gap(px(if app.form == Form::Terminal {
            0.0
        } else {
            12.0
        }))
        .child(picture)
        .child(super::diagram_compare::bar(app, graph))
        .child(
            view()
                .when(wide, |row| row.flex_row().flex_wrap())
                .gap(px(if app.form == Form::Terminal {
                    2.0
                } else {
                    16.0
                }))
                .child(
                    view()
                        .when(wide, |col| col.w(px(LIST_WIDTH)).flex_shrink_0())
                        .child(list),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(if wide { SIDE_BY_SIDE - LIST_WIDTH } else { 0.0 }))
                        .child(inspector),
                ),
        )
        .into_element()
}

/// Asks again for the selected element while corrald answers that its facts are pending.
fn retry_pending(app: AppState, graph: Memo<Option<Graph>>) {
    let wanted = computed(move || {
        graph.with(|g| {
            g.as_ref()
                .filter(|g| diagram::pending(g))
                .and_then(diagram::selected_target)
        })
    });
    // corrald may answer pending again, which leaves `wanted` unchanged; each ask bumps `asked`
    // so the effect runs again and waits for the next one, for as long as it stays pending.
    let asked = signal(0u32);
    effect(move || {
        asked.get();
        if let Some(target) = wanted.get() {
            // The timer ends with this run of the effect, so a selection that moved on or a
            // view that closed never asks again.
            set_timeout(RETRY, move || {
                if wanted.get_untracked().as_ref() == Some(&target) {
                    app.refresh(target.clone());
                    asked.update(|n| *n = n.wrapping_add(1));
                }
            });
        }
    });
}

/// The first-party WebView: the bundled G6 page with only the recipe's IDs and labels. A
/// message selects only an ID this recipe holds; the selection is posted back as it moves.
fn viewer(
    app: AppState,
    reference: String,
    payload: String,
    ids: Vec<String>,
    selected: Memo<Option<String>>,
    marks: Memo<String>,
) -> AnyElement {
    // The data's marker comes before the bundle's in the page, and each is replaced once, so a
    // label can name neither.
    let page = VIEW
        .replacen("/*G6-BUNDLE*/", G6, 1)
        .replacen("<!--DATA-->", &payload, 1);
    let handle = element_ref();
    let ready = signal(false);
    let status = signal(VIEWER_OPENING.to_owned());
    let to_viewer = handle;
    effect(move || {
        let selection = selected.get();
        if ready.get() {
            to_viewer.post_message(diagram::selection_message(selection.as_deref()));
        }
    });
    // How each element's source changed in the chosen comparison; an empty set clears them.
    effect(move || {
        let marks = marks.get();
        if ready.get() {
            to_viewer.post_message(marks);
        }
    });
    view()
        .gap(px(6.0))
        .child(dynamic(move || {
            dim(app, status.get())
                .test_id("diagram-status")
                .into_element()
        }))
        .child(
            view()
                .rounded(px(6.0))
                .border_1()
                .border_color(color(Token::Rule))
                .overflow_hidden()
                .child(
                    web_view_html(page, None)
                        .element_ref(&handle)
                        .test_id("diagram-webview")
                        .label("Workflow diagram")
                        .w_full()
                        .h(px(GRAPH_HEIGHT))
                        .scrollable(false)
                        .on_message(move |message: &str| match message {
                            "ready" => {
                                status.set(VIEWER_READY.into());
                                ready.set(true);
                            }
                            "failed" => status.set(VIEWER_FAILED.into()),
                            message => {
                                if let Some(element) = diagram::picked(message, &ids) {
                                    app.artifacts_act(Action::Pick {
                                        reference: reference.clone(),
                                        element,
                                    });
                                }
                            }
                        })
                        .on_error(move |why: &str| {
                            status.set(format!("{NO_VIEWER} ({why})"));
                        }),
                ),
        )
        .into_element()
}

/// Every element, steps then links, each a row the keyboard reaches; a press selects it.
fn list(app: AppState, graph: Memo<Option<Graph>>) -> AnyElement {
    // The rows are made again only when the recipe changes, not as the selection or its
    // evidence moves, so a row keeps the keyboard focus after it is pressed.
    let entries = computed(move || {
        graph.with(|g| {
            g.as_ref()
                .map(|g| (g.reference.clone(), g.nodes.clone(), g.edges.clone()))
        })
    });
    dynamic(move || {
        let (reference, nodes, edges) = entries.get()?;
        Some(untracked(|| {
            let rows = |entries: Vec<Entry>| -> Vec<AnyElement> {
                entries
                    .into_iter()
                    .map(|entry| row(app, graph, reference.clone(), entry))
                    .collect()
            };
            let (steps, links) = (nodes.len(), edges.len());
            view()
                .child(h3(app, "Steps", Some(steps.to_string())))
                .children(rows(nodes))
                .child(h3(app, "Links", Some(links.to_string())))
                .children(rows(edges))
                .into_element()
        }))
    })
    .into_element()
}

fn row(app: AppState, graph: Memo<Option<Graph>>, reference: String, entry: Entry) -> AnyElement {
    let id = entry.id.clone();
    let chosen = id.clone();
    list_row(
        app,
        format!("{}, {}", entry.words, entry.sub),
        format!("diagram-el-{id}"),
        move || graph.with(|g| g.as_ref().and_then(|g| g.selected.as_deref()) == Some(&chosen)),
        move || {
            app.artifacts_act(Action::Pick {
                reference: reference.clone(),
                element: id.clone(),
            })
        },
        vec![
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .gap(px(8.0))
                .child(plain(app, entry.words).number_of_lines(1u32))
                .child(dim(app, entry.sub).number_of_lines(1u32))
                .into_element(),
        ],
    )
}

/// The selected element: what the recipe states about it, then the committed source and what
/// Corral found, each said apart from what the recipe authored.
fn inspector(app: AppState, graph: Memo<Option<Graph>>) -> AnyElement {
    let shown = computed(move || {
        graph.with(|g| {
            g.as_ref().map(|g| {
                (
                    g.pinned.clone(),
                    g.inspector.clone(),
                    g.compare.as_ref().and_then(|c| c.side.clone()),
                )
            })
        })
    });
    dynamic(move || {
        let (pinned, inspector, side) = shown.get()?;
        Some(untracked(|| match inspector {
            None => view()
                .gap(px(6.0))
                .child(dim(app, format!("Pinned at {pinned}.")))
                .child(dim(app, LIST_HINT))
                .into_element(),
            Some(i) => view()
                .gap(px(if app.form == Form::Terminal {
                    0.0
                } else {
                    12.0
                }))
                .child(inspected(app, i))
                .children(side.map(|side| super::diagram_compare::side(app, side)))
                .into_element(),
        }))
    })
    .into_element()
}

fn inspected(app: AppState, i: Inspector) -> AnyElement {
    let gap = if app.form == Form::Terminal { 0.0 } else { 8.0 };
    let head = view()
        .gap(px(2.0))
        .child(dim(app, i.what))
        .child(plain(app, i.title).font_weight(FontWeight::BOLD))
        .child(dim(app, AUTHORED_NOTE));
    let facts = i
        .facts
        .into_iter()
        .map(|(label, value)| label_value(app, label, plain(app, value).into_element()));
    let evidence = match i.state {
        Evidence::Loading => vec![dim(app, LOADING_CONTENT).into_element()],
        Evidence::Failed(why) => vec![dim(app, why).text_color(hue_ink(Hue::Fail)).into_element()],
        Evidence::Pending { source, note } => vec![
            h3(app, "Source", None),
            source_block(app, source.ok_or_else(|| note.clone()), "diagram-source"),
            h3(app, DERIVED_HEAD, None),
            dim(app, note).test_id("diagram-derived").into_element(),
        ],
        Evidence::Ready {
            source,
            derived,
            basis,
        } => vec![
            h3(app, "Source", None),
            source_block(app, source, "diagram-source"),
            h3(app, DERIVED_HEAD, None),
            view()
                .test_id("diagram-derived")
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 4.0 }))
                .children((!derived.is_empty()).then(|| dim(app, DERIVED_NOTE)))
                .children(
                    derived
                        .into_iter()
                        .map(|line| plain(app, line).into_element()),
                )
                .child(dim(app, basis))
                .into_element(),
        ],
    };
    view()
        .test_id("diagram-inspector")
        .gap(px(gap))
        .child(head)
        .children(facts)
        .children(evidence)
        .into_element()
}

pub(super) fn source_block(
    app: AppState,
    source: Result<crate::model::system::lens::Source, String>,
    id: &'static str,
) -> AnyElement {
    match source {
        Ok(source) => source_lines(app, Some(source), id, ""),
        Err(why) => view().test_id(id).child(dim(app, why)).into_element(),
    }
}
