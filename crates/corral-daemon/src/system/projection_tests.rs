use std::collections::BTreeSet;

use corral_facts::{
    ContainerFact, ContainerKind, Coverage, DependencyChange, DependencyFact, FileChange,
    ModuleReferenceChange, PackageEdge, PackageEdgeKind, Reference, ReferenceBasis, ReferenceKind,
    SpawnFact, WorkspaceStatus,
    provider::{ChangeFact, DefinitionFact, DependentFact, TestCandidate},
};

use super::*;
use SystemImpactCategory as Category;
use SystemMapRow as Map;
use SystemReviewRow as Review;

fn container(path: &str) -> ContainerFact {
    ContainerFact {
        path: path.into(),
        name: path.into(),
        kind: ContainerKind::Folder,
        workspace_status: WorkspaceStatus::SeparateWorkspace,
        other_packages: vec![],
    }
}

fn module(container: &str, path: &str) -> ModuleId {
    ModuleId {
        container: container.into(),
        path: path.into(),
    }
}

fn file(path: &str, module: &ModuleId, lines: u64) -> FileFact {
    FileFact {
        path: path.into(),
        language: "Rust".into(),
        lines,
        test: false,
        read_status: ReadStatus::Complete,
        container: module.container.clone(),
        module: module.clone(),
        definitions: vec![],
    }
}

fn tree(files: Vec<FileFact>, containers: &[&str]) -> TreeFacts {
    TreeFacts {
        modules: files
            .iter()
            .map(|file| file.module.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        files,
        containers: containers.iter().map(|path| container(path)).collect(),
        dependencies: vec![],
        file_references: vec![],
        module_references: vec![],
        package_edges: vec![],
        spawns: vec![],
        coverage: Coverage::default(),
    }
}

fn change(path: &str, added: u64, removed: u64) -> FileChange {
    FileChange {
        path: path.into(),
        added,
        removed,
        binary: false,
    }
}

fn commit(id: &str, committed_at: i64, files: Vec<FileChange>) -> ActivityCommit {
    ActivityCommit {
        commit: id.into(),
        committed_at,
        subject: String::new(),
        files,
    }
}

fn activity(commits: Vec<ActivityCommit>) -> Activity {
    Activity {
        days_7: vec![],
        days_30: vec![],
        commits,
        shallow: false,
        skipped_non_utf8_paths: 0,
    }
}

fn edge(from: &str, to: &str, kind: PackageEdgeKind, references: u64) -> PackageEdge {
    PackageEdge {
        from: from.into(),
        to: to.into(),
        kind,
        references,
    }
}

#[test]
fn project_rows_roll_up_structure_coverage_and_unique_activity() {
    let code = module("app", "core");
    let mut notes = file("app/core/notes.md", &code, 15);
    notes.language = "Markdown".into();
    notes.read_status = ReadStatus::Unsupported;
    let mut tree = tree(
        vec![file("app/core/a.rs", &code, 20), notes],
        &["app", "tools"],
    );
    tree.package_edges
        .push(edge("app", "tools", PackageEdgeKind::DeclaredOnly, 0));
    tree.spawns.push(SpawnFact {
        path: "app/core/a.rs".into(),
        process: "git".into(),
        count: 2,
    });
    let now = 2 * WEEK_SECONDS;
    let activity = activity(vec![
        commit(
            "old",
            now - WEEK_SECONDS - 1,
            vec![change("app/core/a.rs", 8, 2)],
        ),
        commit(
            "latest",
            now,
            vec![
                change("app/core/a.rs", 3, 1),
                change("app/core/notes.md", 4, 2),
            ],
        ),
        commit(
            "week-endpoint",
            now - WEEK_SECONDS,
            vec![change("app/deleted.rs", 0, 5)],
        ),
    ]);
    let rows = map_rows(&tree, &activity, now, None).unwrap();
    let [
        Map::Container { container, stats },
        Map::Container { stats: empty, .. },
        Map::PackageEdge { edge },
        Map::Spawn {
            container: owner,
            spawn,
        },
    ] = rows.as_slice()
    else {
        panic!("unexpected project rows: {rows:?}");
    };
    assert_eq!(container.path, "app");
    assert_eq!(stats.lines, 35);
    assert_eq!(
        stats.languages,
        BTreeMap::from([("Markdown".into(), 15), ("Rust".into(), 20)])
    );
    assert_eq!((stats.complete, stats.unsupported, stats.failed), (1, 1, 0));
    assert_eq!(
        stats.days_7,
        SystemActivity {
            commits: 2,
            added: 7,
            removed: 8,
            last_commit: Some("latest".into())
        }
    );
    assert_eq!(
        stats.days_30,
        SystemActivity {
            commits: 3,
            added: 15,
            removed: 10,
            last_commit: Some("latest".into())
        }
    );
    assert_eq!(empty, &SystemNodeStats::default());
    assert_eq!(edge.kind, PackageEdgeKind::DeclaredOnly);
    assert_eq!(
        (owner.as_str(), spawn.path.as_str(), spawn.count),
        ("app", "app/core/a.rs", 2)
    );
}

#[test]
fn drill_in_keeps_boundary_neighbors_but_not_same_module_edges() {
    let code = module("app", "core");
    let tools = module("tools", "scripts");
    let mut big = file("app/core/b.rs", &code, 20);
    big.definitions = [14, 3]
        .into_iter()
        .map(|line| DefinitionFact {
            name: format!("line{line}"),
            line,
            end_line: line,
            kind: "function".into(),
        })
        .collect();
    let mut tree = tree(
        vec![file("app/core/a.rs", &code, 10), big],
        &["app", "declared", "elsewhere", "tools"],
    );
    tree.package_edges
        .push(edge("app", "declared", PackageEdgeKind::DeclaredOnly, 0));
    let reference = Reference {
        from: tools,
        to: code.clone(),
        kind: ReferenceKind::Import,
        basis: ReferenceBasis::Resolved,
        count: 4,
    };
    tree.module_references = vec![
        reference.clone(),
        Reference {
            from: code.clone(),
            ..reference.clone()
        },
    ];
    let activity = activity(vec![]);
    let map = |at| map_rows(&tree, &activity, 0, Some(&at));
    let rows = map(SystemAt::Container { path: "app".into() }).unwrap();
    let [
        Map::Module { module, stats },
        Map::Neighbor { container: first },
        Map::Neighbor { container: second },
        Map::ModuleReference { reference: shown },
    ] = rows.as_slice()
    else {
        panic!("unexpected container rows: {rows:?}");
    };
    assert_eq!((module, stats.lines), (&code, 30));
    assert_eq!(
        (first.path.as_str(), second.path.as_str(), shown),
        ("declared", "tools", &reference)
    );
    let rows = map(SystemAt::Module {
        module: code.clone(),
    })
    .unwrap();
    assert!(matches!(
        &rows[..],
        [Map::File { lines: 20, .. }, Map::File { lines: 10, .. }]
    ));
    let rows = map(SystemAt::File {
        path: "app/core/b.rs".into(),
    })
    .unwrap();
    assert!(matches!(
        &rows[..],
        [
            Map::Definition {
                definition: DefinitionFact { line: 3, .. }
            },
            Map::Definition {
                definition: DefinitionFact { line: 14, .. }
            }
        ]
    ));
    for at in [
        SystemAt::Container {
            path: "gone".into(),
        },
        SystemAt::Module {
            module: ModuleId {
                path: "gone".into(),
                ..code
            },
        },
        SystemAt::File {
            path: "gone".into(),
        },
    ] {
        assert!(map(at).unwrap_err().is::<NotFound>());
    }
}

fn delta(files: Vec<FileChange>) -> ProjectDelta {
    ProjectDelta {
        files,
        skipped_non_utf8_paths: 0,
        dependencies: vec![],
        module_references: vec![],
        changes: None,
        dependents: None,
        tests: None,
    }
}

fn summary(categories: &[SystemCategorySummary], category: Category) -> (bool, usize) {
    let entry = categories
        .iter()
        .find(|entry| entry.category == category)
        .unwrap();
    (entry.provided, entry.total)
}

#[test]
fn review_uses_head_ownership_base_for_deletes_and_builtin_edge_changes() {
    let old = module("old", "core");
    let new = module("new", "core");
    let mut deleted = file("old/tests/blob.bin", &old, 0);
    deleted.test = true;
    deleted.read_status = ReadStatus::Unsupported;
    let mut before = tree(vec![file("old/src.rs", &old, 10), deleted], &["old"]);
    let mut after = tree(vec![file("old/src.rs", &new, 11)], &["new", "untouched"]);
    use PackageEdgeKind::{DeclaredAndReferenced, DeclaredOnly, ReferencedOnly};
    before.package_edges = vec![
        edge("old", "a", DeclaredOnly, 0),
        edge("old", "b", ReferencedOnly, 2),
        edge("old", "c", DeclaredOnly, 0),
    ];
    after.package_edges = vec![
        edge("old", "a", DeclaredAndReferenced, 1),
        edge("old", "b", ReferencedOnly, 5),
        edge("old", "d", DeclaredOnly, 0),
    ];
    let spawn = SpawnFact {
        path: "old/src.rs".into(),
        process: "git".into(),
        count: 1,
    };
    before.spawns.push(spawn.clone());
    after.spawns.push(SpawnFact { count: 3, ..spawn });
    let mut delta = delta(vec![
        change("old/src.rs", 3, 2),
        FileChange {
            binary: true,
            ..change("old/tests/blob.bin", 0, 0)
        },
    ]);
    delta.dependencies.push(DependencyChange {
        before: None,
        after: Some(DependencyFact {
            container: "old".into(),
            manifest: "old/Cargo.toml".into(),
            section: "dependencies".into(),
            alias: "d".into(),
            package: "d".into(),
            requirement: "1".into(),
            options: None,
            target: Some("d".into()),
        }),
    });
    delta.module_references.push(ModuleReferenceChange {
        from: new.clone(),
        to: old.clone(),
        before: 2,
        after: 5,
    });
    let (categories, rows) = review(&delta, &before, &after, None, None).unwrap();
    assert_eq!(
        categories
            .iter()
            .map(|entry| entry.category)
            .collect::<Vec<_>>(),
        Category::ALL
    );
    assert_eq!(
        summary(&categories, Category::PackageDependencies),
        (true, 4)
    );
    assert_eq!(summary(&categories, Category::ModuleReferences), (true, 1));
    assert_eq!(summary(&categories, Category::ExternalProcesses), (true, 1));
    assert_eq!(summary(&categories, Category::Tests), (true, 1));
    assert_eq!(summary(&categories, Category::NotRead), (true, 1));
    assert!(rows.contains(&Review::Module {
        module: new,
        added: 3,
        removed: 2
    }));
    assert!(rows.contains(&Review::Container {
        path: "old".into(),
        added: 0,
        removed: 0
    }));
    assert!(rows.contains(&Review::File {
        path: "old/tests/blob.bin".into(),
        container: "old".into(),
        module: old,
        added: 0,
        removed: 0,
        binary: true
    }));
    assert!(rows.contains(&Review::TestFile {
        path: "old/tests/blob.bin".into()
    }));
    assert!(rows.contains(&Review::NotRead {
        path: "old/tests/blob.bin".into(),
        read_status: ReadStatus::Unsupported
    }));
    assert!(rows.contains(&Review::Spawn {
        path: "old/src.rs".into(),
        process: "git".into(),
        before: 1,
        after: 3
    }));
    assert!(rows.contains(&Review::Map {
        row: Map::Container {
            container: container("untouched"),
            stats: SystemNodeStats::default(),
        }
    }));
    assert!(!rows.iter().any(|row| matches!(row, Review::Map { row: Map::Container { container, .. } } if container.path == "old")));
}

#[test]
fn review_reports_a_manifest_edit_with_no_dependency_or_edge_delta() {
    let code = module("app", "");
    let before = tree(vec![file("app/Cargo.toml", &code, 4)], &["app"]);
    let delta = delta(vec![change("app/Cargo.toml", 1, 1)]);
    // The manifest's package version changed, not its dependency declarations.
    let (categories, rows) = review(&delta, &before, &before, None, None).unwrap();
    assert_eq!(
        summary(&categories, Category::PackageDependencies),
        (true, 1)
    );
    assert!(rows.contains(&Review::Manifest {
        path: "app/Cargo.toml".into()
    }));
}

/// The task page drills into the submitted head: a package level is served with the impact,
/// so a module the change touched can be drawn where it sits.
#[test]
fn review_serves_the_heads_package_level_when_asked() {
    let code = module("app", "feature");
    let head = tree(vec![file("app/src/feature/mod.rs", &code, 4)], &["app"]);
    let delta = delta(vec![change("app/src/feature/mod.rs", 1, 1)]);
    let at = SystemAt::Container { path: "app".into() };
    let (_, rows) = review(&delta, &head, &head, None, Some(&at)).unwrap();
    assert!(rows.iter().any(|row| matches!(row,
        Review::Map { row: Map::Module { module, .. } } if *module == code)));
    assert!(!rows.iter().any(|row| matches!(
        row,
        Review::Map {
            row: Map::Container { .. }
        }
    )));
}

#[test]
fn review_distinguishes_empty_provider_answers_and_groups_calls_with_anchors() {
    let a = module("a", "code");
    let b = module("b", "code");
    let mut src = file("a/new.rs", &a, 3);
    src.read_status = ReadStatus::Unsupported;
    let before = tree(
        vec![file("a/deleted.rs", &a, 3), file("b/lib.rs", &b, 3)],
        &["a", "b"],
    );
    let after = tree(vec![src, file("b/lib.rs", &b, 3)], &["a", "b"]);
    let mut delta = delta(vec![change("a/deleted.rs", 0, 3), change("a/new.rs", 3, 0)]);
    let (categories, builtin) = review(&delta, &before, &after, None, None).unwrap();
    assert!(builtin.contains(&Review::NotRead {
        path: "a/new.rs".into(),
        read_status: ReadStatus::Unsupported,
    }));
    for cat in [
        Category::Definitions,
        Category::CallEdges,
        Category::Dependents,
    ] {
        assert_eq!(summary(&categories, cat), (false, 0));
    }
    delta.changes = Some(vec![]);
    delta.dependents = Some(vec![]);
    let (categories, _) = review(&delta, &before, &after, None, None).unwrap();
    for cat in [
        Category::Definitions,
        Category::CallEdges,
        Category::Dependents,
    ] {
        assert_eq!(summary(&categories, cat), (true, 0));
    }
    let call = |change, path: &str, line| ChangeFact {
        change,
        path: path.into(),
        name: "main".into(),
        line,
        fact: ChangedFact::Reference {
            to_path: "b/lib.rs".into(),
            to_name: "helper".into(),
        },
    };
    delta.changes = Some(vec![
        ChangeFact {
            change: ChangeKind::Removed,
            fact: ChangedFact::Definition,
            path: "a/deleted.rs".into(),
            name: "main".into(),
            line: 1,
        },
        call(ChangeKind::Removed, "a/deleted.rs", 1),
        call(ChangeKind::Added, "a/new.rs", 3),
        call(ChangeKind::Changed, "a/new.rs", 2),
    ]);
    let dependent = DependentFact {
        path: "b/lib.rs".into(),
        name: "caller".into(),
        line: 2,
        of: "main".into(),
    };
    delta.dependents = Some(vec![
        DependentFact {
            path: "a/new.rs".into(),
            ..dependent.clone()
        },
        dependent.clone(),
    ]);
    let candidate = TestCandidate {
        path: "b/lib.rs".into(),
        name: "tests::caller".into(),
    };
    delta.tests = Some(vec![candidate.clone()]);
    // The delta's semantic row is evidence even without a snapshot at this head.
    let (categories, rows) = review(&delta, &before, &after, None, None).unwrap();
    assert_eq!(summary(&categories, Category::Definitions), (true, 1));
    assert_eq!(summary(&categories, Category::CallEdges), (true, 1));
    assert_eq!(summary(&categories, Category::Dependents), (true, 1));
    assert_eq!(summary(&categories, Category::Tests), (true, 1));
    assert_eq!(summary(&categories, Category::NotRead), (true, 0));
    assert!(rows.contains(&Review::Definition {
        change: ChangeKind::Removed,
        path: "a/deleted.rs".into(),
        name: "main".into(),
        line: 1
    }));
    assert!(rows.contains(&Review::CallEdge {
        from: a,
        to: b,
        added: 1,
        removed: 1,
        changed: 1,
        path: "a/new.rs".into(),
        line: 2
    }));
    assert!(
        rows.iter()
            .any(|row| matches!(row, Review::Dependent { dependent: d, .. } if *d == dependent))
    );
    assert!(rows.contains(&Review::TestCandidate { candidate }));
}
