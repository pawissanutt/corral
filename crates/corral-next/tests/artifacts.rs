use std::{
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;

fn cli(root: &Path, actor: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corral-next"))
        .arg("--root")
        .arg(root)
        .current_dir(root)
        .env("CORRAL_ACTOR", actor)
        .env_remove("CORRAL_TASK")
        .env_remove("CORRAL_RUN")
        .args(args)
        .output()
        .unwrap()
}

fn stdout(output: Output) -> Vec<u8> {
    assert!(output.status.success(), "{output:?}");
    output.stdout
}

fn init_task(root: &Path) -> String {
    stdout(cli(root, "owner", &["init"]));
    let new = stdout(cli(root, "owner", &["--json", "new", "Artifact test"]));
    let task: Value = serde_json::from_slice(&new).unwrap();
    task["task"]["id"].as_str().unwrap().to_owned()
}

#[test]
fn artifact_add_requires_kind_for_agent_and_defaults_to_reference_for_owner() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let id = init_task(root);
    std::fs::write(root.join("file.txt"), "input").unwrap();

    let add = ["artifact", "add", &id, "file.txt"];
    let agent = cli(root, "agent", &add);
    assert_eq!(agent.status.code(), Some(2));
    let refusal = String::from_utf8(agent.stderr).unwrap();
    assert!(refusal.contains("--kind"), "{refusal}");

    stdout(cli(root, "owner", &add));
    let list = stdout(cli(root, "owner", &["--json", "artifact", "list", &id]));
    let rows: Vec<Value> = serde_json::from_slice(&list).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["kind"], "reference");
}

#[test]
fn artifact_commands_refuse_unknown_owner_with_discovery_commands() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    stdout(cli(root, "owner", &["init"]));
    std::fs::write(root.join("x.md"), "reference").unwrap();

    for args in [
        &["artifact", "add", "unknown", "x.md", "--kind", "reference"][..],
        &["artifact", "list", "unknown"][..],
        &["artifact", "show", "unknown:1"][..],
    ] {
        let refusal = cli(root, "owner", args);
        assert_eq!(refusal.status.code(), Some(2), "{refusal:?}");
        let diagnostic = String::from_utf8(refusal.stderr).unwrap();
        for hint in ["unknown", "list --all", "config"] {
            assert!(diagnostic.contains(hint), "{diagnostic}");
        }
    }
}

#[test]
fn project_artifact_add_list_and_show_preserve_replaced_versions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let owner = |args: &[&str]| stdout(cli(root, "owner", args));
    owner(&["init"]);
    let config = root.join(".corral-next/corral.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    std::fs::write(&config, format!("{text}\n[projects.corral]\npath = '.'\n")).unwrap();
    let id = "corral";
    let artifact_ref = |output: Vec<u8>| {
        let added: Value = serde_json::from_slice(&output).unwrap();
        assert!(added["task"].is_null());
        assert_eq!(added["artifact"]["task"], id);
        assert_eq!(added["artifact"]["project"], id);
        format!("{id}:{}", added["artifact"]["id"].as_u64().unwrap())
    };

    let original = b"\x00old\r\n\xff";
    std::fs::write(root.join("x.md"), original).unwrap();
    let old_ref = artifact_ref(owner(&[
        "--json",
        "artifact",
        "add",
        id,
        "x.md",
        "--kind",
        "reference",
    ]));
    assert_eq!(old_ref, "corral:1");
    std::fs::write(root.join("x.md"), b"updated").unwrap();
    let new_ref = artifact_ref(owner(&[
        "--json",
        "artifact",
        "add",
        id,
        "x.md",
        "--kind",
        "reference",
        "--replaces",
        &old_ref,
    ]));
    assert_eq!(new_ref, "corral:2");

    let current = String::from_utf8(owner(&["artifact", "list", id])).unwrap();
    assert_eq!(current.lines().count(), 1, "{current}");
    assert_eq!(current.split_whitespace().next(), Some(new_ref.as_str()));
    let all = String::from_utf8(owner(&["artifact", "list", id, "--all"])).unwrap();
    let refs: Vec<_> = all
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(refs.len(), 2, "{all}");
    assert!(refs.contains(&old_ref.as_str()), "{all}");
    assert!(refs.contains(&new_ref.as_str()), "{all}");

    let rows: Vec<Value> =
        serde_json::from_slice(&owner(&["--json", "artifact", "list", id])).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row["replaced_by"] == new_ref));
    assert!(rows.iter().any(|row| row["replaces"] == old_ref));
    let metadata: Value =
        serde_json::from_slice(&owner(&["--json", "artifact", "show", &old_ref])).unwrap();
    assert_eq!(metadata["replaced_by"], new_ref);
    assert_eq!(owner(&["artifact", "show", &old_ref]), original.as_slice());
}
