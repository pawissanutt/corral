use std::collections::{BTreeMap, BTreeSet};

use crate::model::*;

/// Map a container to its sole binary when that binary is unambiguous.
pub(crate) struct Owners<'a>(BTreeMap<&'a str, Option<&'a RuntimeUnitId>>);

impl<'a> Owners<'a> {
    pub(crate) fn new(units: &'a [RuntimeUnit]) -> Self {
        let mut binaries = BTreeMap::new();
        for unit in units
            .iter()
            .filter(|unit| unit.id.kind == RuntimeUnitKind::Binary)
        {
            binaries
                .entry(unit.container.as_str())
                .and_modify(|candidate| *candidate = None)
                .or_insert(Some(&unit.id));
        }
        Self(binaries)
    }

    pub(crate) fn of(&self, container: &str) -> RuntimeSource {
        match self.0.get(container).copied().flatten() {
            Some(id) => RuntimeSource::Unit { id: id.clone() },
            None => RuntimeSource::Container {
                path: container.to_owned(),
            },
        }
    }
}

pub(crate) fn finish(
    containers: &[ContainerFact],
    runtime: &RuntimeFacts,
    mut coverage: BTreeMap<String, BTreeSet<EntryDetector>>,
    sites: Vec<(&FileFact, EntryKind, String, u64)>,
) -> EntryPoints {
    let owners = Owners::new(&runtime.units);
    for container in containers {
        coverage.entry(container.path.clone()).or_default();
    }
    let mut entries: Vec<_> = sites
        .into_iter()
        .map(|(file, kind, name, line)| EntryPoint {
            kind,
            name,
            path: file.path.clone(),
            line,
            owner: owners.of(&file.container),
        })
        .collect();
    // npm `bin` declarations already have exact names and anchors in the runtime inventory.
    entries.extend(
        runtime
            .units
            .iter()
            .filter(|unit| {
                unit.id.kind == RuntimeUnitKind::Binary
                    && unit.id.path.rsplit('/').next() == Some("package.json")
            })
            .map(|unit| EntryPoint {
                kind: EntryKind::Command,
                name: unit.id.name.clone(),
                path: unit.id.path.clone(),
                line: unit.line,
                owner: owners.of(&unit.container),
            }),
    );
    entries.sort_unstable();
    EntryPoints { entries, coverage }
}

/// Compare entry-point snapshots, with entries sorted by kind, name, path, then line.
/// Changes in line or ownership alone are not reported.
pub fn compare(before: &EntryPoints, after: &EntryPoints) -> EntryPointDelta {
    let old = group(&before.entries);
    let new = group(&after.entries);
    let mut delta = EntryPointDelta {
        extension_containers: before
            .coverage
            .iter()
            .chain(&after.coverage)
            .filter(|(_, detectors)| detectors.contains(&EntryDetector::Extension))
            .map(|(container, _)| container.clone())
            .collect(),
        ..EntryPointDelta::default()
    };
    for key in old.keys().chain(new.keys()).collect::<BTreeSet<_>>() {
        let left = old.get(key).map(Vec::as_slice).unwrap_or_default();
        let right = new.get(key).map(Vec::as_slice).unwrap_or_default();
        if left.len() == right.len() {
            continue;
        }
        // Match identical lines first, then cancel line-only moves. A change in
        // binary ownership alone is not a change in what can be invoked.
        let (mut l, mut r) = (0, 0);
        let mut removed = Vec::new();
        let mut added = Vec::new();
        while l < left.len() && r < right.len() {
            use std::cmp::Ordering::*;
            match left[l].line.cmp(&right[r].line) {
                Less => {
                    removed.push(left[l]);
                    l += 1;
                }
                Greater => {
                    added.push(right[r]);
                    r += 1;
                }
                Equal => {
                    l += 1;
                    r += 1;
                }
            }
        }
        removed.extend_from_slice(&left[l..]);
        added.extend_from_slice(&right[r..]);
        delta
            .removed
            .extend(removed.iter().skip(added.len()).map(|&entry| entry.clone()));
        delta
            .added
            .extend(added.iter().skip(removed.len()).map(|&entry| entry.clone()));
    }
    delta
}

type Groups<'a> = BTreeMap<(EntryKind, &'a str, &'a str), Vec<&'a EntryPoint>>;

fn group(entries: &[EntryPoint]) -> Groups<'_> {
    let mut groups = Groups::new();
    // Snapshot entries are sorted by kind, name, path, then line.
    for entry in entries {
        groups
            .entry((entry.kind, &entry.name, &entry.path))
            .or_default()
            .push(entry);
    }
    groups
}
