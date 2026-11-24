//! The Model impact list's items: one per served impact row, with how it changed, its source
//! anchor at the commit its line belongs to, and the map level and node it selects.

use std::collections::{BTreeMap, BTreeSet};

use corral_types::{
    ChangeKind, ModuleId, PackageEdgeKind, ReadStatus, RuntimeEdgeKind, RuntimeSource, RuntimeUnit,
    RuntimeUnitKind, SystemAt, SystemImpactCategory as Category, SystemProvider,
    SystemReviewRow as Row,
};

use crate::model::system::runtime::{at, basis_word, entry_word};
use crate::model::system::scene::{module_name, unit_word};
use crate::model::system::{Node, count};

/// How an item changed, by shape as well as colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    Added,
    Removed,
    Changed,
    Dependent,
    Neutral,
}

impl Sign {
    pub const fn glyph(self) -> &'static str {
        match self {
            Sign::Added => "+",
            Sign::Removed => "−",
            Sign::Changed => "Δ",
            Sign::Dependent => "d",
            Sign::Neutral => "·",
        }
    }
    pub const fn word(self) -> &'static str {
        match self {
            Sign::Added => "added",
            Sign::Removed => "removed",
            Sign::Changed => "changed",
            Sign::Dependent => "dependent",
            Sign::Neutral => "",
        }
    }
    fn of(change: ChangeKind) -> Sign {
        match change {
            ChangeKind::Added => Sign::Added,
            ChangeKind::Removed => Sign::Removed,
            ChangeKind::Changed => Sign::Changed,
        }
    }
    fn between<T>(before: &Option<T>, after: &Option<T>) -> Sign {
        match (before, after) {
            (None, Some(_)) => Sign::Added,
            (Some(_), None) => Sign::Removed,
            _ => Sign::Changed,
        }
    }
}

/// Where an item sits in the source, at the commit holding that line: the base for what the
/// change removed, the head otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub path: String,
    pub line: Option<u64>,
    pub commit: String,
    pub base: bool,
}

impl Anchor {
    pub fn words(&self) -> String {
        let place = match self.line {
            Some(line) => format!("{}:{line}", self.path),
            None => self.path.clone(),
        };
        let side = if self.base { "base" } else { "head" };
        format!("{place} @ {} ({side})", short(&self.commit))
    }
}

/// The map level an item opens and the node it selects there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The levels drilled into, outermost first; empty at the project level.
    pub trail: Vec<SystemAt>,
    pub node: Node,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
    pub sign: Sign,
    pub main: String,
    pub right: String,
    pub sub: Option<String>,
    pub anchor: Option<Anchor>,
    pub target: Option<Target>,
    /// Why the map selects something else than the item's own node, or nothing.
    pub note: Option<String>,
    /// A lens the change touches, drawn with what it touched marked.
    pub lens: Option<LensItem>,
}

/// A touched lens: which to read, at which commit, and the elements the change touched,
/// added and removed, by the checker's names.
#[derive(Debug, Clone, PartialEq)]
pub struct LensItem {
    pub id: String,
    pub at: String,
    pub touched: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

pub(super) fn short(commit: &str) -> &str {
    commit.get(..10).unwrap_or(commit)
}

/// The container a repository path belongs to: the deepest container holding it.
pub(super) fn container_of<'a>(containers: &'a [String], path: &str) -> Option<&'a String> {
    containers
        .iter()
        .filter(|c| c.is_empty() || path == c.as_str() || path.starts_with(&format!("{c}/")))
        .max_by_key(|c| c.len())
}

pub(super) fn module_words(module: &ModuleId) -> String {
    if module.container.is_empty() {
        module_name(module)
    } else {
        format!("{} › {}", module.container, module_name(module))
    }
}

pub(super) fn churn(added: u64, removed: u64) -> String {
    format!("+{} −{}", count(added), count(removed))
}

fn name_or_root(path: &str) -> String {
    if path.is_empty() {
        "(root)".into()
    } else {
        path.to_owned()
    }
}

pub(super) const fn title(category: Category) -> &'static str {
    match category {
        Category::PackageDependencies => "Package dependencies",
        Category::ModuleReferences => "Module references",
        Category::Definitions => "Definitions",
        Category::CallEdges => "Call edges",
        Category::ExternalProcesses => "External processes",
        Category::Runtime => "Runtime",
        Category::Dependents => "Dependents",
        Category::Tests => "Tests",
        Category::NotRead => "Not read",
        Category::EntryPoints => "Entry points",
        Category::Lenses => "Lenses",
    }
}

pub(super) const fn zero(category: Category) -> &'static str {
    match category {
        Category::PackageDependencies => "No manifest, dependency or package edge changed.",
        Category::ModuleReferences => "No module reference count changed.",
        Category::Definitions => "No definition added, removed or changed.",
        Category::CallEdges => "No call edge added or removed.",
        Category::ExternalProcesses => "No process spawn site added or removed.",
        Category::Runtime => "No runtime unit or edge changed.",
        Category::Dependents => "No code outside the change references a changed definition.",
        Category::Tests => "No test file changed and no test selected.",
        Category::NotRead => "Every changed file was read.",
        Category::EntryPoints => "No entry point added or removed.",
        Category::Lenses => "No lens element touched, added or removed.",
    }
}

/// Whether a facts extension supplies (some of) a category.
pub(super) const fn provider_supplied(category: Category) -> bool {
    matches!(
        category,
        Category::Definitions | Category::CallEdges | Category::Dependents | Category::Tests
    )
}

/// What the extension left out of its answer, by its reason: it does not split them by
/// category, so each category it supplies says them.
pub(super) fn provider_omissions(provider: &SystemProvider) -> Vec<String> {
    let said = |counts: &BTreeMap<String, u64>, verb: &str| {
        counts
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(reason, n)| {
                format!(
                    "{} {verb} by {} ({}).",
                    count(*n),
                    provider.extension,
                    reason.replace('_', " ")
                )
            })
            .collect::<Vec<_>>()
    };
    let mut out = said(&provider.coverage.omitted, "omitted");
    out.extend(said(&provider.coverage.skipped, "skipped"));
    out
}

pub(super) const fn category_of(row: &Row) -> Option<Category> {
    Some(match row {
        Row::Manifest { .. } | Row::Dependency { .. } | Row::PackageEdge { .. } => {
            Category::PackageDependencies
        }
        Row::ModuleReference { .. } => Category::ModuleReferences,
        Row::Definition { .. } => Category::Definitions,
        Row::CallEdge { .. } => Category::CallEdges,
        Row::Spawn { .. } => Category::ExternalProcesses,
        Row::Dependent { .. } => Category::Dependents,
        Row::TestFile { .. } | Row::TestCandidate { .. } => Category::Tests,
        Row::NotRead { .. } => Category::NotRead,
        Row::RuntimeUnit { .. } | Row::RuntimeEdge { .. } => Category::Runtime,
        Row::Entry { .. } => Category::EntryPoints,
        Row::Lens { .. } => Category::Lenses,
        Row::Map { .. } | Row::Container { .. } | Row::Module { .. } | Row::File { .. } => {
            return None;
        }
    })
}

/// What items resolve against: the head's containers and the module of each file the review
/// names.
pub(super) struct Ctx<'a> {
    pub containers: Vec<String>,
    pub files: BTreeMap<String, ModuleId>,
    /// Files the change deleted: they exist at the base only.
    pub deleted: BTreeSet<String>,
    pub head: &'a str,
    pub base: &'a str,
}

impl Ctx<'_> {
    pub fn container(&self, path: &str) -> Option<Target> {
        container_of(&self.containers, path).map(|c| container_target(c))
    }

    /// A file's node in its module's level, or its package where its module is not known.
    pub fn file(&self, path: &str) -> Option<Target> {
        match self.files.get(path) {
            Some(module) => Some(Target {
                trail: vec![
                    SystemAt::Container {
                        path: module.container.clone(),
                    },
                    SystemAt::Module {
                        module: module.clone(),
                    },
                ],
                node: Node::File(path.to_owned()),
            }),
            None => self.container(path),
        }
    }

    fn anchor(&self, path: &str, line: Option<u64>, base: bool) -> Option<Anchor> {
        Some(Anchor {
            path: path.to_owned(),
            line,
            commit: if base { self.base } else { self.head }.to_owned(),
            base,
        })
    }
}

pub(super) fn container_target(path: &str) -> Target {
    Target {
        trail: Vec::new(),
        node: Node::Container(path.to_owned()),
    }
}

pub(super) fn module_target(module: &ModuleId) -> Target {
    Target {
        trail: vec![SystemAt::Container {
            path: module.container.clone(),
        }],
        node: Node::Module(module.clone()),
    }
}

pub(super) fn item(key: String, sign: Sign, main: String, right: String) -> Item {
    Item {
        key,
        sign,
        main,
        right,
        sub: None,
        anchor: None,
        target: None,
        note: None,
        lens: None,
    }
}

fn grew(before: u64, after: u64) -> Sign {
    if after > before {
        Sign::Added
    } else {
        Sign::Removed
    }
}

/// An item; one whose file the change deleted is anchored at the base, and the map selects the
/// deepest package at the head that held it, or says there is none.
pub(super) fn item_of(cx: &Ctx, n: usize, row: &Row) -> Option<Item> {
    let mut item = row_item(cx, n, row)?;
    let Some(anchor) = item.anchor.as_mut() else {
        return Some(item);
    };
    if !cx.deleted.contains(&anchor.path) {
        return Some(item);
    }
    anchor.base = true;
    anchor.commit = cx.base.to_owned();
    let path = anchor.path.clone();
    item.target = cx.container(&path);
    item.note = Some(match &item.target {
        Some(Target {
            node: Node::Container(owner),
            ..
        }) => format!(
            "Deleted by this change, so not on the head's map; {}, the deepest package there \
             that contained it, is selected.",
            if owner.is_empty() { "(root)" } else { owner }
        ),
        _ => "Deleted by this change, and no package that held it is on the head's map.".into(),
    });
    Some(item)
}

fn row_item(cx: &Ctx, n: usize, row: &Row) -> Option<Item> {
    let key = format!("row-{n}");
    Some(match row {
        Row::Manifest { path } => Item {
            sub: Some("manifest changed".into()),
            anchor: cx.anchor(path, None, false),
            target: cx.container(path),
            ..item(key, Sign::Changed, path.clone(), String::new())
        },
        Row::Dependency { before, after } => {
            let fact = after.as_ref().or(before.as_ref())?;
            let requirement = match (before, after) {
                (Some(b), Some(a)) if b.requirement != a.requirement => {
                    format!("{} → {}", b.requirement, a.requirement)
                }
                _ => fact.requirement.clone(),
            };
            Item {
                sub: Some(format!("{} · {}", fact.manifest, fact.section)),
                anchor: cx.anchor(&fact.manifest, None, after.is_none()),
                target: Some(container_target(&fact.container)),
                ..item(
                    key,
                    Sign::between(before, after),
                    format!("{} → {}", name_or_root(&fact.container), fact.package),
                    requirement,
                )
            }
        }
        Row::PackageEdge { before, after } => {
            let edge = after.as_ref().or(before.as_ref())?;
            let kind = |kind: PackageEdgeKind| match kind {
                PackageEdgeKind::DeclaredAndReferenced => "declared and referenced",
                PackageEdgeKind::DeclaredOnly => "declared, no reference",
                PackageEdgeKind::ReferencedOnly => "used in code · no dependency match",
            };
            let right = match (before, after) {
                (Some(b), Some(a)) => format!("{} → {}", kind(b.kind), kind(a.kind)),
                _ => kind(edge.kind).to_owned(),
            };
            Item {
                sub: Some("package edge".into()),
                target: Some(container_target(&edge.from)),
                ..item(
                    key,
                    Sign::between(before, after),
                    format!("{} → {}", name_or_root(&edge.from), name_or_root(&edge.to)),
                    right,
                )
            }
        }
        Row::ModuleReference {
            from,
            to,
            before,
            after,
        } => Item {
            target: Some(module_target(from)),
            ..item(
                key,
                grew(*before, *after),
                format!("{} → {}", module_words(from), module_words(to)),
                format!("{before} → {after}"),
            )
        },
        Row::Definition {
            change,
            path,
            name,
            line,
        } => Item {
            sub: Some(format!("{path} · {}", Sign::of(*change).word())),
            anchor: cx.anchor(path, Some(*line), *change == ChangeKind::Removed),
            target: cx.file(path),
            ..item(key, Sign::of(*change), name.clone(), format!(":{line}"))
        },
        Row::CallEdge {
            from,
            to,
            added,
            removed,
            changed,
            path,
            line,
        } => {
            let sign = match (added, removed) {
                (a, 0) if *a > 0 => Sign::Added,
                (0, r) if *r > 0 => Sign::Removed,
                _ => Sign::Changed,
            };
            let mut right = churn(*added, *removed);
            if *changed > 0 {
                right.push_str(&format!(" Δ{}", count(*changed)));
            }
            // corrald anchors a group at the base only when every call in it was removed.
            let base = *added == 0 && *changed == 0 && *removed > 0;
            Item {
                sub: Some(path.clone()),
                anchor: cx.anchor(path, Some(*line), base),
                target: Some(module_target(from)),
                ..item(
                    key,
                    sign,
                    format!("{} → {}", module_words(from), module_words(to)),
                    right,
                )
            }
        }
        Row::Spawn {
            path,
            process,
            before,
            after,
        } => Item {
            sub: Some(path.clone()),
            anchor: cx.anchor(path, None, *after == 0),
            target: cx.file(path),
            ..item(
                key,
                grew(*before, *after),
                process.clone(),
                format!("{before} → {after} sites"),
            )
        },
        Row::Dependent { dependent, .. } => Item {
            sub: Some(format!("{} · via {}", dependent.path, dependent.of)),
            anchor: cx.anchor(&dependent.path, Some(dependent.line), false),
            target: cx.file(&dependent.path),
            ..item(
                key,
                Sign::Dependent,
                dependent.name.clone(),
                format!(":{}", dependent.line),
            )
        },
        Row::TestFile { path } => Item {
            sub: Some("test file changed".into()),
            anchor: cx.anchor(path, None, false),
            target: cx.file(path),
            ..item(key, Sign::Changed, path.clone(), String::new())
        },
        Row::TestCandidate { candidate } => Item {
            sub: Some(format!("{} · test candidate", candidate.path)),
            anchor: cx.anchor(&candidate.path, None, false),
            target: cx.file(&candidate.path),
            ..item(key, Sign::Neutral, candidate.name.clone(), String::new())
        },
        Row::NotRead { path, read_status } => Item {
            sub: Some(
                match read_status {
                    ReadStatus::Failed => "read failed",
                    _ => "not understood by Corral or an extension",
                }
                .into(),
            ),
            anchor: cx.anchor(path, None, false),
            target: cx.file(path),
            ..item(key, Sign::Neutral, path.clone(), String::new())
        },
        Row::RuntimeUnit { before, after } => {
            let unit = after.as_ref().or(before.as_ref())?;
            Item {
                sub: Some(format!(
                    "{} · {}",
                    unit_word(unit.id.kind),
                    at(&unit.id.path, unit.line)
                )),
                anchor: cx.anchor(&unit.id.path, Some(unit.line), after.is_none()),
                target: cx.file(&unit.id.path),
                ..item(
                    key,
                    Sign::between(before, after),
                    unit.id.name.clone(),
                    unit_change(before.as_ref(), after.as_ref()),
                )
            }
        }
        Row::RuntimeEdge { before, after } => {
            let edge = after.as_ref().or(before.as_ref())?;
            let from = match &edge.from {
                RuntimeSource::Container { path } => name_or_root(path),
                RuntimeSource::Unit { id } => id.name.clone(),
            };
            let kind = match edge.kind {
                RuntimeEdgeKind::Starts => "starts",
                RuntimeEdgeKind::Runs => "runs",
                RuntimeEdgeKind::DependsOn => "depends on",
                RuntimeEdgeKind::Builds => "builds",
            };
            Item {
                sub: Some(at(&edge.site.path, edge.site.line)),
                anchor: cx.anchor(&edge.site.path, Some(edge.site.line), after.is_none()),
                target: cx.file(&edge.site.path),
                ..item(
                    key,
                    Sign::between(before, after),
                    format!("{from} {kind} {}", edge.to.name),
                    basis_word(edge.basis).to_owned(),
                )
            }
        }
        Row::Entry { change, entry } => {
            let owner = match &entry.owner {
                RuntimeSource::Unit { id } => id.name.clone(),
                RuntimeSource::Container { path } => name_or_root(path),
            };
            Item {
                sub: Some(format!("{} · {owner}", at(&entry.path, entry.line))),
                anchor: cx.anchor(
                    &entry.path,
                    Some(entry.line),
                    *change == ChangeKind::Removed,
                ),
                target: cx.file(&entry.path),
                ..item(
                    key,
                    Sign::of(*change),
                    entry.name.clone(),
                    entry_word(entry.kind).to_owned(),
                )
            }
        }
        Row::Lens { impact } => lens_item(cx, key, impact),
        Row::Map { .. } | Row::Container { .. } | Row::Module { .. } | Row::File { .. } => {
            return None;
        }
    })
}

/// A touched lens: its title, how many elements the change touched, added and removed, and
/// the lens to draw: at the base when the change only removed from it.
fn lens_item(cx: &Ctx, key: String, impact: &corral_types::lenses::LensImpact) -> Item {
    let only_removed =
        impact.touched.is_empty() && impact.added.is_empty() && !impact.removed.is_empty();
    let mut right = Vec::new();
    if !impact.touched.is_empty() {
        right.push(format!("{} touched", impact.touched.len()));
    }
    if !impact.added.is_empty() {
        right.push(format!("+{}", impact.added.len()));
    }
    if !impact.removed.is_empty() {
        right.push(format!("−{}", impact.removed.len()));
    }
    let sign = if only_removed {
        Sign::Removed
    } else if impact.touched.is_empty() && impact.removed.is_empty() {
        Sign::Added
    } else {
        Sign::Changed
    };
    let listed = |word: &str, names: &[String]| {
        (!names.is_empty()).then(|| format!("{word}: {}", names.join("; ")))
    };
    let sub = [
        listed("touched", &impact.touched),
        listed("added", &impact.added),
        listed("removed", &impact.removed),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    Item {
        sub: (!sub.is_empty()).then_some(sub),
        anchor: cx.anchor(&impact.path, None, only_removed),
        lens: Some(LensItem {
            id: impact.id.clone(),
            at: if only_removed { cx.base } else { cx.head }.to_owned(),
            touched: impact.touched.clone(),
            added: impact.added.clone(),
            removed: impact.removed.clone(),
        }),
        ..item(key, sign, impact.title.clone(), right.join(" · "))
    }
}

/// What changed in a unit kept across the change: its ports, then its command, else where or
/// how it is declared.
fn unit_change(before: Option<&RuntimeUnit>, after: Option<&RuntimeUnit>) -> String {
    let (Some(b), Some(a)) = (before, after) else {
        return unit_word(
            after
                .or(before)
                .map_or(RuntimeUnitKind::Binary, |u| u.id.kind),
        )
        .into();
    };
    let list = |ports: &[String]| {
        if ports.is_empty() {
            "none".to_owned()
        } else {
            ports.join(", ")
        }
    };
    if b.ports != a.ports {
        format!("ports {} → {}", list(&b.ports), list(&a.ports))
    } else if b.command != a.command || b.entry_point != a.entry_point {
        "command changed".into()
    } else if b.depends_on != a.depends_on {
        "depends_on changed".into()
    } else {
        "declaration changed".into()
    }
}
