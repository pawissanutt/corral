use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixDatagram;
use std::time::Duration;

use corral_herdr::{AgentStatus, ENTER_ATTEMPTS, Herdr};

#[test]
fn live_roundtrip() {
    let root = std::env::temp_dir().join(format!("corral-live-{}", std::process::id()));
    std::fs::create_dir(&root).expect("create isolated root");
    let bin = root.join("herdr");
    std::fs::write(
        &bin,
        r#"#!/usr/bin/env bash
export CORRAL_ROOT="$(cd "$(dirname "$0")" && pwd)"
unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID
exec herdr "$@"
"#,
    )
    .expect("write isolated herdr wrapper");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let h = Herdr::with_bin(bin, Some("corral-test".into()));
    if !h.available() {
        std::fs::remove_dir_all(&root).expect("remove isolated root");
        eprintln!("SKIP: herdr --session corral-test unavailable");
        return;
    }
    println!("live test running against corral-test");
    let created = h
        .workspace_create(&root, "corral-live-test", false)
        .expect("create");
    let pane = created.pane_id.clone();

    // Subscribe before launch instead of racing pane output with fixed sleeps.
    let tmp = tempfile::tempdir().expect("readiness directory");
    let path = tmp.path().join("ready.sock");
    let signal = UnixDatagram::bind(&path).expect("bind readiness socket");
    // The datagram signals readiness; allow host contention and only guard against a hang.
    signal
        .set_read_timeout(Some(Duration::from_secs(120)))
        .expect("read timeout");
    let command = format!(
        "python3 -u -c 'import socket,sys; print(\"hello-corral\", flush=True); \
         s=socket.socket(socket.AF_UNIX,socket.SOCK_DGRAM); s.sendto(b\"READY\", \"{}\"); \
         \nfor line in sys.stdin:\n    print(\"received-corral\",flush=True)'",
        path.display()
    );
    h.pane_run(&pane, &command).expect("run");
    let mut ready = [0; 16];
    let seen = signal
        .recv(&mut ready)
        .is_ok_and(|n| &ready[..n] == b"READY");
    std::fs::remove_file(&path).expect("remove readiness socket");

    let rep = h.pane_report_agent(&pane, "fake", AgentStatus::Blocked, Some("needs input"));
    let agents = h.agent_list();
    let meta = h.pane_report_metadata(&pane, "T-042 test", &[("task", "T-042")]);
    let prompt = h.agent_prompt(&pane, "hi");
    let got = h.pane_get(&pane);
    let focus = h.pane_focus(&pane);
    let missing = h.pane_get("nope:99");

    // cleanup first (closing the last pane drops the workspace)
    let close = h.pane_close(&pane);

    assert!(seen, "hello-corral readiness hang guard expired after 120s");
    rep.expect("report-agent");
    let agents = agents.expect("agent list");
    let a = agents
        .iter()
        .find(|p| p.pane_id == pane)
        .expect("pane in agent list");
    assert_eq!(a.status, AgentStatus::Blocked);
    meta.expect("metadata");
    let delivery = prompt.expect("prompt (blocked agent falls back to send text)");
    assert!(delivery.submitted, "{delivery:?}");
    assert!(
        (1..=ENTER_ATTEMPTS).contains(&delivery.attempts),
        "{delivery:?}"
    );
    assert!(got.expect("get").is_some());
    focus.expect("focus");
    assert!(missing.expect("missing get").is_none());
    close.expect("close");
    std::fs::remove_dir_all(&root).expect("remove isolated root");
}
