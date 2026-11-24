use super::*;

#[test]
fn runtime_cargo_workspace_golden() {
    let repo = Repo::new();
    repo.put(
        "Cargo.toml",
        "[workspace]\nmembers = ['launch', 'worker']\n",
    );
    repo.put("launch/Cargo.toml", "[package]\nname = 'launch'\n");
    repo.put(
        "launch/src/main.rs",
        "use std::process::Command;\nfn main() {\n    Command::new(\"work\");\n    Command::new(\"git\");\n    Command::new(\"decoy\");\n}\n",
    );
    let manifest =
        "[package]\nname = 'worker'\n\n[[bin]]\nname = 'work'\npath = 'src/bin/work.rs'\n";
    repo.put("worker/Cargo.toml", manifest);
    repo.put("worker/src/bin/work.rs", "fn main() {}\n");
    // Cargo 2015 doesn't auto-discover additional binaries alongside [[bin]].
    repo.put("worker/src/bin/decoy.rs", "fn main() {}\n");
    let head = repo.commit("2025-01-01T00:00:00Z", "runtime workspace");
    repo.put(
        "worker/Cargo.toml",
        &manifest.replace("work.rs", "decoy.rs"),
    );
    let retargeted = repo.commit("2025-01-02T00:00:00Z", "retarget work");
    // Discovery must use committed files, not this dirty manifest or untracked binary.
    repo.put("worker/Cargo.toml", "[package]\nname = 'dirty'\n");
    repo.put("worker/src/bin/untracked.rs", "fn main() {}\n");
    let workspace = tempfile::tempdir().unwrap();
    let cache = FactsCache::new(workspace.path());
    let tree = cache.tree_facts(repo.path(), head.tree()).unwrap();
    let mut snapshot = runtime_rows(&tree.runtime);
    rows(&mut snapshot, "external", &tree.spawns);
    let delta = cache
        .delta(repo.path(), &head, &retargeted)
        .unwrap()
        .runtime;
    rows(&mut snapshot, "unit-change", &delta.units);
    assert!(delta.edges.is_empty());
    assert_eq!(snapshot, include_str!("../tests/golden/runtime-cargo.txt"));
    assert_eq!(
        tree,
        cache.tree_facts(workspace.path(), head.tree()).unwrap()
    );
}

#[test]
fn inherited_cargo_edition_and_same_named_binary_links() {
    let repo = Repo::new();
    let workspace = "[workspace]\nmembers = ['a', 'b']\nresolver = '1'\n[workspace.package]\nedition = '2015'\n";
    repo.put("Cargo.toml", workspace);
    let package = "[package]\nname = 'app'\nversion = '0.1.0'\nedition.workspace = true\n[[bin]]\nname = 'app'\npath = 'src/main.rs'\n[[bin]]\nname = 'worker'\n";
    repo.put("a/Cargo.toml", package);
    repo.put(
        "a/src/main.rs",
        "fn main() { std::process::Command::new(\"worker\"); }\n",
    );
    repo.put("a/src/bin/worker.rs", "fn main() {}\n");
    repo.put("a/src/bin/ghost.rs", "fn main() {}\n");
    repo.put(
        "b/Cargo.toml",
        "[package]\nname = 'worker'\nversion = '0.1.0'\nedition = '2024'\n",
    );
    repo.put("b/src/main.rs", "fn main() {}\n");
    repo.put(
        "compose.yaml",
        "services:\n  example:\n    image: scratch\n    command: [worker]\n",
    );
    let head = repo.commit("2025-01-01T00:00:00Z", "two workers");
    repo.put("Cargo.toml", &workspace.replace("2015", "2024"));
    let cache = tempfile::tempdir().unwrap();
    let cache = FactsCache::new(cache.path());
    let tree = cache.tree_facts(repo.path(), head.tree()).unwrap();

    let units = tree
        .runtime
        .units
        .iter()
        .filter(|unit| unit.id.kind == RuntimeUnitKind::Binary)
        .map(|unit| (unit.id.name.as_str(), unit.id.path.as_str()))
        .collect::<Vec<_>>();
    let links = tree
        .runtime
        .edges
        .iter()
        .map(|edge| {
            (
                edge.kind,
                edge.basis,
                edge.site.path.as_str(),
                edge.site.line,
                edge.to.path.as_str(),
            )
        })
        .collect::<Vec<_>>();
    use RuntimeBasis::{Declared, Syntax};
    use RuntimeEdgeKind::{Runs, Starts};
    assert_eq!(
        (units, links),
        (
            vec![
                ("app", "a/Cargo.toml"),
                ("worker", "a/Cargo.toml"),
                ("worker", "b/src/main.rs")
            ],
            vec![
                (Starts, Syntax, "a/src/main.rs", 1, "a/Cargo.toml"),
                (Starts, Syntax, "a/src/main.rs", 1, "b/src/main.rs"),
                (Runs, Declared, "compose.yaml", 4, "a/Cargo.toml"),
                (Runs, Declared, "compose.yaml", 4, "b/src/main.rs"),
            ],
        ),
    );
    assert!(tree.spawns.is_empty());

    // An explicit, non-ancestor workspace overrides the enclosing 2015 workspace.
    repo.put("Cargo.toml", &workspace.replace("'a', 'b'", "'b'"));
    repo.put(
        "work$pace/Cargo.toml",
        "[workspace]\nmembers = ['../a']\nresolver = '1'\n[workspace.package]\nedition = '2024'\n",
    );
    repo.put(
        "a/Cargo.toml",
        &package.replace(
            "edition.workspace = true",
            "edition.workspace = true\nworkspace = '../work$pace'",
        ),
    );
    let modern = repo.commit("2025-01-02T00:00:00Z", "use explicit modern workspace");
    let tree = cache.tree_facts(repo.path(), modern.tree()).unwrap();
    assert!(tree.runtime.units.iter().any(
        |unit| unit.id.kind == RuntimeUnitKind::Binary && unit.id.path == "a/src/bin/ghost.rs"
    ));
}

#[test]
fn runtime_compose_and_delta_golden() {
    let repo = Repo::new();
    let compose = "services:\n  api:\n    build: ./backend\n    ports: ['8080:80']\n    depends_on: [db]\n    command: [serve, '--mode', old]\n  db: {image: 'postgres:16'}\n";
    repo.put("compose.yaml", compose);
    repo.put(
        "backend/Dockerfile",
        "FROM scratch\nENTRYPOINT [\"serve\"]\nCMD [\"--mode\", \"prod\"]\n",
    );
    repo.put(
        "backend/pyproject.toml",
        "[project]\nname = 'backend'\n[project.scripts]\nserve = 'backend:main'\n",
    );
    repo.put("ui/package.json", "{\n  \"name\": \"@demo/ui\",\n  \"bin\": {\"ui\": \"cli.js\"},\n  \"scripts\": {\n    \"build\": \"vite build\"\n  }\n}\n");
    repo.put(
        "deploy/api.service",
        "[Unit]\nDescription=API\n[Service]\nExecStart=serve --foreground\n",
    );
    repo.put(
        ".github/workflows/test.yml",
        "jobs:\n  check:\n    steps:\n      - run: |\n          ui check\n          echo ok\n",
    );
    let base = repo.commit("2025-01-01T00:00:00Z", "runtime deployment");
    let workspace = tempfile::tempdir().unwrap();
    let cache = FactsCache::new(workspace.path());
    let before = cache.tree_facts(repo.path(), base.tree()).unwrap();
    for path in [
        "compose.yaml",
        "backend/Dockerfile",
        "deploy/api.service",
        ".github/workflows/test.yml",
    ] {
        assert_eq!(
            before
                .files
                .iter()
                .find(|file| file.path == path)
                .unwrap()
                .read_status,
            ReadStatus::Complete
        );
    }
    let mut snapshot = runtime_rows(&before.runtime);
    repo.put("compose.yaml", &format!("# moved declarations\n{compose}"));
    let shifted = repo.commit("2025-01-02T00:00:00Z", "shift site positions");
    assert_eq!(
        cache.delta(repo.path(), &base, &shifted).unwrap().runtime,
        RuntimeDelta::default()
    );
    repo.put("compose.yaml", "services:\n  api:\n    build: { context: ./backend }\n    ports: ['9090:80']\n    command: ['another-program']\n  cache: {image: 'redis:7'}\n");
    let head = repo.commit("2025-01-03T00:00:00Z", "change deployment");
    let delta = cache.delta(repo.path(), &base, &head).unwrap().runtime;
    rows(&mut snapshot, "unit-change", &delta.units);
    rows(&mut snapshot, "edge-change", &delta.edges);
    assert_eq!(
        snapshot,
        include_str!("../tests/golden/runtime-compose.txt")
    );
}

fn runtime_rows(facts: &RuntimeFacts) -> String {
    let mut out = String::new();
    rows(&mut out, "unit", &facts.units);
    rows(&mut out, "edge", &facts.edges);
    out
}
