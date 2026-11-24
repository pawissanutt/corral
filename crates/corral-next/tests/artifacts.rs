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
fn artifact_list_hides_replaced_rows_only_in_default_text_output() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let id = init_task(root);
    let owner = |args: &[&str]| stdout(cli(root, "owner", args));
    let artifact_ref = |output: Vec<u8>| {
        let added: Value = serde_json::from_slice(&output).unwrap();
        format!("{id}:{}", added["artifact"]["id"].as_u64().unwrap())
    };

    let original = b"\x00old\r\n\xff";
    std::fs::write(root.join("report.bin"), original).unwrap();
    let old_ref = artifact_ref(owner(&[
        "--json",
        "artifact",
        "add",
        &id,
        "report.bin",
        "--kind",
        "report",
    ]));
    std::fs::write(root.join("report.bin"), b"updated").unwrap();
    let new_ref = artifact_ref(owner(&[
        "--json",
        "artifact",
        "add",
        &id,
        "report.bin",
        "--kind",
        "report",
        "--replaces",
        &old_ref,
    ]));

    let current = String::from_utf8(owner(&["artifact", "list", &id])).unwrap();
    assert_eq!(current.lines().count(), 1, "{current}");
    assert_eq!(current.split_whitespace().next(), Some(new_ref.as_str()));
    let all = String::from_utf8(owner(&["artifact", "list", &id, "--all"])).unwrap();
    let refs: Vec<_> = all
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(refs.len(), 2, "{all}");
    assert!(refs.contains(&old_ref.as_str()), "{all}");
    assert!(refs.contains(&new_ref.as_str()), "{all}");

    let rows: Vec<Value> =
        serde_json::from_slice(&owner(&["--json", "artifact", "list", &id])).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row["replaced_by"] == new_ref));
    assert!(rows.iter().any(|row| row["replaces"] == old_ref));
    let metadata: Value =
        serde_json::from_slice(&owner(&["--json", "artifact", "show", &old_ref])).unwrap();
    assert_eq!(metadata["replaced_by"], new_ref);
    assert_eq!(owner(&["artifact", "show", &old_ref]), original.as_slice());
}
