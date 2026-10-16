#[cfg(target_os = "linux")]
fn concurrent_filing(idea: bool) {
    // Given a real creator stopped immediately after its first publication,
    // when sync runs concurrently, then it must see the complete filing.
    let mut command = std::process::Command::new("python3");
    command
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/queue_support/filing_race.py"),
        )
        .arg(env!("CARGO_BIN_EXE_corral"));
    if idea {
        command.arg("--idea");
    }
    let output = command.stdin(std::process::Stdio::null()).output().unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn new_backlog_dependencies_are_atomic_against_sync() {
    concurrent_filing(false);
}

#[cfg(target_os = "linux")]
#[test]
fn new_idea_policy_fields_are_atomic_against_sync() {
    concurrent_filing(true);
}
