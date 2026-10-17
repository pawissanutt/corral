use super::*;
#[cfg(unix)]
use std::sync::mpsc;

#[cfg(unix)]
fn engine(dir: &std::path::Path, foreground: bool) -> Herdr {
    let bin = dir.join("herdr");
    // A static executable cannot be inherited open-for-writing by parallel forks.
    std::os::unix::fs::symlink(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/doctor/environment/herdr.sh"),
        &bin,
    )
    .unwrap();
    if foreground {
        std::fs::write(dir.join("foreground"), "").unwrap();
    }
    Herdr::with_bin(bin, Some("corral-test".into()))
}

#[test]
fn differences_preserve_unset_empty_and_non_utf8_values() {
    // Given different values for every relevant key and one unrelated variable.
    let current = Environment::from_lookup(|_| Some(b"current".to_vec()));
    let pane = Environment::from_bytes(b"PATH_EXTRA=ignored\0PATH=\0RUSTFLAGS=\xff=flag\0");
    // When comparing snapshots, including unset and empty values.
    let differences = current.differences(&pane);
    // Then every relevant mismatch is identified without decoding values.
    assert_eq!(differences, RELEVANT_ENV_KEYS);
    assert_eq!(pane.values[0], Some(Vec::new()));
    assert_eq!(pane.values[1], None);
    assert_eq!(pane.values[4], Some(b"\xff=flag".to_vec()));
}

#[test]
fn matching_environments_ignore_unrelated_variables() {
    // Given equal relevant values but different unrelated values.
    let current = Environment::from_bytes(b"PATH=/bin\0SECRET=one\0");
    let pane = Environment::from_bytes(b"PATH=/bin\0SECRET=two\0");
    // When comparing only build and session environment.
    let differences = current.differences(&pane);
    // Then unrelated values do not produce warnings.
    assert!(differences.is_empty());
}

#[test]
#[cfg(unix)]
fn foreground_mismatch_reaches_shared_sink() {
    // Given a read-only fake engine and explicitly injected environments.
    let dir = tempfile::tempdir().unwrap();
    let herdr = engine(dir.path(), true);
    let current = Environment::from_bytes(b"PATH=/current\0");
    let (tx, rx) = mpsc::channel();
    let _guard = corral_ops::capture_messages(tx);
    let reads = std::cell::Cell::new(0);
    // When doctor compares the foreground process, not the stale shell.
    report_with(&herdr, &current, |pid| {
        assert_eq!(pid, 42);
        reads.set(reads.get() + 1);
        Ok(Environment::from_bytes(b"PATH=/pane\0"))
    });
    // Then the mismatch reaches the shared sink with pane and key identifiers.
    assert_eq!(reads.get(), 1);
    let message = rx.try_recv().unwrap();
    assert_eq!(message.source, "doctor environment");
    assert_eq!(message.task, None);
    assert!(message.text.contains("w1:p1"));
    assert!(message.text.contains("PATH"));
    assert!(!message.text.contains("/current"));
    assert!(!message.text.contains("/pane"));
    assert!(rx.try_recv().is_err());
}

#[test]
#[cfg(unix)]
fn matching_idle_shell_emits_no_warning() {
    // Given an idle pane and the same injected environment on both sides.
    let dir = tempfile::tempdir().unwrap();
    let herdr = engine(dir.path(), false);
    let current = Environment::from_bytes(b"PATH=/same\0");
    let (tx, rx) = mpsc::channel();
    let _guard = corral_ops::capture_messages(tx);
    let reads = std::cell::Cell::new(0);
    // When doctor reads the shell launch snapshot.
    report_with(&herdr, &current, |pid| {
        assert_eq!(pid, 41);
        reads.set(reads.get() + 1);
        Ok(Environment::from_bytes(b"PATH=/same\0"))
    });
    // Then no diagnostic is emitted.
    assert_eq!(reads.get(), 1);
    assert!(rx.try_recv().is_err());
}

#[test]
#[cfg(unix)]
fn inaccessible_process_is_reported_without_failing_inspection() {
    // Given a pane whose process disappeared or is inaccessible.
    let dir = tempfile::tempdir().unwrap();
    let herdr = engine(dir.path(), false);
    let current = Environment::from_bytes(b"PATH=/current\0");
    let (tx, rx) = mpsc::channel();
    let _guard = corral_ops::capture_messages(tx);
    let reads = std::cell::Cell::new(0);
    // When the read-only snapshot cannot be read.
    report_with(&herdr, &current, |pid| {
        assert_eq!(pid, 41);
        reads.set(reads.get() + 1);
        bail!("fixture permission denied")
    });
    // Then the unavailable diagnostic uses the same sink.
    let message = rx.try_recv().unwrap();
    assert_eq!(reads.get(), 1, "{message:?}");
    assert_eq!(message.source, "doctor environment");
    assert!(rx.try_recv().is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn procfs_reads_child_environment_without_global_mutation() {
    use std::io::Read;
    use std::process::{Command, Stdio};

    // Given a child held alive by stdin, with an isolated exported environment.
    let mut child = Command::new("/bin/sh")
        .args(["-c", "printf ready; read line"])
        .env_clear()
        .env("PATH", "/fixture/bin")
        .env("RUSTC_WRAPPER", "")
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = [0; 5];
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut ready)
        .unwrap();
    // When reading its launch environment, without running a command in a pane.
    let snapshot = process_environment(child.id());
    drop(child.stdin.take());
    child.wait().unwrap();
    // Then unset and empty variables remain distinct in the real procfs reader.
    let expected = Environment::from_bytes(
        b"PATH=/fixture/bin\0RUSTC_WRAPPER=\0CORRAL_HERDR_SESSION=corral-test\0",
    );
    assert_eq!(snapshot.unwrap(), expected);
}
