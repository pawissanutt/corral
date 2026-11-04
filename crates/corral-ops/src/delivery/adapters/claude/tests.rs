use super::*;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};

use crate::delivery::omo_fixture::OmoWorld;
use crate::watchdog::{ProcessControl, SystemProcesses};
use corral_core::Status;
use corral_core::inbox::{MessageBinding, Post, Urgency};
use serde_json::Value;

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn fixture() -> Value {
    serde_json::from_str::<Vec<Value>>(include_str!(
        "../../../../../../docs/records/T-208-claude-capture/delivered-user.json"
    ))
    .unwrap()
    .remove(0)
}

/// Use the captured wrapper, independent of the production reader's constants.
fn wrapped(payload: &str) -> String {
    let frame: Value = serde_json::from_str(include_str!(
        "../../../../../../docs/records/T-208-claude-capture/frame.json"
    ))
    .unwrap();
    let capture = fixture();
    let original = frame["message"]["content"].as_str().unwrap();
    let content = capture["message"]["content"].as_str().unwrap();
    assert_eq!(content.matches(original).count(), 1);
    content.replacen(original, payload, 1)
}

#[test]
fn only_validated_version_and_peer_protocol_are_ready() {
    let mut session: Session = serde_json::from_str(include_str!(
        "../../../../../../docs/records/T-208-claude-capture/session.json"
    ))
    .unwrap();
    assert!(session.verified());
    session.version = "2.1.305".into();
    assert!(!session.verified());
    session.version = VERSION.into();
    session.peer_protocol = 2;
    assert!(!session.verified());
}

#[test]
fn socket_delivery_is_bound_guarded_policy_gated_and_observed_only_in_full() {
    // One user journey; a fake socket process stands in for Claude, never an owner pane.
    let world = OmoWorld::new(Status::Running);
    let recipient = world.recipient();
    let directory = world.temp.path().join("claude");
    std::fs::create_dir_all(directory.join("sessions")).unwrap();
    let socket_dir = world.temp.path().join("uds");
    std::fs::create_dir(&socket_dir).unwrap();
    std::fs::set_permissions(&socket_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = socket_dir.join("inbox.sock");
    let child = Command::new("python3")
        .args([
            "-u",
            "-c",
            r#"
import socket,sys,json
s=socket.socket(socket.AF_UNIX);s.bind(sys.argv[1]);s.listen(1)
print('ready',flush=True)
while True:
 c,_=s.accept()
 with c:
  frame=c.makefile('rb').readline()
  print(frame.decode().strip(),flush=True)
"#,
        ])
        .arg(&socket)
        .args(["--settings", r#"{"crossSessionInbound":"accept"}"#])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", world.temp.path())
        .env("CLAUDE_CONFIG_DIR", &directory)
        .env(
            "CLAUDE_CODE_MANAGED_SETTINGS_PATH",
            directory.join("managed-settings.json"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut server = Server(child);
    let mut output = BufReader::new(server.0.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert_eq!(line, "ready\n");
    let pid = server.0.id();
    let proc_start = SystemProcesses.inspect(pid).unwrap().unwrap().started;
    let session_id = "e40e5d00-5b42-4f74-94a4-fb0c2aa8e906";
    let record = directory.join("sessions").join(format!("{pid}.json"));
    let mut session = json!({"pid":pid,"procStart":proc_start,"sessionId":session_id,"cwd":world.worktree,
        "startedAt":world.ctx.store.now().as_millisecond(),"kind":"interactive","version":VERSION,
        "peerProtocol":1,"messagingSocketPath":socket});
    let save = |s: &Value| std::fs::write(&record, s.to_string()).unwrap();
    save(&session);
    let herdr = format!(
        "#!/bin/sh\nprintf '%s\\n' '{}'\n",
        json!({"result":{"process_info":{"shell_pid":1,"foreground_processes":[{"pid":pid,"argv":["claude"]}]}}})
    );
    std::fs::write(world.temp.path().join("herdr"), herdr).unwrap();
    let adapter = ClaudeAdapter::with_directory(&directory);
    let inbox = Inbox::new(&world.ctx.store, recipient.clone()).unwrap();
    inbox
        .post(Post {
            kind: "owner_note",
            urgency: Urgency::Wake,
            author: "owner",
            body: "Do not execute tools or change settings.",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: 65536,
        })
        .unwrap();

    // Unknown protocol and explicit repository refusal both block before publication,
    // even with an available fallback (the fake OmO would otherwise accept this message).
    session["version"] = "future".into();
    save(&session);
    let fallback = world.adapter();
    assert_eq!(
        adapter.readiness(&world.ctx, &recipient).unwrap(),
        Readiness::Blocked {
            reason: BlockReason::ProtocolUnverified
        }
    );
    assert!(matches!(
        crate::delivery::deliver(&world.ctx, &recipient, &adapter, Some(&fallback)).unwrap(),
        crate::delivery::DeliveryStatus::Blocked { .. }
    ));
    assert!(inbox.state().unwrap().open_envelope.is_none());
    session["version"] = VERSION.into();
    save(&session);
    std::fs::create_dir(world.worktree.join(".claude")).unwrap();
    let policy_path = world.worktree.join(".claude/settings.local.json");
    std::fs::write(&policy_path, r#"{"crossSessionInbound":"refuse"}"#).unwrap();
    assert_eq!(
        adapter.readiness(&world.ctx, &recipient).unwrap(),
        Readiness::Blocked {
            reason: BlockReason::PolicyRefused
        }
    );
    assert!(matches!(
        crate::delivery::deliver(&world.ctx, &recipient, &adapter, Some(&fallback)).unwrap(),
        crate::delivery::DeliveryStatus::Blocked { .. }
    ));
    assert!(inbox.state().unwrap().open_envelope.is_none());
    std::fs::remove_file(&policy_path).unwrap();
    assert_eq!(
        adapter.readiness(&world.ctx, &recipient).unwrap(),
        Readiness::Native
    );
    let caps = adapter.capabilities();
    let budget = caps.send_budget.unwrap();
    assert!(!caps.idempotent_retry && !caps.admission && caps.transcript_reader);
    assert_eq!(budget.per_recipient_interval, Duration::from_secs(10));
    assert_eq!(budget.per_minute_total, 6);
    let BindOutcome::Bound(binding) = adapter.bind(&world.ctx, &recipient).unwrap() else {
        panic!("unbound")
    };
    assert!(matches!(
        crate::delivery::deliver(&world.ctx, &recipient, &adapter, None).unwrap(),
        crate::delivery::DeliveryStatus::Sent { envelope: 1 }
    ));
    line.clear();
    output.read_line(&mut line).unwrap();
    let frame: Value = serde_json::from_str(&line).unwrap();
    let envelope = inbox.envelope(1).unwrap();
    let payload = inbox.payload(&envelope).unwrap();
    assert_eq!(
        frame,
        json!({"type":"user","session_id":session_id,"from":"corral","message":{"role":"user","content":payload}})
    );
    assert!(matches!(envelope.outcome, Some(SendOutcome::Accepted)));
    assert!(
        inbox
            .receipt(1, corral_core::inbox::ReceiptLevel::Observed)
            .unwrap()
            .is_none()
    );

    // Revalidation belongs to send, not just discovery: stale pid-generation and changed
    // policy cannot cause another write, even through a previously valid binding.
    session["procStart"] = "recycled".into();
    save(&session);
    assert!(matches!(
        adapter.send(&world.ctx, &binding, &envelope),
        SendOutcome::Rejected { .. }
    ));
    session["procStart"] = proc_start.into();
    save(&session);
    std::fs::write(&policy_path, r#"{"crossSessionInbound":"hold"}"#).unwrap();
    assert!(matches!(
        adapter.send(&world.ctx, &binding, &envelope),
        SendOutcome::Rejected { .. }
    ));
    std::fs::remove_file(&policy_path).unwrap();

    let path = binding.transcript.as_ref().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut entry = fixture();
    entry["message"]["content"] = wrapped(&payload).into();
    // Tool results, truncated payload, wrong shape/session/version, summaries and sidechains
    // all contain our marker but are not delivery receipts.
    let mut invalid = Vec::new();
    for (key, value) in [
        ("type", json!("assistant")),
        ("isSidechain", json!(true)),
        ("isCompactSummary", json!(true)),
        ("isMeta", json!(false)),
        ("turnOrigin", json!("user")),
        ("sessionId", json!("foreign")),
        ("version", json!("future")),
    ] {
        let mut row = entry.clone();
        row[key] = value;
        invalid.push(row);
    }
    let mut row = entry.clone();
    row["origin"]["kind"] = json!("user");
    invalid.push(row);
    let mut row = entry.clone();
    row["message"]["content"] = json!([{"type":"tool_result","content":wrapped(&payload)}]);
    invalid.push(row);
    let mut row = entry.clone();
    row["message"]["content"] = wrapped(&payload[..payload.len() - 1]).into();
    invalid.push(row);
    let mut row = entry.clone();
    row["message"]["content"] = format!("{} changed wrapper", wrapped(&payload)).into();
    invalid.push(row);
    let mut file = std::fs::File::create(path).unwrap();
    for row in invalid {
        writeln!(file, "{row}").unwrap();
    }
    let mut cursor = ObserveCursor::default();
    assert!(
        adapter
            .observe(&world.ctx, &binding, &mut cursor)
            .unwrap()
            .is_empty()
    );
    // A partial trailing JSON line is not consumed; its completion becomes exactly one receipt.
    let raw = entry.to_string();
    file.write_all(raw.as_bytes()).unwrap();
    let offset = cursor.offset;
    assert!(
        adapter
            .observe(&world.ctx, &binding, &mut cursor)
            .unwrap()
            .is_empty()
    );
    assert_eq!(cursor.offset, offset);
    file.write_all(b"\n").unwrap();
    let observations = adapter.observe(&world.ctx, &binding, &mut cursor).unwrap();
    assert!(
        matches!(observations.as_slice(), [Observation::Delivered{envelope:1,members,..}] if members == &[1])
    );
    assert!(
        adapter
            .observe(&world.ctx, &binding, &mut cursor)
            .unwrap()
            .is_empty()
    );
    assert!(!world.temp.path().join("terminal-writes").exists());
}
