use std::{
    collections::BTreeSet,
    fs,
    process::{Command, Stdio},
};

use corral_engine::{Config, facts::FactsReader};

#[test]
fn detected_top_level_commands_agree_with_candidate_help() {
    let help = Command::new(env!("CARGO_BIN_EXE_corral-next"))
        .arg("--help")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    let (_, commands) = help.split_once("\nCommands:\n").unwrap();
    let mut listed: BTreeSet<_> = commands
        .lines()
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    // Clap synthesizes this command; it has no enum variant or source location.
    assert!(listed.remove("help"));

    let repo = tempfile::tempdir().unwrap();
    fs::write(repo.path().join("args.rs"), include_str!("../src/args.rs")).unwrap();
    git(repo.path(), &["init"]);
    git(repo.path(), &["add", "--", "args.rs"]);
    git(repo.path(), &["commit", "-m", "CLI source"]);
    let config = Config::parse("[projects.corral]\npath = '.'\n").unwrap();
    let reader = FactsReader::new(repo.path(), &config, "corral").unwrap();
    let tree = reader
        .builtin_tree(&reader.resolve_revision("HEAD").unwrap())
        .unwrap();
    // This source also holds nested command enums; limit parity to the top-level Command enum.
    let top_level_end = include_str!("../src/args.rs")
        .lines()
        .enumerate()
        .skip_while(|(_, line)| !line.starts_with("pub(crate) enum Command {"))
        .find(|(_, line)| *line == "}")
        .map(|(index, _)| index as u64 + 1)
        .unwrap();
    let mut detected: BTreeSet<_> = tree
        .entry_points
        .entries
        .iter()
        .filter(|entry| entry.path == "args.rs" && entry.line < top_level_end)
        .map(|entry| entry.name.as_str())
        .collect();
    // Hidden commands are still callable and have real source anchors.
    for hidden in [
        "board-plan",
        "pi-startup",
        "pipeline",
        "takeover-probe",
        "takeover-verify",
        "takeover-writer-lock",
    ] {
        assert!(detected.remove(hidden), "missing hidden command {hidden}");
    }
    println!("help (source-declared): {listed:?}\ndetected (non-hidden):   {detected:?}");
    assert_eq!(listed, detected);
}

#[test]
fn lens_check_selects_committed_files_and_fails_only_for_selected_errors() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-b", "main"]);
    corral_engine::Engine::init(root, None).unwrap();
    fs::create_dir(root.join("diagrams")).unwrap();
    fs::write(
        root.join(".corral-next/corral.toml"),
        "[projects.site]\npath = '.'\nbase = 'main'\nlenses = 'diagrams'\n",
    )
    .unwrap();
    fs::write(root.join("source.txt"), "a committed anchor\n").unwrap();
    git(root, &["add", "--", "source.txt"]);
    git(root, &["commit", "-m", "source"]);
    let lens = serde_json::json!({
        "schema":"corral.repo-diagram", "version":1, "id":"one", "title":"One", "layout":"graph",
        "nodes":[{"id":"source","label":"Source"}],
        "evidence":[{"element":"source","anchor":{"kind":"text","path":"source.txt","text":"committed anchor"}}]
    });
    fs::write(root.join("diagrams/one.json"), lens.to_string()).unwrap();
    // No legacy reader: a malformed TOML file beside the recipe isn't a lens.
    fs::write(root.join("diagrams/old.toml"), "kind = [").unwrap();
    git(root, &["add", "--", "diagrams"]);
    git(root, &["commit", "-m", "valid lens"]);
    fs::write(root.join("diagrams/bad.json"), "kind = 'unknown'\n").unwrap();
    git(root, &["add", "--", "diagrams"]);
    git(root, &["commit", "-m", "malformed sibling"]);
    fs::write(root.join("source.txt"), "uncommitted replacement\n").unwrap();

    let run = |args: &[&str], json: bool| {
        Command::new(env!("CARGO_BIN_EXE_corral-next"))
            .args(["--root", root.to_str().unwrap()])
            .args(json.then_some("--json"))
            .arg("lens")
            .args(args)
            .env_remove("CORRAL_PROJECT")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let all = run(&["check", "--project", "site"], true);
    assert_eq!(all.status.code(), Some(1), "{all:?}");
    let result: serde_json::Value = serde_json::from_slice(&all.stdout).unwrap();
    assert_eq!(result["lenses"][0]["file"], "diagrams/bad.json");
    assert_eq!(result["lenses"][0]["status"], "malformed");
    assert_eq!(result["lenses"][1]["status"], "current");
    assert_eq!(result["lenses"][1]["lens"]["title"], "One");
    assert_eq!(result["lenses"].as_array().unwrap().len(), 2);
    let human = run(&["check", "one.json"], false);
    assert!(human.status.success(), "{human:?}");
    for args in [
        &["list"][..],
        &["check", "one.json"],
        &["check", "--at", "HEAD^"],
    ] {
        let output = run(args, true);
        assert!(output.status.success(), "{output:?}");
    }

    // File through the real CLI. The dirty checkout did not invalidate the bound
    // text above; committing the removal now makes this an actionable repair.
    git(root, &["add", "source.txt"]);
    git(root, &["commit", "-m", "remove anchor"]);
    let file_task = |args: &[&str]| {
        let output = run(args, true);
        assert!(output.status.success(), "{output:?}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(value["run"].is_null());
        assert_eq!(value["task"]["project"], "site");
        value["task"].clone()
    };
    let drafted = file_task(&[
        "draft",
        "site",
        "--about",
        "Message order",
        "--preset",
        "ordered_lanes",
    ]);
    assert_eq!(drafted["status"], "backlog");
    assert!(drafted["body"].as_str().unwrap().contains("ordered_lanes"));
    let repaired = file_task(&["repair", "site", "one.json", "--ready"]);
    assert_ne!(repaired["id"], drafted["id"]);
    assert_eq!(repaired["status"], "ready");
    assert!(
        repaired["body"]
            .as_str()
            .unwrap()
            .contains("committed anchor")
    );
}

/// Check the shipped examples as authored, with the real source files and no provider.
/// The fixture uses a fresh project so worktree edits cannot masquerade as pinned reads.
#[test]
fn shipped_repository_diagrams_have_resolvable_evidence_and_a_derived_gate_reference() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join(".corral-next")).unwrap();
    fs::write(
        root.join(".corral-next/corral.toml"),
        "[projects.demo]\npath = '.'\nbase = 'main'\n",
    )
    .unwrap();
    git(root, &["init", "-b", "main"]);
    let copy = |path: &str| {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(workspace.join(path), target).unwrap();
    };
    for id in [
        "submit-to-land",
        "submit-work",
        "task-lifecycle",
        "engine-dependencies",
        "task-records",
    ] {
        copy(&format!(".corral/lenses/{id}.json"));
    }
    for path in [
        "crates/corral-engine/Cargo.toml",
        "crates/corral-engine/src/lib.rs",
        "crates/corral-engine/src/review.rs",
        "crates/corral-engine/src/store.rs",
        "crates/corral-engine/src/pipeline.rs",
        "crates/corral-engine/src/land.rs",
        "crates/corral-engine/src/tasks.rs",
        "crates/corral-engine/src/dispatch.rs",
        "crates/corral-types/src/lib.rs",
        "crates/corral-next/src/main.rs",
    ] {
        copy(path);
    }
    git(root, &["add", ".corral", "crates"]);
    git(root, &["commit", "-m", "repository lenses"]);
    let config = Config::load(&root.join(".corral-next/corral.toml")).unwrap();
    let reader = FactsReader::new(root, &config, "demo").unwrap();
    let rev = reader.resolve_revision("HEAD").unwrap();
    let (results, provider) = reader.check_lenses(&rev).unwrap();
    assert!(provider.is_none());
    assert_eq!(results.len(), 5);
    for result in &results {
        assert_eq!(result.status.as_str(), "current", "{result:?}");
        assert!(result.not_checked.is_empty(), "{result:?}");
    }
    let workflow = results
        .iter()
        .find(|r| r.file.ends_with("submit-to-land.json"))
        .unwrap();
    assert!(workflow.locations.iter().any(|l| l.element == "gate"
        && l.path == "crates/corral-engine/src/review.rs"
        && l.line > 1));
    assert!(workflow.derived.iter().any(|r| r.element == "gate"
        && r.reference.to == "crates/corral-engine/src/review.rs"
        && r.reference.from != r.reference.to));
}

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
        ])
        .current_dir(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}
