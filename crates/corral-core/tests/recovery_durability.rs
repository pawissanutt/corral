//! Recovery records are rows of the task database: their durability is the commit's, a WAL
//! that synchronous=FULL syncs before the operation reports success. These tests trace the
//! real public API and inject an I/O error into that exact sync.
#![cfg(target_os = "linux")]

mod recovery_support;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use corral_core::{RecoveryLink, RecoveryReceipt, RecoveryReference, RecoveryState, Status, Store};

const TEST: &str = "recovery_commits_sync_the_wal_and_propagate_sync_failures";
const START: &str = "CORRAL_DURABILITY_TARGET";
const OK: &str = "CORRAL_DURABILITY_OK";
const FAILED: &str = "CORRAL_DURABILITY_FAILED";
const WAL: &str = "/db/corral.db-wal";

fn fixture(root: &Path, action: &str) {
    let store = Store::init(root).unwrap();
    let task = store
        .create("Durability", None, Status::NeedsMe, "")
        .unwrap();
    let mut record = recovery_support::recovery(&store);
    if action != "intent" {
        store.save_recovery_record(&task.id, &record).unwrap();
    }
    match action {
        "intent" | "retry" | "ordinary" => {}
        "outcome" => recovery_support::append_outcome(&mut record, RecoveryState::Applying),
        "link" => record.links.push(RecoveryLink {
            recovery: RecoveryReference {
                operation: record.id.clone(),
                contract_digest: record.preview.contract_digest.clone(),
            },
            receipt: RecoveryReceipt::PlanReview {
                plan: "P1".into(),
                attempt: "PR2".into(),
            },
            acceptance: None,
        }),
        unknown => panic!("unknown fixture action {unknown}"),
    }
    let before = store.fixture_task_text(&task.id).unwrap().into_bytes();
    println!("{START}");
    std::io::stdout().flush().unwrap();
    let result = match action {
        "ordinary" => store
            .update(&task.id, |task| {
                task.title = "Ordinary recovery-bearing rewrite".into();
                Ok(())
            })
            .map(|_| ()),
        "intent" | "outcome" | "link" | "retry" => store.save_recovery_record(&task.id, &record),
        unknown => panic!("unknown fixture action {unknown}"),
    };
    match std::env::var("CORRAL_DURABILITY_FAILURE").as_deref() {
        Ok(stage) => {
            assert!(
                result.is_err(),
                "{action}/{stage}: sync failure reported success"
            );
            let reopened = Store::open_unobserved(root).unwrap();
            assert_eq!(
                reopened.fixture_task_text(&task.id).unwrap().into_bytes(),
                before,
                "a failed commit became visible"
            );
            println!("{FAILED}");
        }
        Err(_) => {
            result.unwrap();
            if action == "retry" {
                assert_eq!(
                    store.fixture_task_text(&task.id).unwrap().into_bytes(),
                    before,
                    "retry rewrote immutable bytes"
                );
            }
            println!("{OK}");
        }
    }
}

fn trace(action: &str, failure: Option<(&str, &str, usize)>) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("store");
    fs::create_dir_all(&root).unwrap();
    for directory in ["home", "config", "data", "state", "cache"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    let log = tmp.path().join("syscalls.log");
    let mut command = Command::new("timeout");
    command.args([
        "--kill-after=5",
        "30",
        "strace",
        "--kill-on-exit",
        "-f",
        "-qq",
        "-yy",
        "-s",
        "160",
        "-e",
        "trace=openat,write,fsync,fdatasync,rename,renameat,renameat2,close",
        "-o",
    ]);
    command.arg(&log);
    if let Some((_, syscall, ordinal)) = failure {
        command.args(["-e", &format!("inject={syscall}:error=EIO:when={ordinal}")]);
    }
    command
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("CORRAL_DURABILITY_ROOT", &root)
        .env("CORRAL_DURABILITY_ACTION", action)
        .stdin(Stdio::null());
    if let Some((stage, _, _)) = failure {
        command.env("CORRAL_DURABILITY_FAILURE", stage);
    }
    let output = command.output().unwrap();
    let text = fs::read_to_string(&log).unwrap();
    if let Some(directory) = std::env::var_os("CORRAL_DURABILITY_EVIDENCE") {
        fs::create_dir_all(&directory).unwrap();
        let label = failure.map_or("success", |(stage, _, _)| stage);
        fs::write(
            Path::new(&directory).join(format!("{action}-{label}.trace")),
            &text,
        )
        .unwrap();
        fs::write(
            Path::new(&directory).join(format!("{action}-{label}.stdout")),
            &output.stdout,
        )
        .unwrap();
        fs::write(
            Path::new(&directory).join(format!("{action}-{label}.stderr")),
            &output.stderr,
        )
        .unwrap();
    }
    assert!(
        output.status.success(),
        "{action}/{failure:?}: {:?}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = if failure.is_some() { FAILED } else { OK };
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == expected)
    );
    text
}

/// The sync syscall (`fsync` or `fdatasync`) a trace line makes on the WAL, if any.
fn wal_sync(line: &str) -> Option<&'static str> {
    if !line.contains(&format!("{WAL}>")) {
        return None;
    }
    ["fdatasync(", "fsync("]
        .into_iter()
        .find(|call| line.contains(call))
        .map(|call| call.trim_end_matches('('))
}

#[test]
fn recovery_commits_sync_the_wal_and_propagate_sync_failures() {
    if let Some(root) = std::env::var_os("CORRAL_DURABILITY_ROOT") {
        fixture(
            Path::new(&root),
            &std::env::var("CORRAL_DURABILITY_ACTION").unwrap(),
        );
        return;
    }
    for action in ["intent", "outcome", "link", "ordinary", "retry"] {
        // Given: a trace of the actual public API, bounded by explicit stdout markers.
        let baseline = trace(action, None);
        let lines = baseline.lines().collect::<Vec<_>>();
        let start = lines.iter().position(|line| line.contains(START)).unwrap();
        let end = lines.iter().position(|line| line.contains(OK)).unwrap();
        let sync = (start + 1..end).find(|&i| wal_sync(lines[i]).is_some());
        if action == "retry" {
            // Then: an equivalent retry commits nothing, so nothing reaches the WAL.
            assert!(sync.is_none(), "equivalent retry must not write the record");
            assert!(
                !lines[start + 1..end]
                    .iter()
                    .any(|line| line.contains("write(") && line.contains(&format!("{WAL}>"))),
                "equivalent retry wrote to the WAL"
            );
            continue;
        }
        let index = sync.unwrap_or_else(|| panic!("{action}: missing WAL sync before success"));
        let syscall = wal_sync(lines[index]).unwrap();
        let ordinal = lines[..=index]
            .iter()
            .filter(|line| {
                line.contains(&format!(" {syscall}(")) || line.starts_with(&format!("{syscall}("))
            })
            .count();
        // When: strace injects EIO into that exact sync, not a mocked store.
        let failed = trace(action, Some(("wal", syscall, ordinal)));
        // Then: the injected sync failed, and the operation refused success and left nothing.
        assert!(
            failed
                .lines()
                .any(|line| wal_sync(line).is_some() && line.contains("= -1 EIO")),
            "{action}: fault did not hit the WAL sync"
        );
    }
}
