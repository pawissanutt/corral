//! The Runtime view (docs/plans/system-map.md, runtime and entry points): the served
//! `system_runtime` as a scene the System map's own layout, canvas, tree and inspector draw.
//! Cards are the units (binaries, npm scripts, compose services, images, systemd units, CI
//! jobs) and the packages that start or build them or hold entry points; edges are `starts`,
//! `runs`, `depends_on` and `builds`, one per pair of cards with every site that establishes
//! it. Connected cards are layered; the rest are grouped by kind. The npm scripts fold into one
//! card unless the owner unfolds them.

use std::collections::{BTreeMap, BTreeSet};

use corral_types::{
    EntryDetector, EntryKind, EntryPoint, RuntimeBasis, RuntimeEdgeKind, RuntimeSource,
    RuntimeUnitId, RuntimeUnitKind, SystemRow, SystemRuntime, SystemRuntimeRow, SystemStatus,
};

use super::scene::{Card, Edge, EdgeKind, Kind, Level, Scene, Site, container_name};
use super::words::{Notice, Swatch};
use super::{Node, count, plural};
use crate::model::Hue;

/// A unit kind in a test id.
pub const fn kind_slug(kind: RuntimeUnitKind) -> &'static str {
    match kind {
        RuntimeUnitKind::Binary => "binary",
        RuntimeUnitKind::Script => "script",
        RuntimeUnitKind::Service => "service",
        RuntimeUnitKind::Image => "image",
        RuntimeUnitKind::Unit => "unit",
        RuntimeUnitKind::Job => "job",
    }
}

/// Where a card's kind sits in the view's order, and the title of its group.
pub fn group(kind: Kind) -> (usize, &'static str) {
    match kind {
        Kind::Unit(RuntimeUnitKind::Binary) => (0, "Binaries"),
        Kind::Unit(RuntimeUnitKind::Script) | Kind::Scripts => (1, "npm scripts"),
        Kind::Unit(RuntimeUnitKind::Service) => (2, "Compose services"),
        Kind::Unit(RuntimeUnitKind::Image) => (3, "Images"),
        Kind::Unit(RuntimeUnitKind::Unit) => (4, "Systemd units"),
        Kind::Unit(RuntimeUnitKind::Job) => (5, "CI jobs"),
        _ => (6, "Packages and folders"),
    }
}

/// The map's bands, top to bottom: what starts, runs or builds sits above what it starts, runs
/// or builds.
const BANDS: [Kind; 7] = [
    Kind::Owner,
    Kind::Unit(RuntimeUnitKind::Job),
    Kind::Unit(RuntimeUnitKind::Unit),
    Kind::Unit(RuntimeUnitKind::Image),
    Kind::Unit(RuntimeUnitKind::Service),
    Kind::Unit(RuntimeUnitKind::Binary),
    Kind::Scripts,
];

/// A card's band as the layered layout ranks it: the higher, the nearer the top.
pub fn band(kind: Kind) -> i32 {
    let kind = match kind {
        Kind::Unit(RuntimeUnitKind::Script) => Kind::Scripts,
        Kind::Unit(_) | Kind::Scripts => kind,
        _ => Kind::Owner,
    };
    let at = BANDS.iter().position(|k| *k == kind).unwrap_or(0);
    (BANDS.len() - 1 - at) as i32
}

/// The caption of a band.
pub fn band_title(band: i32) -> &'static str {
    let at = usize::try_from(BANDS.len() as i32 - 1 - band).unwrap_or(0);
    BANDS.get(at).map_or("", |kind| group(*kind).1)
}

pub const fn detector_word(detector: EntryDetector) -> &'static str {
    match detector {
        EntryDetector::Clap => "clap",
        EntryDetector::Axum => "axum",
        EntryDetector::Express => "express",
        EntryDetector::NpmBin => "npm bin",
        EntryDetector::OperationEnum => "operation enum",
        EntryDetector::Extension => "extension",
    }
}

pub const fn entry_word(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Command => "command",
        EntryKind::Route => "route",
        EntryKind::Operation => "operation",
    }
}

pub const fn basis_word(basis: RuntimeBasis) -> &'static str {
    match basis {
        RuntimeBasis::Declared => "declared",
        RuntimeBasis::Syntax => "syntax",
    }
}

const fn edge_kind(kind: RuntimeEdgeKind) -> EdgeKind {
    match kind {
        RuntimeEdgeKind::Starts => EdgeKind::Starts,
        RuntimeEdgeKind::Runs => EdgeKind::Runs,
        RuntimeEdgeKind::DependsOn => EdgeKind::DependsOn,
        RuntimeEdgeKind::Builds => EdgeKind::Builds,
    }
}

/// `path:line`.
pub fn at(path: &str, line: u64) -> String {
    format!("{path}:{line}")
}

/// The detectors as words; `no detector` for an empty set.
pub fn detectors_words(detectors: &BTreeSet<EntryDetector>) -> String {
    if detectors.is_empty() {
        return "no detector".into();
    }
    detectors
        .iter()
        .map(|d| detector_word(*d))
        .collect::<Vec<_>>()
        .join(", ")
}

/// An edge in a line: its kind, basis and source line, or how many sites hold it.
pub fn edge_words(edge: &Edge) -> String {
    match edge.sites.as_slice() {
        [one] => format!(
            "{} · {} · {}",
            one.kind.word(),
            basis_word(one.basis),
            at(&one.site.path, one.site.line)
        ),
        sites => format!(
            "{} · {}",
            edge.kind.word(),
            plural(sites.len() as u64, "site", "sites")
        ),
    }
}

/// An edge in a narrow row: its kind and basis, or how many sites hold it; selecting it shows
/// the lines.
pub fn edge_brief(edge: &Edge) -> String {
    match edge.sites.as_slice() {
        [one] => format!("{} · {}", one.kind.word(), basis_word(one.basis)),
        sites => format!(
            "{} · {}",
            edge.kind.word(),
            plural(sites.len() as u64, "site", "sites")
        ),
    }
}

/// What a Runtime card says under its name.
pub fn figure(card: &Card) -> String {
    match card.kind {
        Kind::Scripts => "folded · select again to list them".into(),
        Kind::Owner => match &card.detectors {
            None if card.entries.is_empty() => "entry points not reported".into(),
            None => plural(card.entries.len() as u64, "entry point", "entry points"),
            Some(set) if set.is_empty() => "no detector".into(),
            // An entry point whose package declares one binary is listed under that binary.
            Some(set) if card.entries.is_empty() && card.references > 0 => format!(
                "{} · {} under its units",
                detectors_words(set),
                plural(card.references, "entry point", "entry points")
            ),
            Some(set) => format!(
                "{} · {}",
                plural(card.entries.len() as u64, "entry point", "entry points"),
                detectors_words(set)
            ),
        },
        _ => {
            let Some(unit) = &card.unit else {
                return String::new();
            };
            let mut parts = Vec::new();
            if !card.entries.is_empty() {
                parts.push(plural(
                    card.entries.len() as u64,
                    "entry point",
                    "entry points",
                ));
            }
            if !unit.ports.is_empty() {
                parts.push(format!("ports {}", unit.ports.join(", ")));
            }
            // A binary's entry point is its source file, not something it runs.
            let runs = match unit.id.kind {
                RuntimeUnitKind::Binary => unit.command.as_ref(),
                _ => unit.command.as_ref().or(unit.entry_point.as_ref()),
            };
            if let Some(command) = runs {
                parts.push(format!("$ {command}"));
            }
            if parts.is_empty() {
                parts.push(at(&unit.id.path, unit.line));
            }
            parts.join(" · ")
        }
    }
}

struct Builder {
    scene: Scene,
    index: BTreeMap<Node, usize>,
    scripts: bool,
}

impl Builder {
    fn node(&self, id: &RuntimeUnitId) -> Node {
        if id.kind == RuntimeUnitKind::Script && !self.scripts {
            Node::Scripts
        } else {
            Node::Unit(id.clone())
        }
    }

    fn push(&mut self, card: Card) -> usize {
        let at = self.scene.cards.len();
        self.index.insert(card.node.clone(), at);
        self.scene.cards.push(card);
        at
    }

    /// The card of a package or folder, made when first named.
    fn owner(&mut self, path: &str) -> usize {
        let node = Node::Container(path.to_owned());
        match self.index.get(&node) {
            Some(at) => *at,
            None => self.push(Card::new(node, container_name(path), Kind::Owner)),
        }
    }
}

/// The deepest package `known` names that holds `path`.
fn holder<'a>(known: impl Iterator<Item = &'a String>, path: &str) -> String {
    known
        .filter(|c| c.is_empty() || path.starts_with(&format!("{c}/")))
        .max_by_key(|c| c.len())
        .cloned()
        .unwrap_or_default()
}

/// The view's scene; `scripts`: the npm scripts listed one by one.
pub fn build(runtime: &SystemRuntime, scripts: bool) -> Scene {
    let mut b = Builder {
        scene: Scene {
            level: Level::Runtime,
            cards: Vec::new(),
            graph: Vec::new(),
            top: Vec::new(),
            bottom: Vec::new(),
            band: Vec::new(),
            band_title: None,
            edges: Vec::new(),
            undrawn: Vec::new(),
            max_lines: 1,
        },
        index: BTreeMap::new(),
        scripts,
    };
    let mut folded = Vec::new();
    let mut entries: Vec<&EntryPoint> = Vec::new();
    let mut edges = Vec::new();
    let mut coverage: BTreeMap<&String, &BTreeSet<EntryDetector>> = BTreeMap::new();
    for row in &runtime.rows {
        match row {
            SystemRuntimeRow::Unit { unit } => {
                let node = b.node(&unit.id);
                if node == Node::Scripts {
                    folded.push(unit.clone());
                } else if !b.index.contains_key(&node) {
                    let mut card = Card::new(node, unit.id.name.clone(), Kind::Unit(unit.id.kind));
                    card.tag = Some(super::scene::unit_word(unit.id.kind).into());
                    card.unit = Some(unit.clone());
                    b.push(card);
                }
            }
            SystemRuntimeRow::Edge { edge } => edges.push(edge),
            SystemRuntimeRow::Entry { entry } => entries.push(entry),
            SystemRuntimeRow::EntryCoverage {
                container,
                detectors,
            } => {
                coverage.insert(container, detectors);
            }
        }
    }
    if !folded.is_empty() {
        let n = folded.len() as u64;
        let mut card = Card::new(
            Node::Scripts,
            plural(n, "npm script", "npm scripts"),
            Kind::Scripts,
        );
        card.more = Some((n, "scripts"));
        card.folded = folded;
        b.push(card);
    }
    for (container, detectors) in &coverage {
        let at = b.owner(container);
        b.scene.cards[at].detectors = Some((*detectors).clone());
    }
    for entry in entries {
        let at = match &entry.owner {
            RuntimeSource::Unit { id } => b.index.get(&b.node(id)).copied(),
            RuntimeSource::Container { path } => Some(b.owner(path)),
        };
        // A unit left out of the reply still has its entry listed, under the package holding it.
        let at = match at {
            Some(at) => at,
            None => {
                let path = holder(coverage.keys().copied(), &entry.path);
                b.owner(&path)
            }
        };
        b.scene.cards[at].entries.push(entry.clone());
    }
    let mut merged: BTreeMap<(Node, Node), usize> = BTreeMap::new();
    for edge in edges {
        // A unit left out of the reply has no card to draw an edge to.
        let to = b.node(&edge.to);
        if !b.index.contains_key(&to) {
            continue;
        }
        let from = match &edge.from {
            RuntimeSource::Container { path } => {
                b.owner(path);
                Node::Container(path.clone())
            }
            RuntimeSource::Unit { id } => b.node(id),
        };
        if from == to || !b.index.contains_key(&from) {
            continue;
        }
        let site = Site {
            kind: edge_kind(edge.kind),
            basis: edge.basis,
            site: edge.site.clone(),
        };
        match merged.get(&(from.clone(), to.clone())) {
            Some(at) => {
                let drawn = &mut b.scene.edges[*at];
                drawn.sites.push(site);
                drawn.count += 1;
            }
            None => {
                merged.insert((from.clone(), to.clone()), b.scene.edges.len());
                b.scene.edges.push(Edge {
                    from,
                    to,
                    kind: site.kind,
                    count: 1,
                    basis: None,
                    pairs: Vec::new(),
                    sites: vec![site],
                });
            }
        }
    }
    let mut held: BTreeMap<String, u64> = BTreeMap::new();
    for card in &mut b.scene.cards {
        card.entries
            .sort_by(|a, c| (&a.path, a.line).cmp(&(&c.path, c.line)));
        if let Some(unit) = &card.unit {
            *held.entry(unit.container.clone()).or_default() += card.entries.len() as u64;
        }
    }
    for card in &mut b.scene.cards {
        if let (Kind::Owner, Node::Container(path)) = (card.kind, &card.node) {
            card.references = held.get(path).copied().unwrap_or(0);
        }
    }
    // Every card is in the graph, connected or not: the layout lays each in its kind's band.
    let cards = &b.scene.cards;
    let mut graph: Vec<usize> = (0..cards.len()).collect();
    graph.sort_by(|a, c| {
        (group(cards[*a].kind).0, &cards[*a].name).cmp(&(group(cards[*c].kind).0, &cards[*c].name))
    });
    b.scene.graph = graph;
    b.scene
}

/// Whether the reply carries entry points: corrald sends a coverage row for every package
/// once it serves them.
pub fn entries_served(runtime: &SystemRuntime) -> bool {
    runtime.rows.iter().any(|row| {
        matches!(
            row,
            SystemRuntimeRow::EntryCoverage { .. } | SystemRuntimeRow::Entry { .. }
        )
    })
}

/// The figures over the map: units by kind, edges by kind, entry points and their coverage.
pub fn summary(runtime: &SystemRuntime) -> Vec<String> {
    let mut units: BTreeMap<usize, (u64, RuntimeUnitKind)> = BTreeMap::new();
    let mut edges: BTreeMap<EdgeKind, u64> = BTreeMap::new();
    let (mut entries, mut covered, mut none) = (0u64, 0u64, 0u64);
    for row in &runtime.rows {
        match row {
            SystemRuntimeRow::Unit { unit } => {
                units
                    .entry(group(Kind::Unit(unit.id.kind)).0)
                    .or_insert((0, unit.id.kind))
                    .0 += 1
            }
            SystemRuntimeRow::Edge { edge } => *edges.entry(edge_kind(edge.kind)).or_default() += 1,
            SystemRuntimeRow::Entry { .. } => entries += 1,
            SystemRuntimeRow::EntryCoverage { detectors, .. } => {
                if detectors.is_empty() {
                    none += 1
                } else {
                    covered += 1
                }
            }
        }
    }
    let mut out = Vec::new();
    if units.is_empty() {
        out.push("No runtime units".into());
    } else {
        out.push(
            units
                .values()
                .map(|(n, kind)| {
                    let word = super::scene::unit_word(*kind);
                    let many = match kind {
                        RuntimeUnitKind::Binary => "binaries".to_owned(),
                        _ => format!("{word}s"),
                    };
                    plural(*n, word, &many)
                })
                .collect::<Vec<_>>()
                .join(" · "),
        );
    }
    let total: u64 = edges.values().sum();
    if total > 0 {
        let kinds: Vec<String> = edges
            .iter()
            .map(|(kind, n)| format!("{} {}", count(*n), kind.word()))
            .collect();
        out.push(format!(
            "{}: {}",
            plural(total, "edge", "edges"),
            kinds.join(", ")
        ));
    }
    if entries_served(runtime) {
        out.push(format!(
            "{} · detectors ran in {} · {} with no detector",
            plural(entries, "entry point", "entry points"),
            plural(covered, "package", "packages"),
            count(none)
        ));
    } else {
        out.push("Entry points not provided by this corrald".into());
    }
    out
}

/// The edge kinds as the map draws them.
pub fn legend(scene: &Scene) -> Vec<(Swatch, String)> {
    let mut out: Vec<(Swatch, String)> = [
        (
            EdgeKind::Starts,
            "starts: a spawn site names the binary (syntax)",
        ),
        (
            EdgeKind::Runs,
            "runs: a command, entry point, ExecStart or job step names the binary",
        ),
        (EdgeKind::DependsOn, "depends on: compose depends_on"),
        (
            EdgeKind::Builds,
            "builds: the build context or Dockerfile lies in the package",
        ),
    ]
    .into_iter()
    .map(|(kind, words)| (Swatch::Edge(kind), words.to_owned()))
    .collect();
    if scene.cards.iter().any(|c| c.kind == Kind::Owner) {
        out.push((Swatch::Folder, "package or folder".into()));
    }
    out
}

/// What the view says over the map: facts building, a failed read, rows left out.
pub fn notices(runtime: Option<&SystemRuntime>, row: Option<&SystemRow>) -> Vec<Notice> {
    let mut out = Vec::new();
    let building = runtime.map(|r| r.status) == Some(SystemStatus::Building)
        || row.map(|r| r.status) == Some(SystemStatus::Building);
    if building {
        out.push(Notice {
            hue: Hue::Corral,
            words: "Building this project's facts. The view fills in when they are ready.".into(),
        });
    }
    if let Some(error) = runtime
        .and_then(|r| r.error.clone())
        .or_else(|| row.and_then(|r| r.error.clone()))
    {
        out.push(Notice {
            hue: Hue::Fail,
            words: format!("The runtime facts could not be built: {error}"),
        });
    }
    if let Some(runtime) = runtime.filter(|r| r.omitted_rows > 0) {
        out.push(Notice {
            hue: Hue::Neutral,
            words: format!(
                "{} left out to stay within the wire budget.",
                plural(runtime.omitted_rows as u64, "row", "rows")
            ),
        });
    }
    out
}

/// The one provenance line, with what the view cannot see.
pub fn provenance(runtime: Option<&SystemRuntime>, row: Option<&SystemRow>) -> String {
    let head = runtime
        .and_then(|r| r.head.clone())
        .or_else(|| row.and_then(|r| r.head.clone()));
    let at = match &head {
        Some(head) => format!("Facts at {}", head.chars().take(9).collect::<String>()),
        None => "No commit read yet".into(),
    };
    format!(
        "{at} · built in from manifests, compose files, Dockerfiles, systemd units, CI workflows \
         and spawn sites · ports are exact only where a file declares them; a process name built \
         at run time is not seen"
    )
}

pub const EMPTY: &str = "No runtime units at this commit: no binary, npm script, compose service, \
                         Dockerfile, systemd unit or CI workflow was found.";
