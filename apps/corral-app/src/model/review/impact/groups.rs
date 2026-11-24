//! Category figures and groupings. The wire's total counts rows, which is not always the
//! unit the owner needs: a call row can account for several edges, and tests mix two units.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use corral_types::{ModuleId, ReadStatus, SystemImpactCategory as Category, SystemReview};

use super::items::{
    Ctx, container_of, module_words, provider_omissions, provider_supplied, title, zero,
};
use super::{Item, LIMIT, NOTHING_SUPPLIES, Row, Section, Sign, State, Supply, unprovided};
use crate::model::system::{count, plural};

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub heading: Option<(String, String)>,
    pub items: Vec<Item>,
}

impl Section {
    pub fn foldable(&self) -> bool {
        !self.groups.is_empty()
    }

    pub fn open(&self, state: Option<&State>) -> bool {
        state
            .and_then(|s| s.category_open.get(&self.index).copied())
            .unwrap_or_else(|| {
                self.default_open
                    || state.and_then(|s| s.item.as_ref()).is_some_and(|key| {
                        self.groups
                            .iter()
                            .any(|g| g.items.iter().any(|i| &i.key == key))
                    })
            })
    }
}

impl Group {
    pub fn show_all(&self, state: Option<&State>, category: usize, index: usize) -> bool {
        state.is_some_and(|s| {
            s.more.contains(&(category, index))
                || s.item
                    .as_ref()
                    .is_some_and(|key| self.items.iter().skip(LIMIT).any(|i| &i.key == key))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum By<'a> {
    Flat,
    Module(Option<&'a ModuleId>),
    Pair(&'a ModuleId, &'a ModuleId),
    Package(Option<&'a str>),
    TestFile,
    Candidate,
    Unread(Unread),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Unread {
    Unsupported,
    Failed,
    NoFactMoved,
}

impl Unread {
    fn title(self) -> &'static str {
        match self {
            Self::Unsupported => "Not read by the code index",
            Self::Failed => "Read failed",
            Self::NoFactMoved => "Read, but no fact moved",
        }
    }

    fn summary(self, n: u64) -> String {
        let reason = match self {
            Self::Unsupported => "not indexed",
            Self::Failed => "failed to read",
            Self::NoFactMoved => "read, no fact moved",
        };
        format!("{} {reason}", count(n))
    }
}

impl<'a> By<'a> {
    fn of(cx: &'a Ctx<'_>, row: &'a Row) -> Self {
        match row {
            Row::Definition { path, .. } => Self::Module(cx.files.get(path)),
            Row::CallEdge { from, to, .. } => Self::Pair(from, to),
            Row::Dependent { dependent, module } => Self::Package(
                module
                    .as_ref()
                    .map(|m| &m.container)
                    .or_else(|| container_of(&cx.containers, &dependent.path))
                    .map(String::as_str),
            ),
            Row::TestFile { .. } => Self::TestFile,
            Row::TestCandidate { .. } => Self::Candidate,
            Row::NotRead { read_status, .. } => Self::Unread(match read_status {
                ReadStatus::Unsupported => Unread::Unsupported,
                ReadStatus::Failed => Unread::Failed,
                ReadStatus::Complete => Unread::NoFactMoved,
            }),
            _ => Self::Flat,
        }
    }

    fn heading(self, n: u64, changes: Changes) -> Option<(String, String)> {
        let title = match self {
            Self::Flat => return None,
            Self::Module(module) => {
                module.map_or_else(|| "Module not provided".into(), module_words)
            }
            Self::Pair(from, to) => format!("{} → {}", module_words(from), module_words(to)),
            Self::Package(Some("")) => "(root)".into(),
            Self::Package(Some(path)) => path.into(),
            Self::Package(None) => "Package not provided".into(),
            Self::TestFile => "Changed test files".into(),
            Self::Candidate => "Test candidates".into(),
            Self::Unread(reason) => reason.title().into(),
        };
        let figure = if matches!(self, Self::Pair(..)) {
            changes.delta()
        } else {
            count(n)
        };
        Some((title, figure))
    }
}

#[derive(Clone, Copy, Default)]
struct Changes {
    added: u64,
    removed: u64,
    changed: u64,
}

impl Changes {
    fn see(&mut self, row: &Row, sign: Sign) {
        if let Row::CallEdge {
            added,
            removed,
            changed,
            ..
        } = row
        {
            self.added = self.added.saturating_add(*added);
            self.removed = self.removed.saturating_add(*removed);
            self.changed = self.changed.saturating_add(*changed);
            return;
        }
        match sign {
            Sign::Added => self.added += 1,
            Sign::Removed => self.removed += 1,
            Sign::Changed => self.changed += 1,
            Sign::Dependent | Sign::Neutral => {}
        }
    }

    fn total(self) -> u64 {
        self.added
            .saturating_add(self.removed)
            .saturating_add(self.changed)
    }

    fn words(self) -> String {
        format!(
            "{} added · {} removed · {} changed",
            count(self.added),
            count(self.removed),
            count(self.changed)
        )
    }

    fn delta(self) -> String {
        let mut words = super::items::churn(self.added, self.removed);
        if self.changed > 0 {
            words.push_str(&format!(" Δ{}", count(self.changed)));
        }
        words
    }
}

#[derive(Default)]
struct Bucket {
    changes: Changes,
    items: Vec<Item>,
}

/// Counts by unit for categories where a wire row can belong to more than one kind of fact.
#[derive(Default)]
struct Units {
    manifests: u64,
    dependencies: u64,
    package_edges: u64,
    runtime_units: u64,
    runtime_edges: u64,
    test_files: u64,
    candidates: u64,
}

impl Units {
    fn see(&mut self, row: &Row) {
        match row {
            Row::Manifest { .. } => self.manifests += 1,
            Row::Dependency { .. } => self.dependencies += 1,
            Row::PackageEdge { .. } => self.package_edges += 1,
            Row::RuntimeUnit { .. } => self.runtime_units += 1,
            Row::RuntimeEdge { .. } => self.runtime_edges += 1,
            Row::TestFile { .. } => self.test_files += 1,
            Row::TestCandidate { .. } => self.candidates += 1,
            _ => {}
        }
    }

    fn mixed(&self, category: Category, omitted: u64) -> String {
        let units = match category {
            Category::PackageDependencies => &[
                (self.manifests, "manifest", "manifests"),
                (self.dependencies, "dependency", "dependencies"),
                (self.package_edges, "package edge", "package edges"),
            ][..],
            Category::Runtime => &[
                (self.runtime_units, "runtime unit", "runtime units"),
                (self.runtime_edges, "runtime edge", "runtime edges"),
            ],
            _ => &[
                (self.test_files, "test file", "test files"),
                (self.candidates, "test candidate", "test candidates"),
            ],
        };
        units
            .iter()
            .filter(|(n, ..)| *n > 0)
            .map(|(n, one, many)| plural(*n, one, many))
            .chain((omitted > 0).then(|| format!("{} unlisted", count(omitted))))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

struct Figures {
    units: Units,
    changes: Changes,
    groups: usize,
    omitted: u64,
    /// The dependents' depths, none where the provider did not say.
    depths: BTreeSet<Option<u32>>,
}

/// A dependent's depth as said beside it; the provider may not supply one, which is said
/// rather than guessed.
pub(super) fn depth_words(depth: Option<u32>) -> String {
    match depth {
        Some(depth) => format!("depth {depth}"),
        None => "depth not supplied".into(),
    }
}

impl Supply {
    fn figure(self, category: Category, f: &Figures) -> (String, u64) {
        let n = match self {
            Self::NotProvided => return ("not provided".into(), 0),
            Self::Unanswered => return ("not answered".into(), 0),
            Self::None => return ("none".into(), 0),
            Self::Rows(total) => total as u64,
        };
        let (figure, count) = match category {
            Category::PackageDependencies | Category::Runtime | Category::Tests => {
                (f.units.mixed(category, f.omitted), n)
            }
            Category::CallEdges => {
                // The wire only tells us how many module-pair rows were omitted, not their edge counts.
                let edges = f.changes.total().saturating_add(f.omitted);
                let lower = if f.omitted > 0 { "≥" } else { "" };
                (format!("{lower}{}", plural(edges, "edge", "edges")), edges)
            }
            _ => {
                let (one, many) = match category {
                    Category::ModuleReferences => ("module pair", "module pairs"),
                    Category::Definitions => ("definition", "definitions"),
                    Category::ExternalProcesses => ("process change", "process changes"),
                    Category::Dependents => ("dependent", "dependents"),
                    Category::NotRead => ("file", "files"),
                    Category::EntryPoints => ("entry point", "entry points"),
                    Category::Lenses => ("lens", "lenses"),
                    _ => ("data change", "data changes"),
                };
                (plural(n, one, many), n)
            }
        };
        (figure, count)
    }
}

impl Figures {
    fn summary(&self, category: Category, answered_tests: bool) -> String {
        match category {
            Category::Dependents => {
                let known: Vec<String> = self.depths.iter().flatten().map(u32::to_string).collect();
                let depth = match (known.is_empty(), self.depths.contains(&None)) {
                    (true, _) => depth_words(None),
                    (false, false) => format!("depth {}", known.join(", ")),
                    (false, true) => format!("depth {}, some not supplied", known.join(", ")),
                };
                format!(
                    "in {} · {depth}",
                    plural(self.groups as u64, "package", "packages")
                )
            }
            Category::Tests => format!(
                "{} changed · {}",
                plural(self.units.test_files, "test file", "test files"),
                if answered_tests || self.units.candidates > 0 {
                    plural(self.units.candidates, "test candidate", "test candidates")
                } else {
                    "test candidate count unavailable".into()
                }
            ),
            Category::ModuleReferences | Category::ExternalProcesses => format!(
                "{} increased · {} decreased",
                count(self.changes.added),
                count(self.changes.removed)
            ),
            Category::CallEdges => format!(
                "{} · {}",
                self.changes.words(),
                plural(self.groups as u64, "module pair", "module pairs")
            ),
            _ => self.changes.words(),
        }
    }
}

pub(super) fn section(
    review: &SystemReview,
    extension: Option<&str>,
    cx: &Ctx<'_>,
    index: usize,
    category: Category,
    rows: Vec<(&Row, Item)>,
) -> Section {
    // Continuation pages repeat the summaries; the first is the answer.
    let summary = review.categories.iter().find(|c| c.category == category);
    let provided = summary.is_some_and(|s| s.provided);
    let total = summary.map_or(0, |s| s.total);
    let omitted = total.saturating_sub(rows.len());
    let mut notes = Vec::new();
    if provided && omitted > 0 {
        notes.push(format!(
            "{} not listed: left out to stay within the reply's size.",
            count(omitted as u64)
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
    let mut grouped = BTreeMap::<_, Bucket>::new();
    let mut units = Units::default();
    let mut changes = Changes::default();
    let depths = rows
        .iter()
        .filter_map(|(row, _)| match row {
            Row::Dependent { dependent, .. } => Some(dependent.depth),
            _ => None,
        })
        .collect();
    for (row, item) in rows {
        let bucket = grouped.entry(By::of(cx, row)).or_default();
        bucket.changes.see(row, item.sign);
        changes.see(row, item.sign);
        units.see(row);
        bucket.items.push(item);
    }
    if category == Category::Tests && units.candidates == 0 {
        notes.push(candidate_note(review, extension, answered_tests, omitted));
    }
    let (supply, empty) = if !provided && provider_supplied(category) {
        unprovided(review, extension)
    } else if !provided {
        (Supply::NotProvided, Some(NOTHING_SUPPLIES.to_owned()))
    } else if total == 0 {
        (Supply::None, Some(zero(category).to_owned()))
    } else {
        (Supply::Rows(total), None)
    };
    let figures = Figures {
        units,
        changes,
        groups: grouped
            .keys()
            .filter(|by| **by != By::Package(None))
            .count(),
        omitted: omitted as u64,
        depths,
    };
    let (figure, n) = supply.figure(category, &figures);
    let summary = (!grouped.is_empty()).then(|| {
        let words = if category == Category::NotRead {
            grouped
                .iter()
                .filter_map(|(by, bucket)| match by {
                    By::Unread(reason) => Some(reason.summary(bucket.items.len() as u64)),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" · ")
        } else {
            figures.summary(category, answered_tests.is_some())
        };
        if omitted > 0 {
            format!("Listed: {words}")
        } else {
            words
        }
    });
    let mut groups: Vec<_> = grouped.into_iter().collect();
    if matches!(
        category,
        Category::Definitions | Category::CallEdges | Category::Dependents
    ) {
        groups.sort_by_key(|(_, group)| {
            Reverse(if category == Category::CallEdges {
                group.changes.total()
            } else {
                group.items.len() as u64
            })
        });
    }
    let groups = groups
        .into_iter()
        .map(|(by, bucket)| Group {
            heading: by.heading(bucket.items.len() as u64, bucket.changes),
            items: bucket.items,
        })
        .collect();
    Section {
        index,
        title: title(category),
        supply,
        figure,
        summary,
        empty,
        notes,
        groups,
        default_open: (1..=12).contains(&n),
    }
}

fn candidate_note(
    review: &SystemReview,
    extension: Option<&str>,
    answered: Option<&corral_types::SystemProvider>,
    omitted: usize,
) -> String {
    if let Some(p) = answered {
        // Changed test files are built in and share the category, so they must not hide an
        // empty answer; rows left out of the reply or by the extension might be candidates.
        return if omitted > 0 {
            format!(
                "No test candidate among the items listed from {}.",
                p.extension
            )
        } else if p.coverage.omitted.values().any(|n| *n > 0) {
            format!(
                "No test candidate listed: {} answered with none, but left out what is counted above.",
                p.extension
            )
        } else {
            format!(
                "No test candidate: {} answered with none for this change.",
                p.extension
            )
        };
    }
    match (&review.provider, extension) {
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
    }
}

#[cfg(test)]
mod tests {
    use corral_types::SystemCategorySummary;

    use super::super::{
        Action, impact,
        tests::{module, review},
    };
    use super::*;

    #[test]
    fn aggregated_calls_fold_by_edge_count_not_module_pairs() {
        let categories = vec![SystemCategorySummary {
            category: Category::CallEdges,
            provided: true,
            total: 1,
        }];
        let rows = vec![Row::CallEdge {
            from: module("a", "one"),
            to: module("b", "two"),
            added: 12,
            removed: 4,
            changed: 1,
            path: "a/src/one.rs".into(),
            line: 42,
        }];
        let impact = impact(&review(categories, rows), None);
        let index = Category::ALL
            .iter()
            .position(|c| *c == Category::CallEdges)
            .unwrap();
        let section = &impact.sections[index];
        assert_eq!(section.groups.len(), 1);
        assert_eq!(section.figure.split_whitespace().next(), Some("17"));
        assert!(!section.open(None));
        let mut state = State::default();
        state.act(Action::Category {
            index: section.index,
            open: true,
        });
        assert!(section.open(Some(&state)));
    }
}
