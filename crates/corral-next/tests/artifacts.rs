use std::process::Command;

use serde_json::Value;

#[test]
fn artifact_add_requires_kind_for_agent_and_defaults_to_reference_for_owner() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cli = |actor: &str, args: &[&str]| {
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
    };
    assert!(cli("owner", &["init"]).status.success());
    let new = cli("owner", &["--json", "new", "Artifact test"]);
    assert!(new.status.success());
    let task: Value = serde_json::from_slice(&new.stdout).unwrap();
    let id = task["task"]["id"].as_str().unwrap();
    std::fs::write(root.join("file.txt"), "input").unwrap();

    let add = ["artifact", "add", id, "file.txt"];
    let agent = cli("agent", &add);
    assert_eq!(agent.status.code(), Some(2));
    let refusal = String::from_utf8(agent.stderr).unwrap();
    assert!(refusal.contains("--kind"), "{refusal}");

    let owner = cli("owner", &add);
    assert!(owner.status.success(), "{:?}", owner);
    let list = cli("owner", &["--json", "artifact", "list", id]);
    assert!(list.status.success());
    let rows: Vec<Value> = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["kind"], "reference");
}
