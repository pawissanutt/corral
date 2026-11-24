//! A task page's Changes › Impact (docs/plans/system-map.md §3.3): what the task's submitted
//! change moved in its project's structure, read from the `system_review` detail. The map is
//! the task's head, drawn by Activity › System's map at the level drilled into (packages, a
//! package's modules, a module's files, a file's definitions), each node marked with the lines
//! the change added and removed there and the dependents it holds, the rest dimmed. Under it
//! the touched modules, then the Model impact list: the ten fixed categories in their order,
//! each with its items, an explicit "none" or "not provided" when nothing supplies it, and what
//! was left out of the list. `reach` says where "Open in System" can take an item.

mod items;
mod reach;

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::detail::{DetailBody, DetailRef};
use corral_types::{
    ChangeKind, ModuleId, SystemAt, SystemImpactCategory as Category, SystemMap, SystemMapRow,
    SystemReview, SystemReviewRow as Row, SystemStatus,
};

pub use items::{Anchor, Item, LensItem, Sign, Target};
pub use reach::{CHECKING, Reach, reach, wanted};

use items::{
    Ctx, category_of, churn, container_of, item, item_of, module_target, module_words,
    provider_omissions, provider_supplied, short, title, zero,
};

use crate::model::daemon::Feed;
use crate::model::system::marks::Marks;
use crate::model::system::scene::{Scene, build, module_name};
use crate::model::system::{self as map, Node, Pick, base_name, count, plural};

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

/// The view's state on this device; the item, the map's level and pick and the open lists
/// belong to one task.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub view: View,
    pub task: Option<String>,
    /// The selected item's key.
    pub item: Option<String>,
    /// The map's trail and pick, with Activity › System's rules for both.
    pub map: map::State,
    /// Categories listed in full, by their place in `Category::ALL`.
    pub more: BTreeSet<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    View(View),
    /// Selects an item and opens its node's level with the node selected; selecting it again
    /// lets go.
    Item {
        key: String,
        target: Option<Target>,
    },
    /// The owner on the map: a pick, opening a level, a breadcrumb.
    Map(map::Action),
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
            Action::Item { key, target } => {
                if self.item.as_deref() == Some(key.as_str()) {
                    self.item = None;
                    self.map.picked = None;
                } else {
                    self.item = Some(key);
                    if let Some(target) = target {
                        self.map.trail = target.trail;
                        self.map.picked = Some(Pick::Node(target.node));
                    }
                }
            }
            Action::Map(action) => {
                self.item = None;
                self.map.act(action);
            }
            Action::More(index) => {
                self.more.insert(index);
            }
        }
    }
}

/// The impact with the head's map at `at` (the project level when none).
pub fn request(task: &str, at: Option<&SystemAt>) -> DetailRef {
    DetailRef::SystemReview {
        task: task.to_owned(),
        at: at.cloned(),
    }
}

/// Items a category lists before "Show more".
pub const LIMIT: usize = 8;

pub const HEADING: &str = "Model impact";
pub const SOURCE_UNAVAILABLE: &str =
    "Source unavailable: corrald served no committed lines for this change's anchor.";
pub const LEAD: &str = "What this change moved in the project's facts. Select an item for its source anchor; the map follows it.";
pub const TOUCHED: &str = "Touched";
pub const OPEN_IN_SYSTEM: &str = "Open in System";
pub const NO_ANCHOR: &str = "No source line for this item.";
pub const NO_NODE: &str = "No node on the map for this item.";
pub const READING: &str = "Reading the change's impact…";
pub const READING_LEVEL: &str = "Reading this level at the task's head…";
pub const NO_LEVELS: &str =
    "This corrald serves a task's map at the project level only; update it to open a level.";
pub const NO_MODULE: &str = "No module changed.";
pub const STALE: &str = "Shown as last read; corrald has not confirmed it since.";
/// The map's legend where it is drawn (rings) and where it is listed (figures).
pub const LEGEND_DRAWN: &str = "Ringed: touched, with lines added and removed. Dashed ring: holds dependents. Dimmed: untouched. Select a node again to open it.";
pub const LEGEND_LISTED: &str = "Each node shows its lines added and removed, the dependents it holds, or untouched (dimmed). Select a node again to open it.";

/// What a category's answer is: nothing supplies it, an answer still awaited from the
/// configured extension, an empty answer, or rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supply {
    NotProvided,
    Unanswered,
    None,
    Rows(usize),
}

impl Supply {
    pub fn figure(self) -> String {
        match self {
            Supply::NotProvided => "not provided".into(),
            Supply::Unanswered => "not answered".into(),
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
    /// The level drilled into, as a scene; none while it is read or when it cannot be.
    pub scene: Option<Scene>,
    /// Said in the map's place when there is no scene.
    pub map_said: Option<String>,
    /// The breadcrumb: the project, then each level drilled into.
    pub crumbs: Vec<String>,
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

fn review_of<'a>(feed: &'a Feed, target: &DetailRef) -> Option<(&'a SystemReview, bool)> {
    let cache = feed.detail(target)?;
    match cache.body()? {
        // A kept body after a failed or pending revalidation is not confirmed current.
        DetailBody::SystemReview(review) => Some((review, cache.stale || cache.error.is_some())),
        _ => None,
    }
}

/// The impact of task `task` with its map at the end of `trail`; `more`: the categories listed
/// in full.
pub fn shown(feed: &Feed, task: &str, trail: &[SystemAt], more: &BTreeSet<usize>) -> Shown {
    let root = request(task, None);
    let Some((review, stale)) = review_of(feed, &root) else {
        return match feed.detail(&root).and_then(|cache| cache.error.as_ref()) {
            // corrald answers not_found for a missing task or project too; only this one is ours.
            Some(error) if error.code == "not_found" && error.message.contains("submitted") => {
                Shown::Said(NO_CHANGE.into())
            }
            Some(error) => Shown::Said(format!("Could not read the impact: {}", error.message)),
            None => Shown::Loading,
        };
    };
    if review.status == SystemStatus::Building && review.rows.is_empty() {
        return Shown::Said(BUILDING.into());
    }
    let extension = feed
        .systems
        .get(review.project.as_str())
        .and_then(|row| row.extension.as_deref());
    let mut impact = impact(review, extension, more);
    if stale {
        impact.notices.insert(0, STALE.into());
    }
    let containers = impact.scene.clone();
    if let Some(at) = trail.last() {
        let level = request(task, Some(at));
        impact.scene = None;
        match review_of(feed, &level) {
            // A corrald that does not know `at` answers with the project level.
            Some((level, _)) if !at_level(level) => impact.map_said = Some(NO_LEVELS.into()),
            // A level read before a newer submission is not this head's.
            Some((level, _)) if level.head != review.head => {
                impact.map_said = Some(READING_LEVEL.into())
            }
            Some((level, level_stale)) => {
                if level_stale && !stale {
                    impact.notices.insert(0, STALE.into());
                }
                impact.scene = Some(build(&head_map(level), Some(at)));
            }
            None => {
                impact.map_said = Some(
                    match feed.detail(&level).and_then(|cache| cache.error.as_ref()) {
                        Some(error) if error.code == "not_found" => {
                            format!("This level is not at the task's head: {}", error.message)
                        }
                        Some(error) => format!("Could not read this level: {}", error.message),
                        None => READING_LEVEL.into(),
                    },
                )
            }
        }
        if let SystemAt::File { path } = at {
            let containers: Vec<String> = head_map(review)
                .rows
                .iter()
                .filter_map(|row| match row {
                    SystemMapRow::Container { container, .. } => Some(container.path.clone()),
                    _ => None,
                })
                .collect();
            impact.marks = marks_of(review, &containers, Some(path));
        }
    }
    impact.crumbs.extend(trail.iter().map(|at| {
        match at {
            SystemAt::Container { path } => containers
                .as_ref()
                .and_then(|scene| scene.card(&Node::Container(path.clone())))
                .map(|card| card.name.clone())
                .unwrap_or_else(|| base_name(path).to_owned()),
            SystemAt::Module { module } => module_name(module),
            SystemAt::File { path } => base_name(path).to_owned(),
        }
    }));
    Shown::Ready(Box::new(impact))
}

/// Whether a level's reply holds that level's geometry, not the project's.
fn at_level(level: &SystemReview) -> bool {
    !level.rows.iter().any(|row| {
        matches!(
            row,
            Row::Map {
                row: SystemMapRow::Container { .. } | SystemMapRow::PackageEdge { .. }
            }
        )
    })
}

/// The served head's geometry at the review's level.
fn head_map(review: &SystemReview) -> SystemMap {
    SystemMap {
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
    }
}

/// What the change did to every node it names, with the definitions of `file`, the file level
/// shown.
fn marks_of(review: &SystemReview, containers: &[String], file: Option<&str>) -> Marks {
    let mut marks = Marks::new();
    let mut touch = |node: Node, added: u64, removed: u64| {
        let mark = marks.entry(node).or_default();
        mark.touched = true;
        mark.added += added;
        mark.removed += removed;
    };
    for row in &review.rows {
        match row {
            Row::Container {
                path,
                added,
                removed,
            } => touch(Node::Container(path.clone()), *added, *removed),
            Row::Module {
                module,
                added,
                removed,
            } => touch(Node::Module(module.clone()), *added, *removed),
            Row::File {
                path,
                added,
                removed,
                ..
            } => touch(Node::File(path.clone()), *added, *removed),
            Row::Definition {
                change,
                path,
                name,
                line,
            } if *change != ChangeKind::Removed => {
                touch(Node::File(path.clone()), 0, 0);
                // A definition's node names no file: only the file shown gets its marks.
                if file != Some(path.as_str()) {
                    continue;
                }
                touch(
                    Node::Definition {
                        line: *line,
                        name: name.clone(),
                    },
                    0,
                    0,
                );
            }
            _ => {}
        }
    }
    for row in &review.rows {
        let Row::Dependent { dependent, module } = row else {
            continue;
        };
        let container = module
            .as_ref()
            .map(|module| module.container.clone())
            .or_else(|| container_of(containers, &dependent.path).cloned());
        let nodes = [
            container.map(Node::Container),
            module.clone().map(Node::Module),
            Some(Node::File(dependent.path.clone())),
            (file == Some(dependent.path.as_str())).then(|| Node::Definition {
                line: dependent.line,
                name: dependent.name.clone(),
            }),
        ];
        for node in nodes.into_iter().flatten() {
            marks.entry(node).or_default().dependents += 1;
        }
    }
    marks
}

/// Why a category an extension can supply has no answer: the extension's answer, its failure,
/// or no answer yet from the configured `extension` each say so.
fn unprovided(review: &SystemReview, extension: Option<&str>) -> (Supply, Option<String>) {
    let said = match (&review.provider, extension) {
        (Some(p), _) if p.error.is_some() => {
            format!("Not provided: {} failed on this change.", p.extension)
        }
        (Some(p), _) => {
            format!(
                "Not provided: {} does not supply this category.",
                p.extension
            )
        }
        // corrald asks the extension only after its own read of the change succeeds.
        (None, Some(name)) if review.error.is_some() => {
            format!("Not provided: corrald could not read the change, so {name} was not asked.")
        }
        (None, Some(name)) => {
            return (
                Supply::Unanswered,
                Some(format!(
                    "Not answered yet: corrald has no answer from {name} for this change."
                )),
            );
        }
        (None, None) => NOTHING_SUPPLIES.into(),
    };
    (Supply::NotProvided, Some(said))
}

const NOTHING_SUPPLIES: &str = "Not provided: nothing in this project supplies this category.";

/// `extension`: the facts extension the project's `system` row names, whose answer the
/// provider categories wait for while the review carries none.
pub fn impact(review: &SystemReview, extension: Option<&str>, more: &BTreeSet<usize>) -> Impact {
    let map = head_map(review);
    let containers: Vec<String> = map
        .rows
        .iter()
        .filter_map(|row| match row {
            SystemMapRow::Container { container, .. } => Some(container.path.clone()),
            _ => None,
        })
        .collect();
    let mut files: BTreeMap<String, ModuleId> = BTreeMap::new();
    let mut deleted = BTreeSet::new();
    for row in &review.rows {
        match row {
            Row::File {
                path,
                module,
                deleted: gone,
                ..
            } => {
                if *gone {
                    deleted.insert(path.clone());
                } else {
                    files.insert(path.clone(), module.clone());
                }
            }
            Row::Dependent {
                dependent,
                module: Some(module),
            } => {
                files.insert(dependent.path.clone(), module.clone());
            }
            _ => {}
        }
    }
    let marks = marks_of(review, &containers, None);
    let cx = Ctx {
        containers,
        files,
        deleted,
        head: &review.head,
        base: &review.base,
    };

    let mut touched = Vec::new();
    let mut listed: BTreeMap<usize, Vec<Item>> = BTreeMap::new();
    for (n, row) in review.rows.iter().enumerate() {
        if let Row::Module {
            module,
            added,
            removed,
        } = row
        {
            touched.push(Item {
                sub: Some(plural(added + removed, "line changed", "lines changed")),
                target: Some(module_target(module)),
                ..item(
                    format!("row-{n}"),
                    Sign::Changed,
                    module_words(module),
                    churn(*added, *removed),
                )
            });
        }
        if let Some(category) = category_of(row)
            && let Some(index) = Category::ALL.iter().position(|c| *c == category)
            && let Some(item) = item_of(&cx, n, row)
        {
            listed.entry(index).or_default().push(item);
        }
    }
    let count_of = |kind: fn(&Node) -> bool,
                    held: fn(&crate::model::system::marks::Mark) -> bool| {
        marks
            .iter()
            .filter(|(node, mark)| kind(node) && held(mark))
            .count() as u64
    };
    let is_container = |node: &Node| matches!(node, Node::Container(_));
    let summary = format!(
        "Touched {} · {} · {} · dependents in {}",
        plural(count_of(is_container, |m| m.touched), "package", "packages"),
        plural(
            count_of(|node| matches!(node, Node::Module(_)), |m| m.touched),
            "module",
            "modules"
        ),
        plural(
            review
                .rows
                .iter()
                .filter(|row| matches!(row, Row::File { .. }))
                .count() as u64,
            "file",
            "files"
        ),
        plural(
            count_of(is_container, |m| m.dependents > 0),
            "package",
            "packages"
        ),
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
        let answered_tests = review
            .provider
            .as_ref()
            .filter(|p| p.error.is_none() && p.coverage.provides("tests"));
        let candidates = items.iter().any(|i| i.sign == Sign::Neutral);
        if category == Category::Tests
            && !candidates
            && let Some(p) = answered_tests
        {
            // Changed test files are built in and share the category, so they must not hide an
            // empty answer; rows left out of the reply or by the extension might be candidates.
            notes.push(
                if total > items.len() {
                    format!("No test candidate among the items listed from {}.", p.extension)
                } else if p.coverage.omitted.values().any(|n| *n > 0) {
                    format!(
                        "No test candidate listed: {} answered with none, but left out what is counted above.",
                        p.extension
                    )
                } else {
                    format!("No test candidate: {} answered with none for this change.", p.extension)
                },
            );
        }
        if category == Category::Tests && !candidates && answered_tests.is_none() {
            notes.push(match (&review.provider, extension) {
                (Some(p), _) if p.error.is_some() => format!(
                    "Test candidates not provided: {} failed on this change.",
                    p.extension
                ),
                (Some(p), _) => format!(
                    "Test candidates not provided: {} did not supply them.",
                    p.extension
                ),
                (None, Some(name)) if review.error.is_some() => format!(
                    "Test candidates not provided: corrald could not read the change, so {name} was not asked."
                ),
                (None, Some(name)) => {
                    format!("Test candidates: {name} has not answered for this change yet.")
                }
                (None, None) => "Test candidates not provided: no extension supplies them.".into(),
            });
        }
        let hidden = if more.contains(&index) {
            0
        } else {
            items.len().saturating_sub(LIMIT)
        };
        let (supply, empty) = if !provided && provider_supplied(category) {
            unprovided(review, extension)
        } else if !provided {
            (Supply::NotProvided, Some(NOTHING_SUPPLIES.to_owned()))
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
        scene: Some(build(&map, None)),
        map_said: None,
        crumbs: vec![review.project.clone()],
        marks,
        summary,
        notices,
        touched,
        sections,
    }
}

#[cfg(test)]
mod tests;
