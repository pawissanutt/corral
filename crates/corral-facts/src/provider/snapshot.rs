use std::collections::{BTreeMap, BTreeSet};

use crate::{
    DataFacts, DataSource, DataType, EntryDetector, EntryPoint, EntryPoints, FileFact, ReadStatus,
    Reference, ReferenceBasis, RuntimeUnit, TreeFacts, entries::Owners, structure,
};

use super::{ProviderCoverage, Reply, Row};

impl Reply {
    /// Paths the provider explicitly could not read, independent of built-in
    /// read statuses for paths with no provider file row.
    pub fn unread_files(&self) -> BTreeSet<String> {
        let mut unread = BTreeSet::new();
        if self.coverage.provides("files") {
            for row in &self.rows {
                if let Row::File { path, read } = row {
                    if *read == ReadStatus::Complete {
                        unread.remove(path);
                    } else {
                        unread.insert(path.clone());
                    }
                }
            }
        }
        unread
    }

    /// Overlay a validated snapshot on the built-in structure. Git still owns
    /// the file list and the hierarchy, even when the provider supplies files.
    pub fn merge_snapshot(self, tree: &mut TreeFacts) -> ProviderCoverage {
        let Self { rows, mut coverage } = self;
        let mut by_path: BTreeMap<_, _> = tree
            .files
            .iter_mut()
            .map(|file| (file.path.clone(), file))
            .collect();
        let mut refs = BTreeMap::<_, u64>::new();
        let mut types = Vec::new();
        let mut entries = EntryOverlay::new(&mut tree.entry_points, &tree.runtime.units);
        for row in rows {
            let Some(row) = entries.consume(
                row,
                |path| by_path.get(path).map(|file| file.container.as_str()),
                &mut coverage,
            ) else {
                continue;
            };
            if !row.has_known_paths(|path| by_path.contains_key(path)) {
                coverage.dropped_unknown_files += 1;
                continue;
            }
            match row {
                Row::File { path, read } if coverage.provides("files") => {
                    if let Some(file) = by_path.get_mut(&path) {
                        file.read_status = read;
                    }
                }
                Row::Definition { path, definition } if coverage.provides("definitions") => {
                    if let Some(file) = by_path.get_mut(&path) {
                        file.definitions.push(definition);
                    }
                }
                Row::Reference {
                    from,
                    to,
                    kind,
                    count,
                } if coverage.provides("references") => {
                    let total = refs.entry((from, to, kind)).or_default();
                    *total = total.saturating_add(count);
                }
                row if coverage.provides("types") => types.extend(data_type(row)),
                _ => {} // Don't apply rows for categories the provider did not answer.
            }
        }
        entries.finish();
        tree.data
            .overlay(types, file_languages(by_path.values().map(|file| &**file)));
        if coverage.provides("files") {
            tree.coverage.complete = 0;
            tree.coverage.unsupported = 0;
            tree.coverage.failed = 0;
            for file in by_path.values() {
                match file.read_status {
                    ReadStatus::Complete => tree.coverage.complete += 1,
                    ReadStatus::Unsupported => tree.coverage.unsupported += 1,
                    ReadStatus::Failed => tree.coverage.failed += 1,
                }
            }
        }
        if coverage.provides("references") {
            let basis = ReferenceBasis::Resolved;
            tree.file_references = refs
                .into_iter()
                .map(|((from, to, kind), count)| Reference {
                    from,
                    to,
                    kind,
                    count,
                    basis,
                })
                .collect();
            let by_path = by_path
                .iter()
                .map(|(path, file)| (path.as_str(), &**file))
                .collect();
            (tree.module_references, tree.package_edges) =
                structure::roll_up(&tree.file_references, basis, &by_path, &tree.dependencies);
        }
        coverage
    }

    /// Apply only the type rows of a validated snapshot reply to the built-in model.
    pub fn snapshot_data(self, tree: &TreeFacts) -> (DataFacts, ProviderCoverage) {
        let Self { rows, mut coverage } = self;
        let mut facts = tree.data.clone();
        if coverage.provides("types") {
            let paths: BTreeSet<_> = tree.files.iter().map(|file| file.path.as_str()).collect();
            let types = rows
                .into_iter()
                .filter_map(data_type)
                .filter(|row| {
                    let known = paths.contains(row.path.as_str());
                    coverage.dropped_unknown_files += u64::from(!known);
                    known
                })
                .collect();
            facts.overlay(types, file_languages(tree.files.iter()));
        }
        (facts, coverage)
    }

    /// Apply only the entry rows of a validated snapshot reply to the built-in
    /// entry points. The returned coverage counts unknown paths only in entry rows.
    pub fn snapshot_entries(self, tree: &TreeFacts) -> (EntryPoints, ProviderCoverage) {
        let Self { rows, mut coverage } = self;
        let mut points = tree.entry_points.clone();
        let mut entries = EntryOverlay::new(&mut points, &tree.runtime.units);
        let mut by_path = None;
        for row in rows {
            let _ = entries.consume(
                row,
                |path| {
                    by_path
                        .get_or_insert_with(|| {
                            tree.files
                                .iter()
                                .map(|file| (file.path.as_str(), file.container.as_str()))
                                .collect::<BTreeMap<_, _>>()
                        })
                        .get(path)
                        .copied()
                },
                &mut coverage,
            );
        }
        entries.finish();
        (points, coverage)
    }
}

fn file_languages<'a>(
    files: impl Iterator<Item = &'a FileFact>,
) -> impl Iterator<Item = (&'a str, &'a str)> {
    files.map(|file| (file.path.as_str(), file.language.as_str()))
}

fn data_type(row: Row) -> Option<DataType> {
    let Row::Type {
        path,
        name,
        line,
        kind,
        fields,
    } = row
    else {
        return None;
    };
    Some(DataType {
        path,
        name,
        line,
        kind,
        source: DataSource::Extension,
        fields,
    })
}

struct EntryOverlay<'a> {
    points: &'a mut EntryPoints,
    units: &'a [RuntimeUnit],
    owners: Option<Owners<'a>>,
    entries: Vec<EntryPoint>,
}

impl<'a> EntryOverlay<'a> {
    fn new(points: &'a mut EntryPoints, units: &'a [RuntimeUnit]) -> Self {
        Self {
            points,
            units,
            owners: None,
            entries: Vec::new(),
        }
    }

    // Consume entry rows, passing other categories back for full snapshot merges.
    fn consume<'file>(
        &mut self,
        row: Row,
        container_of: impl FnOnce(&str) -> Option<&'file str>,
        coverage: &mut ProviderCoverage,
    ) -> Option<Row> {
        let Row::Entry {
            path,
            line,
            kind,
            name,
        } = row
        else {
            return Some(row);
        };
        let Some(container) = container_of(&path) else {
            coverage.dropped_unknown_files += 1;
            return None;
        };
        if coverage.provides("entries") {
            let owners = self.owners.get_or_insert_with(|| Owners::new(self.units));
            self.entries.push(EntryPoint {
                kind,
                name,
                path,
                line,
                owner: owners.of(container),
            });
            self.points
                .coverage
                .entry(container.to_owned())
                .or_default()
                .insert(EntryDetector::Extension);
        }
        None
    }

    fn finish(self) {
        if self.entries.is_empty() {
            return;
        }
        let covered = self
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<BTreeSet<_>>();
        let existing = &mut self.points.entries;
        existing.retain(|entry| !covered.contains(entry.path.as_str()));
        existing.extend(self.entries);
        existing.sort_unstable();
    }
}
