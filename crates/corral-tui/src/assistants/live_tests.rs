//! Run only against scripts/test-herdr.sh's fresh plugin-free server.
use super::*;
use crate::board::tests::seeded;
use std::{net::UdpSocket, os::unix::fs::PermissionsExt, time::Duration};

struct Cleanup(corral_herdr::Herdr, String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.0.workspace_close(&self.1);
    }
}

#[test]
fn isolated_new_pane_runs_task_focused_pi_without_touching_existing_panes() {
    let herdr = corral_herdr::Herdr::with_bin("herdr".into(), Some("corral-test".into()));
    if !herdr.available() {
        println!("SKIP: isolated corral-test server unavailable");
        return;
    }
    let mut fx = seeded();
    fx.ctx.herdr = herdr.clone();
    // Check our own existing pane, never a concurrent fixture's transient inventory.
    let existing = herdr
        .workspace_create(fx.ctx.store.root(), "existing assistant", false)
        .unwrap();
    let _existing_cleanup = Cleanup(herdr.clone(), existing.workspace_id.clone());
    let received = UdpSocket::bind("127.0.0.1:0").unwrap();
    received
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    // Stand in only for the Pi process, not the pane launcher. The exact argv arrives
    // through an event barrier; no sleeps or polling and no model/credentials needed.
    let probe = fx.ctx.store.root().join("corral probe's");
    std::fs::write(&probe, format!(
        "#!/usr/bin/env python3\nimport socket,sys,os\nsocket.socket(socket.AF_INET,socket.SOCK_DGRAM).sendto(('|'.join(sys.argv[1:])+'|'+os.getcwd()).encode(),('127.0.0.1',{}))\n",
        received.local_addr().unwrap().port()
    )).unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let pane = open_task_with(&fx.ctx, "T-003", &probe).unwrap();
    let info = herdr.pane_get(&pane).unwrap().unwrap();
    let _cleanup = Cleanup(herdr.clone(), info.workspace_id.clone());
    let mut buffer = [0; 4096];
    let n = received.recv(&mut buffer).unwrap();
    let invocation = String::from_utf8_lossy(&buffer[..n]);
    assert_eq!(
        invocation,
        format!("pi|--task|T-003|{}", fx.ctx.store.root().display())
    );
    assert_ne!(existing.pane_id, pane);
    assert!(herdr.pane_get(&existing.pane_id).unwrap().is_some());
    println!(
        "LIVE: new pane {pane} in isolated server invoked {invocation}; existing pane preserved; no pane input handoff"
    );
}
