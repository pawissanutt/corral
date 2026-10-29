//! `corral storage` (T-334): a dry run in a fresh process changes no file and no change
//! sequence; a backup taken by the CLI opens and checks clean with `--root` on the copy.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::Store;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
            command.env_remove(name);
        }
    }
    command
        .env("CORRAL_ROOT", root)
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn success(root: &Path, args: &[&str]) -> Output {
    let out = command(root, args).output().unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    out
}

fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

fn tree(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            let what = if meta.is_dir() {
                pending.push(path.clone());
                "dir".to_owned()
            } else {
                format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap()))
            };
            out.push((path.strip_prefix(root).unwrap().display().to_string(), what));
        }
    }
    out.sort();
    out
}

#[test]
fn cold_dry_run_changes_nothing_and_a_backup_checks_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let seq = {
        let store = Store::init(&root).unwrap();
        store
            .put_evidence(
                "cli-test",
                "kept",
                b"referenced evidence",
                |r| json!({"blob": r}),
            )
            .unwrap();
        store.view().unwrap().seq().0
    };
    let before = tree(&root);
    let report = json_of(&success(&root, &["storage", "gc", "--dry-run", "--json"]));
    assert_eq!(report["dry_run"], json!(true));
    let after = tree(&root);
    let changed: Vec<_> = after
        .iter()
        .filter(|entry| !before.contains(entry))
        .chain(before.iter().filter(|entry| !after.contains(entry)))
        .collect();
    assert!(changed.is_empty(), "the dry run changed files: {changed:?}");
    assert_eq!(Store::open(&root).unwrap().view().unwrap().seq().0, seq);

    let check = json_of(&success(&root, &["storage", "check", "--json"]));
    assert_eq!(check["blobs_checked"], json!(1));
    let copy = root.join("backup");
    let backup = json_of(&success(
        &root,
        &["storage", "backup", &copy.to_string_lossy(), "--json"],
    ));
    assert_eq!(backup["blobs"], json!(1));
    let copied = json_of(&success(&copy, &["storage", "check", "--json"]));
    assert_eq!(copied["blobs_checked"], json!(1));
    assert_eq!(copied["database"], json!([]));
    assert_eq!(copied["blobs"], json!([]));
}

#[test]
fn gc_needs_dry_run_or_yes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    Store::init(&root).unwrap();
    let out = command(&root, &["storage", "gc"]).output().unwrap();
    assert!(!out.status.success());
}
