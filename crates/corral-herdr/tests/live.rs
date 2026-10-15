use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::time::Duration;

use corral_herdr::{AgentStatus, Herdr};

#[test]
fn live_roundtrip() {
    let h = Herdr::new(Some("corral-test".into()));
    if !h.available() {
        eprintln!("SKIP: herdr --session corral-test unavailable");
        return;
    }
    println!("live test running against corral-test");
    let created = h
        .workspace_create(Path::new("/tmp"), "corral-live-test", false)
        .expect("create");
    let pane = created.pane_id.clone();

    // Subscribe before launch instead of racing pane output with fixed sleeps.
    let path = std::env::temp_dir().join(format!("corral-live-{}.sock", std::process::id()));
    let signal = UnixDatagram::bind(&path).expect("bind readiness socket");
    signal
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let command = format!(
        "python3 -c 'import socket; print(\"hello-corral\", flush=True); \
         s=socket.socket(socket.AF_UNIX,socket.SOCK_DGRAM); s.sendto(b\"READY\", \"{}\")'",
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

    assert!(seen, "hello-corral not seen within 5s");
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
    assert!(got.expect("get").is_some());
    focus.expect("focus");
    assert!(missing.expect("missing get").is_none());
    close.expect("close");
}
