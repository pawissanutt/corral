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
    for args in [
        &["init"][..],
        &["add", "--", "args.rs"],
        &["commit", "-m", "CLI source"],
    ] {
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
            .current_dir(repo.path())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }
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
