//! The sample's `system_review`: a task's change on its project's map, derived from the sample
//! diff's files and the sample `system_map`, so Changes › Impact shows without a daemon. Each
//! changed file touches its package and that package's first module; where the project's
//! extension answered (`corral`), each changed source file changes a definition and one
//! definition in the next untouched package depends on it. `site`'s extension failed, so its
//! extension categories are not provided. The head's map at a level is the sample's own.

use std::collections::BTreeMap;

use corral_types::{
    ChangeKind, DependentFact, ModuleId, ReadStatus, SystemAt, SystemCategorySummary,
    SystemImpactCategory as Category, SystemMapRow, SystemReview, SystemReviewRow as Row,
    SystemStatus,
};

use super::system::detail;

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
    let containers: Vec<String> = map
        .rows
        .iter()
        .filter_map(|row| match row {
            SystemMapRow::Container { container, .. } => Some(container.path.clone()),
            _ => None,
        })
        .collect();
    let owner = |path: &str| {
        containers
            .iter()
            .filter(|c| c.is_empty() || path.starts_with(&format!("{c}/")))
            .max_by_key(|c| c.len())
            .cloned()
    };
    let module_of = |container: &str| {
        let at = SystemAt::Container {
            path: container.to_owned(),
        };
        detail(project, Some(&at))
            .ok()
            .and_then(|level| {
                level.rows.into_iter().find_map(|row| match row {
                    SystemMapRow::Module { module, .. } => Some(module),
                    _ => None,
                })
            })
            .unwrap_or_else(|| ModuleId {
                container: container.to_owned(),
                path: String::new(),
            })
    };
    let extension = map.provider.as_ref().is_some_and(|p| p.error.is_none());
    let mut churn: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut rows = Vec::new();
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
        let module = module_of(&container);
        let total = churn.entry(container.clone()).or_default();
        total.0 += added;
        total.1 += removed;
        rows.push(Row::File {
            path: path.clone(),
            container,
            module,
            added: *added,
            removed: *removed,
            binary: false,
            deleted: false,
        });
        let source = path.ends_with(".rs") || path.ends_with(".ts") || path.ends_with(".tsx");
        if path.contains("test") {
            facts.push((Category::Tests, Row::TestFile { path: path.clone() }));
        } else if !source {
            facts.push((
                Category::NotRead,
                Row::NotRead {
                    path: path.clone(),
                    read_status: ReadStatus::Unsupported,
                },
            ));
        } else if extension {
            let name = crate::model::system::base_name(path)
                .split('.')
                .next()
                .unwrap_or_default()
                .to_owned();
            facts.push((
                Category::Definitions,
                Row::Definition {
                    change: ChangeKind::Changed,
                    path: path.clone(),
                    name: name.clone(),
                    line: 1,
                },
            ));
            if let Some(other) = containers
                .iter()
                .find(|c| !c.is_empty() && !churn.contains_key(*c))
            {
                facts.push((
                    Category::Dependents,
                    Row::Dependent {
                        dependent: DependentFact {
                            path: format!("{other}/src/lib.rs"),
                            name: format!("uses_{name}"),
                            line: 12,
                            of: name,
                        },
                        module: Some(module_of(other)),
                    },
                ));
            }
        }
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
    let mut categories = Vec::new();
    for category in Category::ALL
        .into_iter()
        .filter(|c| *c != Category::EntryPoints)
    {
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
    let level = match at {
        None => map.rows,
        Some(_) => detail(project, at)?.rows,
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
