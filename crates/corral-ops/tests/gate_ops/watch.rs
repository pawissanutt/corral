use super::*;
use corral_ops::watch::{ProgressState, load, read_gate_log};
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn completed_attempts_retain_rounds_ranges_and_archived_logs() {
    // Given a completed attempt followed by a replacement in another round.
    let (_tmp, ctx, id, _worktree) = running_task(&["printf first", "printf second"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    corral_ops::gate_collect(&ctx, &id).unwrap().unwrap();
    let first = load(&ctx, &id).unwrap().gates.remove(0);
    let original = read_gate_log(&ctx, &first, 0, 4096).unwrap();
    ctx.store
        .update(&id, |task| {
            task.rounds = 1;
            Ok(())
        })
        .unwrap();
    corral_ops::gate_start(&ctx, &id).unwrap();
    corral_ops::gate_collect(&ctx, &id).unwrap().unwrap();
    let second = load(&ctx, &id)
        .unwrap()
        .gates
        .into_iter()
        .find(|gate| gate.round == 1)
        .unwrap();
    // Wall time can move backwards between rounds; causal history must not.
    for (attempt, started) in [
        (&first, "2026-10-17T01:00:00Z"),
        (&second, "2026-10-17T00:59:59Z"),
    ] {
        let path = attempt.log.with_extension("runner.json");
        let mut runner: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        runner["started"] = serde_json::json!(started);
        std::fs::write(path, serde_json::to_vec(&runner).unwrap()).unwrap();
    }
    // When the original live file disappears, the retained artifact is used.
    std::fs::remove_file(&first.log).unwrap();
    let history = load(&ctx, &id).unwrap();
    // Then both attempts retain identity, times, command ranges, and readable output.
    assert_eq!(history.gates.len(), 2);
    assert_eq!(
        history.gates.iter().map(|g| g.round).collect::<Vec<_>>(),
        [0, 1]
    );
    assert_ne!(history.gates[0].attempt, history.gates[1].attempt);
    assert!(
        history
            .gates
            .iter()
            .all(|g| g.state == ProgressState::Passed
                && g.ended.is_some()
                && g.live_pane.is_none())
    );
    let old = &history.gates[0];
    assert_eq!(old.commands[0].log_end, old.commands[1].log_start);
    assert!(old.commands.iter().all(|c| c.state == ProgressState::Passed
        && c.started.is_some()
        && c.ended.is_some()
        && c.duration_ms.is_some()
        && c.exit_code == Some(0)));
    assert_eq!(read_gate_log(&ctx, old, 0, 4096).unwrap(), original);
    assert_eq!(read_gate_log(&ctx, old, 2, 3).unwrap(), original[2..5]);
    assert!(read_gate_log(&ctx, old, u64::MAX, 3).unwrap().is_empty());
}

#[test]
fn failed_command_keeps_later_commands_pending_without_authorizing_pass() {
    // Given one failed command and another command that must never execute.
    let (_tmp, ctx, id, _worktree) = running_task(&["printf failure; exit 7", "printf forbidden"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    // When monitor data is loaded before authoritative collection.
    let attempt = load(&ctx, &id).unwrap().gates.remove(0);
    // Then command failure is visible without changing the verification receipt.
    assert_eq!(attempt.state, ProgressState::Failed);
    assert_eq!(attempt.record.result, GateResult::Running);
    assert_eq!(attempt.commands[0].exit_code, Some(7));
    assert_eq!(attempt.commands[0].state, ProgressState::Failed);
    assert_eq!(attempt.commands[1].state, ProgressState::Pending);
    assert!(attempt.commands[1].started.is_none());
    assert_eq!(
        ctx.store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .result,
        GateResult::Running
    );
    assert_eq!(
        corral_ops::gate_collect(&ctx, &id).unwrap().unwrap().result,
        GateResult::Fail
    );
}

#[test]
fn legacy_durations_and_reversed_timestamps_do_not_break_gate_collection() {
    // Given real runner receipts with old or clock-skewed monitor sidecars.
    for duration in [Some(-1534), None, Some(250)] {
        let (_tmp, ctx, id, _worktree) = running_task(&["true"]);
        corral_ops::gate_start(&ctx, &id).unwrap();
        let attempt = load(&ctx, &id).unwrap().gates.remove(0);
        let metadata = ctx
            .store
            .state_dir()
            .join("watch")
            .join(&id)
            .join(format!("{}-{}.json", attempt.run, attempt.attempt));
        for path in [metadata, attempt.log.with_extension("command-0.json")] {
            let mut json: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            json["started"] = serde_json::json!("2026-10-16T00:00:01Z");
            json["ended"] = serde_json::json!("2026-10-15T23:59:59Z");
            if let Some(duration) = duration {
                json["duration_ms"] = serde_json::json!(duration);
            } else {
                json.as_object_mut().unwrap().remove("duration_ms");
            }
            std::fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
        }
        // When authoritative collection reconciles those display-only sidecars.
        let result = corral_ops::gate_collect(&ctx, &id).unwrap().unwrap();
        let observed = load(&ctx, &id).unwrap().gates.remove(0);
        // Then the gate still passes and completed monotonic durations are not overwritten.
        assert_eq!(result.result, GateResult::Pass);
        assert_eq!(
            observed.commands[0].duration_ms,
            Some(u64::try_from(duration.unwrap_or(0)).unwrap_or(0))
        );
        assert!(observed.duration_ms.is_some());
    }
}

/// The command connects only after the runner persisted Running, and blocks for an
/// explicit response. No sleep, polling, or assumption about scheduler speed.
fn blocked_gate(kill_runner: bool) {
    let (tmp, mut ctx, id, _worktree) = running_task(&["true"]);
    let socket = tmp.path().join("signal.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let (connected, accepted) = mpsc::channel();
    let subscriber = std::thread::spawn(move || connected.send(listener.accept()).unwrap());
    let command = format!(
        r#"python3 -c 'import socket; s=socket.socket(socket.AF_UNIX); s.settimeout(15); s.connect("{}"); s.sendall(b"READY"); s.recv(1)'"#,
        socket.display()
    );
    ctx.store.config.review.gate = Some(vec![command.clone(), "true".into()]);
    ctx.store.save_config().unwrap();
    let launcher_ctx = ctx.clone();
    let launcher_id = id.clone();
    let launcher = std::thread::spawn(move || corral_ops::gate_start(&launcher_ctx, &launcher_id));
    let (mut signal, _) = accepted
        .recv_timeout(Duration::from_secs(15))
        .unwrap()
        .unwrap();
    signal
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let mut ready = [0; 5];
    signal.read_exact(&mut ready).unwrap();
    assert_eq!(&ready, b"READY");
    subscriber.join().unwrap();
    let active = load(&ctx, &id).unwrap().gates.remove(0);
    assert_eq!(active.state, ProgressState::Running);
    assert_eq!(active.commands[0].state, ProgressState::Running);
    assert_eq!(active.commands[1].state, ProgressState::Pending);
    assert!(active.commands[0].started.is_some());
    assert!(active.commands[0].ended.is_none());
    assert_eq!(active.commands[0].command, command);
    let live = read_gate_log(&ctx, &active, 0, 8192).unwrap();
    assert_eq!(
        u64::try_from(live.len()).unwrap(),
        active.commands[0].log_end
    );
    assert!(
        String::from_utf8(live)
            .unwrap()
            .contains(&format!("$ {command}"))
    );
    if kill_runner {
        let runtime: serde_json::Value = serde_json::from_slice(
            &std::fs::read(active.log.with_extension("runner.json")).unwrap(),
        )
        .unwrap();
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(
                i32::try_from(runtime["pid"].as_u64().unwrap()).unwrap(),
            )
            .unwrap(),
            rustix::process::Signal::KILL,
        )
        .unwrap();
    }
    signal.write_all(b"X").unwrap();
    launcher.join().unwrap().unwrap();
    let observed = load(&ctx, &id).unwrap().gates.remove(0);
    if kill_runner {
        assert_eq!(observed.state, ProgressState::Interrupted);
        assert_eq!(observed.commands[0].state, ProgressState::Interrupted);
        assert!(observed.live_pane.is_none());
        assert!(corral_ops::gate_collect(&ctx, &id).unwrap().is_none());
        let again = load(&ctx, &id).unwrap().gates.remove(0);
        assert_eq!(observed.ended, again.ended);
    } else {
        assert_eq!(observed.state, ProgressState::Passed);
        assert!(
            observed
                .commands
                .iter()
                .all(|c| c.state == ProgressState::Passed)
        );
        assert_eq!(observed.record.result, GateResult::Running);
    }
}

#[test]
fn running_commands_are_observable_before_process_completion() {
    // Given a blocked command, when the exact READY signal arrives, then progress
    // and live log bytes are observable before allowing it to finish.
    blocked_gate(false);
}

#[test]
fn lost_runner_is_interrupted_even_when_its_pane_shell_survives() {
    // Given a blocked command, when its runner is killed and reaped, then the
    // monitor reports interruption, never Running forever or a verified pass.
    blocked_gate(true);
}
