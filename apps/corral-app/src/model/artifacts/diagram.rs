//! A workflow recipe artifact as a diagram: corrald checks the complete artifact
//! (`artifact.diagram.v1`) and the app draws only what it served as `ready`. The graph, its list
//! and the inspector share one selection, a recipe element ID that Rust accepts only when the
//! open artifact's served recipe holds it. The viewer gets IDs and plain labels, never source or
//! commands; what an element's binding resolves to is corrald's element detail, read for the
//! selected ID at the digest of the recipe it came from.

use std::collections::BTreeMap;

use corral_protocol::app::ARTIFACT_DIAGRAM_V1;
use corral_protocol::detail::{
    ArtifactEncoding, DetailBody, DetailRef, DiagramCandidate, DiagramElementDetail,
    DiagramRelated, DiagramResult, EntityEvidence, EntityLocation, EntityResult, SourceResult,
};
use corral_protocol::diagram::{EdgeKind, MAX_RECIPE_BYTES, NodeKind, WorkflowRecipe};
use corral_types::{Artifact, ReadStatus, ReferenceBasis, ReferenceKind};
use serde_json::json;

use crate::model::daemon::{DetailCache, Feed};
use crate::model::system::lens::Source;

/// Bump with any change to the messages the viewer and the app exchange.
pub const BRIDGE_VERSION: u64 = 1;
/// How far apart, in points, edges between the same two nodes bow from each other.
const SPREAD: f32 = 64.0;

pub const LIST_HINT: &str = "Select a step or a link to read its evidence.";
pub const VIEWER_OPENING: &str = "Opening the diagram viewer…";
pub const VIEWER_READY: &str = "Click a step or a link; the list below shares the selection.";
pub const VIEWER_FAILED: &str =
    "The diagram viewer could not draw this recipe. The list below shows every element.";
/// Said where no graph is drawn (a phone or a terminal) or the WebView cannot run.
pub const NO_VIEWER: &str = "The interactive graph needs the desktop or web app; the list below shows every element and its evidence.";
pub const DERIVED_HEAD: &str = "Found by Corral";
pub const DERIVED_NOTE: &str = "Read from Corral's facts at the pinned commit, not from the recipe. File-level references, not callers.";
pub const AUTHORED_NOTE: &str = "Authored in the recipe; neither checked nor verified by Corral.";

/// The request that checks the whole artifact as a recipe.
pub fn target(reference: &str) -> DetailRef {
    DetailRef::ArtifactDiagram {
        reference: reference.to_owned(),
    }
}

/// The request for one selected element's evidence at the recipe's blob digest.
pub fn element_target(reference: &str, sha256: &str, element: &str) -> DetailRef {
    DetailRef::ArtifactDiagramElement {
        reference: reference.to_owned(),
        sha256: sha256.to_owned(),
        element: element.to_owned(),
    }
}

/// Whether to ask corrald to check the artifact as a recipe: it grants the capability, the
/// whole artifact fits a recipe's bound, and its served head starts as a JSON object does. Only
/// corrald decides; this keeps the app from asking about every artifact it shows.
pub fn wanted(feed: &Feed, artifact: &Artifact) -> bool {
    if !feed.capable(ARTIFACT_DIAGRAM_V1) || artifact.bytes > MAX_RECIPE_BYTES as u64 {
        return false;
    }
    let Some(DetailBody::Artifact(page)) = feed
        .detail(&super::content_target(&artifact.reference()))
        .and_then(DetailCache::body)
    else {
        return false;
    };
    page.encoding == ArtifactEncoding::Text
        && page
            .content
            .trim_start_matches('\u{feff}')
            .trim_start()
            .starts_with('{')
}

/// One element of the list: its recipe ID and what the row says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub words: String,
    /// Under the words: the kind, and whether the recipe binds evidence to it.
    pub sub: String,
}

/// A recipe served `ready`, as every form shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    pub reference: String,
    /// The catalog digest the recipe was checked at; element reads are bound to it.
    pub sha256: String,
    /// `corral @ 9f9f9314d3fc`.
    pub pinned: String,
    pub nodes: Vec<Entry>,
    pub edges: Vec<Entry>,
    /// What the viewer draws, as JSON: IDs, plain labels, kinds, ends and each edge's bow.
    pub payload: String,
    /// The selected element, only when this recipe holds it.
    pub selected: Option<String>,
    pub inspector: Option<Inspector>,
    /// The comparison with a later commit, while one is chosen.
    pub compare: Option<super::diagram_compare::View>,
    /// corrald can compare this recipe with a later commit.
    pub can_compare: bool,
}

/// What a recipe artifact shows in its content's place.
#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    Graph(Box<Graph>),
    /// A recipe corrald refused: why, over the raw text.
    Refused(String),
}

/// The served recipe for `reference` and the digest it was checked at, when corrald found one.
fn recipe<'a>(feed: &'a Feed, reference: &str) -> Option<(&'a WorkflowRecipe, &'a str)> {
    match feed.detail(&target(reference)).and_then(DetailCache::body) {
        Some(DetailBody::ArtifactDiagram(detail)) => match &detail.result {
            DiagramResult::Ready { recipe } => Some((recipe, detail.artifact.sha256.as_str())),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `element` names a node or an edge of the recipe served for `reference`.
pub fn holds(feed: &Feed, reference: &str, element: &str) -> bool {
    recipe(feed, reference).is_some_and(|(recipe, _)| {
        recipe.nodes.iter().any(|node| node.id == element)
            || recipe.edges.iter().any(|edge| edge.id == element)
    })
}

/// The selected element named in a viewer message, when the message is this bridge's and the
/// open recipe holds the ID; anything else is dropped.
pub fn picked(message: &str, ids: &[String]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(message).ok()?;
    if value.get("v")?.as_u64()? != BRIDGE_VERSION {
        return None;
    }
    let id = value.get("pick")?.as_str()?;
    ids.iter().any(|known| known == id).then(|| id.to_owned())
}

/// The message that tells the viewer which element is selected.
pub fn selection_message(selected: Option<&str>) -> String {
    json!({"v": BRIDGE_VERSION, "selection": selected}).to_string()
}

/// What the artifact shows when it is a recipe: the graph, or corrald's refusal; none while
/// corrald has not answered, or for an artifact that is not a recipe.
pub fn shown(feed: &Feed, reference: &str, selected: Option<&str>) -> Option<Shown> {
    let detail = match feed.detail(&target(reference)).and_then(DetailCache::body) {
        Some(DetailBody::ArtifactDiagram(detail)) => detail,
        _ => return None,
    };
    let recipe = match &detail.result {
        DiagramResult::NotRecipe => return None,
        DiagramResult::Invalid { diagnostic } => {
            return Some(Shown::Refused(format!(
                "corrald refused this workflow recipe ({:?} at {}): {}. Its text shows as written.",
                diagnostic.code,
                if diagnostic.at.is_empty() {
                    "the document"
                } else {
                    diagnostic.at.as_str()
                },
                diagnostic.message
            )));
        }
        DiagramResult::Ready { recipe } => recipe,
    };
    let sha256 = detail.artifact.sha256.clone();
    let selected = selected
        .filter(|id| holds(feed, reference, id))
        .map(str::to_owned);
    let inspector = selected
        .as_deref()
        .map(|id| inspect(feed, recipe, reference, &sha256, id));
    Some(Shown::Graph(Box::new(Graph {
        reference: reference.to_owned(),
        pinned: format!(
            "{} @ {}",
            recipe.subject.project,
            short(&recipe.subject.commit)
        ),
        nodes: recipe.nodes.iter().map(node_entry).collect(),
        edges: recipe
            .edges
            .iter()
            .map(|edge| edge_entry(recipe, edge))
            .collect(),
        payload: payload(recipe),
        sha256,
        selected,
        inspector,
        compare: None,
        can_compare: false,
    })))
}

/// For an artifact's preview: the recipe check to keep loaded, and the graph to draw in its
/// content's place. A refused recipe keeps its text, with corrald's diagnostic put over it.
pub fn of(
    feed: &Feed,
    artifact: &Artifact,
    content: &mut super::Content,
    pick: Option<&(String, String)>,
    compare: Option<&super::diagram_compare::Compare>,
) -> (Option<DetailRef>, Option<Graph>) {
    let reference = artifact.reference();
    let picked = pick
        .filter(|(at, _)| *at == reference)
        .map(|(_, element)| element.as_str());
    let graph = match shown(feed, &reference, picked) {
        Some(Shown::Graph(graph)) => Some(compared(feed, *graph, compare)),
        Some(Shown::Refused(why)) => {
            content.cut = Some(match content.cut.take() {
                Some(cut) => format!("{why} {cut}"),
                None => why,
            });
            None
        }
        None => None,
    };
    (wanted(feed, artifact).then(|| target(&reference)), graph)
}

/// The graph with its comparison, when one is chosen for it and corrald can compare: each
/// compared element's row says how its source changed.
fn compared(
    feed: &Feed,
    mut graph: Graph,
    compare: Option<&super::diagram_compare::Compare>,
) -> Graph {
    use super::diagram_compare as cmp;
    graph.can_compare = cmp::capable(feed);
    let Some(compare) = compare.filter(|c| c.reference == graph.reference && cmp::capable(feed))
    else {
        return graph;
    };
    let view = cmp::view(feed, &graph, compare);
    for entry in graph.nodes.iter_mut().chain(graph.edges.iter_mut()) {
        if let Some(mark) = cmp::mark_of(&view, &entry.id) {
            entry.sub = format!("{} · {}", entry.sub, mark.word());
        }
    }
    graph.compare = Some(view);
    graph
}

pub(super) fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

fn node_kind(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Step => "step",
        NodeKind::Decision => "decision",
        NodeKind::End => "end",
    }
}

fn edge_kind(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Next => "next",
        EdgeKind::Branch => "branch",
    }
}

fn bound(bindings: Option<&corral_protocol::diagram::DiagramBindings>) -> &'static str {
    match bindings {
        Some(b) if b.source.is_some() || b.entity.is_some() => " · evidence bound",
        _ => "",
    }
}

fn node_entry(node: &corral_protocol::diagram::WorkflowNode) -> Entry {
    Entry {
        id: node.id.clone(),
        words: node.label.clone(),
        sub: format!("{}{}", node_kind(node.kind), bound(node.bindings.as_ref())),
    }
}

fn label_of<'a>(recipe: &'a WorkflowRecipe, id: &'a str) -> &'a str {
    recipe
        .nodes
        .iter()
        .find(|node| node.id == id)
        .map_or(id, |node| node.label.as_str())
}

fn edge_words(recipe: &WorkflowRecipe, edge: &corral_protocol::diagram::WorkflowEdge) -> String {
    let ends = format!(
        "{} → {}",
        label_of(recipe, &edge.from),
        label_of(recipe, &edge.to)
    );
    match &edge.label {
        Some(label) => format!("{ends} · {label}"),
        None => ends,
    }
}

fn edge_entry(recipe: &WorkflowRecipe, edge: &corral_protocol::diagram::WorkflowEdge) -> Entry {
    Entry {
        id: edge.id.clone(),
        words: edge_words(recipe, edge),
        sub: format!(
            "{} link{}",
            edge_kind(edge.kind),
            bound(edge.bindings.as_ref())
        ),
    }
}

/// Each edge's bow, in recipe order: edges between the same two nodes, either way, spread
/// evenly about the straight line so none hides another; a lone edge is straight. The viewer
/// bends an edge to its own left for a positive bow, so an edge running the other way between
/// the pair takes the opposite sign to land on its own side.
pub fn bows(recipe: &WorkflowRecipe) -> Vec<f32> {
    let mut pairs: BTreeMap<(&str, &str), Vec<usize>> = BTreeMap::new();
    for (at, edge) in recipe.edges.iter().enumerate() {
        let pair = if edge.from <= edge.to {
            (edge.from.as_str(), edge.to.as_str())
        } else {
            (edge.to.as_str(), edge.from.as_str())
        };
        pairs.entry(pair).or_default().push(at);
    }
    let mut bows = vec![0.0; recipe.edges.len()];
    for ((low, _), members) in pairs {
        let middle = (members.len() as f32 - 1.0) / 2.0;
        for (k, at) in members.into_iter().enumerate() {
            let place = (k as f32 - middle) * SPREAD;
            let forward = recipe.edges[at].from == low;
            bows[at] = if forward { place } else { -place };
        }
    }
    bows
}

/// Which edges run back against the flow: walking depth first from `start`, edges in recipe
/// order, an edge into a node still being walked (a retry, a loop, a self link). The viewer
/// lays such an edge out from its target so the flow reads from its start, and still points it
/// at its own target.
pub fn back_edges(recipe: &WorkflowRecipe) -> Vec<bool> {
    let at = |id: &str| recipe.nodes.iter().position(|node| node.id == id);
    let mut back = vec![false; recipe.edges.len()];
    // 0 unseen, 1 on the walk, 2 done.
    let mut state = vec![0u8; recipe.nodes.len()];
    let Some(start) = at(&recipe.start) else {
        return back;
    };
    // Each frame: a node and the next of its edges (in recipe order) to follow.
    let mut stack = vec![(start, 0usize)];
    state[start] = 1;
    while let Some((node, next)) = stack.last_mut() {
        let from = recipe.nodes[*node].id.as_str();
        let found = recipe.edges[*next..]
            .iter()
            .position(|edge| edge.from == from)
            .map(|offset| *next + offset);
        let Some(edge) = found else {
            state[*node] = 2;
            stack.pop();
            continue;
        };
        *next = edge + 1;
        let Some(to) = at(&recipe.edges[edge].to) else {
            continue;
        };
        match state[to] {
            0 => {
                state[to] = 1;
                stack.push((to, 0));
            }
            // A self link has no direction to reverse; its loop is placed apart instead.
            1 => back[edge] = to != *node,
            _ => {}
        }
    }
    back
}

/// Each self link's place among its node's self links, so the viewer draws every loop apart
/// (G6 bends a loop by placement and distance, not by bow); none for an edge between two nodes.
pub fn loops(recipe: &WorkflowRecipe) -> Vec<Option<u32>> {
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    recipe
        .edges
        .iter()
        .map(|edge| {
            (edge.from == edge.to).then(|| {
                let count = seen.entry(edge.from.as_str()).or_default();
                *count += 1;
                *count - 1
            })
        })
        .collect()
}

/// The viewer's whole input: no bindings, paths or commit, only what it draws.
pub fn payload(recipe: &WorkflowRecipe) -> String {
    let nodes: Vec<_> = recipe
        .nodes
        .iter()
        .map(|node| {
            json!({
                "id": node.id,
                "label": node.label,
                "kind": node_kind(node.kind),
                "start": node.id == recipe.start,
            })
        })
        .collect();
    let edges: Vec<_> = recipe
        .edges
        .iter()
        .zip(bows(recipe))
        .zip(back_edges(recipe))
        .zip(loops(recipe))
        .map(|(((edge, bow), back), place)| {
            json!({
                "id": edge.id,
                "source": edge.from,
                "target": edge.to,
                "label": edge.label,
                "kind": edge_kind(edge.kind),
                "bow": bow,
                "back": back,
                "loop": place,
            })
        })
        .collect();
    json!({"v": BRIDGE_VERSION, "nodes": nodes, "edges": edges})
        .to_string()
        // Inside the page's data script, `<` must never close it.
        .replace('<', "\\u003c")
}

/// The selected element's evidence as the inspector says it.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspector {
    pub title: String,
    /// `Decision`, `Branch link · Gate → Worker submits`.
    pub what: String,
    /// What the recipe states about it (its kind and ends) and its pinning and submission.
    pub facts: Vec<(&'static str, String)>,
    pub state: Evidence,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Evidence {
    Loading,
    Failed(String),
    /// corrald is preparing facts for the commit; the same selection is asked again.
    Pending {
        source: Option<Source>,
        note: String,
    },
    Ready {
        /// The committed lines, or why none show.
        source: Result<Source, String>,
        /// What Corral derived, each a line.
        derived: Vec<String>,
        /// Coverage and basis of what was derived, or why nothing was.
        basis: String,
    },
}

fn inspect(
    feed: &Feed,
    recipe: &WorkflowRecipe,
    reference: &str,
    sha256: &str,
    id: &str,
) -> Inspector {
    let (title, what) = match recipe.nodes.iter().find(|node| node.id == id) {
        Some(node) => (node.label.clone(), capital(node_kind(node.kind))),
        None => {
            let edge = recipe.edges.iter().find(|edge| edge.id == id);
            (
                edge.and_then(|edge| edge.label.clone())
                    .unwrap_or_else(|| id.to_owned()),
                edge.map_or_else(String::new, |edge| {
                    format!(
                        "{} link · {}",
                        capital(edge_kind(edge.kind)),
                        format_args!(
                            "{} → {}",
                            label_of(recipe, &edge.from),
                            label_of(recipe, &edge.to)
                        )
                    )
                }),
            )
        }
    };
    let mut facts = vec![
        ("ID", id.to_owned()),
        (
            "Pinned",
            format!("{} @ {}", recipe.subject.project, recipe.subject.commit),
        ),
    ];
    let cache = feed.detail(&element_target(reference, sha256, id));
    let state = match cache {
        None => Evidence::Loading,
        // A failed read wins over the body the cache keeps from before it, so a pending answer
        // followed by an error says so and is not asked again.
        Some(cache) => match (&cache.error, cache.body()) {
            (Some(error), _) => Evidence::Failed(format!(
                "corrald could not resolve this element: {}",
                error.message
            )),
            (None, Some(DetailBody::ArtifactDiagramElement(detail))) => {
                facts.push(("Submission", candidate(detail.candidate.as_ref())));
                evidence(detail)
            }
            _ => Evidence::Loading,
        },
    };
    Inspector {
        title,
        what,
        facts,
        state,
    }
}

fn capital(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn candidate(candidate: Option<&DiagramCandidate>) -> String {
    match candidate {
        Some(c) => format!(
            "{} {} · head {}{}",
            c.task,
            c.run,
            short(&c.head),
            c.base
                .as_deref()
                .map(|base| format!(" · base {}", short(base)))
                .unwrap_or_default()
        ),
        None => "No recorded submission is known for this commit".into(),
    }
}

pub(super) fn source(result: &SourceResult) -> Result<Source, String> {
    match result {
        SourceResult::NotProvided => Err("The recipe binds no source to this element.".into()),
        SourceResult::Missing { reason } => Err(format!("Missing: {reason}")),
        SourceResult::Unavailable { reason } => Err(format!("Unavailable: {reason}")),
        SourceResult::Ambiguous { reason } => Err(format!("Ambiguous: {reason}")),
        SourceResult::Resolved { excerpt, truncated } => {
            let last = excerpt.first + excerpt.lines.len().saturating_sub(1) as u64;
            Ok(Source {
                heading: format!(
                    "{}:{}–{} @ {}{}",
                    excerpt.path,
                    excerpt.first,
                    last,
                    short(&excerpt.commit),
                    if *truncated { " · cut short" } else { "" }
                ),
                lines: excerpt
                    .lines
                    .iter()
                    .enumerate()
                    .map(|(n, line)| {
                        let at = excerpt.first + n as u64;
                        (at, line.clone(), at == excerpt.line)
                    })
                    .collect(),
            })
        }
    }
}

fn basis_words(evidence: &EntityEvidence) -> String {
    let c = &evidence.coverage;
    let read = match evidence.read_status {
        ReadStatus::Complete => "read",
        ReadStatus::Unsupported => "not readable by Corral's facts",
        ReadStatus::Failed => "failed to read",
    };
    let provider = match &evidence.provider {
        Some(p) => match &p.error {
            Some(error) => format!("provider {} failed: {error}", p.extension),
            None => format!("provider {}", p.extension),
        },
        None => "built-in syntax facts, no provider".into(),
    };
    format!(
        "File {read} · {provider}{} · {} files covered, {} unsupported, {} failed",
        if evidence.provider_pending {
            " (still preparing)"
        } else {
            ""
        },
        c.complete,
        c.unsupported,
        c.failed
    )
}

pub(super) fn related(row: &DiagramRelated) -> String {
    match row {
        DiagramRelated::FileReference {
            path,
            basis,
            reference_kind,
            count,
        } => {
            let kind = match reference_kind {
                ReferenceKind::Use => "use",
                ReferenceKind::Import => "import",
                ReferenceKind::Call => "call site",
            };
            let basis = match basis {
                ReferenceBasis::Syntax => "syntax",
                ReferenceBasis::Resolved => "resolved",
            };
            format!("Referenced from {path} · {kind} ×{count} · {basis}")
        }
    }
}

fn evidence(detail: &DiagramElementDetail) -> Evidence {
    let source = source(&detail.source);
    let (derived, basis) = match &detail.entity {
        EntityResult::NotProvided => (
            Vec::new(),
            "The recipe binds no entity, so Corral looked nothing up.".to_owned(),
        ),
        EntityResult::Pending => {
            return Evidence::Pending {
                source: source.ok(),
                note: "Corral is preparing facts for this commit; asking again shortly.".into(),
            };
        }
        EntityResult::Missing { reason, evidence }
        | EntityResult::Unavailable { reason, evidence } => (
            Vec::new(),
            match evidence {
                Some(evidence) => format!("{reason} · {}", basis_words(evidence)),
                None => reason.clone(),
            },
        ),
        EntityResult::Ambiguous { reason, evidence } => {
            (Vec::new(), format!("{reason} · {}", basis_words(evidence)))
        }
        EntityResult::Resolved {
            location,
            evidence,
            related: rows,
            omitted,
        } => {
            let mut derived = vec![match location {
                EntityLocation::File => "The bound file exists at the pinned commit.".to_owned(),
                EntityLocation::Definition { line, end_line } => {
                    format!("Definition found at lines {line}–{end_line}.")
                }
            }];
            if rows.is_empty() {
                derived.push("No inbound file references within this coverage.".into());
            }
            derived.extend(rows.iter().map(related));
            if *omitted > 0 {
                derived.push(format!("{omitted} more references not shown."));
            }
            (derived, basis_words(evidence))
        }
    };
    Evidence::Ready {
        source,
        derived,
        basis,
    }
}

/// Whether the selected element's evidence is pending, so the same selection is asked again.
pub fn pending(graph: &Graph) -> bool {
    matches!(
        graph.inspector.as_ref().map(|i| &i.state),
        Some(Evidence::Pending { .. })
    )
}

/// The selected element's evidence read, while one is selected.
pub fn selected_target(graph: &Graph) -> Option<DetailRef> {
    graph
        .selected
        .as_deref()
        .map(|id| element_target(&graph.reference, &graph.sha256, id))
}

/// Every element ID of the graph, nodes first, as the list and the viewer know them.
pub fn ids(graph: &Graph) -> Vec<String> {
    graph
        .nodes
        .iter()
        .chain(&graph.edges)
        .map(|entry| entry.id.clone())
        .collect()
}
