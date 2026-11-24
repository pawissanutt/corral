//! The sample's `system_review`: a task's change on its project's map, derived from the sample
//! diff's files and the sample `system_map`, so Changes › Impact shows without a daemon. Each
//! changed file touches its package and the module its path names. Each touched module moves a
//! reference count across its package's boundary and one inside it, its package edge and a call
//! edge with them; the first touched package's manifest gains a dependency and its first file a
//! spawn site. Where the project's extension answered (`corral`), each changed source file
//! changes a definition and a definition in a neighbouring untouched package depends on it, in
//! the largest file of its module (and, for the first, in the smallest too, which a module level
//! counts under "N more"). `site`'s extension failed, so its extension categories are not
//! provided. The head's map at a level is the sample's own, with the changed files in their
//! module and their definitions.

use std::collections::BTreeMap;

use corral_types::{
    ChangeKind, ContainerKind, DefinitionFact, DependencyFact, DependentFact, ModuleId,
    PackageEdge, ReadStatus, RuntimeSource, SystemAt, SystemCategorySummary,
    SystemImpactCategory as Category, SystemMap, SystemMapRow, SystemReview,
    SystemReviewRow as Row, SystemRuntimeRow, SystemStatus,
};

use super::system::detail;
use super::system_lenses::{impact, results};
use crate::model::system::base_name;

/// The modules of a package, from its level.
fn modules(project: &str, container: &str) -> Vec<ModuleId> {
    let at = SystemAt::Container {
        path: container.to_owned(),
    };
    detail(project, Some(&at))
        .map(|level| {
            level
                .rows
                .into_iter()
                .filter_map(|row| match row {
                    SystemMapRow::Module { module, .. } => Some(module),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The module holding `path`: the one its first segment under the package's `src/` names, or
/// the package's first.
fn module_of(project: &str, container: &str, path: &str) -> ModuleId {
    let all = modules(project, container);
    let segment = path
        .strip_prefix(&format!("{container}/src/"))
        .and_then(|rest| rest.split('/').next());
    all.iter()
        .find(|m| Some(m.path.as_str()) == segment)
        .or(all.first())
        .cloned()
        .unwrap_or_else(|| ModuleId {
            container: container.to_owned(),
            path: String::new(),
        })
}

/// The files of a module's level, largest first.
fn files_in(project: &str, module: &ModuleId) -> Vec<String> {
    let at = SystemAt::Module {
        module: module.clone(),
    };
    detail(project, Some(&at))
        .map(|level| {
            level
                .rows
                .into_iter()
                .filter_map(|row| match row {
                    SystemMapRow::File { path, .. } => Some(path),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A definition of a sample file other than its first, so a dependent is a real card there.
fn definition_in(project: &str, path: &str) -> Option<DefinitionFact> {
    let at = SystemAt::File {
        path: path.to_owned(),
    };
    detail(project, Some(&at)).ok().and_then(|level| {
        level
            .rows
            .into_iter()
            .filter_map(|row| match row {
                SystemMapRow::Definition { definition } => Some(definition),
                _ => None,
            })
            .nth(1)
    })
}

/// The definition a changed file's sample change touches: the one named after the file.
fn changed_definition(path: &str) -> String {
    base_name(path)
        .split('.')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// A changed file at its file level: the changed definition, then the file's tests.
fn changed_level(path: &str, added: u64) -> Vec<SystemMapRow> {
    let end = 6 + added;
    [
        (changed_definition(path), 1, end, "fn"),
        ("tests".to_owned(), end + 3, end + 40, "mod"),
    ]
    .into_iter()
    .map(|(name, line, end_line, kind)| SystemMapRow::Definition {
        definition: DefinitionFact {
            name,
            line,
            end_line,
            kind: kind.into(),
        },
    })
    .collect()
}

/// The head's map at `at`: the sample's, with each changed file in its module's level and its
/// definitions at its own.
fn level(
    project: &str,
    at: Option<&SystemAt>,
    changed: &[(String, ModuleId, u64)],
    language: impl Fn(&str) -> String,
) -> Result<SystemMap, String> {
    if let Some(SystemAt::File { path }) = at
        && let Some((_, _, added)) = changed.iter().find(|(p, ..)| p == path)
    {
        let mut map = detail(project, None)?;
        map.rows = changed_level(path, *added);
        return Ok(map);
    }
    let mut map = detail(project, at)?;
    if let Some(SystemAt::Module { module }) = at {
        for (path, holder, added) in changed {
            let listed = map
                .rows
                .iter()
                .any(|row| matches!(row, SystemMapRow::File { path: p, .. } if p == path));
            if holder == module && !listed {
                map.rows.push(SystemMapRow::File {
                    path: path.clone(),
                    language: language(&holder.container),
                    lines: 40 + 6 * added,
                    test: path.contains("test"),
                    read_status: ReadStatus::Complete,
                });
            }
        }
        map.rows.sort_by_key(|row| match row {
            SystemMapRow::File { lines, .. } => std::cmp::Reverse(*lines),
            _ => std::cmp::Reverse(0),
        });
    }
    Ok(map)
}

/// `files`: the change's paths with lines added and removed.
pub(super) fn review(
    task: &str,
    project: &str,
    base: &str,
    head: &str,
    files: &[(String, u64, u64)],
    at: Option<&SystemAt>,
) -> Result<SystemReview, String> {
    let map = detail(project, None)?;
    let mut containers: Vec<(String, ContainerKind, String)> = Vec::new();
    let mut package_edges: Vec<PackageEdge> = Vec::new();
    for row in &map.rows {
        match row {
            SystemMapRow::Container { container, stats } => containers.push((
                container.path.clone(),
                container.kind,
                stats.languages.keys().next().cloned().unwrap_or_default(),
            )),
            SystemMapRow::PackageEdge { edge } => package_edges.push(edge.clone()),
            _ => {}
        }
    }
    let owner = |path: &str| {
        containers
            .iter()
            .map(|c| &c.0)
            .filter(|c| c.is_empty() || path.starts_with(&format!("{c}/")))
            .max_by_key(|c| c.len())
            .cloned()
    };
    let extension = map.provider.as_ref().is_some_and(|p| p.error.is_none());
    let mut churn: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut rows = Vec::new();
    let mut changed: Vec<(String, ModuleId, u64)> = Vec::new();
    let mut facts: Vec<(Category, Row)> = Vec::new();
    for (path, added, removed) in files {
        // The sample diffs name the app's files from its own directory.
        let path = &match owner(path) {
            Some(_) => path.clone(),
            None => format!("apps/corral-app/{path}"),
        };
        let Some(container) = owner(path) else {
            continue;
        };
        let module = module_of(project, &container, path);
        let total = churn.entry(container.clone()).or_default();
        total.0 += added;
        total.1 += removed;
        changed.push((path.clone(), module.clone(), *added));
        rows.push(Row::File {
            path: path.clone(),
            container,
            module,
            added: *added,
            removed: *removed,
            binary: false,
            deleted: false,
        });
    }
    let first_source = changed.iter().position(|(path, ..)| source(path));
    for (n, (path, module, _)) in changed.iter().enumerate() {
        if path.contains("test") {
            facts.push((Category::Tests, Row::TestFile { path: path.clone() }));
        } else if !source(path) {
            facts.push((
                Category::NotRead,
                Row::NotRead {
                    path: path.clone(),
                    read_status: ReadStatus::Unsupported,
                },
            ));
        } else if extension {
            let name = changed_definition(path);
            facts.push((
                Category::Definitions,
                Row::Definition {
                    change: ChangeKind::Changed,
                    path: path.clone(),
                    name: name.clone(),
                    line: 1,
                },
            ));
            // A package that references the changed one and the change left alone, else any.
            let untouched = |c: &&String| !c.is_empty() && !churn.contains_key(*c);
            let other = package_edges
                .iter()
                .filter(|e| e.to == module.container)
                .map(|e| &e.from)
                .find(untouched)
                .or_else(|| containers.iter().map(|c| &c.0).find(untouched));
            if let Some(other) = other {
                let held = modules(project, other).into_iter().next();
                let listed = held
                    .as_ref()
                    .map(|m| files_in(project, m))
                    .unwrap_or_default();
                // The smallest file of a long module is one its level counts under "N more".
                let homes = if Some(n) == first_source {
                    vec![listed.first(), listed.last()]
                } else {
                    vec![listed.first()]
                };
                let mut homes: Vec<&String> = homes.into_iter().flatten().collect();
                homes.dedup();
                for home in homes {
                    let Some(definition) = definition_in(project, home) else {
                        continue;
                    };
                    facts.push((
                        Category::Dependents,
                        Row::Dependent {
                            dependent: DependentFact {
                                path: home.clone(),
                                name: definition.name,
                                line: definition.line,
                                of: name.clone(),
                                // The sample's dependents reference the change directly.
                                depth: Some(1),
                            },
                            module: held.clone(),
                        },
                    ));
                }
            }
        }
    }
    facts.extend(references(project, &changed, &package_edges, extension));
    if let Some((path, module, _)) = changed.first()
        && let Some((_, kind, _)) = containers.iter().find(|c| c.0 == module.container)
    {
        facts.extend(manifest(&module.container, *kind));
        facts.push((
            Category::ExternalProcesses,
            Row::Spawn {
                path: path.clone(),
                process: "git".into(),
                before: 0,
                after: 1,
            },
        ));
    }
    let mut modules: BTreeMap<ModuleId, (u64, u64)> = BTreeMap::new();
    for row in &rows {
        if let Row::File {
            module,
            added,
            removed,
            ..
        } = row
        {
            let total = modules.entry(module.clone()).or_default();
            total.0 += added;
            total.1 += removed;
        }
    }
    let mut out: Vec<Row> = churn
        .into_iter()
        .map(|(path, (added, removed))| Row::Container {
            path,
            added,
            removed,
        })
        .collect();
    out.extend(
        modules
            .into_iter()
            .map(|(module, (added, removed))| Row::Module {
                module,
                added,
                removed,
            }),
    );
    out.extend(rows);
    // Runtime and entry points: a unit or entry point of a touched package moved a line.
    let touched = |container: &str| {
        out.iter()
            .any(|row| matches!(row, Row::Container { path, .. } if path == container))
    };
    let runtime = super::system_runtime::detail(project)?;
    for row in &runtime.rows {
        match row {
            SystemRuntimeRow::Unit { unit } if touched(&unit.container) => {
                let mut moved = unit.clone();
                moved.line += 1;
                facts.push((
                    Category::Runtime,
                    Row::RuntimeUnit {
                        before: Some(unit.clone()),
                        after: Some(moved),
                    },
                ));
            }
            SystemRuntimeRow::Entry { entry } => {
                let owner = match &entry.owner {
                    RuntimeSource::Container { path } => path.clone(),
                    RuntimeSource::Unit { .. } => entry
                        .path
                        .split("/src/")
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                };
                if touched(&owner) {
                    facts.push((
                        Category::EntryPoints,
                        Row::Entry {
                            change: ChangeKind::Added,
                            entry: entry.clone(),
                        },
                    ));
                }
            }
            _ => {}
        }
    }
    for (change, excerpt) in super::system_data::data_changes(project, base, head) {
        facts.push((Category::DataModel, Row::DataChange { change, excerpt }));
    }
    for result in results(project)? {
        if let Some(impact) = impact(result, files) {
            facts.push((Category::Lenses, Row::Lens { impact }));
        }
    }
    let mut categories = Vec::new();
    for category in Category::ALL {
        let listed: Vec<Row> = facts
            .iter()
            .filter(|(c, _)| *c == category)
            .map(|(_, row)| row.clone())
            .collect();
        let by_extension = matches!(
            category,
            Category::Definitions | Category::CallEdges | Category::Dependents
        );
        categories.push(SystemCategorySummary {
            category,
            provided: !by_extension || extension,
            total: listed.len(),
        });
        out.extend(listed);
    }
    let language = |container: &str| {
        containers
            .iter()
            .find(|c| c.0 == container)
            .map(|c| c.2.clone())
            .unwrap_or_default()
    };
    let level = match at {
        None => map.rows,
        Some(_) => level(project, at, &changed, language)?.rows,
    };
    out.extend(level.into_iter().map(|row| Row::Map { row }));
    Ok(SystemReview {
        task: task.into(),
        project: project.into(),
        base: base.into(),
        base_estimated: false,
        head: head.into(),
        status: SystemStatus::Ready,
        error: None,
        provider: map.provider,
        skipped_non_utf8_paths: 0,
        omitted_rows: 0,
        categories,
        rows: out,
    })
}

fn source(path: &str) -> bool {
    path.ends_with(".rs") || path.ends_with(".ts") || path.ends_with(".tsx")
}

/// Each touched module's strongest reference across its package's boundary gains three, the
/// first inside it loses two, and a call edge follows the first; the package edges the boundary
/// references fold into gain what their pairs gained.
fn references(
    project: &str,
    changed: &[(String, ModuleId, u64)],
    package_edges: &[PackageEdge],
    extension: bool,
) -> Vec<(Category, Row)> {
    let mut out = Vec::new();
    let mut gained: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut seen: Vec<&ModuleId> = Vec::new();
    for (path, module, _) in changed {
        if seen.contains(&module) {
            continue;
        }
        seen.push(module);
        let at = SystemAt::Container {
            path: module.container.clone(),
        };
        let Ok(level) = detail(project, Some(&at)) else {
            continue;
        };
        let from_here: Vec<_> = level
            .rows
            .iter()
            .filter_map(|row| match row {
                SystemMapRow::ModuleReference { reference } if &reference.from == module => {
                    Some(reference)
                }
                _ => None,
            })
            .collect();
        let across = from_here
            .iter()
            .filter(|r| r.to.container != module.container)
            .max_by_key(|r| r.count);
        let inside = from_here
            .iter()
            .find(|r| r.to.container == module.container);
        if let Some(r) = across {
            out.push((
                Category::ModuleReferences,
                Row::ModuleReference {
                    from: r.from.clone(),
                    to: r.to.clone(),
                    before: r.count.saturating_sub(3),
                    after: r.count,
                },
            ));
            *gained
                .entry((r.from.container.clone(), r.to.container.clone()))
                .or_default() += r.count.min(3);
            if extension {
                out.push((
                    Category::CallEdges,
                    Row::CallEdge {
                        from: r.from.clone(),
                        to: r.to.clone(),
                        added: 2,
                        removed: 1,
                        changed: 0,
                        path: path.clone(),
                        line: 1,
                    },
                ));
            }
        }
        if let Some(r) = inside {
            out.push((
                Category::ModuleReferences,
                Row::ModuleReference {
                    from: r.from.clone(),
                    to: r.to.clone(),
                    before: r.count + 2,
                    after: r.count,
                },
            ));
        }
    }
    for ((from, to), gain) in gained {
        if let Some(edge) = package_edges.iter().find(|e| e.from == from && e.to == to) {
            let mut before = edge.clone();
            before.references = edge.references.saturating_sub(gain);
            out.push((
                Category::PackageDependencies,
                Row::PackageEdge {
                    before: Some(before),
                    after: Some(edge.clone()),
                },
            ));
        }
    }
    out
}

/// The touched package's manifest, with the dependency it gained.
fn manifest(container: &str, kind: ContainerKind) -> Vec<(Category, Row)> {
    let (file, section, package, requirement) = match kind {
        ContainerKind::Npm => ("package.json", "dependencies", "zod", "^3.23.0"),
        ContainerKind::Python => ("pyproject.toml", "project.dependencies", "httpx", ">=0.27"),
        ContainerKind::Cargo => ("Cargo.toml", "dependencies", "jiff", "0.2"),
        // A folder has no manifest.
        ContainerKind::Folder => return Vec::new(),
    };
    let manifest = format!("{container}/{file}");
    vec![
        (
            Category::PackageDependencies,
            Row::Manifest {
                path: manifest.clone(),
            },
        ),
        (
            Category::PackageDependencies,
            Row::Dependency {
                before: None,
                after: Some(DependencyFact {
                    container: container.to_owned(),
                    manifest,
                    section: section.into(),
                    alias: package.into(),
                    package: package.into(),
                    requirement: requirement.into(),
                    options: None,
                    target: None,
                }),
            },
        ),
    ]
}
