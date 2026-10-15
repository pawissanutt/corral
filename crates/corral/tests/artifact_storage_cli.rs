use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{Artifact, ArtifactKind, PinRef, RetentionAction, Status, Store};
use serde_json::Value;

fn invoke(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CORRAL_GATE_") || name.starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
    command
        .env("CORRAL_ROOT", root)
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn success(root: &Path, args: &[&str]) -> Output {
    let output = invoke(root, args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    output
}

fn fixture(root: &Path, action: RetentionAction) -> (Store, String, Artifact, Vec<u8>) {
    let mut store = Store::init(root).unwrap();
    store.config.artifacts.retention.action = action;
    store.config.artifacts.retention.larger_than = Some("1B".into());
    store.save_config().unwrap();
    let task = store
        .create("Storage CLI", None, Status::Ready, "")
        .unwrap();
    let content = b"test output\n".repeat(1024);
    let source = root.join("gate.log");
    std::fs::write(&source, &content).unwrap();
    success(
        root,
        &[
            "artifact",
            "add",
            &task.id,
            source.to_str().unwrap(),
            "--kind",
            "log",
        ],
    );
    let artifact = store.get(&task.id).unwrap().artifacts.remove(0);
    assert_eq!(artifact.kind, ArtifactKind::Log);
    (store, task.id, artifact, content)
}

#[test]
fn compression_is_transparent_to_show_and_open_after_confirmed_gc() {
    // Given a raw log and a viewer that copies exactly the file it receives.
    let tmp = tempfile::tempdir().unwrap();
    let (mut store, id, artifact, content) = fixture(tmp.path(), RetentionAction::Compress);
    let copied = tmp.path().join("opened.log");
    store
        .config
        .viewers
        .insert("log".into(), format!("cp {{path}} '{}'", copied.display()));
    store.save_config().unwrap();
    let reference = PinRef::of_artifact(&id, &artifact).to_string();

    // When the CLI previews the policy, refuses an unconfirmed run, then applies it.
    let dry_run = success(tmp.path(), &["artifact", "gc", "--dry-run", "--json"]);
    let dry: Value = serde_json::from_slice(&dry_run.stdout).unwrap();
    assert!(dry["bytes_saved"].as_u64().unwrap() > 0);
    assert_eq!(
        std::fs::read(store.dir().join(&artifact.path)).unwrap(),
        content
    );
    assert!(!invoke(tmp.path(), &["artifact", "gc"]).status.success());
    success(tmp.path(), &["artifact", "gc", "--yes", "--json"]);
    let shown = success(tmp.path(), &["artifact", "show", &reference]);
    success(tmp.path(), &["open", &reference]);

    // Then both public readers receive the original bytes, not gzip bytes.
    assert_eq!(shown.stdout, content);
    assert_eq!(std::fs::read(copied).unwrap(), content);
    let stored = store.get(&id).unwrap().artifacts.remove(0);
    assert!(stored.compressed_bytes.unwrap() < stored.bytes);
    assert_eq!(stored.sha256, artifact.sha256);
}

#[test]
fn tier_override_and_pins_protect_content_from_purge() {
    // Given one eligible log, explicitly promoted to keep via the CLI.
    let tmp = tempfile::tempdir().unwrap();
    let (store, id, artifact, content) = fixture(tmp.path(), RetentionAction::Purge);
    let reference = PinRef::of_artifact(&id, &artifact).to_string();
    success(tmp.path(), &["artifact", "set-tier", &reference, "keep"]);

    // When retention runs with an explicit keep override, then again with a pin.
    success(tmp.path(), &["artifact", "gc", "--yes"]);
    assert_eq!(
        success(tmp.path(), &["artifact", "show", &reference]).stdout,
        content
    );
    success(
        tmp.path(),
        &["artifact", "set-tier", &reference, "transient"],
    );
    success(tmp.path(), &["pin", &reference]);
    success(tmp.path(), &["artifact", "gc", "--yes"]);

    // Then neither protected representation is purged.
    assert_eq!(
        success(tmp.path(), &["artifact", "show", &reference]).stdout,
        content
    );
    assert!(store.get(&id).unwrap().artifacts[0].purged_at.is_none());
}

#[test]
fn purge_preserves_artifact_metadata_and_storage_remains_queryable() {
    // Given an eligible log whose title, size and hash belong in history.
    let tmp = tempfile::tempdir().unwrap();
    let (mut store, id, artifact, _) = fixture(tmp.path(), RetentionAction::Purge);
    let copied = tmp.path().join("purged-stub.txt");
    store
        .config
        .viewers
        .insert("txt".into(), format!("cp {{path}} '{}'", copied.display()));
    store.save_config().unwrap();
    let reference = PinRef::of_artifact(&id, &artifact).to_string();

    // When the CLI purges it and lists the resulting records.
    success(tmp.path(), &["artifact", "gc", "--yes", "--json"]);
    let listed = success(tmp.path(), &["artifact", "list", "--task", &id, "--json"]);
    let rows: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let stats = success(tmp.path(), &["stats", "--storage", "--json"]);
    success(tmp.path(), &["open", &reference]);
    let stub = success(tmp.path(), &["artifact", "show", &reference]);

    // Then the stub retains identity and totals can still be read without herdr.
    assert_eq!(rows[0]["artifact"]["sha256"], artifact.sha256);
    assert_eq!(rows[0]["artifact"]["bytes"], artifact.bytes);
    assert!(rows[0]["artifact"]["purged_at"].is_string());
    assert_eq!(stub.stdout, std::fs::read(copied).unwrap());
    assert!(
        String::from_utf8(stub.stdout)
            .unwrap()
            .contains(&artifact.sha256)
    );
    assert!(
        serde_json::from_slice::<Value>(&stats.stdout)
            .unwrap()
            .is_object()
    );
}
