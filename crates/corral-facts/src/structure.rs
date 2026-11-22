use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;

use crate::{git, manifests, model::*, syntax::Scanner};

pub(crate) fn collect(repo: &Path, tree: &str) -> Result<TreeFacts> {
    let mut paths = Vec::new();
    let mut line_counts = Vec::new();
    let mut manifest_text = BTreeMap::new();
    let mut source_text = BTreeMap::new();
    let skipped_non_utf8_paths = git::tree(repo, tree, |file| {
        if let Some(text) = file.text {
            if is_manifest(file.path) {
                manifest_text.insert(file.path.to_owned(), text.to_owned());
            } else if scans(language(file.path)) {
                source_text.insert(file.path.to_owned(), text.to_owned());
            }
        }
        paths.push(file.path.to_owned());
        line_counts.push(file.lines);
        Ok(())
    })?;
    let manifests::Extraction {
        containers,
        dependencies,
        parsed,
        entries,
    } = manifests::extract(&manifest_text, &paths);
    let container_paths: BTreeSet<&str> = containers.iter().map(|c| c.path.as_str()).collect();
    let mut coverage = Coverage {
        skipped_non_utf8_paths,
        ..Coverage::default()
    };
    let mut files: Vec<_> = paths
        .into_iter()
        .zip(line_counts)
        .map(|(path, lines)| {
            let lang = language(&path);
            let read_status = if parsed.contains(&path) || source_text.contains_key(&path) {
                coverage.complete += 1;
                ReadStatus::Complete
            } else if is_manifest(&path) || scans(lang) {
                coverage.failed += 1;
                ReadStatus::Failed
            } else {
                coverage.unsupported += 1;
                ReadStatus::Unsupported
            };
            let container = owner(&path, &container_paths);
            let module = module(&path, container);
            FileFact {
                test: is_test(&path, lang),
                container: container.to_owned(),
                module,
                path,
                language: lang.to_owned(),
                lines,
                read_status,
                definitions: vec![],
            }
        })
        .collect();
    files.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    let by_path: BTreeMap<_, _> = files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let modules: BTreeSet<_> = files.iter().map(|file| file.module.clone()).collect();
    let scanner = Scanner::new(&files, &containers, &dependencies, &entries);
    let mut refs = BTreeMap::new();
    let mut spawn_counts = BTreeMap::new();
    for (path, text) in source_text {
        let from = by_path[path.as_str()];
        let scan = scanner.scan(from, &text);
        coverage.unresolved_references += scan.unresolved;
        for (target, kind) in scan.references {
            let Some(to) = by_path.get(target.as_str()) else {
                coverage.unresolved_references += 1;
                continue;
            };
            coverage.resolved_references += 1;
            *refs.entry((&from.path, &to.path, kind)).or_default() += 1;
        }
        for process in scan.spawns {
            *spawn_counts.entry((&from.path, process)).or_default() += 1;
        }
    }
    let file_references = references(refs, ReferenceBasis::Syntax);
    let (module_references, package_edges) = roll_up(
        &file_references,
        ReferenceBasis::Syntax,
        &by_path,
        &dependencies,
    );
    Ok(TreeFacts {
        file_references,
        module_references,
        package_edges,
        spawns: spawn_counts
            .into_iter()
            .map(|((path, process), count)| SpawnFact {
                path: path.clone(),
                process,
                count,
            })
            .collect(),
        files,
        containers: directory_nodes(containers),
        modules: modules.into_iter().collect(),
        dependencies,
        coverage,
    })
}

// The scanner sees every manifest identity, but the map has exactly one node per directory.
fn directory_nodes(mut packages: Vec<ContainerFact>) -> Vec<ContainerFact> {
    packages.sort_unstable_by(|a, b| (&a.path, a.kind).cmp(&(&b.path, b.kind)));
    let mut nodes: Vec<ContainerFact> = Vec::new();
    for package in packages {
        if let Some(node) = nodes.last_mut().filter(|node| node.path == package.path) {
            node.other_packages.push(PackageIdentity {
                name: package.name,
                kind: package.kind,
                workspace_status: package.workspace_status,
            });
        } else {
            nodes.push(package);
        }
    }
    nodes
}

/// Build the two rollups for a snapshot's uniform reference basis. Declared
/// dependencies contribute package edges even if there are no references.
pub(crate) fn roll_up(
    file_references: &[Reference<String>],
    basis: ReferenceBasis,
    by_path: &BTreeMap<&str, &FileFact>,
    dependencies: &[DependencyFact],
) -> (Vec<Reference<ModuleId>>, Vec<PackageEdge>) {
    let mut module_counts = BTreeMap::<_, u64>::new();
    let mut package_counts = BTreeMap::<_, (bool, u64)>::new();
    for dep in dependencies {
        if let Some(target) = &dep.target
            && dep.section != "workspace.dependencies"
            && dep.container != *target
        {
            package_counts
                .entry((&dep.container, target))
                .or_default()
                .0 = true;
        }
    }
    for reference in file_references {
        let from = by_path[reference.from.as_str()];
        let to = by_path[reference.to.as_str()];
        let count = module_counts
            .entry((&from.module, &to.module, reference.kind))
            .or_default();
        *count = count.saturating_add(reference.count);
        if from.container != to.container {
            let (_, count) = package_counts
                .entry((&from.container, &to.container))
                .or_default();
            *count = count.saturating_add(reference.count);
        }
    }
    let packages = package_counts
        .into_iter()
        .map(|((from, to), (declared, references))| {
            let kind = match (declared, references) {
                (true, 0) => PackageEdgeKind::DeclaredOnly,
                (true, _) => PackageEdgeKind::DeclaredAndReferenced,
                (false, _) => PackageEdgeKind::ReferencedOnly,
            };
            PackageEdge {
                from: from.clone(),
                to: to.clone(),
                kind,
                references,
            }
        })
        .collect();
    (references(module_counts, basis), packages)
}

fn references<T: Clone>(
    counts: BTreeMap<(&T, &T, ReferenceKind), u64>,
    basis: ReferenceBasis,
) -> Vec<Reference<T>> {
    counts
        .into_iter()
        .map(|((from, to, kind), count)| Reference {
            from: from.clone(),
            to: to.clone(),
            kind,
            count,
            basis,
        })
        .collect()
}

fn owner<'a>(path: &'a str, containers: &BTreeSet<&str>) -> &'a str {
    let mut dir = path;
    while let Some((parent, _)) = dir.rsplit_once('/') {
        if containers.contains(parent) {
            return parent;
        }
        dir = parent;
    }
    ""
}

fn module(path: &str, container: &str) -> ModuleId {
    let local = path
        .strip_prefix(container)
        .unwrap_or(path)
        .trim_start_matches('/');
    let path = if let Some(source) = local.strip_prefix("src/") {
        if let Some((first, _)) = source.split_once('/') {
            first
        } else if matches!(
            source,
            "lib.rs" | "main.rs" | "mod.rs" | "index.ts" | "index.js" | "__init__.py"
        ) {
            ""
        } else {
            source.rsplit_once('.').map_or(source, |(stem, _)| stem)
        }
    } else {
        local.split_once('/').map_or("", |(first, _)| first)
    };
    ModuleId {
        container: container.to_owned(),
        path: path.to_owned(),
    }
}

/// Whether this repository-relative path names a manifest the built-in provider recognizes.
pub fn is_manifest(path: &str) -> bool {
    matches!(
        path.rsplit('/').next(),
        Some("Cargo.toml" | "package.json" | "pyproject.toml")
    )
}

fn scans(language: &str) -> bool {
    matches!(language, "Rust" | "TypeScript" | "JavaScript" | "Python")
}

fn language(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("rs") => "Rust",
        Some("ts" | "tsx" | "mts" | "cts") => "TypeScript",
        Some("js" | "jsx" | "mjs" | "cjs") => "JavaScript",
        Some("py" | "pyi") => "Python",
        Some("toml") => "TOML",
        Some("json" | "jsonc") => "JSON",
        Some("yaml" | "yml") => "YAML",
        Some("md" | "mdx" | "markdown") => "Markdown",
        Some("sh" | "bash" | "zsh") => "Shell",
        Some("css" | "scss" | "sass") => "CSS",
        Some("html" | "htm") => "HTML",
        Some("go") => "Go",
        Some("c" | "h") => "C",
        Some("cc" | "cpp" | "cxx" | "hpp") => "C++",
        Some("kt" | "kts") => "Kotlin",
        Some("java") => "Java",
        Some("cs") => "C#",
        Some("rb") => "Ruby",
        Some("swift") => "Swift",
        Some("sql") => "SQL",
        Some("svelte") => "Svelte",
        Some("vue") => "Vue",
        _ => "Other",
    }
}

fn is_test(path: &str, language: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.rsplit_once('/').is_some_and(|(dir, _)| {
        dir.split('/')
            .any(|part| matches!(part, "tests" | "test" | "__tests__"))
    }) || match language {
        "Rust" => name == "tests.rs" || name.ends_with("_test.rs") || name.ends_with("_tests.rs"),
        "Python" => name.starts_with("test_") || name.ends_with("_test.py"),
        "JavaScript" | "TypeScript" => name
            .rsplit_once('.')
            .is_some_and(|(stem, _)| stem.ends_with(".test") || stem.ends_with(".spec")),
        "Go" => name.ends_with("_test.go"),
        _ => false,
    }
}
