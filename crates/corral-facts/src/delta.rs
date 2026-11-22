use std::collections::{BTreeMap, BTreeSet};

use crate::model::*;

pub(crate) fn compare(
    before: &TreeFacts,
    after: &TreeFacts,
    files: Vec<FileChange>,
    skipped_non_utf8_paths: u64,
) -> ProjectDelta {
    let old = dependencies(before);
    let new = dependencies(after);
    let mut dependencies = Vec::new();
    for key in keys(&old, &new) {
        // A Python dependency list can contain several declarations of the same name
        // (e.g. different version constraints for different platforms).
        let empty = BTreeSet::new();
        let left = old.get(key).unwrap_or(&empty);
        let right = new.get(key).unwrap_or(&empty);
        let mut removed = left.difference(right).copied();
        let mut added = right.difference(left).copied();
        loop {
            let (before, after) = (removed.next(), added.next());
            if before.is_none() && after.is_none() {
                break;
            }
            dependencies.push(DependencyChange {
                before: before.cloned(),
                after: after.cloned(),
            });
        }
    }
    let old = counts(before);
    let new = counts(after);
    let module_references = keys(&old, &new)
        .filter_map(|&(from, to)| {
            let key = (from, to);
            let before = old.get(&key).copied().unwrap_or(0);
            let after = new.get(&key).copied().unwrap_or(0);
            (before != after).then(|| ModuleReferenceChange {
                from: from.clone(),
                to: to.clone(),
                before,
                after,
            })
        })
        .collect();
    ProjectDelta {
        files,
        skipped_non_utf8_paths,
        dependencies,
        module_references,
    }
}

type Declarations<'a> = BTreeMap<(&'a str, &'a str, &'a str), BTreeSet<&'a DependencyFact>>;

fn dependencies(facts: &TreeFacts) -> Declarations<'_> {
    let mut by_key = Declarations::new();
    for dep in &facts.dependencies {
        by_key
            .entry((&dep.manifest, &dep.section, &dep.alias))
            .or_default()
            .insert(dep);
    }
    by_key
}

fn counts(facts: &TreeFacts) -> BTreeMap<(&ModuleId, &ModuleId), u64> {
    let mut counts = BTreeMap::new();
    for reference in &facts.module_references {
        *counts.entry((&reference.from, &reference.to)).or_default() += reference.count;
    }
    counts
}

fn keys<'a, K: Ord, V>(
    left: &'a BTreeMap<K, V>,
    right: &'a BTreeMap<K, V>,
) -> impl Iterator<Item = &'a K> {
    left.keys()
        .chain(right.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
}
