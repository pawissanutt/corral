use std::path::Path;
use std::process::{Command, Stdio};
use std::{fmt::Write, fs};

use serde::Serialize;

use super::*;

#[path = "entry_tests.rs"]
mod entry_tests;
#[path = "runtime_tests.rs"]
mod runtime_tests;

struct Repo(tempfile::TempDir);

impl Repo {
    fn new() -> Self {
        let repo = Self(tempfile::tempdir().unwrap());
        repo.git(&["init", "-b", "main"], &[]);
        repo
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn put(&self, path: &str, contents: &str) {
        let path = self.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn git(&self, args: &[&str], env: &[(&str, &str)]) {
        let out = Command::new("git")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .envs(env.iter().copied())
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
            ])
            .arg("-C")
            .arg(self.path())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn commit(&self, date: &str, subject: &str) -> Revision {
        self.git(&["add", "-A"], &[]);
        self.git(
            &["commit", "--allow-empty", "-m", subject],
            &[("GIT_AUTHOR_DATE", date), ("GIT_COMMITTER_DATE", date)],
        );
        resolve_revision(self.path(), "HEAD").unwrap()
    }
}

fn rows<T: Serialize>(out: &mut String, label: &str, rows: &[T]) {
    for row in rows {
        writeln!(out, "{label} {}", serde_json::to_string(row).unwrap()).unwrap();
    }
}

fn snapshot(facts: &ProjectFacts) -> String {
    let mut out = String::new();
    let tree = &facts.structure;
    rows(&mut out, "file", &tree.files);
    rows(&mut out, "container", &tree.containers);
    rows(&mut out, "module", &tree.modules);
    rows(&mut out, "dependency", &tree.dependencies);
    rows(&mut out, "file-reference", &tree.file_references);
    rows(&mut out, "module-reference", &tree.module_references);
    rows(&mut out, "package-edge", &tree.package_edges);
    rows(&mut out, "spawn", &tree.spawns);
    rows(&mut out, "coverage", std::slice::from_ref(&tree.coverage));
    for (label, entries) in [
        ("activity-7", &facts.activity.days_7),
        ("activity-30", &facts.activity.days_30),
    ] {
        for entry in entries {
            let mut entry = entry.clone();
            assert_eq!(entry.last_commit.len(), facts.revision.commit.len());
            entry.last_commit = if entry.last_commit == facts.revision.commit {
                "head"
            } else {
                "ancestor"
            }
            .into();
            rows(&mut out, label, &[entry]);
        }
    }
    out
}

#[test]
fn cargo_workspace_golden() {
    let repo = Repo::new();
    let workspace = tempfile::tempdir().unwrap();
    let cache = FactsCache::new(workspace.path());
    repo.put(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/c\"]\n",
    );
    let a = "[package]\nname = \"a\"\nversion = \"0.1.0\"\n[dependencies]\nrenamed = { package = \"b\", path = \"../b\" }\nc = { path = \"../c\" }\nregistry_c = { package = \"c\", version = \"99\" }\n";
    repo.put("crates/a/Cargo.toml", a);
    for package in ["c", "d"] {
        repo.put(
            &format!("crates/{package}/Cargo.toml"),
            &format!("[package]\nname = \"{package}\"\nversion = \"0.1.0\"\n"),
        );
        repo.put(
            &format!("crates/{package}/src/lib.rs"),
            "pub struct Thing;\n",
        );
    }
    repo.put(
        "crates/b/Cargo.toml",
        "[package]\nname = \"b\"\nversion = \"0.1.0\"\n[lib]\npath = \"code/entry.rs\"\n",
    );
    repo.put(
        "crates/b/code/entry.rs",
        "pub mod api;\npub struct Thing;\n",
    );
    repo.put("crates/b/code/api.rs", "pub struct One;\npub struct Two;\n");
    repo.put("crates/a/tests/smoke.rs", "use a::util::X;\n");
    repo.put("crates/a/src/util.rs", "pub struct X;");
    repo.put(
        "crates/a/src/lib.rs",
        "use renamed::api::One;\nuse crate::util::X;\n",
    );
    let base = repo.commit("2025-01-01T00:00:00Z", "base");
    repo.put(
        "crates/a/Cargo.toml",
        &a.replace("path = \"../c\"", "path = \"../c\", features = [\"extra\"]"),
    );
    repo.put("crates/a/src/lib.rs", "use renamed::api::{One, Two};\nuse renamed::Thing;\nuse crate::util::X;\nuse d::Thing;\nuse std::process::Command;\nfn run() { Command::new(\"git\"); }\n");
    let head = repo.commit("2025-01-21T00:00:00Z", "change");
    let replacement = repo.commit("2025-03-01T00:00:00Z", "same tree, new history window");
    assert_eq!(replacement.tree, head.tree);
    repo.put("crates/a/src/lib.rs", "not the committed source\n");
    repo.put("untracked.md", "not committed");
    repo.put(".git/info/attributes", "* -diff\n");
    let facts = cache.project_facts(repo.path(), &head).unwrap();
    assert!(
        facts
            .activity
            .days_30
            .iter()
            .any(|entry| entry.last_commit == base.commit)
    );
    assert_eq!(
        facts
            .activity
            .commits
            .iter()
            .map(|commit| commit.commit.as_str())
            .collect::<Vec<_>>(),
        [head.commit(), base.commit()]
    );
    let delta = cache.delta(repo.path(), &base, &head).unwrap();
    assert_eq!(facts.activity.commits[0].files, delta.files);
    assert!(
        facts
            .structure
            .dependencies
            .iter()
            .any(|dep| dep.alias == "registry_c" && dep.target.is_none())
    );
    let mut out = snapshot(&facts);
    rows(&mut out, "delta-file", &delta.files);
    rows(&mut out, "delta-dependency", &delta.dependencies);
    rows(&mut out, "delta-module-reference", &delta.module_references);
    assert_eq!(out, include_str!("../tests/golden/cargo.txt"));

    // An inaccessible repository proves a cache hit did not rely on checked-out files
    // or a successful Git call, including after constructing a fresh cache handle.
    let absent = workspace.path().join("not-a-repository");
    let reopened = FactsCache::new(workspace.path());
    assert_eq!(reopened.project_facts(&absent, &head).unwrap(), facts);
    assert_eq!(reopened.delta(&absent, &base, &head).unwrap(), delta);
    assert_eq!(
        reopened.tree_facts(&absent, &replacement.tree).unwrap(),
        facts.structure
    );
    let other = reopened.project_facts(repo.path(), &replacement).unwrap();
    assert_eq!(other.structure, facts.structure);
    assert!(other.activity.days_30.is_empty());

    // Simulate resolving the same commit before and after deepening a shallow clone.
    let shallow_path = repo.path().join(".git/shallow");
    fs::write(&shallow_path, format!("{}\n", head.commit())).unwrap();
    let shallow = resolve_revision(repo.path(), head.commit()).unwrap();
    assert_eq!(shallow.tree(), head.tree());
    let partial = reopened.project_facts(repo.path(), &shallow).unwrap();
    assert!(partial.activity.shallow && partial.activity.days_30.is_empty());
    assert_eq!(reopened.project_facts(&absent, &shallow).unwrap(), partial);
    fs::remove_file(shallow_path).unwrap();
    let deepened = resolve_revision(repo.path(), head.commit()).unwrap();
    assert_eq!(reopened.project_facts(&absent, &deepened).unwrap(), facts);

    fs::remove_dir_all(workspace.path().join(".corral-next/cache")).unwrap();
    assert_eq!(reopened.project_facts(repo.path(), &head).unwrap(), facts);
}

#[test]
fn npm_relative_imports_golden() {
    let repo = Repo::new();
    let workspace = tempfile::tempdir().unwrap();
    repo.put(
        "package.json",
        r#"{"name":"app","workspaces":["packages/*"],"dependencies":{"@local/lib":"workspace:*"}}"#,
    );
    repo.put(
        "packages/lib/package.json",
        r#"{"name":"@local/lib","main":"src/api.js"}"#,
    );
    repo.put("src/util.ts", "export const util = 1;\n");
    repo.put("src/main.ts", "import { spawn } from 'node:child_process';\nimport { util } from './util.js';\nimport { lib } from '@local/lib';\nspawn('git', ['status']);\n");
    repo.put("src/util.test.ts", "import { util } from './util';\n");
    repo.put(
        "packages/lib/src/api.js",
        "export const lib = () => import('../../../src/util');\n",
    );
    repo.put("notes/readme.md", "No scanner.\n");
    let head = repo.commit("2025-01-01T00:00:00Z", "npm base");
    let facts = FactsCache::new(workspace.path())
        .project_facts(repo.path(), &head)
        .unwrap();
    assert_eq!(snapshot(&facts), include_str!("../tests/golden/npm.txt"));
}
