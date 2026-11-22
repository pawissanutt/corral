//! A served level as cards and edges, independent of position. The project level is the
//! packages and folders with the package edges between them; a package level its modules
//! with the neighbouring packages on its boundary; a module level its files, largest first;
//! a file level its definitions in source order. A level that lists more than stays legible
//! counts the rest in one card.

use std::collections::BTreeMap;

use corral_types::{
    ContainerFact, ContainerKind, DefinitionFact, ModuleId, PackageEdgeKind, ReadStatus,
    ReferenceBasis, SystemAt, SystemMap, SystemMapRow, SystemNodeStats, WorkspaceStatus,
};

use super::{Node, base_name};

/// How deep the map is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Project,
    Package,
    Module,
    File,
}

impl Level {
    pub fn of(at: Option<&SystemAt>) -> Level {
        match at {
            None => Level::Project,
            Some(SystemAt::Container { .. }) => Level::Package,
            Some(SystemAt::Module { .. }) => Level::Module,
            Some(SystemAt::File { .. }) => Level::File,
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Level::Project => "project",
            Level::Package => "package",
            Level::Module => "module",
            Level::File => "file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Package,
    Folder,
    Port,
    Module,
    File,
    Definition,
    More,
}

impl Kind {
    pub const fn word(self) -> &'static str {
        match self {
            Kind::Package => "package",
            Kind::Folder => "folder",
            Kind::Port => "neighbouring package",
            Kind::Module => "module",
            Kind::File => "file",
            Kind::Definition => "definition",
            Kind::More => "not listed",
        }
    }
}

/// One node's facts as the map draws them.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub node: Node,
    pub name: String,
    pub kind: Kind,
    /// Kind or workspace status for a container, `test · rust` for a file, the definition's
    /// kind; absent where nothing is worth a tag.
    pub tag: Option<String>,
    pub lines: u64,
    pub stats: Option<SystemNodeStats>,
    pub container: Option<ContainerFact>,
    /// A file's language, test flag and read status.
    pub file: Option<(String, bool, ReadStatus)>,
    pub definition: Option<DefinitionFact>,
    /// External processes the container spawns, with their sites, most sites first.
    pub spawns: Vec<(String, u64)>,
    /// For a port, its references across the boundary.
    pub references: u64,
    /// For the More card: how many and of what.
    pub more: Option<(u64, &'static str)>,
}

impl Card {
    fn new(node: Node, name: String, kind: Kind) -> Card {
        Card {
            node,
            name,
            kind,
            tag: None,
            lines: 0,
            stats: None,
            container: None,
            file: None,
            definition: None,
            spawns: Vec::new(),
            references: 0,
            more: None,
        }
    }

    /// Files under the node, as the coverage counts them.
    pub fn files(&self) -> u64 {
        match (&self.stats, &self.file) {
            (Some(stats), _) => stats.complete + stats.unsupported + stats.failed,
            (None, Some(_)) => 1,
            _ => 0,
        }
    }

    /// Workspace status worth saying: not a plain member.
    pub fn excluded(&self) -> bool {
        self.container
            .as_ref()
            .is_some_and(|c| c.workspace_status == WorkspaceStatus::Excluded)
    }
}

/// How an edge was established, which is how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// A package edge declared in a manifest and referenced in source.
    Declared,
    /// Declared in a manifest with no reference found.
    DeclaredOnly,
    /// Referenced in source without a declaration.
    ReferencedOnly,
    /// Module to module inside the package shown.
    Module,
    /// Module references across the package's boundary, summed per neighbour.
    Boundary,
}

impl EdgeKind {
    pub const fn word(self) -> &'static str {
        match self {
            EdgeKind::Declared => "declared and referenced",
            EdgeKind::DeclaredOnly => "declared, no reference found",
            EdgeKind::ReferencedOnly => "referenced, not declared",
            EdgeKind::Module => "module reference",
            EdgeKind::Boundary => "references with a neighbouring package",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub from: Node,
    pub to: Node,
    pub kind: EdgeKind,
    pub count: u64,
    pub basis: Option<ReferenceBasis>,
    /// The module pairs a boundary edge sums, strongest first.
    pub pairs: Vec<(ModuleId, ModuleId, u64)>,
}

/// A level ready to place or list.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub level: Level,
    pub cards: Vec<Card>,
    /// Indexes into `cards`: what edges connect, above the band.
    pub graph: Vec<usize>,
    /// Ports pinned above (into the package) and below (out of it).
    pub top: Vec<usize>,
    pub bottom: Vec<usize>,
    /// Unconnected cards, packed under the graph.
    pub band: Vec<usize>,
    pub band_title: Option<&'static str>,
    /// Edges the map draws.
    pub edges: Vec<Edge>,
    /// Edges left undrawn to stay legible; the inspector lists them.
    pub undrawn: Vec<Edge>,
    pub max_lines: u64,
}

impl Scene {
    pub fn card(&self, node: &Node) -> Option<&Card> {
        self.cards.iter().find(|card| &card.node == node)
    }

    pub fn name(&self, node: &Node) -> String {
        self.card(node)
            .map(|card| card.name.clone())
            .unwrap_or_else(|| fallback_name(node))
    }

    /// Every edge, drawn first.
    pub fn all_edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.iter().chain(self.undrawn.iter())
    }

    /// The cards in the order the map reads: graph, then band.
    pub fn order(&self) -> impl Iterator<Item = &Card> {
        self.top
            .iter()
            .chain(self.graph.iter())
            .chain(self.bottom.iter())
            .chain(self.band.iter())
            .filter_map(|i| self.cards.get(*i))
    }
}

/// Files a module level lists before counting the rest.
pub const MAX_FILES: usize = 48;
/// Definitions a file level lists before counting the rest.
pub const MAX_DEFINITIONS: usize = 120;
/// Module references a package level draws; each module's strongest first.
pub const MAX_MODULE_EDGES: usize = 40;

pub fn build(map: &SystemMap, at: Option<&SystemAt>) -> Scene {
    let level = Level::of(at);
    let mut scene = Scene {
        level,
        cards: Vec::new(),
        graph: Vec::new(),
        top: Vec::new(),
        bottom: Vec::new(),
        band: Vec::new(),
        band_title: None,
        edges: Vec::new(),
        undrawn: Vec::new(),
        max_lines: 1,
    };
    match at {
        None => project(&mut scene, &map.rows),
        Some(SystemAt::Container { path }) => package(&mut scene, &map.rows, path),
        Some(SystemAt::Module { .. }) => module(&mut scene, &map.rows),
        Some(SystemAt::File { .. }) => file(&mut scene, &map.rows),
    }
    scene.max_lines = scene.cards.iter().map(|c| c.lines).max().unwrap_or(0).max(1);
    scene
}

fn fallback_name(node: &Node) -> String {
    match node {
        Node::Container(path) | Node::Port { path, .. } => container_name(path),
        Node::Module(module) => module_name(module),
        Node::File(path) => base_name(path).to_owned(),
        Node::Definition { name, .. } => name.clone(),
        Node::More => "more".into(),
    }
}

fn container_name(path: &str) -> String {
    if path.is_empty() {
        "(root)".into()
    } else {
        base_name(path).to_owned()
    }
}

/// A module's name: its first segment, or the package itself when files sit at its root.
pub fn module_name(module: &ModuleId) -> String {
    if module.path.is_empty() {
        "(root)".into()
    } else {
        module.path.clone()
    }
}

fn container_tag(container: &ContainerFact) -> String {
    match container.workspace_status {
        WorkspaceStatus::Excluded => "excluded".into(),
        WorkspaceStatus::SeparateWorkspace => "separate workspace".into(),
        WorkspaceStatus::Member => kind_word(container.kind).into(),
    }
}

pub fn kind_word(kind: ContainerKind) -> &'static str {
    match kind {
        ContainerKind::Cargo => "cargo",
        ContainerKind::Npm => "npm",
        ContainerKind::Python => "python",
        ContainerKind::Folder => "folder",
    }
}

fn container_card(container: &ContainerFact, stats: Option<&SystemNodeStats>, node: Node) -> Card {
    let kind = match (&node, container.kind) {
        (Node::Port { .. }, _) => Kind::Port,
        (_, ContainerKind::Folder) => Kind::Folder,
        _ => Kind::Package,
    };
    let name = if container.name.is_empty() {
        container_name(&container.path)
    } else {
        container.name.clone()
    };
    let mut card = Card::new(node, name, kind);
    card.tag = Some(container_tag(container));
    card.lines = stats.map_or(0, |s| s.lines);
    card.stats = stats.cloned();
    card.container = Some(container.clone());
    card
}

fn by_lines_desc(cards: &[Card], indexes: &mut [usize]) {
    indexes.sort_by(|a, b| {
        cards[*b]
            .lines
            .cmp(&cards[*a].lines)
            .then_with(|| cards[*a].name.cmp(&cards[*b].name))
    });
}

fn project(scene: &mut Scene, rows: &[SystemMapRow]) {
    let mut spawns: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    for row in rows {
        match row {
            SystemMapRow::Container { container, stats } => {
                let node = Node::Container(container.path.clone());
                scene.cards.push(container_card(container, Some(stats), node));
            }
            SystemMapRow::PackageEdge { edge } => scene.edges.push(Edge {
                from: Node::Container(edge.from.clone()),
                to: Node::Container(edge.to.clone()),
                kind: match edge.kind {
                    PackageEdgeKind::DeclaredAndReferenced => EdgeKind::Declared,
                    PackageEdgeKind::DeclaredOnly => EdgeKind::DeclaredOnly,
                    PackageEdgeKind::ReferencedOnly => EdgeKind::ReferencedOnly,
                },
                count: edge.references,
                basis: None,
                pairs: Vec::new(),
            }),
            SystemMapRow::Spawn { container, spawn } => {
                *spawns
                    .entry(container.clone())
                    .or_default()
                    .entry(spawn.process.clone())
                    .or_default() += spawn.count;
            }
            _ => {}
        }
    }
    for card in &mut scene.cards {
        if let Node::Container(path) = &card.node
            && let Some(list) = spawns.remove(path)
        {
            card.spawns = list.into_iter().collect();
            card.spawns.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        }
    }
    // An edge to a container the level does not list cannot be drawn.
    let known: Vec<Node> = scene.cards.iter().map(|c| c.node.clone()).collect();
    scene
        .edges
        .retain(|e| e.from != e.to && known.contains(&e.from) && known.contains(&e.to));
    let (mut graph, mut band): (Vec<usize>, Vec<usize>) = (0..scene.cards.len()).partition(|i| {
        let node = &scene.cards[*i].node;
        scene.edges.iter().any(|e| &e.from == node || &e.to == node)
    });
    by_lines_desc(&scene.cards, &mut graph);
    by_lines_desc(&scene.cards, &mut band);
    scene.graph = graph;
    scene.band = band;
    scene.band_title = Some("No package reference found");
}

fn package(scene: &mut Scene, rows: &[SystemMapRow], path: &str) {
    let mut neighbours: BTreeMap<String, ContainerFact> = BTreeMap::new();
    let mut boundary: BTreeMap<(Node, Node), Vec<(ModuleId, ModuleId, u64)>> = BTreeMap::new();
    let mut inside = Vec::new();
    for row in rows {
        match row {
            SystemMapRow::Module { module, stats } => {
                let mut card = Card::new(
                    Node::Module(module.clone()),
                    module_name(module),
                    Kind::Module,
                );
                card.lines = stats.lines;
                card.stats = Some(stats.clone());
                scene.cards.push(card);
            }
            SystemMapRow::Neighbor { container } => {
                neighbours.insert(container.path.clone(), container.clone());
            }
            SystemMapRow::ModuleReference { reference } => {
                let (from, to) = (&reference.from, &reference.to);
                let pair = (from.clone(), to.clone(), reference.count);
                if from.container == path && to.container == path {
                    inside.push(Edge {
                        from: Node::Module(from.clone()),
                        to: Node::Module(to.clone()),
                        kind: EdgeKind::Module,
                        count: reference.count,
                        basis: Some(reference.basis),
                        pairs: Vec::new(),
                    });
                } else if from.container == path {
                    let port = Node::Port {
                        path: to.container.clone(),
                        into: false,
                    };
                    boundary
                        .entry((Node::Module(from.clone()), port))
                        .or_default()
                        .push(pair);
                } else if to.container == path {
                    let port = Node::Port {
                        path: from.container.clone(),
                        into: true,
                    };
                    boundary
                        .entry((port, Node::Module(to.clone())))
                        .or_default()
                        .push(pair);
                }
            }
            _ => {}
        }
    }
    let mut port_refs: BTreeMap<Node, u64> = BTreeMap::new();
    let mut all = inside;
    for ((from, to), mut pairs) in boundary {
        pairs.sort_by(|a, b| b.2.cmp(&a.2));
        let count = pairs.iter().map(|p| p.2).sum();
        let port = if matches!(from, Node::Port { .. }) {
            &from
        } else {
            &to
        };
        *port_refs.entry(port.clone()).or_default() += count;
        all.push(Edge {
            from,
            to,
            kind: EdgeKind::Boundary,
            count,
            basis: None,
            pairs,
        });
    }
    for (port, refs) in &port_refs {
        if let Node::Port { path, .. } = port {
            let fact = neighbours.get(path).cloned().unwrap_or_else(|| ContainerFact {
                path: path.clone(),
                name: container_name(path),
                kind: ContainerKind::Folder,
                workspace_status: WorkspaceStatus::Member,
                other_packages: Vec::new(),
            });
            let mut card = container_card(&fact, None, port.clone());
            card.references = *refs;
            scene.cards.push(card);
        }
    }
    // A neighbour brought only by a declared package edge has no module reference to draw.
    for (path, fact) in &neighbours {
        let linked = port_refs.keys().any(|port| matches!(port, Node::Port { path: p, .. } if p == path));
        if !linked {
            let node = Node::Port {
                path: path.clone(),
                into: false,
            };
            scene.cards.push(container_card(fact, None, node));
        }
    }
    keep_legible(scene, all);
    let drawn = |node: &Node, edges: &[Edge]| edges.iter().any(|e| &e.from == node || &e.to == node);
    let mut modules = Vec::new();
    let mut band = Vec::new();
    let mut ports: Vec<usize> = Vec::new();
    for (i, card) in scene.cards.iter().enumerate() {
        match (&card.node, drawn(&card.node, &scene.edges)) {
            (Node::Port { .. }, true) => ports.push(i),
            (_, true) => modules.push(i),
            _ => band.push(i),
        }
    }
    by_lines_desc(&scene.cards, &mut modules);
    by_lines_desc(&scene.cards, &mut band);
    ports.sort_by(|a, b| scene.cards[*b].references.cmp(&scene.cards[*a].references));
    for i in ports {
        match scene.cards[i].node {
            Node::Port { into: true, .. } => scene.top.push(i),
            _ => scene.bottom.push(i),
        }
    }
    scene.graph = modules;
    scene.band = band;
    scene.band_title = Some("No module reference found");
}

/// Keeps each node's strongest reference, then the strongest overall, up to the cap.
fn keep_legible(scene: &mut Scene, mut all: Vec<Edge>) {
    all.sort_by(|a, b| b.count.cmp(&a.count));
    let mut keep = vec![false; all.len()];
    let mut seen: Vec<&Node> = Vec::new();
    for (i, edge) in all.iter().enumerate() {
        if !seen.contains(&&edge.from) || !seen.contains(&&edge.to) {
            keep[i] = true;
            seen.push(&edge.from);
            seen.push(&edge.to);
        }
    }
    let mut kept = keep.iter().filter(|k| **k).count();
    for k in keep.iter_mut() {
        if kept >= MAX_MODULE_EDGES {
            break;
        }
        if !*k {
            *k = true;
            kept += 1;
        }
    }
    for (edge, keep) in all.into_iter().zip(keep) {
        if keep {
            scene.edges.push(edge);
        } else {
            scene.undrawn.push(edge);
        }
    }
}

fn more_card(scene: &mut Scene, count: u64, lines: u64, noun: &'static str) {
    if count == 0 {
        return;
    }
    let mut card = Card::new(Node::More, format!("{count} more {noun}"), Kind::More);
    card.lines = lines;
    card.more = Some((count, noun));
    scene.band.push(scene.cards.len());
    scene.cards.push(card);
}

fn module(scene: &mut Scene, rows: &[SystemMapRow]) {
    let (mut rest, mut rest_lines) = (0, 0);
    for row in rows {
        if let SystemMapRow::File {
            path,
            language,
            lines,
            test,
            read_status,
        } = row
        {
            if scene.cards.len() >= MAX_FILES {
                rest += 1;
                rest_lines += lines;
                continue;
            }
            let mut card = Card::new(Node::File(path.clone()), base_name(path).into(), Kind::File);
            card.lines = *lines;
            card.tag = Some(if *test {
                format!("test · {language}")
            } else {
                language.clone()
            });
            card.file = Some((language.clone(), *test, *read_status));
            scene.band.push(scene.cards.len());
            scene.cards.push(card);
        }
    }
    more_card(scene, rest, rest_lines, "files");
}

fn file(scene: &mut Scene, rows: &[SystemMapRow]) {
    let mut rest = 0;
    for row in rows {
        if let SystemMapRow::Definition { definition } = row {
            if scene.cards.len() >= MAX_DEFINITIONS {
                rest += 1;
                continue;
            }
            let node = Node::Definition {
                line: definition.line,
                name: definition.name.clone(),
            };
            let mut card = Card::new(node, definition.name.clone(), Kind::Definition);
            card.tag = Some(definition.kind.clone());
            card.lines = definition.end_line.saturating_sub(definition.line) + 1;
            card.definition = Some(definition.clone());
            scene.band.push(scene.cards.len());
            scene.cards.push(card);
        }
    }
    more_card(scene, rest, 0, "definitions");
}
