use super::*;

#[test]
fn launch_wrapper_mounts_devices_proc_and_read_only_source() {
    // Given paths containing shell metacharacters.
    let repo = Path::new("/repo's source");
    let store = repo.join(".corral");
    // When rendering the actual planner wrapper.
    let wrapper = command(repo, &store, "bun -e 'console.log(1)'");
    // Then OS mount options, quoting and stdin isolation survive rendering.
    assert!(wrapper.contains("--dev /dev --proc /proc"));
    assert!(wrapper.contains("--ro-bind '/repo'\\''s source' '/repo'\\''s source'"));
    assert!(wrapper.contains("--bind '/repo'\\''s source/.corral' '/repo'\\''s source/.corral'"));
    assert!(wrapper.ends_with("</dev/null"));
}

#[cfg(target_os = "linux")]
#[test]
fn bun_executes_javascript_in_exact_planner_wrapper() {
    // Given installed Linux sandbox tools; missing optional tools skip explicitly.
    for executable in ["bwrap", "bun"] {
        if Command::new(executable)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .is_err()
        {
            eprintln!("skipping sandbox integration: {executable} is not installed");
            return;
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let store = repo.join(".corral");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(repo.join("protected"), "original").unwrap();
    // When executing JS and testing both repository and output writes.
    let script = "bun -e 'console.log(1)'; code=$?; \
                  test \"$code\" -eq 0 || exit \"$code\"; \
                  if printf changed > protected; then exit 9; fi; \
                  printf artifact > .corral/result";
    let output = Command::new("/bin/sh")
        .args(["-c", &command(&repo, &store, script)])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    // Then Bun ran, only the repository's board output was writable.
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1");
    assert_eq!(
        std::fs::read_to_string(repo.join("protected")).unwrap(),
        "original"
    );
    assert_eq!(
        std::fs::read_to_string(store.join("result")).unwrap(),
        "artifact"
    );
}
