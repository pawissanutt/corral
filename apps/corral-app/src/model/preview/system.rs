//! The sample's system facts: a `system` row per sample project and the `system_map` detail of
//! every level, generated from a small table so the page shows without a daemon. `corral` has
//! an extension that answered (definitions at the file level); `site`'s extension failed, so
//! its built-in map is drawn under the extension's error.

use std::collections::BTreeMap;

// The facts' module reference; the crate root's `Reference` is an artifact citation.
use corral_types::facts::Reference;
use corral_types::{
    ContainerFact, ContainerKind, Coverage, DefinitionFact, ModuleId, PackageEdge, PackageEdgeKind,
    ProviderCoverage, ReadStatus, ReferenceBasis, ReferenceKind, SpawnFact, SystemActivity,
    SystemAt, SystemMap, SystemMapRow, SystemNodeStats, SystemProvider, SystemRow, SystemStatus,
    WorkspaceStatus,
};

// The fact model's reference, not a citation in task text (`corral_types::Reference`).
use corral_types::facts::Reference;

use crate::model::Id;

use ContainerKind::{Cargo, Folder, Npm, Python};
use WorkspaceStatus::{Excluded, Member, SeparateWorkspace};

struct Container {
    path: &'static str,
    kind: ContainerKind,
    status: WorkspaceStatus,
    language: &'static str,
    lines: u64,
    modules: &'static [&'static str],
}

const fn c(
    path: &'static str,
    kind: ContainerKind,
    status: WorkspaceStatus,
    language: &'static str,
    lines: u64,
    modules: &'static [&'static str],
) -> Container {
    Container {
        path,
        kind,
        status,
        language,
        lines,
        modules,
    }
}

const CORRAL: &[Container] = &[
    c(
        "crates/corral-core",
        Cargo,
        Member,
        "Rust",
        48_200,
        &["store", "task", "config", "events", "lock", "frontmatter"],
    ),
    c(
        "crates/corral-types",
        Cargo,
        Member,
        "Rust",
        9_800,
        &["facts", "system", "status", "op"],
    ),
    c(
        "crates/corral-protocol",
        Cargo,
        Member,
        "Rust",
        21_400,
        &["detail", "rows", "subscription", "app", "session"],
    ),
    c(
        "crates/corral-git",
        Cargo,
        Member,
        "Rust",
        7_300,
        &["worktree", "diff", "merge"],
    ),
    c(
        "crates/corral-herdr",
        Cargo,
        Member,
        "Rust",
        5_100,
        &["client", "pane"],
    ),
    c(
        "crates/corral-engine",
        Cargo,
        Member,
        "Rust",
        61_000,
        &["dispatch", "review", "gate", "land", "facts", "pipeline"],
    ),
    c(
        "crates/corral-ops",
        Cargo,
        Member,
        "Rust",
        34_700,
        &["sync", "artifacts", "pm", "review"],
    ),
    c(
        "crates/corral-daemon",
        Cargo,
        Member,
        "Rust",
        52_300,
        &["views", "system", "topics", "app", "state"],
    ),
    c(
        "crates/corral-tui",
        Cargo,
        Member,
        "Rust",
        28_900,
        &["board", "screens", "keys"],
    ),
    c(
        "crates/corral",
        Cargo,
        Member,
        "Rust",
        18_600,
        &["cli", "plugin"],
    ),
    c(
        "crates/corral-legacy-board",
        Cargo,
        Excluded,
        "Rust",
        22_400,
        &["board", "render"],
    ),
    c(
        "apps/corral-app",
        Cargo,
        SeparateWorkspace,
        "Rust",
        141_000,
        &["model", "screens", "client", "state", "ui", "shell"],
    ),
    c(
        "packages/corral-pi",
        Npm,
        Member,
        "TypeScript",
        38_500,
        &["host", "tools", "session", "test"],
    ),
    c(
        "tools/pi-compare",
        Npm,
        Member,
        "TypeScript",
        2_900,
        &["compare"],
    ),
    c(
        "tools/load-sandbox",
        Folder,
        Member,
        "Shell",
        600,
        &["sandbox"],
    ),
    c(
        "scripts",
        Folder,
        Member,
        "Python",
        9_400,
        &["gate", "slow", "walk"],
    ),
    c(
        "docs",
        Folder,
        Member,
        "Markdown",
        31_000,
        &["plans", "guide", "mockup", "app-design"],
    ),
    c(".config", Folder, Member, "Config", 900, &["nextest"]),
];

/// (from, to, kind, references).
const CORRAL_EDGES: &[(usize, usize, PackageEdgeKind, u64)] = &[
    (0, 1, PackageEdgeKind::DeclaredAndReferenced, 212),
    (2, 1, PackageEdgeKind::DeclaredAndReferenced, 340),
    (3, 0, PackageEdgeKind::DeclaredAndReferenced, 18),
    (5, 0, PackageEdgeKind::DeclaredAndReferenced, 402),
    (5, 1, PackageEdgeKind::DeclaredAndReferenced, 288),
    (5, 3, PackageEdgeKind::DeclaredAndReferenced, 91),
    (5, 4, PackageEdgeKind::DeclaredAndReferenced, 37),
    (6, 5, PackageEdgeKind::DeclaredAndReferenced, 233),
    (6, 0, PackageEdgeKind::DeclaredAndReferenced, 145),
    (6, 4, PackageEdgeKind::DeclaredOnly, 0),
    (7, 6, PackageEdgeKind::DeclaredAndReferenced, 176),
    (7, 2, PackageEdgeKind::DeclaredAndReferenced, 520),
    (7, 5, PackageEdgeKind::DeclaredAndReferenced, 98),
    (8, 6, PackageEdgeKind::DeclaredAndReferenced, 121),
    (8, 2, PackageEdgeKind::DeclaredAndReferenced, 64),
    (9, 7, PackageEdgeKind::DeclaredAndReferenced, 44),
    (9, 8, PackageEdgeKind::DeclaredAndReferenced, 29),
    (9, 6, PackageEdgeKind::DeclaredAndReferenced, 81),
    (10, 0, PackageEdgeKind::DeclaredOnly, 0),
    (11, 2, PackageEdgeKind::DeclaredAndReferenced, 612),
    (11, 1, PackageEdgeKind::DeclaredAndReferenced, 455),
    (13, 12, PackageEdgeKind::ReferencedOnly, 6),
];

const CORRAL_SPAWNS: &[(usize, &str, u64)] = &[
    (3, "git", 41),
    (4, "herdr", 23),
    (5, "git", 6),
    (12, "claude", 3),
    (9, "herdr", 2),
];

const SITE: &[Container] = &[
    c(
        "apps/web",
        Npm,
        Member,
        "TypeScript",
        24_600,
        &["routes", "components", "hooks", "styles"],
    ),
    c(
        "packages/ui",
        Npm,
        Member,
        "TypeScript",
        11_200,
        &["button", "form", "table"],
    ),
    c(
        "packages/api-client",
        Npm,
        Member,
        "TypeScript",
        3_800,
        &["client", "types"],
    ),
    c(
        "services/api",
        Python,
        Member,
        "Python",
        18_900,
        &["handlers", "models", "auth", "db"],
    ),
    c(
        "services/worker",
        Python,
        Member,
        "Python",
        6_300,
        &["jobs", "queue"],
    ),
    c("infra", Folder, Member, "Config", 2_100, &["deploy"]),
    c("docs", Folder, Member, "Markdown", 4_700, &["guides"]),
];

const SITE_EDGES: &[(usize, usize, PackageEdgeKind, u64)] = &[
    (0, 1, PackageEdgeKind::DeclaredAndReferenced, 188),
    (0, 2, PackageEdgeKind::DeclaredAndReferenced, 73),
    (1, 2, PackageEdgeKind::DeclaredOnly, 0),
    (4, 3, PackageEdgeKind::ReferencedOnly, 12),
];

const SITE_SPAWNS: &[(usize, &str, u64)] = &[(4, "ffmpeg", 2)];

const FILE_WORDS: &[&str] = &[
    "mod", "parse", "render", "state", "apply", "read", "write", "tests", "view", "keys",
];
const DEFINITION_WORDS: &[(&str, &str)] = &[
    ("struct", "State"),
    ("fn", "new"),
    ("fn", "apply"),
    ("fn", "read"),
    ("enum", "Kind"),
    ("fn", "render"),
    ("impl", "Display for Kind"),
    ("fn", "parse"),
];

/// (from, to, kind, references) by index into the containers.
type Edges = &'static [(usize, usize, PackageEdgeKind, u64)];
/// (container, process, sites).
type Spawns = &'static [(usize, &'static str, u64)];

fn table(project: &str) -> Option<(&'static [Container], Edges, Spawns)> {
    match project {
        "corral" => Some((CORRAL, CORRAL_EDGES, CORRAL_SPAWNS)),
        "site" => Some((SITE, SITE_EDGES, SITE_SPAWNS)),
        _ => None,
    }
}

const HEAD: &str = "85b8f96031c4e2a7d9b0";

/// The sample's `system` rows.
pub(super) fn rows(projects: impl Iterator<Item = Id>) -> BTreeMap<Id, SystemRow> {
    projects
        .filter(|id| table(id.as_str()).is_some())
        .map(|id| {
            let corral = id.as_str() == "corral";
            let row = SystemRow {
                project: id.to_string(),
                head: Some(HEAD.into()),
                status: SystemStatus::Ready,
                extension: Some(if corral { "cxm-facts" } else { "site-facts" }.into()),
                snapshot_provided: corral.then(|| vec!["definitions".into(), "references".into()]),
                review_provided: None,
                last_extension_error: (!corral)
                    .then(|| "site-facts exited with status 1 after 30 s (index locked)".into()),
                error: None,
                revision: 1,
            };
            (id, row)
        })
        .collect()
}

fn spread(seed: &str, modulo: u64) -> u64 {
    seed.bytes()
        .fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)))
        % modulo
}

fn stats(seed: &str, language: &str, lines: u64, files: u64) -> SystemNodeStats {
    let commits = spread(seed, 40);
    let week = commits / 4;
    let unsupported = if matches!(language, "Markdown" | "Config" | "Shell") {
        files
    } else {
        files / 12
    };
    let failed = u64::from(spread(seed, 9) == 0);
    SystemNodeStats {
        lines,
        languages: BTreeMap::from([(language.to_owned(), lines)]),
        complete: files.saturating_sub(unsupported + failed),
        unsupported,
        failed,
        days_7: SystemActivity {
            commits: week,
            added: week * 37,
            removed: week * 12,
            last_commit: (week > 0).then(|| HEAD.into()),
        },
        days_30: SystemActivity {
            commits,
            added: commits * 41,
            removed: commits * 17,
            last_commit: (commits > 0).then(|| HEAD.into()),
        },
    }
}

fn fact(container: &Container) -> ContainerFact {
    ContainerFact {
        path: container.path.into(),
        name: container
            .path
            .rsplit('/')
            .next()
            .unwrap_or(container.path)
            .into(),
        kind: container.kind,
        workspace_status: container.status,
        other_packages: Vec::new(),
    }
}

fn module_lines(container: &Container, index: usize) -> u64 {
    // The first module is the largest; each next one is about two thirds of the one before.
    let weights: Vec<u64> = (0..container.modules.len())
        .map(|i| 3u64.pow((container.modules.len() - i) as u32))
        .collect();
    let total: u64 = weights.iter().sum();
    container.lines * weights[index] / total.max(1)
}

fn file_count(container: &Container, index: usize) -> u64 {
    (module_lines(container, index) / 420).clamp(2, 64)
}

fn extension(language: &str) -> &'static str {
    match language {
        "Rust" => "rs",
        "TypeScript" => "ts",
        "Python" => "py",
        "Markdown" => "md",
        "Shell" => "sh",
        _ => "toml",
    }
}

fn file_path(container: &Container, module: &str, n: u64) -> String {
    let word = FILE_WORDS[n as usize % FILE_WORDS.len()];
    let suffix = if n as usize >= FILE_WORDS.len() {
        format!("_{}", n as usize / FILE_WORDS.len())
    } else {
        String::new()
    };
    format!(
        "{}/src/{module}/{word}{suffix}.{}",
        container.path,
        extension(container.language)
    )
}

fn files_of(container: &Container, index: usize) -> Vec<SystemMapRow> {
    let module = container.modules[index];
    let count = file_count(container, index);
    let lines = module_lines(container, index);
    (0..count)
        .map(|n| {
            let path = file_path(container, module, n);
            let share = lines * 2 / (count * (n + 2)).max(1);
            SystemMapRow::File {
                read_status: if matches!(container.language, "Markdown" | "Config" | "Shell") {
                    ReadStatus::Unsupported
                } else {
                    ReadStatus::Complete
                },
                test: module == "test" || path.contains("tests"),
                language: container.language.into(),
                lines: share.max(12),
                path,
            }
        })
        .collect()
}

/// The `system_map` detail the preview serves.
pub(super) fn detail(project: &str, at: Option<&SystemAt>) -> Result<SystemMap, String> {
    let (containers, edges, spawns) =
        table(project).ok_or_else(|| format!("not_found: project {project}"))?;
    let corral = project == "corral";
    let module_id = |i: usize, m: usize| ModuleId {
        container: containers[i].path.into(),
        path: containers[i].modules[m].into(),
    };
    let mut rows = Vec::new();
    match at {
        None => {
            for container in containers {
                let files: u64 = (0..container.modules.len())
                    .map(|m| file_count(container, m))
                    .sum();
                rows.push(SystemMapRow::Container {
                    container: fact(container),
                    stats: stats(container.path, container.language, container.lines, files),
                });
            }
            for &(from, to, kind, references) in edges {
                rows.push(SystemMapRow::PackageEdge {
                    edge: PackageEdge {
                        from: containers[from].path.into(),
                        to: containers[to].path.into(),
                        kind,
                        references,
                    },
                });
            }
            for &(i, process, count) in spawns {
                rows.push(SystemMapRow::Spawn {
                    container: containers[i].path.into(),
                    spawn: SpawnFact {
                        path: file_path(&containers[i], containers[i].modules[0], 0),
                        process: process.into(),
                        count,
                    },
                });
            }
        }
        Some(SystemAt::Container { path }) => {
            let i = containers
                .iter()
                .position(|c| c.path == path)
                .ok_or("not_found: system container not found")?;
            let container = &containers[i];
            for m in 0..container.modules.len() {
                let seed = format!("{}/{}", container.path, container.modules[m]);
                rows.push(SystemMapRow::Module {
                    module: module_id(i, m),
                    stats: stats(
                        &seed,
                        container.language,
                        module_lines(container, m),
                        file_count(container, m),
                    ),
                });
            }
            let mut references = Vec::new();
            // Inside: each module uses the next two; a quarter of them stay unconnected.
            let linked = container.modules.len() - container.modules.len() / 4;
            for m in 0..linked {
                for n in [m + 1, m + 2] {
                    if n < linked {
                        references.push((
                            module_id(i, m),
                            module_id(i, n),
                            4 + spread(&format!("{m}{n}{path}"), 60),
                        ));
                    }
                }
            }
            for &(from, to, kind, count) in edges {
                if kind == PackageEdgeKind::DeclaredOnly {
                    continue;
                }
                if from == i {
                    references.push((module_id(i, 0), module_id(to, 0), count * 2 / 3));
                    if container.modules.len() > 1 {
                        references.push((module_id(i, 1), module_id(to, 0), count / 3));
                    }
                } else if to == i {
                    // The same pairs the referencing package's level lists.
                    references.push((module_id(from, 0), module_id(i, 0), count * 2 / 3));
                    if containers[from].modules.len() > 1 {
                        references.push((module_id(from, 1), module_id(i, 0), count / 3));
                    }
                }
            }
            let mut neighbours: Vec<usize> = edges
                .iter()
                .filter_map(|&(from, to, _, _)| match (from == i, to == i) {
                    (true, _) => Some(to),
                    (_, true) => Some(from),
                    _ => None,
                })
                .collect();
            neighbours.sort_unstable();
            neighbours.dedup();
            rows.extend(neighbours.into_iter().map(|n| SystemMapRow::Neighbor {
                container: fact(&containers[n]),
            }));
            rows.extend(
                references
                    .into_iter()
                    .filter(|r| r.2 > 0)
                    .map(|(from, to, count)| SystemMapRow::ModuleReference {
                        reference: Reference {
                            from,
                            to,
                            kind: if container.language == "Rust" {
                                ReferenceKind::Use
                            } else {
                                ReferenceKind::Import
                            },
                            basis: ReferenceBasis::Syntax,
                            count,
                        },
                    }),
            );
        }
        Some(SystemAt::Module { module }) => {
            let i = containers
                .iter()
                .position(|c| c.path == module.container)
                .ok_or("not_found: system module not found")?;
            let m = containers[i]
                .modules
                .iter()
                .position(|name| *name == module.path)
                .ok_or("not_found: system module not found")?;
            let mut files = files_of(&containers[i], m);
            files.sort_by_key(|row| match row {
                SystemMapRow::File { lines, .. } => std::cmp::Reverse(*lines),
                _ => std::cmp::Reverse(0),
            });
            rows = files;
        }
        Some(SystemAt::File { path }) => {
            let known = containers.iter().enumerate().any(|(i, c)| {
                (0..c.modules.len()).any(|m| {
                    files_of(&containers[i], m)
                        .iter()
                        .any(|row| matches!(row, SystemMapRow::File { path: p, .. } if p == path))
                })
            });
            if !known {
                return Err("not_found: system file not found".into());
            }
            // Only an extension supplies definitions; `site`'s failed.
            if corral && !path.ends_with(".md") {
                let mut line = 1;
                for (n, (kind, name)) in DEFINITION_WORDS
                    .iter()
                    .enumerate()
                    .take(4 + spread(path, 5) as usize)
                {
                    let length = 6 + spread(&format!("{path}{n}"), 40);
                    rows.push(SystemMapRow::Definition {
                        definition: DefinitionFact {
                            name: (*name).into(),
                            line,
                            end_line: line + length,
                            kind: (*kind).into(),
                        },
                    });
                    line += length + 2;
                }
            }
        }
    }
    let files: u64 = containers
        .iter()
        .map(|c| (0..c.modules.len()).map(|m| file_count(c, m)).sum::<u64>())
        .sum();
    let provider = SystemProvider {
        extension: if corral { "cxm-facts" } else { "site-facts" }.into(),
        coverage: if corral {
            ProviderCoverage {
                provided: vec!["definitions".into(), "references".into()],
                not_provided: Vec::new(),
                skipped: BTreeMap::new(),
                omitted: BTreeMap::new(),
                dropped_unknown_files: 0,
            }
        } else {
            ProviderCoverage::unavailable(&["definitions", "references"])
        },
        error: (!corral).then(|| "exited with status 1 after 30 s (index locked)".into()),
    };
    Ok(SystemMap {
        project: project.into(),
        head: Some(HEAD.into()),
        status: SystemStatus::Ready,
        error: None,
        provider: Some(provider),
        coverage: Some(Coverage {
            complete: files * 9 / 10,
            unsupported: files / 10,
            failed: 0,
            skipped_non_utf8_paths: 0,
            resolved_references: if corral { 74_556 } else { 0 },
            unresolved_references: if corral { 18_204 } else { 3_410 },
        }),
        omitted_rows: 0,
        rows,
    })
}
