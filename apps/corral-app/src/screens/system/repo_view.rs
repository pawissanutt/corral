//! Repository recipe adapter for the bundled graph viewer. Only display primitives leave Rust.
use super::lens_canvas::OnPick;
use crate::model::Form;
use crate::model::system::lens::{Diagram, Mark};
use crate::state::AppState;
use corral_types::lenses::RecipeLayout;
use nsh_core::prelude::*;
use serde_json::json;

fn payload(d: &Diagram) -> String {
    let nodes: Vec<_> = d
        .nodes
        .iter()
        .map(|n| json!({"id":n.name,"label":n.label,"group":n.group}))
        .collect();
    let groups: Vec<_> = d
        .about
        .iter()
        .map(|g| json!({"id":g.name,"label":g.label}))
        .collect();
    let edges: Vec<_> = d.edges.iter().enumerate().filter_map(|(i, e)| {
        let (a, b) = e.ends?;
        // Opposing edges must bend to opposite sides in graph coordinates.
        let parallel = |p: &(usize, usize)| *p == (a, b) || *p == (b, a);
        let count = d.edges.iter().filter_map(|e| e.ends).filter(parallel).count();
        let before = d.edges[..i].iter().filter_map(|e| e.ends).filter(parallel).count();
        let bow = (before as f32 - (count as f32 - 1.0) / 2.0) * 64.0 * if a <= b {1.0} else {-1.0};
        let endpoint = |key| e.authored.iter().find(|(k,_)| *k == key).map(|(_,v)| v.as_str());
        Some(json!({
            "id": e.name, "source": d.nodes.get(a)?.name, "target": d.nodes.get(b)?.name,
            "label": e.label, "fromLabel": endpoint("Source label"), "toLabel": endpoint("Target label"),
            "bow": bow, "loop": before
        }))
    }).collect();
    json!({
        "v": 1, "ordered": d.kind == RecipeLayout::OrderedLanes,
        "groups": groups, "nodes": nodes, "edges": edges
    })
    .to_string()
    .replace('<', "\\u003c")
}

pub(super) fn picture(
    app: AppState,
    shown: Memo<Option<Diagram>>,
    picked: Memo<Option<String>>,
    on_pick: OnPick,
) -> AnyElement {
    let list = super::lenses::listing(app, shown, picked, on_pick.clone());
    if app.form != Form::Desktop {
        return list;
    }
    let marks = computed(move || {
        let mut marks = serde_json::Map::new();
        shown.with(|d| {
            if let Some(d) = d {
                for e in d.elements() {
                    if let Some(m) = e.marked() {
                        marks.insert(
                            e.name.clone(),
                            json!(match m {
                                Mark::Added => "added",
                                Mark::Removed => "removed",
                                Mark::Failed => "failed",
                                Mark::Extra => "unknown",
                                Mark::Touched => "changed",
                            }),
                        );
                    }
                }
            }
        });
        json!({"v":1,"marks":marks}).to_string()
    });
    view()
        .child(dynamic(move || {
            let d = shown.get()?;
            Some(untracked(|| {
                crate::screens::artifacts::diagram::viewer(
                    app,
                    payload(&d),
                    d.elements().map(|e| e.name.clone()).collect(),
                    picked,
                    marks,
                    on_pick.clone(),
                )
            }))
        }))
        .child(list)
        .into_element()
}
