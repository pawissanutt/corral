//! Pure, unpaged display rows from immutable facts. Git and provider work happens elsewhere.
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};

use corral_facts::{
    Activity, ActivityCommit, FileFact, ModuleId, ProjectDelta, ProjectFacts, ReadStatus,
    TreeFacts, is_manifest,
    provider::{ChangeKind, ChangedFact},
};
use corral_types::{
    SystemActivity, SystemAt, SystemCategorySummary, SystemImpactCategory, SystemMapRow,
    SystemNodeStats, SystemReviewRow,
};

use crate::views::NotFound;

const WEEK_SECONDS: i64 = 7 * 24 * 60 * 60;

type Files<'a> = HashMap<&'a str, &'a FileFact>;

fn files(tree: &TreeFacts) -> Files<'_> {
    tree.files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect()
}

pub(crate) fn map(
    snapshot: &ProjectFacts,
    at: Option<&SystemAt>,
) -> anyhow::Result<Vec<SystemMapRow>> {
    map_rows(
        &snapshot.structure,
        &snapshot.activity,
        snapshot.revision.committed_at(),
        at,
    )
}

fn map_rows(
    tree: &TreeFacts,
    activity: &Activity,
    committed_at: i64,
    at: Option<&SystemAt>,
) -> anyhow::Result<Vec<SystemMapRow>> {
    let week = committed_at.saturating_sub(WEEK_SECONDS);
    match at {
        None => Ok(project_rows(tree, Some((activity, week)))),
        Some(SystemAt::Container { path }) => container_rows(tree, activity, week, path),
        Some(SystemAt::Module { module }) => {
            if !tree.modules.contains(module) {
                return Err(NotFound("system module not found").into());
            }
            let mut files: Vec<_> = tree
                .files
                .iter()
                .filter(|file| &file.module == module)
                .collect();
            files.sort_unstable_by_key(|file| (Reverse(file.lines), file.path.as_str()));
            Ok(files
                .into_iter()
                .map(|file| SystemMapRow::File {
                    path: file.path.clone(),
                    language: file.language.clone(),
                    lines: file.lines,
                    test: file.test,
                    read_status: file.read_status,
                })
                .collect())
        }
        Some(SystemAt::File { path }) => {
            let file = tree
                .files
                .iter()
                .find(|file| &file.path == path)
                .ok_or(NotFound("system file not found"))?;
            let mut definitions: Vec<_> = file.definitions.iter().collect();
            definitions.sort_unstable_by_key(|def| {
                (def.line, def.end_line, def.name.as_str(), def.kind.as_str())
            });
            Ok(definitions
                .into_iter()
                .map(|definition| SystemMapRow::Definition {
                    definition: definition.clone(),
                })
                .collect())
        }
    }
}

fn project_rows(tree: &TreeFacts, activity: Option<(&Activity, i64)>) -> Vec<SystemMapRow> {
    let files = files(tree);
    let containers: HashSet<_> = tree.containers.iter().map(|c| c.path.as_str()).collect();
    let mut stats = node_stats(&files, activity, |path, file| {
        // A deleted path can still contribute activity to its closest surviving container.
        file.map(|file| file.container.as_str())
            .or_else(|| container_owner(path, &containers))
    });
    let mut rows: Vec<_> = tree
        .containers
        .iter()
        .map(|container| SystemMapRow::Container {
            container: container.clone(),
            stats: stats.remove(container.path.as_str()).unwrap_or_default(),
        })
        .collect();
    rows.extend(
        tree.package_edges
            .iter()
            .map(|edge| SystemMapRow::PackageEdge { edge: edge.clone() }),
    );
    rows.extend(tree.spawns.iter().filter_map(|spawn| {
        let file = files.get(spawn.path.as_str())?;
        Some(SystemMapRow::Spawn {
            container: file.container.clone(),
            spawn: spawn.clone(),
        })
    }));
    rows
}

fn container_owner<'a>(mut path: &str, containers: &HashSet<&'a str>) -> Option<&'a str> {
    while let Some((parent, _)) = path.rsplit_once('/') {
        if let Some(&owner) = containers.get(parent) {
            return Some(owner);
        }
        path = parent;
    }
    containers.get("").copied()
}

fn container_rows(
    tree: &TreeFacts,
    activity: &Activity,
    week: i64,
    path: &str,
) -> anyhow::Result<Vec<SystemMapRow>> {
    if !tree
        .containers
        .iter()
        .any(|container| container.path == path)
    {
        return Err(NotFound("system container not found").into());
    }
    // Module membership is known for files that survive in this tree.
    let mut stats = node_stats(&files(tree), Some((activity, week)), |_, file| {
        file.filter(|file| file.container == path)
            .map(|file| &file.module)
    });
    let mut rows: Vec<_> = tree
        .modules
        .iter()
        .filter(|module| module.container == path)
        .map(|module| SystemMapRow::Module {
            module: module.clone(),
            stats: stats.remove(module).unwrap_or_default(),
        })
        .collect();
    let references: Vec<_> = tree
        .module_references
        .iter()
        .filter(|reference| {
            reference.from != reference.to
                && (reference.from.container == path || reference.to.container == path)
        })
        .collect();
    // Declared-only package edges can also bring a neighbor onto the boundary.
    let neighbors: HashSet<_> = tree
        .package_edges
        .iter()
        .map(|edge| (&edge.from, &edge.to))
        .chain(
            references
                .iter()
                .map(|reference| (&reference.from.container, &reference.to.container)),
        )
        .filter_map(|(from, to)| {
            if from == path && to != path {
                Some(to)
            } else if to == path && from != path {
                Some(from)
            } else {
                None
            }
        })
        .collect();
    rows.extend(
        tree.containers
            .iter()
            .filter(|container| neighbors.contains(&container.path))
            .map(|container| SystemMapRow::Neighbor {
                container: container.clone(),
            }),
    );
    rows.extend(
        references
            .into_iter()
            .map(|reference| SystemMapRow::ModuleReference {
                reference: reference.clone(),
            }),
    );
    Ok(rows)
}

#[derive(Default)]
struct Churn {
    added: u64,
    removed: u64,
}

impl Churn {
    fn add(&mut self, added: u64, removed: u64) {
        self.added = self.added.saturating_add(added);
        self.removed = self.removed.saturating_add(removed);
    }
}

fn node_stats<'a, K: Ord + ?Sized>(
    files: &Files<'a>,
    activity: Option<(&Activity, i64)>,
    owner: impl Fn(&str, Option<&'a FileFact>) -> Option<&'a K>,
) -> BTreeMap<&'a K, SystemNodeStats> {
    let mut nodes = BTreeMap::<_, (SystemNodeStats, Option<i64>)>::new();
    for (&path, &file) in files {
        let Some(key) = owner(path, Some(file)) else {
            continue;
        };
        let node = &mut nodes.entry(key).or_default().0;
        node.lines = node.lines.saturating_add(file.lines);
        let lines = node.languages.entry(file.language.clone()).or_default();
        *lines = lines.saturating_add(file.lines);
        match file.read_status {
            ReadStatus::Complete => node.complete += 1,
            ReadStatus::Unsupported => node.unsupported += 1,
            ReadStatus::Failed => node.failed += 1,
        }
    }
    for (commit, week) in activity
        .into_iter()
        .flat_map(|(activity, week)| activity.commits.iter().map(move |commit| (commit, week)))
    {
        let mut touched = BTreeMap::<_, Churn>::new();
        for file in &commit.files {
            if let Some(key) = owner(&file.path, files.get(file.path.as_str()).copied()) {
                touched
                    .entry(key)
                    .or_default()
                    .add(file.added, file.removed);
            }
        }
        for (key, churn) in touched {
            let (node, last_at) = nodes.entry(key).or_default();
            // Git walks descendants before ancestors when commit timestamps tie.
            let newer = last_at.is_none_or(|last| last < commit.committed_at);
            record_touch(&mut node.days_30, commit, &churn, newer);
            if commit.committed_at >= week {
                record_touch(&mut node.days_7, commit, &churn, newer);
            }
            if newer {
                *last_at = Some(commit.committed_at);
            }
        }
    }
    nodes
        .into_iter()
        .map(|(key, (stats, _))| (key, stats))
        .collect()
}

fn record_touch(
    activity: &mut SystemActivity,
    commit: &ActivityCommit,
    churn: &Churn,
    newer: bool,
) {
    activity.commits += 1;
    activity.added = activity.added.saturating_add(churn.added);
    activity.removed = activity.removed.saturating_add(churn.removed);
    if newer {
        activity.last_commit = Some(commit.commit.clone());
    }
}

pub(crate) fn review(
    delta: &ProjectDelta,
    before: &TreeFacts,
    after: &TreeFacts,
    enriched_head: Option<&TreeFacts>,
    at: Option<&SystemAt>,
) -> anyhow::Result<(Vec<SystemCategorySummary>, Vec<SystemReviewRow>)> {
    use SystemImpactCategory as Category;
    use SystemReviewRow as Row;

    let base_files = files(before);
    let head_files = files(after);
    let enriched_files = enriched_head.map(files);
    // A delta has no file-status slot, but a returned semantic change is
    // positive evidence its source (and a resolved target, if any) was read.
    let provider_files: HashSet<_> = delta
        .changes
        .iter()
        .flatten()
        .flat_map(|change| {
            let target = match &change.fact {
                ChangedFact::Definition => None,
                ChangedFact::Reference { to_path, .. } => Some(to_path.as_str()),
            };
            let base_side = change.change == ChangeKind::Removed;
            std::iter::once((base_side, change.path.as_str()))
                .chain(target.map(|path| (base_side, path)))
        })
        .collect();
    let mut footprint = Footprint::default();
    let mut test_files = Vec::new();
    let mut unread = Vec::new();
    for change in &delta.files {
        let path = change.path.as_str();
        let head = head_files.get(path);
        // Git's diff paths exist in at least one of the two trees; deletions use base ownership.
        let Some(file) = head.or_else(|| base_files.get(path)) else {
            continue;
        };
        footprint.add(file, change.added, change.removed, change.binary);
        if file.test {
            test_files.push(Row::TestFile {
                path: change.path.clone(),
            });
        }
        let read_by_extension = provider_files.contains(&(head.is_none(), path))
            || (head.is_some()
                && enriched_files
                    .as_ref()
                    .and_then(|files| files.get(path))
                    .is_some_and(|file| file.read_status == ReadStatus::Complete));
        if file.read_status != ReadStatus::Complete && !read_by_extension {
            unread.push(Row::NotRead {
                path: change.path.clone(),
                read_status: file.read_status,
            });
        }
    }
    let mut review = ReviewRows::new(footprint.into_rows());

    review.rows.extend(
        delta
            .files
            .iter()
            .filter(|file| is_manifest(&file.path))
            .map(|file| Row::Manifest {
                path: file.path.clone(),
            }),
    );
    review
        .rows
        .extend(delta.dependencies.iter().map(|dep| Row::Dependency {
            before: dep.before.clone(),
            after: dep.after.clone(),
        }));
    review.rows.extend(package_edge_changes(before, after));
    review.finish_category(Category::PackageDependencies, true);

    review.rows.extend(
        delta
            .module_references
            .iter()
            .map(|reference| Row::ModuleReference {
                from: reference.from.clone(),
                to: reference.to.clone(),
                before: reference.before,
                after: reference.after,
            }),
    );
    review.finish_category(Category::ModuleReferences, true);

    let (definitions, calls) = provider_changes(delta, &base_files, &head_files);
    review.rows.extend(definitions);
    review.finish_category(Category::Definitions, delta.changes.is_some());
    review.rows.extend(calls);
    review.finish_category(Category::CallEdges, delta.changes.is_some());

    review.rows.extend(spawn_changes(before, after));
    review.finish_category(Category::ExternalProcesses, true);

    if let Some(dependents) = &delta.dependents {
        let changed: HashSet<_> = delta.files.iter().map(|file| file.path.as_str()).collect();
        review.rows.extend(
            dependents
                .iter()
                .filter(|dep| !changed.contains(dep.path.as_str()))
                .map(|dep| Row::Dependent {
                    dependent: dep.clone(),
                    module: head_files
                        .get(dep.path.as_str())
                        .map(|file| file.module.clone()),
                }),
        );
    }
    review.finish_category(Category::Dependents, delta.dependents.is_some());

    review.rows.extend(test_files);
    if let Some(candidates) = &delta.tests {
        review
            .rows
            .extend(candidates.iter().map(|candidate| Row::TestCandidate {
                candidate: candidate.clone(),
            }));
    }
    // Changed test files are built in; candidate availability is reported by provider coverage.
    review.finish_category(Category::Tests, true);
    review.rows.extend(unread);
    review.finish_category(Category::NotRead, true);

    // The Review map is structural, at the submitted head and the level asked for. History
    // overlays are not collected for Review and its map rows are not impact categories.
    let tree = enriched_head.unwrap_or(after);
    let map = match at {
        None => project_rows(tree, None),
        Some(at) => map_rows(tree, &no_activity(), 0, Some(at))?,
    };
    review
        .rows
        .extend(map.into_iter().map(|row| Row::Map { row }));
    Ok((review.categories, review.rows))
}

/// A review's map carries no history: its activity windows stay empty.
fn no_activity() -> Activity {
    Activity {
        days_7: Vec::new(),
        days_30: Vec::new(),
        commits: Vec::new(),
        shallow: false,
        skipped_non_utf8_paths: 0,
    }
}

struct ReviewRows {
    rows: Vec<SystemReviewRow>,
    categories: Vec<SystemCategorySummary>,
    category_start: usize,
}

impl ReviewRows {
    fn new(rows: Vec<SystemReviewRow>) -> Self {
        Self {
            category_start: rows.len(),
            rows,
            categories: Vec::new(),
        }
    }

    fn finish_category(&mut self, category: SystemImpactCategory, provided: bool) {
        self.categories.push(SystemCategorySummary {
            category,
            provided,
            total: self.rows.len() - self.category_start,
        });
        self.category_start = self.rows.len();
    }
}

#[derive(Default)]
struct Footprint<'a> {
    containers: BTreeMap<&'a str, Churn>,
    modules: BTreeMap<&'a ModuleId, Churn>,
    files: Vec<SystemReviewRow>,
}

impl<'a> Footprint<'a> {
    fn add(&mut self, file: &'a FileFact, added: u64, removed: u64, binary: bool) {
        self.containers
            .entry(&file.container)
            .or_default()
            .add(added, removed);
        self.modules
            .entry(&file.module)
            .or_default()
            .add(added, removed);
        self.files.push(SystemReviewRow::File {
            path: file.path.clone(),
            container: file.container.clone(),
            module: file.module.clone(),
            added,
            removed,
            binary,
        });
    }

    fn into_rows(self) -> Vec<SystemReviewRow> {
        self.containers
            .into_iter()
            .map(
                |(path, Churn { added, removed })| SystemReviewRow::Container {
                    path: path.to_owned(),
                    added,
                    removed,
                },
            )
            .chain(
                self.modules
                    .into_iter()
                    .map(
                        |(module, Churn { added, removed })| SystemReviewRow::Module {
                            module: module.clone(),
                            added,
                            removed,
                        },
                    ),
            )
            .chain(self.files)
            .collect()
    }
}

fn package_edge_changes(before: &TreeFacts, after: &TreeFacts) -> Vec<SystemReviewRow> {
    let mut pairs = BTreeMap::<_, (Option<_>, Option<_>)>::new();
    for edge in &before.package_edges {
        pairs.entry((&edge.from, &edge.to)).or_default().0 = Some(edge);
    }
    for edge in &after.package_edges {
        pairs.entry((&edge.from, &edge.to)).or_default().1 = Some(edge);
    }
    pairs
        .into_values()
        // Reference count-only changes are already represented by the module rollup.
        .filter(|(before, after)| before.map(|edge| edge.kind) != after.map(|edge| edge.kind))
        .map(|(before, after)| SystemReviewRow::PackageEdge {
            before: before.cloned(),
            after: after.cloned(),
        })
        .collect()
}

fn spawn_changes(before: &TreeFacts, after: &TreeFacts) -> Vec<SystemReviewRow> {
    let mut counts = BTreeMap::<_, (u64, u64)>::new();
    for spawn in &before.spawns {
        counts.entry((&spawn.path, &spawn.process)).or_default().0 = spawn.count;
    }
    for spawn in &after.spawns {
        counts.entry((&spawn.path, &spawn.process)).or_default().1 = spawn.count;
    }
    counts
        .into_iter()
        .filter(|(_, (before, after))| before != after)
        .map(
            |((path, process), (before, after))| SystemReviewRow::Spawn {
                path: path.clone(),
                process: process.clone(),
                before,
                after,
            },
        )
        .collect()
}

#[derive(Default)]
struct Calls<'a> {
    added: u64,
    removed: u64,
    changed: u64,
    // Prefer a source anchor in head if the group also contains removals.
    anchor: Option<(bool, &'a str, u64)>,
}

fn provider_changes(
    delta: &ProjectDelta,
    base: &Files<'_>,
    head: &Files<'_>,
) -> (Vec<SystemReviewRow>, Vec<SystemReviewRow>) {
    let mut definitions = Vec::new();
    let mut calls = BTreeMap::<_, Calls<'_>>::new();
    for fact in delta.changes.iter().flatten() {
        match &fact.fact {
            ChangedFact::Definition => definitions.push(SystemReviewRow::Definition {
                change: fact.change,
                path: fact.path.clone(),
                name: fact.name.clone(),
                line: fact.line,
            }),
            ChangedFact::Reference { to_path, .. } => {
                let removed = fact.change == ChangeKind::Removed;
                let files = if removed { base } else { head };
                let (Some(from), Some(to)) =
                    (files.get(fact.path.as_str()), files.get(to_path.as_str()))
                else {
                    continue;
                };
                let group = calls.entry((&from.module, &to.module)).or_default();
                match fact.change {
                    ChangeKind::Added => group.added += 1,
                    ChangeKind::Removed => group.removed += 1,
                    ChangeKind::Changed => group.changed += 1,
                }
                let anchor = (removed, fact.path.as_str(), fact.line);
                group.anchor = Some(group.anchor.map_or(anchor, |current| current.min(anchor)));
            }
        }
    }
    let calls = calls
        .into_iter()
        .filter_map(|((from, to), group)| {
            let (_, path, line) = group.anchor?;
            Some(SystemReviewRow::CallEdge {
                from: from.clone(),
                to: to.clone(),
                added: group.added,
                removed: group.removed,
                changed: group.changed,
                path: path.to_owned(),
                line,
            })
        })
        .collect();
    (definitions, calls)
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;
