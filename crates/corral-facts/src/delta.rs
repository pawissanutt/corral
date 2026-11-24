use std::collections::{BTreeMap, BTreeSet};

use crate::{DataFacts, model::*};

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
    let data_unreadable = files
        .iter()
        .filter(|file| {
            [&before.data, &after.data]
                .into_iter()
                .any(|data| data.unreadable_paths().contains(&file.path))
        })
        .map(|file| file.path.clone())
        .collect();
    ProjectDelta {
        files,
        skipped_non_utf8_paths,
        dependencies,
        module_references,
        runtime: runtime(&before.runtime, &after.runtime),
        entry_points: crate::entries::compare(&before.entry_points, &after.entry_points),
        data: compare_data(&before.data, &after.data),
        data_unreadable,
        changes: None,
        dependents: None,
        tests: None,
    }
}

/// Compare declarations and field shapes, not their positions or order in a source file.
pub fn compare_data(before: &DataFacts, after: &DataFacts) -> Vec<DataChange> {
    let old = data_declarations(before);
    let new = data_declarations(after);
    let mut changes = Vec::new();
    for id in keys(&old, &new) {
        if before.unreadable.contains(id.0) || after.unreadable.contains(id.0) {
            continue;
        }
        let make = |ty: &DataType, contract, change| DataChange {
            path: ty.path.clone(),
            name: ty.name.clone(),
            line: ty.line,
            kind: ty.kind,
            source: ty.source,
            contract,
            change,
        };
        let left = old.get(id);
        let right = new.get(id);
        let old_contract = left.is_some_and(|ty| before.contracts.contains(&ty.id()));
        let new_contract = right.is_some_and(|ty| after.contracts.contains(&ty.id()));
        match (left, right) {
            (Some(a), Some(b)) if a.kind == b.kind => {
                changes.extend(field_changes(a, b).map(|change| {
                    let ty = if matches!(change, DataChangeKind::FieldRemoved { .. }) {
                        a
                    } else {
                        b
                    };
                    make(ty, old_contract || new_contract, change)
                }));
            }
            (a, b) => {
                if let Some(a) = a {
                    changes.push(make(a, old_contract, DataChangeKind::TypeRemoved));
                }
                if let Some(b) = b {
                    changes.push(make(b, new_contract, DataChangeKind::TypeAdded));
                }
            }
        }
    }
    changes
}

#[derive(PartialOrd, Ord, PartialEq, Eq)]
enum DeclarationKey<'a> {
    Named(&'a str),
    Pointer(&'a str),
}

fn data_declarations(data: &DataFacts) -> BTreeMap<(&str, DeclarationKey<'_>), &DataType> {
    let pointers = data.schema_pointers();
    data.types
        .iter()
        .map(|ty| {
            let path = ty.path.as_str();
            let name = ty.name.as_str();
            let key = if ty.source == DataSource::JsonSchema {
                pointers
                    .get(&(path, name))
                    .map_or(DeclarationKey::Named(name), |pointer| {
                        DeclarationKey::Pointer(pointer)
                    })
            } else {
                DeclarationKey::Named(name)
            };
            ((path, key), ty)
        })
        .collect()
}

fn field_changes(before: &DataType, after: &DataType) -> impl Iterator<Item = DataChangeKind> {
    let old: BTreeMap<_, _> = before
        .fields
        .iter()
        .map(|field| (&field.name, field))
        .collect();
    let new: BTreeMap<_, _> = after
        .fields
        .iter()
        .map(|field| (&field.name, field))
        .collect();
    let mut changes = Vec::new();
    for name in keys(&old, &new) {
        use DataChangeKind::*;
        let change = match (old.get(name).copied(), new.get(name).copied()) {
            (None, Some(field)) => FieldAdded {
                field: field.clone(),
            },
            (Some(field), None) => FieldRemoved {
                field: field.clone(),
            },
            (Some(a), Some(b)) if a.ty != b.ty => FieldRetyped {
                before: a.clone(),
                after: b.clone(),
            },
            (Some(a), Some(b)) if a.optional != b.optional => FieldOptionalityChanged {
                before: a.clone(),
                after: b.clone(),
            },
            _ => continue,
        };
        changes.push(change);
    }
    changes.into_iter()
}

fn runtime(before: &RuntimeFacts, after: &RuntimeFacts) -> RuntimeDelta {
    let old: BTreeMap<_, _> = before.units.iter().map(|unit| (&unit.id, unit)).collect();
    let new: BTreeMap<_, _> = after.units.iter().map(|unit| (&unit.id, unit)).collect();
    let units = keys(&old, &new)
        .filter_map(|key| {
            let before = old.get(key).copied();
            let after = new.get(key).copied();
            if let (Some(a), Some(b)) = (before, after)
                && a.container == b.container
                && a.command == b.command
                && a.entry_point == b.entry_point
                && a.image == b.image
                && a.build_context == b.build_context
                && a.dockerfile == b.dockerfile
                && a.ports == b.ports
                && a.depends_on == b.depends_on
            {
                return None;
            }
            Some(RuntimeUnitChange {
                before: before.cloned(),
                after: after.cloned(),
            })
        })
        .collect();
    let old = edge_sites(before);
    let new = edge_sites(after);
    let mut edges = Vec::new();
    for key in keys(&old, &new) {
        let left = old.get(key).map(Vec::as_slice).unwrap_or_default();
        let right = new.get(key).map(Vec::as_slice).unwrap_or_default();
        if left.len() == right.len() {
            continue; // A line shift by itself is not a runtime change.
        }
        // Retain identical sites first, then cancel line-only moves. Equal lines can
        // occur more than once (e.g. two spawn calls on one line).
        let mut removed = Vec::new();
        let mut added = Vec::new();
        let (mut l, mut r) = (0, 0);
        while l < left.len() && r < right.len() {
            use std::cmp::Ordering::*;
            match left[l].site.line.cmp(&right[r].site.line) {
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
        edges.extend(
            removed
                .iter()
                .skip(added.len())
                .map(|&edge| RuntimeEdgeChange {
                    before: Some(edge.clone()),
                    after: None,
                }),
        );
        edges.extend(
            added
                .iter()
                .skip(removed.len())
                .map(|&edge| RuntimeEdgeChange {
                    before: None,
                    after: Some(edge.clone()),
                }),
        );
    }
    RuntimeDelta { units, edges }
}

type EdgeKey<'a> = (
    RuntimeEdgeKind,
    &'a RuntimeSource,
    &'a RuntimeUnitId,
    RuntimeBasis,
    &'a str,
);

fn edge_sites(facts: &RuntimeFacts) -> BTreeMap<EdgeKey<'_>, Vec<&RuntimeEdge>> {
    let mut sites = BTreeMap::<_, Vec<_>>::new();
    // RuntimeFacts sorts edges by kind, endpoints, basis, path and line.
    for edge in &facts.edges {
        sites
            .entry((
                edge.kind,
                &edge.from,
                &edge.to,
                edge.basis,
                edge.site.path.as_str(),
            ))
            .or_default()
            .push(edge);
    }
    sites
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
