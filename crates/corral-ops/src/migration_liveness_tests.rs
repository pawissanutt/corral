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
        assert!(!processes_absent(proc, &task).unwrap());
    }
    std::fs::write(path.join("cmdline"), b"cargo\0test\0").unwrap();
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
    assert!(!processes_absent(proc, &task).unwrap());
    std::fs::remove_file(path.join("cwd")).unwrap();
    std::os::unix::fs::symlink("/repo/.worktrees/T-2970", path.join("cwd")).unwrap();
    assert!(processes_absent(proc, &task).unwrap());
    std::fs::write(path.join("stat"), "malformed").unwrap();
    assert!(processes_absent(proc, &task).is_err());
}
