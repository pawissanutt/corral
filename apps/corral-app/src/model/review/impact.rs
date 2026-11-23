//! A task page's Changes › Impact (docs/plans/system-map.md §3.3): what the task's submitted
//! change moved in its project's structure, read from the `system_review` detail. The map is
//! the project level at the task's head, drawn by Activity › System's map with the touched
//! packages marked with their lines added and removed, the packages holding dependents marked
//! apart, and the rest dimmed. Under it the touched modules, then the Model impact list: the
//! eight fixed categories in their order, each with its items, an explicit "none" or "not
//! provided" when nothing supplies it, and what was left out of the list.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::detail::{DetailBody, DetailRef};
use corral_types::{
    ChangeKind, ModuleId, PackageEdgeKind, ReadStatus, SystemImpactCategory as Category, SystemMap,
    SystemMapRow, SystemProvider, SystemReview, SystemReviewRow as Row, SystemStatus,
};

use crate::model::daemon::Feed;
use crate::model::system::marks::{Mark, Marks};
use crate::model::system::scene::{Scene, build, module_name};
use crate::model::system::{Node, Pick, count, plural};

/// The Changes tab's two views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Diff,
    Impact,
}

impl View {
    pub const ALL: [(View, &'static str); 2] = [(View::Diff, "Diff"), (View::Impact, "Impact")];
}

/// The view's state on this device; the pick and the open lists belong to one task.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub view: View,
    pub task: Option<String>,
    /// The selected item's key.
    pub item: Option<String>,
    pub picked: Option<Pick>,
    /// Categories listed in full, by their place in `Category::ALL`.
    pub more: BTreeSet<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    View(View),
    /// Selects an item and its node; selecting it again lets go.
    Item {
        key: String,
        node: Option<Node>,
    },
    /// Selects a node or an edge on the map; selecting it again lets go.
    Pick(Pick),
    More(usize),
}

impl State {
    /// Starts over when another task's page shows the view.
    pub fn follow(&mut self, task: &str) {
        if self.task.as_deref() != Some(task) {
            *self = State {
                view: self.view,
                task: Some(task.to_owned()),
                ..State::default()
            };
        }
    }

    pub fn act(&mut self, action: Action) {
        match action {
            Action::View(view) => self.view = view,
            Action::Item { key, node } => {
                if self.item.as_deref() == Some(key.as_str()) {
                    self.item = None;
                    self.picked = None;
                } else {
                    self.item = Some(key);
                    self.picked = node.map(Pick::Node);
                }
            }
            Action::Pick(pick) => {
                self.item = None;
                self.picked = (self.picked.as_ref() != Some(&pick)).then_some(pick);
            }
            Action::More(index) => {
                self.more.insert(index);
            }
        }
    }
}

pub fn request(task: &str) -> DetailRef {
    DetailRef::SystemReview {
        task: task.to_owned(),
    }
}

/// Items a category lists before "Show more".
pub const LIMIT: usize = 8;

pub const HEADING: &str = "Model impact";
pub const LEAD: &str = "What this change moved in the project's facts. Select an item for its source anchor; the map follows it.";
pub const TOUCHED: &str = "Touched";
pub const OPEN_IN_SYSTEM: &str = "Open in System";
pub const NO_ANCHOR: &str = "No source line for this item.";
pub const READING: &str = "Reading the change's impact…";
pub const NO_MODULE: &str = "No module changed.";
/// The map's legend where it is drawn (rings) and where it is listed (figures).
pub const LEGEND_DRAWN: &str = "Ringed: touched, with lines added and removed. Dashed ring: holds dependents. Dimmed: untouched.";
pub const LEGEND_LISTED: &str = "Each package shows its lines added and removed, the dependents it holds, or untouched (dimmed).";

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

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
    pub sign: Sign,
    pub main: String,
    pub right: String,
    pub sub: Option<String>,
    /// `path:line @ head`, where the item has a place in the source.
    pub anchor: Option<String>,
    /// The map's node it selects.
    pub node: Option<Node>,
}

/// What a category's answer is: nothing supplies it, an empty answer, or rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supply {
    NotProvided,
    None,
    Rows(usize),
}

impl Supply {
    pub fn figure(self) -> String {
        match self {
            Supply::NotProvided => "not provided".into(),
            Supply::None => "none".into(),
            Supply::Rows(n) => plural(n as u64, "item", "items"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// Its place in `Category::ALL`.
    pub index: usize,
    pub title: &'static str,
    pub supply: Supply,
    /// Said when the list is empty.
    pub empty: Option<String>,
    /// What the list leaves out and why.
    pub notes: Vec<String>,
    pub items: Vec<Item>,
    /// Items behind "Show more".
    pub hidden: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Impact {
    pub project: String,
    pub scene: Scene,
    pub marks: Marks,
    pub summary: String,
    pub notices: Vec<String>,
    pub touched: Vec<Item>,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    Loading,
    /// Why there is nothing to map: no submitted change, still reading, or a failed read.
    Said(String),
    Ready(Box<Impact>),
}

pub const NO_CHANGE: &str =
    "No submitted change: the task's worker has not submitted a run, so there is nothing to map.";
pub const BUILDING: &str =
    "Corral is still reading this change's facts. The map and the list appear when it is done.";

/// `more`: the categories listed in full.
pub fn shown(feed: &Feed, task: &str, more: &BTreeSet<usize>) -> Shown {
    let Some(cache) = feed.detail(&request(task)) else {
        return Shown::Loading;
    };
    let review = match (cache.body(), &cache.error) {
        (Some(DetailBody::SystemReview(review)), _) => review,
        // corrald answers not_found for a missing task or project too; only this one is ours.
        (_, Some(error)) if error.code == "not_found" && error.message.contains("submitted") => {
            return Shown::Said(NO_CHANGE.into());
        }
        (_, Some(error)) => {
            return Shown::Said(format!("Could not read the impact: {}", error.message));
        }
        _ => return Shown::Loading,
    };
    if review.status == SystemStatus::Building && review.rows.is_empty() {
        return Shown::Said(BUILDING.into());
    }
    Shown::Ready(Box::new(impact(review, more)))
}

/// The container a repository path belongs to: the deepest container holding it.
fn container_of<'a>(containers: &'a [String], path: &str) -> Option<&'a String> {
    containers
        .iter()
        .filter(|c| c.is_empty() || path == c.as_str() || path.starts_with(&format!("{c}/")))
        .max_by_key(|c| c.len())
}

fn short(commit: &str) -> &str {
    commit.get(..10).unwrap_or(commit)
}

fn module_words(module: &ModuleId) -> String {
    if module.container.is_empty() {
        module_name(module)
    } else {
        format!("{} › {}", module.container, module_name(module))
    }
}

fn churn(added: u64, removed: u64) -> String {
    format!("+{} −{}", count(added), count(removed))
}

const fn title(category: Category) -> &'static str {
    match category {
        Category::PackageDependencies => "Package dependencies",
        Category::ModuleReferences => "Module references",
        Category::Definitions => "Definitions",
        Category::CallEdges => "Call edges",
        Category::ExternalProcesses => "External processes",
        Category::Dependents => "Dependents",
        Category::Tests => "Tests",
        Category::NotRead => "Not read",
    }
}

const fn zero(category: Category) -> &'static str {
    match category {
        Category::PackageDependencies => "No manifest, dependency or package edge changed.",
        Category::ModuleReferences => "No module reference count changed.",
        Category::Definitions => "No definition added, removed or changed.",
        Category::CallEdges => "No call edge added or removed.",
        Category::ExternalProcesses => "No process spawn site added or removed.",
        Category::Dependents => "No code outside the change references a changed definition.",
        Category::Tests => "No test file changed and no test selected.",
        Category::NotRead => "Every changed file was read.",
    }
}

/// Whether a facts extension supplies (some of) a category.
const fn provider_supplied(category: Category) -> bool {
    matches!(
        category,
        Category::Definitions | Category::CallEdges | Category::Dependents | Category::Tests
    )
}

/// What the extension left out of its answer, by its reason: it does not split them by
/// category, so each category it supplies says them.
fn provider_omissions(provider: &SystemProvider) -> Vec<String> {
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

const fn category_of(row: &Row) -> Option<Category> {
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
        Row::Map { .. } | Row::Container { .. } | Row::Module { .. } | Row::File { .. } => {
            return None;
        }
    })
}

struct Ctx<'a> {
    containers: Vec<String>,
    head: &'a str,
    base: &'a str,
}

impl Ctx<'_> {
    fn node(&self, path: &str) -> Option<Node> {
        container_of(&self.containers, path).map(|c| Node::Container(c.clone()))
    }
    fn anchor(&self, path: &str, line: Option<u64>, at_base: bool) -> String {
        let place = match line {
            Some(line) => format!("{path}:{line}"),
            None => path.to_owned(),
        };
        if at_base {
            format!("{place} @ {} (base)", short(self.base))
        } else {
            format!("{place} @ {} (head)", short(self.head))
        }
    }
}

fn item(key: String, sign: Sign, main: String, right: String) -> Item {
    Item {
        key,
        sign,
        main,
        right,
        sub: None,
        anchor: None,
        node: None,
    }
}

fn item_of(cx: &Ctx, n: usize, row: &Row) -> Option<Item> {
    let key = format!("row-{n}");
    Some(match row {
        Row::Manifest { path } => Item {
            sub: Some("manifest changed".into()),
            anchor: Some(cx.anchor(path, None, false)),
            node: cx.node(path),
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
                anchor: Some(cx.anchor(&fact.manifest, None, after.is_none())),
                node: Some(Node::Container(fact.container.clone())),
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
                PackageEdgeKind::ReferencedOnly => "referenced, not declared",
            };
            let right = match (before, after) {
                (Some(b), Some(a)) => format!("{} → {}", kind(b.kind), kind(a.kind)),
                _ => kind(edge.kind).to_owned(),
            };
            Item {
                sub: Some("package edge".into()),
                node: Some(Node::Container(edge.from.clone())),
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
            node: Some(Node::Container(from.container.clone())),
            ..item(
                key,
                if after > before {
                    Sign::Added
                } else {
                    Sign::Removed
                },
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
            anchor: Some(cx.anchor(path, Some(*line), *change == ChangeKind::Removed)),
            node: cx.node(path),
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
            Item {
                sub: Some(path.clone()),
                anchor: Some(cx.anchor(path, Some(*line), false)),
                node: Some(Node::Container(from.container.clone())),
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
            anchor: Some(cx.anchor(path, None, *after == 0)),
            node: cx.node(path),
            ..item(
                key,
                if after > before {
                    Sign::Added
                } else {
                    Sign::Removed
                },
                process.clone(),
                format!("{before} → {after} sites"),
            )
        },
        Row::Dependent { dependent } => Item {
            sub: Some(format!("{} · via {}", dependent.path, dependent.of)),
            anchor: Some(cx.anchor(&dependent.path, Some(dependent.line), false)),
            node: cx.node(&dependent.path),
            ..item(
                key,
                Sign::Dependent,
                dependent.name.clone(),
                format!(":{}", dependent.line),
            )
        },
        Row::TestFile { path } => Item {
            sub: Some("test file changed".into()),
            anchor: Some(cx.anchor(path, None, false)),
            node: cx.node(path),
            ..item(key, Sign::Changed, path.clone(), String::new())
        },
        Row::TestCandidate { candidate } => Item {
            sub: Some(format!("{} · test candidate", candidate.path)),
            anchor: Some(cx.anchor(&candidate.path, None, false)),
            node: cx.node(&candidate.path),
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
            anchor: Some(cx.anchor(path, None, false)),
            node: cx.node(path),
            ..item(key, Sign::Neutral, path.clone(), String::new())
        },
        Row::Map { .. } | Row::Container { .. } | Row::Module { .. } | Row::File { .. } => {
            return None;
        }
    })
}

fn name_or_root(path: &str) -> String {
    if path.is_empty() {
        "(root)".into()
    } else {
        path.to_owned()
    }
}

pub fn impact(review: &SystemReview, more: &BTreeSet<usize>) -> Impact {
    let map = SystemMap {
        project: review.project.clone(),
        head: Some(review.head.clone()),
        status: review.status,
        error: None,
        provider: None,
        coverage: None,
        omitted_rows: 0,
        rows: review
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Map { row } => Some(row.clone()),
                _ => None,
            })
            .collect(),
    };
    let containers: Vec<String> = map
        .rows
        .iter()
        .filter_map(|row| match row {
            SystemMapRow::Container { container, .. } => Some(container.path.clone()),
            _ => None,
        })
        .collect();
    let cx = Ctx {
        containers,
        head: &review.head,
        base: &review.base,
    };
    let scene = build(&map, None);

    let mut marks = Marks::new();
    let mut touched = Vec::new();
    let mut modules = 0u64;
    let mut files = 0u64;
    let mut dependents: BTreeMap<Node, u64> = BTreeMap::new();
    let mut listed: BTreeMap<usize, Vec<Item>> = BTreeMap::new();
    for (n, row) in review.rows.iter().enumerate() {
        match row {
            Row::Container {
                path,
                added,
                removed,
            } => {
                marks.insert(
                    Node::Container(path.clone()),
                    Mark::Touched {
                        added: *added,
                        removed: *removed,
                    },
                );
            }
            Row::Module {
                module,
                added,
                removed,
            } => {
                modules += 1;
                touched.push(Item {
                    sub: Some(plural(added + removed, "line changed", "lines changed")),
                    node: Some(Node::Container(module.container.clone())),
                    ..item(
                        format!("row-{n}"),
                        Sign::Changed,
                        module_words(module),
                        churn(*added, *removed),
                    )
                });
            }
            Row::File { .. } => files += 1,
            Row::Dependent { dependent } => {
                if let Some(node) = cx.node(&dependent.path) {
                    *dependents.entry(node).or_default() += 1;
                }
            }
            _ => {}
        }
        if let Some(category) = category_of(row)
            && let Some(index) = Category::ALL.iter().position(|c| *c == category)
            && let Some(item) = item_of(&cx, n, row)
        {
            listed.entry(index).or_default().push(item);
        }
    }
    let dependent_packages = dependents.len();
    for (node, definitions) in dependents {
        marks.entry(node).or_insert(Mark::Dependent { definitions });
    }
    let packages = marks
        .values()
        .filter(|m| matches!(m, Mark::Touched { .. }))
        .count() as u64;
    let summary = format!(
        "Touched {} · {} · {} · dependents in {}",
        plural(packages, "package", "packages"),
        plural(modules, "module", "modules"),
        plural(files, "file", "files"),
        plural(dependent_packages as u64, "package", "packages"),
    );

    let mut sections = Vec::new();
    for (index, category) in Category::ALL.into_iter().enumerate() {
        // Continuation pages repeat the summaries; the first is the answer.
        let summary = review.categories.iter().find(|c| c.category == category);
        let provided = summary.is_some_and(|s| s.provided);
        let total = summary.map_or(0, |s| s.total);
        let items: Vec<Item> = listed.remove(&index).unwrap_or_default();
        let mut notes = Vec::new();
        if provided && total > items.len() {
            notes.push(format!(
                "{} not listed: left out to stay within the reply's size.",
                count((total - items.len()) as u64)
            ));
        }
        if provider_supplied(category)
            && let Some(provider) = &review.provider
        {
            notes.extend(provider_omissions(provider));
        }
        if category == Category::Tests
            && !items.iter().any(|i| i.sign == Sign::Neutral)
            && !review
                .provider
                .as_ref()
                .is_some_and(|p| p.coverage.provided.iter().any(|c| c == "tests"))
        {
            notes.push("Test candidates not provided: no extension supplies them.".into());
        }
        let hidden = if more.contains(&index) {
            0
        } else {
            items.len().saturating_sub(LIMIT)
        };
        let (supply, empty) = if !provided {
            (
                Supply::NotProvided,
                Some("Not provided: nothing in this project supplies this category.".to_owned()),
            )
        } else if total == 0 {
            (Supply::None, Some(zero(category).to_owned()))
        } else {
            (Supply::Rows(total), None)
        };
        sections.push(Section {
            index,
            title: title(category),
            supply,
            empty,
            notes,
            items,
            hidden,
        });
    }

    let mut notices = Vec::new();
    if let Some(error) = &review.error {
        notices.push(format!("Reading the change failed: {error}"));
    }
    if review.status == SystemStatus::Building {
        notices.push("Corral is still reading this change; what is shown may grow.".into());
    }
    if let Some(error) = review.provider.as_ref().and_then(|p| p.error.as_ref()) {
        notices.push(format!("The facts extension failed: {error}"));
    }
    if review.base_estimated {
        notices.push(format!(
            "The base {} was inferred: this submission predates recorded bases.",
            short(&review.base)
        ));
    }
    if review.skipped_non_utf8_paths > 0 {
        notices.push(format!(
            "{} skipped: not UTF-8.",
            plural(review.skipped_non_utf8_paths, "path", "paths")
        ));
    }
    if review.omitted_rows > 0 {
        notices.push(format!(
            "{} left out to stay within the reply's size.",
            plural(review.omitted_rows as u64, "row", "rows")
        ));
    }
    Impact {
        project: review.project.clone(),
        scene,
        marks,
        summary,
        notices,
        touched,
        sections,
    }
}

#[cfg(test)]
#[path = "impact_tests.rs"]
mod tests;
