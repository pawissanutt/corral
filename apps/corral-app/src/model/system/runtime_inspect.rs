//! The Runtime view's inspector: where a unit is declared, its ports and command when a file
//! says them, its entry points and what starts, runs or builds it; a package's entry points
//! and the detectors that ran there ("no detector" when none applies, never zero); an edge's
//! kind with every site that establishes it, its basis and source line.

use corral_types::RuntimeUnitKind;

use super::inspect::{Inspector, Row, Section};
use super::runtime::{at, basis_word, detectors_words, edge_brief, entry_word};
use super::scene::{Card, Edge, Kind, Scene, unit_word};
use super::{Node, Pick, plural};

fn fact(label: &str, value: String) -> Row {
    Row {
        label: label.into(),
        value,
        pick: None,
        swatch: None,
    }
}

fn section(title: Option<String>, rows: Vec<Row>) -> Section {
    Section {
        title,
        rows,
        note: None,
    }
}

fn capital(words: &str) -> String {
    let mut chars = words.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

pub(super) fn inspect(scene: &Scene, pick: &Pick) -> Option<Inspector> {
    match pick {
        Pick::Node(node) => scene.card(node).map(|card| node_inspector(scene, card)),
        Pick::Edge(from, to) => scene
            .all_edges()
            .find(|e| &e.from == from && &e.to == to)
            .map(|edge| edge_inspector(scene, edge)),
    }
}

fn node_inspector(scene: &Scene, card: &Card) -> Inspector {
    let mut sections = Vec::new();
    let mut facts = Vec::new();
    let mut open = None;
    let kind = match card.kind {
        Kind::Scripts => {
            let rows = card
                .folded
                .iter()
                .map(|unit| fact(&unit.id.name, at(&unit.id.path, unit.line)))
                .collect();
            sections.push(section(None, rows));
            open = Some((
                format!(
                    "Show the {}",
                    plural(card.folded.len() as u64, "script", "scripts")
                ),
                Node::Scripts,
            ));
            "npm scripts, folded".to_owned()
        }
        Kind::Owner => {
            let path = match &card.node {
                Node::Container(path) if path.is_empty() => "(repository root)".to_owned(),
                Node::Container(path) => path.clone(),
                _ => String::new(),
            };
            facts.push(fact("Path", path));
            facts.push(fact(
                "Detectors",
                match &card.detectors {
                    None => "not reported by corrald".into(),
                    Some(set) if set.is_empty() => "no detector applies here".into(),
                    Some(set) => detectors_words(set),
                },
            ));
            "Package or folder".to_owned()
        }
        _ => {
            if let Some(unit) = &card.unit {
                facts.push(fact("Declared", at(&unit.id.path, unit.line)));
                facts.push(fact(
                    "Package",
                    if unit.container.is_empty() {
                        "(root)".into()
                    } else {
                        unit.container.clone()
                    },
                ));
                let entry = match unit.id.kind {
                    RuntimeUnitKind::Binary => "Source",
                    _ => "Entry point",
                };
                let said = [
                    ("Command", &unit.command),
                    (entry, &unit.entry_point),
                    ("Image", &unit.image),
                    ("Context", &unit.build_context),
                    ("Dockerfile", &unit.dockerfile),
                ];
                for (label, value) in said {
                    if let Some(value) = value {
                        facts.push(fact(label, value.clone()));
                    }
                }
                if !unit.ports.is_empty() {
                    facts.push(fact("Ports", unit.ports.join(", ")));
                } else if unit.id.kind == RuntimeUnitKind::Service {
                    facts.push(fact("Ports", "none declared".into()));
                }
                if !unit.depends_on.is_empty() {
                    facts.push(fact("Depends on", unit.depends_on.join(", ")));
                }
                if unit.id.kind == RuntimeUnitKind::Script {
                    open = Some(("Fold the npm scripts".into(), Node::Scripts));
                }
                format!("{} · declared", capital(unit_word(unit.id.kind)))
            } else {
                capital(card.kind.word())
            }
        }
    };
    if !facts.is_empty() {
        sections.insert(0, section(None, facts));
    }
    if card.kind != Kind::Scripts {
        let rows: Vec<Row> = card
            .entries
            .iter()
            .map(|entry| {
                fact(
                    &entry.name,
                    format!(
                        "{} · {}",
                        entry_word(entry.kind),
                        at(&entry.path, entry.line)
                    ),
                )
            })
            .collect();
        if !rows.is_empty() {
            sections.push(section(
                Some(format!("Entry points · {}", rows.len())),
                rows,
            ));
        }
    }
    let edges = |out: bool| -> Vec<Row> {
        scene
            .all_edges()
            .filter(|e| {
                if out {
                    e.from == card.node
                } else {
                    e.to == card.node
                }
            })
            .map(|e| {
                let other = if out { &e.to } else { &e.from };
                Row {
                    label: scene.name(other),
                    value: edge_brief(e),
                    pick: Some(Pick::Edge(e.from.clone(), e.to.clone())),
                    swatch: Some(e.kind),
                }
            })
            .collect()
    };
    for (title, rows) in [
        ("Started, run or built by", edges(false)),
        ("Starts, runs, builds or depends on", edges(true)),
    ] {
        if !rows.is_empty() {
            sections.push(section(Some(format!("{title} · {}", rows.len())), rows));
        }
    }
    Inspector {
        kind,
        title: card.name.clone(),
        sections,
        open,
    }
}

fn edge_inspector(scene: &Scene, edge: &Edge) -> Inspector {
    let from = scene.name(&edge.from);
    let to = scene.name(&edge.to);
    let sites = edge
        .sites
        .iter()
        .map(|site| {
            fact(
                &at(&site.site.path, site.site.line),
                format!("{} · {}", site.kind.word(), basis_word(site.basis)),
            )
        })
        .collect();
    let mut established = section(
        Some(format!("Established by · {}", edge.sites.len())),
        sites,
    );
    established.note =
        Some("Declared: a file names it. Syntax: a spawn site in source names the process.".into());
    let ends = [&edge.from, &edge.to]
        .into_iter()
        .map(|node| Row {
            label: format!("Select {}", scene.name(node)),
            value: String::new(),
            pick: Some(Pick::Node(node.clone())),
            swatch: None,
        })
        .collect();
    Inspector {
        kind: format!("Runtime edge · {}", edge.kind.word()),
        title: format!("{from} {} {to}", edge.kind.word()),
        sections: vec![established, section(None, ends)],
        open: None,
    }
}
