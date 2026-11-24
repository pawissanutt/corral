use super::*;

#[test]
fn entry_points_golden() {
    let repo = Repo::new();
    let workspace = tempfile::tempdir().unwrap();
    let default_cache = FactsCache::new(workspace.path());
    let cache = default_cache.clone().with_operation_enums(&["Op".into()]);
    repo.put("backend/Cargo.toml", "[package]\nname = 'backend'\n");
    repo.put("backend/src/main.rs", "fn main() {}\n");
    let commands = "#[derive(clap::Subcommand)]\n#[command(rename_all = \"snake_case\")]\nenum Command {\n    DoWork { tuple: (i32, i32) },\n    #[command(name = \"chosen\")]\n    AnAlias,\n    #[command(skip)]\n    Internal,\n}\n";
    repo.put("backend/src/commands.rs", commands);
    repo.put(
        "backend/src/http.rs",
        "use axum::{Router, routing::get};\nfn routes() {\n    Router::new()\n        .route(\"/tasks\", get(list));\n}\n",
    );
    repo.put(
        "backend/src/ops.rs",
        "enum Op {\n    Read,\n    Write { name: String },\n}\n",
    );
    repo.put(
        "web/package.json",
        "{\n  \"name\": \"@demo/web\",\n  \"bin\": {\n    \"alpha\": \"cli.js\",\n    \"beta\": \"cli.js\"\n  }\n}\n",
    );
    let express = "import express from 'express';\nconst app = express();\n// app.get('/comment', fn);\nconst hint = \"app.get('/string', fn)\";\napp.post('/jobs', jobs);\napp.get(dynamic, handler);\n";
    repo.put("web/app.ts", express);
    repo.put("docs/readme.md", "No entry detector here.\n");
    let base = repo.commit("2025-01-01T00:00:00Z", "entry declarations");
    // Same cache root and tree, different operation selector configuration.
    let before = default_cache.tree_facts(repo.path(), base.tree()).unwrap();
    assert!(!before.entry_points.coverage["backend"].contains(&EntryDetector::OperationEnum));
    let before = cache.tree_facts(repo.path(), base.tree()).unwrap();
    repo.put(
        "backend/src/commands.rs",
        &format!("// Line-only move\n{commands}"),
    );
    repo.put(
        "web/app.ts",
        &express.replace("app.post('/jobs', jobs)", "app.put('/jobs', jobs)"),
    );
    repo.put(
        "backend/src/ops.rs",
        "enum Op {\n    Read,\n    Reject,\n}\n",
    );
    let head = repo.commit("2025-01-02T00:00:00Z", "change entry declarations");
    let builtin = default_cache.delta(repo.path(), &base, &head).unwrap();
    assert_eq!(
        (
            builtin.entry_points.added.len(),
            builtin.entry_points.removed.len()
        ),
        (1, 1)
    );
    let delta = cache.delta(repo.path(), &base, &head).unwrap();
    let mut snapshot = String::new();
    rows(&mut snapshot, "entry", &before.entry_points.entries);
    rows(&mut snapshot, "detectors", &[before.entry_points.coverage]);
    rows(&mut snapshot, "added", &delta.entry_points.added);
    rows(&mut snapshot, "removed", &delta.entry_points.removed);
    assert_eq!(snapshot, include_str!("../tests/golden/entries.txt"));
}
