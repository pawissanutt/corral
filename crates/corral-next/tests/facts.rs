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
    for hidden in ["pi-startup", "pipeline"] {
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
    fs::create_dir(root.join(".corral-next")).unwrap();
    fs::create_dir(root.join("diagrams")).unwrap();
    fs::write(
        root.join(".corral-next/corral.toml"),
        "[projects.site]\npath = '.'\nbase = 'main'\nlenses = 'diagrams'\n",
    )
    .unwrap();
    fs::write(root.join("source.txt"), "a committed anchor\n").unwrap();
    git(root, &["add", "--", "source.txt"]);
    git(root, &["commit", "-m", "source"]);
    let read_at = git(root, &["rev-parse", "HEAD"]);
    let lens = format!(
        "id = 'one'\ntitle = \"One\\u001B[2J\"\nkind = 'sequence'\nread_at = '{}'\n\
         [[participants]]\nid = 'source'\nlabel = 'Source'\n\
         anchor = {{ path = 'source.txt', text = 'committed anchor' }}\n",
        read_at.trim(),
    );
    fs::write(root.join("diagrams/one.toml"), &lens).unwrap();
    git(root, &["add", "--", "diagrams"]);
    git(root, &["commit", "-m", "valid lens"]);
    fs::write(root.join("diagrams/bad.toml"), "kind = 'unknown'\n").unwrap();
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
    assert_eq!(result["lenses"][0]["file"], "diagrams/bad.toml");
    assert_eq!(result["lenses"][0]["status"], "malformed");
    assert_eq!(result["lenses"][1]["status"], "current");
    assert_eq!(result["lenses"][1]["lens"]["title"], "One\x1b[2J");
    let human = run(&["check", "one.toml"], false);
    assert!(human.status.success(), "{human:?}");
    assert!(!human.stdout.contains(&0x1b));
    for args in [
        &["list"][..],
        &["check", "one.toml"],
        &["check", "--at", "HEAD^"],
    ] {
        let output = run(args, true);
        assert!(output.status.success(), "{output:?}");
    }
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
