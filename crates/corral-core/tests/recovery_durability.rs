#![cfg(target_os = "linux")]

mod recovery_support;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use corral_core::{RecoveryLink, RecoveryReceipt, RecoveryReference, RecoveryState, Status, Store};

const TEST: &str = "recovery_publication_syncs_and_propagates_file_and_directory_failures";
const START: &str = "CORRAL_DURABILITY_TARGET";
const OK: &str = "CORRAL_DURABILITY_OK";
const FAILED: &str = "CORRAL_DURABILITY_FAILED";
const RETRY: &str = "CORRAL_UPGRADE_RETRY_START";

fn fixture(root: &Path, action: &str) {
    if action == "initial-upgrade" {
        initial_upgrade_fixture(root);
        return;
    }
    let store = Store::init(root).unwrap();
    let task = store
        .create("Durability", None, Status::NeedsMe, "")
        .unwrap();
    store.upgrade_schema().unwrap();
    let mut record = recovery_support::recovery(&store);
    if action != "intent" {
        store.save_recovery_record(&task.id, &record).unwrap();
    }
    match action {
        "intent" | "retry" | "ordinary" | "upgrade" => {}
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
    let before = fs::read(&task.path).unwrap();
    println!("{START}");
    std::io::stdout().flush().unwrap();
    let result = match action {
        "ordinary" => store
            .update(&task.id, |task| {
                task.title = "Ordinary recovery-bearing rewrite".into();
                Ok(())
            })
            .map(|_| ()),
        "upgrade" => store.upgrade_schema().map(|_| ()),
        "intent" | "outcome" | "link" | "retry" => store.save_recovery_record(&task.id, &record),
        unknown => panic!("unknown fixture action {unknown}"),
    };
    match std::env::var("CORRAL_DURABILITY_FAILURE").as_deref() {
        Ok(stage) => {
            assert!(
                result.is_err(),
                "{action}/{stage}: sync failure reported success"
            );
            if stage == "file" {
                assert_eq!(
                    fs::read(&task.path).unwrap(),
                    before,
                    "published before file sync"
                );
            }
            println!("{FAILED}");
        }
        Err(_) => {
            result.unwrap();
            if action == "retry" {
                assert_eq!(
                    fs::read(&task.path).unwrap(),
                    before,
                    "retry rewrote immutable bytes"
                );
            }
            println!("{OK}");
        }
    }
}

fn trace(action: &str, failure: Option<(&str, usize)>) -> String {
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
    if let Some((_, ordinal)) = failure {
        command.args(["-e", &format!("inject=fsync:error=EIO:when={ordinal}")]);
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
    if let Some((stage, _)) = failure {
        command.env("CORRAL_DURABILITY_FAILURE", stage);
    }
    let output = command.output().unwrap();
    let text = fs::read_to_string(&log).unwrap();
    if let Some(directory) = std::env::var_os("CORRAL_DURABILITY_EVIDENCE") {
        fs::create_dir_all(&directory).unwrap();
        let label = failure.map_or("success", |(stage, _)| stage);
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
    let expected = if failure.is_some() && action != "initial-upgrade" {
        FAILED
    } else {
        OK
    };
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == expected)
    );
    text
}

fn sync_line(line: &str, suffix: &str) -> bool {
    line.contains("fsync(") && line.contains(&format!("{suffix}>"))
}

fn initial_upgrade_fixture(root: &Path) {
    let store = Store::init(root).unwrap();
    let task = store
        .create("Initial upgrade", None, Status::NeedsMe, "Original spec\n")
        .unwrap();
    let original = fs::read(&task.path).unwrap();
    let config = fs::read(store.config_path()).unwrap();
    println!("{START}");
    std::io::stdout().flush().unwrap();
    let first = store.upgrade_schema();
    if std::env::var_os("CORRAL_DURABILITY_FAILURE").is_some() {
        assert!(
            first.is_err(),
            "injected initial-upgrade failure was hidden"
        );
    } else {
        first.unwrap();
    }
    println!("{RETRY}");
    std::io::stdout().flush().unwrap();
    let backup = store.upgrade_schema().unwrap();
    assert_eq!(
        fs::read(backup.join("tasks").join(task.path.file_name().unwrap())).unwrap(),
        original
    );
    assert_eq!(fs::read(backup.join("schema-version")).unwrap(), b"2\n");
    assert_eq!(
        fs::read(backup.join("complete")).unwrap(),
        b"schema-3 backup complete\n"
    );
    assert_eq!(fs::read(store.config_path()).unwrap(), config);
    assert_eq!(
        fs::read(store.state_dir().join("schema-version")).unwrap(),
        b"3\n"
    );
    assert_eq!(store.get(&task.id).unwrap(), task);
    println!("{OK}");
}

#[test]
fn initial_upgrade_retries_repair_every_failed_durability_boundary() {
    // Given: the first schema-2-to-3 upgrade, not an already-upgraded rewrite.
    let baseline = trace("initial-upgrade", None);
    let lines = baseline.lines().collect::<Vec<_>>();
    let start = lines.iter().position(|line| line.contains(START)).unwrap();
    let retry = lines.iter().position(|line| line.contains(RETRY)).unwrap();
    let boundaries = [
        (
            "task-backup",
            "/schema-3-backup/tasks/T-001-initial-upgrade.md",
        ),
        ("marker-backup", "/schema-3-backup/schema-version"),
        ("completion-backup", "/schema-3-backup/complete"),
        ("backup-tasks-directory", "/schema-3-backup/tasks"),
        ("backup-directory", "/schema-3-backup"),
        ("schema-marker", "/.corral/state/schema-version"),
        ("state-directory", "/.corral/state"),
        ("task-file", "/.corral/tasks/T-001-initial-upgrade.md"),
        ("task-directory", "/.corral/tasks"),
    ];
    let mut unrepaired = Vec::new();
    for (label, suffix) in boundaries {
        let index = (start + 1..retry)
            .find(|&i| sync_line(lines[i], suffix))
            .unwrap_or_else(|| panic!("missing first-upgrade sync: {label}"));
        let ordinal = lines[..=index]
            .iter()
            .filter(|line| line.contains("fsync("))
            .count();
        // When: one exact first-upgrade fsync fails, followed by a real public-API retry.
        let failed = trace("initial-upgrade", Some((label, ordinal)));
        let attempted = failed.lines().collect::<Vec<_>>();
        let retry = attempted
            .iter()
            .position(|line| line.contains(RETRY))
            .unwrap();
        let success = attempted.iter().position(|line| line.contains(OK)).unwrap();
        // Then: the injected boundary must fail first, then sync successfully before retry success.
        assert!(
            attempted[..retry]
                .iter()
                .any(|line| sync_line(line, suffix) && line.contains("= -1 EIO")),
            "{label}: injection missed the intended boundary"
        );
        if !attempted[retry + 1..success]
            .iter()
            .any(|line| sync_line(line, suffix) && line.ends_with("= 0"))
        {
            unrepaired.push(label);
        }
    }
    assert!(
        unrepaired.is_empty(),
        "upgrade retry reported success without repairing: {unrepaired:?}"
    );
}

#[test]
fn recovery_publication_syncs_and_propagates_file_and_directory_failures() {
    if let Some(root) = std::env::var_os("CORRAL_DURABILITY_ROOT") {
        fixture(
            Path::new(&root),
            &std::env::var("CORRAL_DURABILITY_ACTION").unwrap(),
        );
        return;
    }
    for action in ["intent", "outcome", "link", "ordinary", "retry", "upgrade"] {
        // Given: a trace of the actual public API, bounded by explicit stdout markers.
        let baseline = trace(action, None);
        let lines = baseline.lines().collect::<Vec<_>>();
        let start = lines.iter().position(|line| line.contains(START)).unwrap();
        let end = lines.iter().position(|line| line.contains(OK)).unwrap();
        let file_suffix = if action == "retry" {
            "/T-001-durability.md"
        } else {
            "/T-001-durability.md.tmp"
        };
        let file_sync = (start + 1..end)
            .find(|&i| sync_line(lines[i], file_suffix))
            .unwrap_or_else(|| panic!("{action}: missing replacement-file fsync before success"));
        let dir_sync = (file_sync + 1..end)
            .find(|&i| sync_line(lines[i], "/.corral/tasks"))
            .unwrap_or_else(|| panic!("{action}: missing task-directory fsync before success"));
        let rename = (start + 1..end).find(|&i| lines[i].contains("rename("));
        if action == "retry" {
            assert!(
                rename.is_none(),
                "equivalent retry must not rewrite the record"
            );
        } else {
            let rename = rename.unwrap();
            assert!(
                file_sync < rename && rename < dir_sync,
                "{action}: incorrect durable publication order"
            );
        }
        for (stage, index) in [("file", file_sync), ("directory", dir_sync)] {
            // When: strace injects EIO into that exact fsync, not a mocked store.
            let ordinal = lines[..=index]
                .iter()
                .filter(|line| line.contains("fsync("))
                .count();
            let failed = trace(action, Some((stage, ordinal)));
            // Then: the public operation must refuse success for either sync failure.
            let suffix = if stage == "file" {
                file_suffix
            } else {
                "/.corral/tasks"
            };
            assert!(
                failed
                    .lines()
                    .any(|line| sync_line(line, suffix) && line.contains("= -1 EIO")),
                "{action}/{stage}: fault did not hit the intended durability boundary"
            );
        }
    }
}
