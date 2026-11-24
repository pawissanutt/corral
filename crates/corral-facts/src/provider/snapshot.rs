use std::collections::{BTreeMap, BTreeSet};

use crate::{
    EntryDetector, EntryPoint, ReadStatus, Reference, ReferenceBasis, TreeFacts, entries::Owners,
    structure,
};

use super::{ProviderCoverage, Reply, Row};

impl Reply {
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
        let mut entries = Vec::new();
        let mut owners = None;
        for row in rows {
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
                Row::Entry {
                    path,
                    line,
                    kind,
                    name,
                } if coverage.provides("entries") => {
                    if let Some(file) = by_path.get(&path) {
                        let container = &file.container;
                        let owners = owners.get_or_insert_with(|| Owners::new(&tree.runtime.units));
                        entries.push(EntryPoint {
                            kind,
                            name,
                            path,
                            line,
                            owner: owners.of(container),
                        });
                        tree.entry_points
                            .coverage
                            .entry(container.clone())
                            .or_default()
                            .insert(EntryDetector::Extension);
                    }
                }
                _ => {} // Don't apply rows for categories the provider did not answer.
            }
        }
        if !entries.is_empty() {
            let covered = entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<BTreeSet<_>>();
            let existing = &mut tree.entry_points.entries;
            existing.retain(|entry| !covered.contains(entry.path.as_str()));
            existing.extend(entries);
            existing.sort_unstable();
        }
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
}
