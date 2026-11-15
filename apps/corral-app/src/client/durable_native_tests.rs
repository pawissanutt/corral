use super::*;
use serde_json::json;
use std::{
    io::{BufRead, BufReader},
    os::unix::fs::{PermissionsExt, symlink},
    process::{Command, Stdio},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "corral-app-durable-{}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond(),
            NEXT_STAGE.fetch_add(1, Ordering::Relaxed),
        )))
    }

    fn directory(&self) -> PathBuf {
        self.0.join("corral-app")
    }

    fn data(&self) -> PathBuf {
        self.directory().join("durable.json")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn patch(key: &str, value: Option<Value>) -> BTreeMap<String, Option<Value>> {
    BTreeMap::from([(key.into(), value)])
}

#[test]
fn independent_interleaved_cas_preserves_entries_and_cancellation() {
    const CHILD_ROOT: &str = "CORRAL_NATIVE_DURABILITY_CAS_TEST_ROOT";
    const READY: &str = "durable-child-snapshot-ready";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let (stale_revision, stale_records) = read_in(&root).unwrap();
        assert_eq!(stale_records["message-a"]["cancel"], false);
        println!("{READY}");
        std::io::stdout().flush().unwrap();
        let mut signal = [0];
        std::io::stdin().read_exact(&mut signal).unwrap();

        // A stale writer must not revive A, even when explicitly trying to write it.
        let mut stale_patch = patch("message-b", Some(json!({"text": "second"})));
        stale_patch.insert("message-a".into(), Some(stale_records["message-a"].clone()));
        assert!(!compare_exchange_in(&root, stale_revision, stale_patch).unwrap());
        let (revision, latest) = read_in(&root).unwrap();
        assert_eq!(revision, 3);
        assert_eq!(latest["message-a"]["cancel"], true);
        assert!(latest.contains_key("draft-c"));
        // Recompute the unrelated record patch, not a replacement of stale_records.
        assert!(
            compare_exchange_in(
                &root,
                revision,
                patch("message-b", Some(json!({"text": "second"}))),
            )
            .unwrap()
        );
        return;
    }

    let fixture = Fixture::new();
    assert_eq!(read_in(&fixture.0).unwrap(), (0, BTreeMap::new()));
    assert!(
        compare_exchange_in(
            &fixture.0,
            0,
            patch("message-a", Some(json!({"text": "first", "cancel": false}))),
        )
        .unwrap()
    );
    let lock_path = fixture.directory().join("durable.lock");
    let lock_inode = fs::metadata(&lock_path).unwrap().ino();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "client::durable_native::tests::independent_interleaved_cas_preserves_entries_and_cancellation",
            "--nocapture",
        ])
        .env(CHILD_ROOT, &fixture.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(
            output.read_line(&mut line).unwrap(),
            0,
            "child exited before reading its snapshot"
        );
        if line.trim_end().ends_with(READY) {
            break;
        }
    }
    assert!(
        compare_exchange_in(
            &fixture.0,
            1,
            patch("message-a", Some(json!({"text": "first", "cancel": true}))),
        )
        .unwrap()
    );
    assert!(
        compare_exchange_in(
            &fixture.0,
            2,
            patch("draft-c", Some(json!({"text": "unsent"}))),
        )
        .unwrap()
    );
    child.stdin.take().unwrap().write_all(&[1]).unwrap();
    let mut rest = String::new();
    output.read_to_string(&mut rest).unwrap();
    assert!(child.wait().unwrap().success(), "{rest}");
    let (revision, records) = read_in(&fixture.0).unwrap();
    assert_eq!(revision, 4);
    assert_eq!(records.len(), 3);
    assert_eq!(records["message-a"]["cancel"], true);
    assert_eq!(records["message-b"]["text"], "second");
    assert_eq!(records["draft-c"]["text"], "unsent");
    assert_eq!(fs::metadata(&lock_path).unwrap().ino(), lock_inode);
    assert_eq!(
        fs::metadata(fixture.directory()).unwrap().mode() & 0o777,
        0o700
    );
    for path in [&fixture.data(), &lock_path] {
        assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o600);
    }
    // Deletion is a record patch too; it must not remove unrelated queued entries.
    assert!(compare_exchange_in(&fixture.0, 4, patch("draft-c", None)).unwrap());
    let (revision, records) = read_in(&fixture.0).unwrap();
    assert_eq!(revision, 5);
    assert_eq!(records.len(), 2);
    assert!(!records.contains_key("draft-c"));
    assert_eq!(records["message-a"]["cancel"], true);
}

#[test]
fn io_failure_at_every_commit_boundary_is_never_acknowledged() {
    for point in [
        Fault::Write,
        Fault::FileSync,
        Fault::Rename,
        Fault::DirectorySync,
    ] {
        let fixture = Fixture::new();
        assert!(compare_exchange_in(&fixture.0, 0, patch("record", Some(json!("old")))).unwrap());
        FAILURE.with(|failure| failure.set(Some(point)));
        let result = compare_exchange_in(&fixture.0, 1, patch("record", Some(json!("new"))));
        FAILURE.with(|failure| failure.set(None));
        let error = result.unwrap_err();
        assert!(error.starts_with("native durable store:"), "{error}");
        let (revision, records) = read_in(&fixture.0).unwrap();
        if point == Fault::DirectorySync {
            // Rename already happened: visible does not mean durably acknowledged.
            assert_eq!(revision, 2);
            assert_eq!(records["record"], "new");
        } else {
            assert_eq!(revision, 1);
            assert_eq!(records["record"], "old");
        }
        assert_eq!(fs::read_dir(fixture.directory()).unwrap().count(), 2);
    }
}

#[test]
fn unsafe_real_filesystem_state_refuses_reads_and_commits() {
    let fixture = Fixture::new();
    assert!(compare_exchange_in(&fixture.0, 0, patch("record", Some(json!("kept")))).unwrap());
    let original = fs::read(fixture.data()).unwrap();
    fs::set_permissions(fixture.data(), fs::Permissions::from_mode(0o640)).unwrap();
    assert!(read_in(&fixture.0).is_err());
    assert!(compare_exchange_in(&fixture.0, 1, patch("record", None)).is_err());
    assert_eq!(fs::read(fixture.data()).unwrap(), original);
    fs::set_permissions(fixture.data(), fs::Permissions::from_mode(0o600)).unwrap();

    let lock_path = fixture.directory().join("durable.lock");
    fs::remove_file(&lock_path).unwrap();
    symlink("durable.json", &lock_path).unwrap();
    assert!(read_in(&fixture.0).is_err());
    assert!(compare_exchange_in(&fixture.0, 1, patch("record", None)).is_err());
    assert_eq!(fs::read(fixture.data()).unwrap(), original);
    fs::remove_file(&lock_path).unwrap();

    let saved = fixture.directory().join("saved.json");
    fs::rename(fixture.data(), &saved).unwrap();
    symlink("saved.json", fixture.data()).unwrap();
    assert!(read_in(&fixture.0).is_err());
    assert!(compare_exchange_in(&fixture.0, 1, patch("record", None)).is_err());
    assert_eq!(fs::read(&saved).unwrap(), original);
    fs::remove_file(fixture.data()).unwrap();
    fs::create_dir(fixture.data()).unwrap();
    assert!(compare_exchange_in(&fixture.0, 1, patch("record", None)).is_err());
    fs::remove_dir(fixture.data()).unwrap();
    fs::rename(saved, fixture.data()).unwrap();

    fs::set_permissions(fixture.directory(), fs::Permissions::from_mode(0o750)).unwrap();
    assert!(read_in(&fixture.0).is_err());
    assert!(compare_exchange_in(&fixture.0, 1, patch("record", None)).is_err());
    fs::set_permissions(fixture.directory(), fs::Permissions::from_mode(0o700)).unwrap();
    let (revision, records) = read_in(&fixture.0).unwrap();
    assert_eq!(revision, 1);
    assert_eq!(records["record"], "kept");
}

#[test]
fn aggregate_bound_refuses_without_acknowledging_or_replacing_records() {
    let fixture = Fixture::new();
    assert!(compare_exchange_in(&fixture.0, 0, patch("record", Some(json!("kept")))).unwrap());
    let original = fs::read(fixture.data()).unwrap();
    let error = compare_exchange_in(
        &fixture.0,
        1,
        patch("too-large", Some(Value::String("x".repeat(MAX_BYTES)))),
    )
    .unwrap_err();
    assert!(
        error.contains("native durable store: cannot encode durable.json"),
        "{error}"
    );
    assert!(error.contains("8 MiB"), "{error}");
    assert_eq!(fs::read(fixture.data()).unwrap(), original);
    assert_eq!(read_in(&fixture.0).unwrap().0, 1);

    let file = fs::OpenOptions::new()
        .write(true)
        .open(fixture.data())
        .unwrap();
    file.set_len(MAX_BYTES as u64 + 1).unwrap();
    let error = read_in(&fixture.0).unwrap_err();
    assert!(
        error.contains("native durable store: cannot read durable.json"),
        "{error}"
    );
    assert!(error.contains("8 MiB"), "{error}");
    assert!(compare_exchange_in(&fixture.0, 1, patch("record", None)).is_err());
}
