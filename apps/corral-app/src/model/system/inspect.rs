//! The inspector: what the picked node or edge is, its facts under every overlay, and its
//! edges, each of which picks the other end. Every form shows the same sections.

use corral_types::{ReadStatus, ReferenceBasis, SystemActivity, SystemAt, WorkspaceStatus};

use super::scene::{Card, Edge, EdgeKind, Kind, Scene, kind_word, module_name};
use super::{Node, Pick, count, plural};

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    pub value: String,
    /// What pressing the row picks.
    pub pick: Option<Pick>,
    /// The edge's kind, drawn as its line sample.
    pub swatch: Option<EdgeKind>,
}

impl Row {
    fn fact(label: &str, value: String) -> Row {
        Row {
            label: label.into(),
            value,
            pick: None,
            swatch: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub title: Option<String>,
    pub rows: Vec<Row>,
    pub note: Option<String>,
}

impl Section {
    fn facts(title: Option<&str>, rows: Vec<Row>) -> Section {
        Section {
            title: title.map(str::to_owned),
            rows,
            note: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Inspector {
    pub kind: String,
    pub title: String,
    pub sections: Vec<Section>,
    /// The level the node opens, with the button's words.
    pub open: Option<(String, Node)>,
}

pub fn inspect(scene: &Scene, pick: &Pick, at: Option<&SystemAt>) -> Option<Inspector> {
    match pick {
        Pick::Node(node) => scene.card(node).map(|card| node_inspector(scene, card, at)),
        Pick::Edge(from, to) => scene
            .all_edges()
            .find(|e| &e.from == from && &e.to == to)
            .map(|edge| edge_inspector(scene, edge)),
    }
}

fn workspace(status: WorkspaceStatus) -> &'static str {
    match status {
        WorkspaceStatus::Member => "member",
        WorkspaceStatus::Excluded => "excluded from its workspace",
        WorkspaceStatus::SeparateWorkspace => "separate workspace",
    }
}

fn node_inspector(scene: &Scene, card: &Card, at: Option<&SystemAt>) -> Inspector {
    let mut sections = Vec::new();
    let mut kind = card.kind.word().to_owned();
    let mut facts = Vec::new();
    if let Some(container) = &card.container {
        kind = match container.kind {
            corral_types::ContainerKind::Folder => "Folder, no manifest".into(),
            other => format!(
                "{} package · {}",
                capital(kind_word(other)),
                workspace(container.workspace_status)
            ),
        };
        if card.kind == Kind::Port {
            kind = if matches!(card.node, Node::Port { into: true, .. }) {
                "Package that references this one".into()
            } else {
                "Package this one references".into()
            };
        }
        let path = if container.path.is_empty() { "(repository root)".into() } else { container.path.clone() };
        facts.push(Row::fact("Path", path));
        for other in &container.other_packages {
            facts.push(Row::fact("Also", format!("{} ({})", other.name, kind_word(other.kind))));
        }
    }
    if let Node::Module(module) = &card.node {
        kind = format!("Module · {}", if module.container.is_empty() { "(root)" } else { &module.container });
    }
    if let Some((language, test, read)) = &card.file {
        kind = format!("File · {language}{}", if *test { " · test" } else { "" });
        if let Node::File(path) = &card.node {
            facts.push(Row::fact("Path", path.clone()));
        }
        facts.push(Row::fact(
            "Read",
            match read {
                ReadStatus::Complete => "complete".into(),
                ReadStatus::Unsupported => "not read: unsupported".into(),
                ReadStatus::Failed => "read failed".into(),
            },
        ));
    }
    if let Some(definition) = &card.definition {
        kind = format!("Definition · {}", definition.kind);
        let file = match at {
            Some(SystemAt::File { path }) => path.as_str(),
            _ => "",
        };
        facts.push(Row::fact("Source", format!("{file}:{}", definition.line)));
        facts.push(Row::fact("Lines", format!("{}–{}", definition.line, definition.end_line)));
    }
    if let Some((n, noun)) = card.more {
        kind = "Not listed".into();
        facts.push(Row::fact("Count", format!("{} {noun}", count(n))));
        if card.lines > 0 {
            facts.push(Row::fact("Lines", count(card.lines)));
        }
        sections.push(Section {
            title: None,
            rows: facts,
            note: Some(format!("The level lists the first {noun} and counts the rest here.")),
        });
        return Inspector { kind, title: card.name.clone(), sections, open: None };
    }
    if let Some(stats) = &card.stats {
        facts.push(Row::fact("Files", count(card.files())));
        facts.push(Row::fact("Lines", count(stats.lines)));
    } else if card.file.is_some() {
        facts.push(Row::fact("Lines", count(card.lines)));
    }
    sections.push(Section::facts(None, facts));
    if let Some(stats) = &card.stats {
        let mut langs: Vec<_> = stats.languages.iter().collect();
        langs.sort_by(|a, b| b.1.cmp(a.1));
        if !langs.is_empty() {
            let rows = langs
                .into_iter()
                .map(|(l, n)| Row::fact(l, format!("{} · {}%", count(*n), percent(*n, stats.lines))))
                .collect();
            sections.push(Section::facts(Some("Languages"), rows));
        }
        sections.push(Section::facts(
            Some("Activity"),
            vec![
                Row::fact("30 days", activity(&stats.days_30)),
                Row::fact("7 days", activity(&stats.days_7)),
                Row::fact(
                    "Last commit",
                    stats
                        .days_30
                        .last_commit
                        .clone()
                        .map(|c| c.chars().take(9).collect())
                        .unwrap_or_else(|| "none in 30 days".into()),
                ),
            ],
        ));
        sections.push(Section::facts(
            Some("Coverage"),
            vec![
                Row::fact("Read", count(stats.complete)),
                Row::fact("Unsupported", count(stats.unsupported)),
                Row::fact("Failed", count(stats.failed)),
            ],
        ));
    }
    let edges = |out: bool| -> Vec<Row> {
        let mut list: Vec<&Edge> = scene
            .all_edges()
            .filter(|e| if out { e.from == card.node } else { e.to == card.node })
            .collect();
        list.sort_by(|a, b| b.count.cmp(&a.count));
        list.into_iter()
            .map(|e| {
                let other = if out { &e.to } else { &e.from };
                Row {
                    label: scene.name(other),
                    value: edge_value(e),
                    pick: Some(Pick::Edge(e.from.clone(), e.to.clone())),
                    swatch: Some(e.kind),
                }
            })
            .collect()
    };
    for (title, rows) in [("References", edges(true)), ("Referenced by", edges(false))] {
        if !rows.is_empty() {
            sections.push(Section::facts(Some(&format!("{title} · {}", rows.len())), rows));
        }
    }
    if !card.spawns.is_empty() {
        let rows = card
            .spawns
            .iter()
            .map(|(process, sites)| Row::fact(process, plural(*sites, "site", "sites")))
            .collect();
        sections.push(Section::facts(Some("Spawns"), rows));
    }
    let open = card.node.opens().map(|_| {
        let words = match card.kind {
            Kind::File => format!("Open {} definitions", card.name),
            _ => format!("Open {}", card.name),
        };
        (words, card.node.clone())
    });
    Inspector { kind, title: card.name.clone(), sections, open }
}

fn edge_value(edge: &Edge) -> String {
    match edge.kind {
        EdgeKind::Declared => format!("declared · {}", count(edge.count)),
        EdgeKind::DeclaredOnly => "declared · 0".into(),
        EdgeKind::ReferencedOnly => format!("not declared · {}", count(edge.count)),
        EdgeKind::Module | EdgeKind::Boundary => count(edge.count),
    }
}

fn edge_inspector(scene: &Scene, edge: &Edge) -> Inspector {
    let what = match edge.kind {
        EdgeKind::Declared => "Declared in the manifest and referenced in source.",
        EdgeKind::DeclaredOnly => "Declared in the manifest; no reference found in source.",
        EdgeKind::ReferencedOnly => "Referenced in source, not declared in the manifest.",
        EdgeKind::Module => "Source in one module refers to the other.",
        EdgeKind::Boundary => "Module references across the package's boundary.",
    };
    let mut facts = vec![Row::fact("References", count(edge.count))];
    if let Some(basis) = edge.basis {
        facts.push(Row::fact(
            "Basis",
            match basis {
                ReferenceBasis::Syntax => "syntax: an import or path seen in source text".into(),
                ReferenceBasis::Resolved => "resolved by an extension".into(),
            },
        ));
    }
    let mut sections = vec![Section {
        title: None,
        rows: facts,
        note: Some(what.into()),
    }];
    if !edge.pairs.is_empty() {
        let rows = edge
            .pairs
            .iter()
            .take(12)
            .map(|(a, b, n)| Row::fact(&format!("{} → {}", module_name(a), module_name(b)), count(*n)))
            .collect();
        sections.push(Section::facts(Some(&format!("Module pairs · {}", edge.pairs.len())), rows));
    }
    let ends = [&edge.from, &edge.to]
        .into_iter()
        .map(|node| Row {
            label: format!("Select {}", scene.name(node)),
            value: String::new(),
            pick: Some(Pick::Node(node.clone())),
            swatch: None,
        })
        .collect();
    sections.push(Section::facts(None, ends));
    Inspector {
        kind: capital(edge.kind.word()),
        title: format!("{} → {}", scene.name(&edge.from), scene.name(&edge.to)),
        sections,
        open: None,
    }
}

fn activity(days: &SystemActivity) -> String {
    if days.commits == 0 {
        return "no commits".into();
    }
    format!(
        "{} · +{} −{}",
        plural(days.commits, "commit", "commits"),
        count(days.added),
        count(days.removed)
    )
}

fn percent(n: u64, of: u64) -> u64 {
    if of == 0 { 0 } else { (n * 100 + of / 2) / of }
}

fn capital(words: &str) -> String {
    let mut chars = words.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
