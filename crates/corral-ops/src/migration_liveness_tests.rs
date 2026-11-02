use super::*;
use serde_json::json;

#[test]
fn missing_panes_are_proof_only_on_the_owning_server_and_no_live_process() {
    let socket = Path::new("/fixture.sock");
    for doc in [
        json!({"herdr":{"pane_id":"w1:p1","socket":socket}}),
        json!({"gate":{"pane":"w1:p1","socket":socket}}),
        json!({"reviews":[{"herdr":{"pane_id":"w1:p1","socket":socket}}]}),
        json!({"review_wait":"reviewer", "runs":[{"review_runs":[{"herdr":{"pane_id":"w1:p1","socket":socket}}]}]}),
    ] {
        let mut handles = Handles::default();
        handles.collect(&doc, None);
        assert!(handles.absent(socket, None, &BTreeSet::new()).unwrap());
        assert!(
            !handles
                .absent(socket, None, &BTreeSet::from(["w1:p1".into()]))
                .unwrap()
        );
        assert!(
            handles
                .absent(Path::new("/foreign.sock"), None, &BTreeSet::new())
                .is_err()
        );
    }
    // Old aggregate gates never recorded a pane: a store binding is still required.
    let mut handles = Handles::default();
    assert!(handles.absent(socket, None, &BTreeSet::new()).is_err());
    assert!(
        handles
            .absent(socket, Some(socket), &BTreeSet::new())
            .unwrap()
    );
    let me = ProcessIdentity::current().unwrap();
    handles.collect(
        &json!({"bound":{"pid":me.pid,"start_time":me.start_time}}),
        Some(socket),
    );
    assert!(
        !handles
            .absent(socket, Some(socket), &BTreeSet::new())
            .unwrap()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn detached_task_processes_and_pane_less_gates_are_not_lost() {
    let fixture = tempfile::tempdir().unwrap();
    let proc = fixture.path();
    std::fs::create_dir(proc.join("self")).unwrap();
    std::fs::write(proc.join("stat"), "btime 0\n").unwrap();
    std::fs::create_dir(proc.join("123")).unwrap();
    let path = proc.join("123");
    let task: Task = serde_json::from_value(json!({"id":"T-297", "title":"Fixture", "priority":"P2", "status":"dropped", "created":"2026-10-01T00:00:00Z", "updated":"2026-10-01T00:00:00Z"})).unwrap();
    assert!(!task_command(b"grep\0T-297\0", &task.id));
    assert!(!task_command(b"jq\0select(.id == \"T-297\")\0", &task.id));
    assert!(!task_command(b"corral\0show\0T-297\0", &task.id));
    assert!(!task_command(
        b"bash\0-c\0corral show T-297 | jq .status\0",
        &task.id
    ));
    assert!(task_command(
        b"bash\0/w/.corral/state/plan-gates/T-297-A1.sh\0",
        &task.id
    ));
    for argv in [
        b"corral\0plan\0verify\0T-297\0".as_slice(),
        b"corral\0gate\0T-297\0",
    ] {
        std::fs::write(path.join("cmdline"), argv).unwrap();
        assert!(!processes_absent(proc, &task, &LaunchOwners::default()).unwrap());
    }
    std::fs::write(path.join("cmdline"), b"cargo\0test\0").unwrap();
    std::fs::write(path.join("environ"), b"").unwrap();
    let mut fields = vec!["0"; 20];
    fields[0] = "S";
    // Future start forces the cwd inspection; no wall-clock sleeps or real burner process.
    fields[19] = "9999999999999";
    std::fs::write(
        path.join("stat"),
        format!("123 (cargo) {}", fields.join(" ")),
    )
    .unwrap();
    std::os::unix::fs::symlink("/repo/.worktrees/T-297-aggregate", path.join("cwd")).unwrap();
    assert!(!processes_absent(proc, &task, &LaunchOwners::default()).unwrap());
    std::fs::remove_file(path.join("cwd")).unwrap();
    std::os::unix::fs::symlink("/repo/.worktrees/T-2970", path.join("cwd")).unwrap();
    assert!(processes_absent(proc, &task, &LaunchOwners::default()).unwrap());
    std::fs::write(path.join("stat"), "malformed").unwrap();
    assert!(processes_absent(proc, &task, &LaunchOwners::default()).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn owned_detached_process_outside_task_cwd_must_block() {
    let fixture = tempfile::tempdir().unwrap();
    let proc = fixture.path();
    std::fs::create_dir(proc.join("self")).unwrap();
    std::fs::write(proc.join("stat"), "btime 0\n").unwrap();
    std::fs::create_dir(proc.join("123")).unwrap();
    let path = proc.join("123");
    let task: Task = serde_json::from_value(json!({"id":"T-297", "title":"Fixture", "priority":"P2", "status":"dropped", "created":"2026-10-01T00:00:00Z", "updated":"2026-10-01T00:00:00Z"})).unwrap();
    std::fs::write(path.join("cmdline"), b"sleep\0").unwrap();
    let mut fields = vec!["0"; 20];
    fields[0] = "S";
    fields[19] = "9999999999999";
    std::fs::write(
        path.join("stat"),
        format!("123 (sleep) {}", fields.join(" ")),
    )
    .unwrap();
    std::os::unix::fs::symlink("/tmp", path.join("cwd")).unwrap();
    let directory = proc.join("owners");
    std::fs::create_dir(&directory).unwrap();
    let marker = "a".repeat(64);
    // All callers share the same probe. The filename contract includes coding/gate,
    // review-child, planner, plan-review and aggregate attempts, not just R<n>.
    for attempt in ["R1", "R1-R1-review1", "D1", "P1-P1-review1", "A1"] {
        std::fs::write(directory.join(format!("T-297-{attempt}.owner")), &marker).unwrap();
        let owners = LaunchOwners::read(&directory).unwrap();
        for environment in [
            format!("CORRAL_PROCESS_OWNER={marker}\0"),
            format!("CORRAL_SANDBOX_TASK=T-297\0CORRAL_PROCESS_OWNER={marker}\0"),
        ] {
            std::fs::write(path.join("environ"), environment).unwrap();
            assert!(
                !processes_absent(proc, &task, &owners).unwrap(),
                "live {attempt} descendant was incorrectly reported absent"
            );
        }
    }
    // A task hint alone also identifies a live detached descendant without a marker file.
    std::fs::write(path.join("environ"), b"CORRAL_SANDBOX_TASK=T-297\0").unwrap();
    assert!(!processes_absent(proc, &task, &LaunchOwners::default()).unwrap());
    let owners = LaunchOwners::read(&directory).unwrap();
    // Do not skip inherited ownership after a birth-time heuristic (exec retains PID age).
    fields[19] = "0";
    std::fs::write(
        path.join("stat"),
        format!("123 (sleep) {}", fields.join(" ")),
    )
    .unwrap();
    std::fs::write(
        path.join("environ"),
        format!("CORRAL_PROCESS_OWNER={marker}\0"),
    )
    .unwrap();
    assert!(!processes_absent(proc, &task, &owners).unwrap());
    // Retained markers without residents and unrelated PM query monitors stay non-blocking.
    std::fs::write(path.join("cmdline"), b"grep\0T-297\0").unwrap();
    std::fs::write(path.join("environ"), b"").unwrap();
    assert!(processes_absent(proc, &task, &owners).unwrap());
    std::fs::write(directory.join("T-2970-R1.owner"), "b".repeat(64)).unwrap();
    let owners = LaunchOwners::read(&directory).unwrap();
    std::fs::write(
        path.join("environ"),
        format!(
            "CORRAL_SANDBOX_TASK=T-2970\0CORRAL_PROCESS_OWNER={}\0",
            "b".repeat(64)
        ),
    )
    .unwrap();
    assert!(processes_absent(proc, &task, &owners).unwrap());
    std::fs::write(directory.join("PM-wR0_p1.owner"), "d".repeat(64)).unwrap();
    let owners = LaunchOwners::read(&directory).unwrap();
    std::fs::write(
        path.join("environ"),
        format!("CORRAL_PROCESS_OWNER={}\0", "d".repeat(64)),
    )
    .unwrap();
    assert!(processes_absent(proc, &task, &owners).unwrap());
    // Missing ownership correlation or unreadable environment is not proof of absence.
    std::fs::write(
        path.join("environ"),
        format!("CORRAL_PROCESS_OWNER={}\0", "c".repeat(64)),
    )
    .unwrap();
    assert!(processes_absent(proc, &task, &owners).is_err());
    std::fs::remove_file(path.join("environ")).unwrap();
    assert!(processes_absent(proc, &task, &owners).is_err());
}

#[test]
fn invalid_launch_ownership_inventory_cannot_confirm_absence() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    assert!(
        LaunchOwners::read(&root.join("absent"))
            .unwrap()
            .0
            .is_empty()
    );
    for (name, marker) in [
        ("T-297-R1.owner", "bad".into()),
        ("T-297-R1.owner", "z".repeat(64)),
        ("invalid.owner", "a".repeat(64)),
    ] {
        let path = root.join(name);
        std::fs::write(&path, marker).unwrap();
        assert!(LaunchOwners::read(root).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[cfg(unix)]
    {
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(outside.path(), "a".repeat(64)).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("T-297-R1.owner")).unwrap();
        assert!(LaunchOwners::read(root).is_err());
    }
}
