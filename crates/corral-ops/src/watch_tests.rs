use super::*;

#[test]
fn native_runner_liveness_rejects_recycled_identity() {
    // Given a sidecar for this live native process.
    let mut runner = Runner {
        pid: std::process::id(),
        process: crate::runtime_probe(Some(std::process::id())).unwrap(),
        boot: "native".into(),
        state: ProgressState::Running,
        started: Timestamp::now(),
        ended: None,
    };
    assert!(runner_alive(&runner));
    // When that persisted identity no longer matches the process at the PID.
    runner.process.push_str("-stale");
    // Then display monitoring reports interruption rather than PID-only liveness.
    assert!(!runner_alive(&runner));
}

#[test]
fn legacy_command_durations_parse_without_losing_unsigned_range() {
    // Given legacy negative, missing, null, and current unsigned duration values.
    for (field, expected) in [
        (r#","duration_ms":-1534"#, Some(0)),
        ("", None),
        (r#","duration_ms":null"#, None),
        (r#","duration_ms":250"#, Some(250)),
        (r#","duration_ms":18446744073709551615"#, Some(u64::MAX)),
    ] {
        let json = format!(
            r#"{{"command":"true","state":"passed","started":null,"ended":null,
                "exit_code":0,"log_start":0,"log_end":0{field}}}"#
        );
        // When a sidecar from any supported version is read.
        let command: CommandProgress = serde_json::from_str(&json).unwrap();
        // Then durations are nonnegative or unknown, and serialize as unsigned values.
        assert_eq!(command.duration_ms, expected);
        let roundtrip: CommandProgress =
            serde_json::from_slice(&serde_json::to_vec(&command).unwrap()).unwrap();
        assert_eq!(roundtrip, command);
    }
}

#[test]
fn elapsed_saturates_when_wall_time_moves_backwards() {
    // Given a backwards midnight crossing.
    let start = "2026-10-16T00:00:01Z".parse().unwrap();
    let end = "2026-10-15T23:59:59Z".parse().unwrap();
    // When persisted gate timestamps are converted to unsigned elapsed time.
    // Then negative elapsed time saturates, while forward elapsed time is preserved.
    assert_eq!(elapsed(start, end), 0);
    assert_eq!(elapsed(end, start), 2000);
}
