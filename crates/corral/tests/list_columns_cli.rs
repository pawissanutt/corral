use std::path::Path;
use std::process::{Command, Output, Stdio};

fn corral(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
            command.env_remove(name);
        }
    }
    command
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .expect("corral runs")
}

fn workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let init = corral(tmp.path(), &["init"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let add = corral(tmp.path(), &["new", "First task"]);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    tmp
}

#[test]
fn columns_print_the_chosen_board_card_fields() {
    let ws = workspace();
    let out = corral(ws.path(), &["list", "--columns", "id,cost,priority,title"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next().expect("one task");
    let cells: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(cells[1], "$0.00", "{line}");
    assert!(
        cells[0].starts_with("T-") && line.ends_with("First task"),
        "{line}"
    );
}

#[test]
fn unknown_column_lists_the_available_ones() {
    let ws = workspace();
    let out = corral(ws.path(), &["list", "--columns", "id,bogus"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("unknown column 'bogus'") && err.contains("cost"),
        "{err}"
    );
}

#[test]
fn columns_conflict_with_json() {
    let ws = workspace();
    assert!(
        !corral(ws.path(), &["list", "--columns", "id", "--json"])
            .status
            .success()
    );
}
