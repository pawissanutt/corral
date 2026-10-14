use std::path::Path;
use std::time::{Duration, Instant};

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

    h.pane_run(&pane, "echo hello-corral").expect("run");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = false;
    while Instant::now() < deadline {
        if h.pane_read(&pane, 20)
            .expect("read")
            .contains("hello-corral")
        {
            seen = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

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
    prompt.expect("prompt (blocked agent falls back to send text)");
    assert!(got.expect("get").is_some());
    focus.expect("focus");
    assert!(missing.expect("missing get").is_none());
    close.expect("close");
}
