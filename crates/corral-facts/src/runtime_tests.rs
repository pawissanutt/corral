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
