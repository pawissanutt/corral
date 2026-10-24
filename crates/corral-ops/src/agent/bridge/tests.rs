//! The bridge v1 client against a scripted host socket, and the fresh-process probe the
//! cross-lane fixture (packages/corral-pi/test/cross-lane.test.ts) runs against the real host.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::sync::mpsc;

use corral_core::Store;
use corral_core::agent::ContainerRef;
use corral_core::inbox::Recipient;
use serde_json::{Value, json};

use super::*;
use crate::agent::launch::{CTL_CAP_FILE, SOCKET_FILE, generation_dir, runtime_dir, write_ctl_cap};

const CAP: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Host {
    _tmp: tempfile::TempDir,
    store: Store,
    recipient: Recipient,
    frames: mpsc::Receiver<Value>,
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Ok(dir) = runtime_dir(&self.store, &self.recipient) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// A scripted host on generation 1's socket: it records each frame and answers `reply(frame)`.
fn host(cap: Option<&str>, reply: fn(&Value) -> Value) -> Host {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let recipient = Recipient::Run {
        task: "T-001".into(),
        run: "R1".into(),
    };
    let dir = generation_dir(&store, &recipient, 1).unwrap();
    crate::agent::launch::ensure_private_dir(&crate::agent::launch::runtime_root(), &dir).unwrap();
    if let Some(cap) = cap {
        write_ctl_cap(&dir, cap).unwrap();
    }
    let listener = UnixListener::bind(dir.join(SOCKET_FILE)).unwrap();
    let (tx, frames) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut writer = stream.try_clone().unwrap();
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            let frame: Value = serde_json::from_str(&line).unwrap();
            let mut answer = reply(&frame);
            answer["id"] = frame["id"].clone();
            answer["generation"] = frame["generation"].clone();
            answer["version"] = json!(1);
            if tx.send(frame).is_err() || writeln!(writer, "{answer}").is_err() {
                return;
            }
        }
    });
    Host {
        _tmp: tmp,
        store,
        recipient,
        frames,
    }
}

fn scripted(frame: &Value) -> Value {
    match frame["command"].as_str() {
        Some("get_state") => json!({"ok": true, "result": {"streaming": false}}),
        Some("submit_input") => json!({"ok": true, "result": {
            "request_id": frame["id"], "idempotency_key": frame["args"]["idempotency_key"],
            "disposition": frame["args"]["text"].as_str().map_or("queued", |text| if text == "late" { "pending" } else { "queued" })}}),
        Some("events_since") => json!({"ok": true, "result": {
            "generation": 1, "events": [{"seq": 7, "generation": 1, "kind": "turn_ended"}], "last": 9,
            "next": 7, "more": true,
            "gap": {"after": 2, "resumed": 7, "missing": 4, "reconcile": true}}}),
        _ => json!({"ok": false, "error": {"code": "unknown_command", "reason": "no"}}),
    }
}

fn code(error: anyhow::Error) -> String {
    error.downcast::<BridgeError>().unwrap().code
}

#[test]
fn a_fresh_client_reads_ctl_cap_and_sends_it_with_every_request() {
    let host = host(Some(CAP), scripted);
    let mut client = BridgeClient::connect(&host.store, &host.recipient, 1).unwrap();
    assert_eq!(client.get_state().unwrap(), json!({"streaming": false}));
    let reply = client
        .submit_input(&SubmitInput {
            text: "hello".into(),
            busy_policy: BusyPolicy::FollowUp,
            idempotency_key: "msg-1".into(),
            origin: "inbox".into(),
        })
        .unwrap();
    assert_eq!(reply.disposition, Disposition::Queued);
    assert_eq!(reply.idempotency_key, "msg-1");
    let late = client
        .submit_input(&SubmitInput {
            text: "late".into(),
            busy_policy: BusyPolicy::Prompt,
            idempotency_key: "msg-2".into(),
            origin: "inbox".into(),
        })
        .unwrap();
    assert_eq!(late.disposition, Disposition::Pending);
    let frames: Vec<Value> = host.frames.try_iter().collect();
    assert_eq!(frames.len(), 3);
    for frame in &frames {
        assert_eq!(frame["ctl_cap"], json!(CAP));
        assert_eq!(frame["generation"], json!(1));
    }
    assert_eq!(frames[1]["args"]["busy_policy"], json!("follow_up"));
    assert!(!format!("{client:?}").contains(CAP));
}

#[test]
fn a_missing_or_loose_ctl_cap_refuses_before_any_request() {
    let missing = host(None, scripted);
    let error = BridgeClient::connect(&missing.store, &missing.recipient, 1).unwrap_err();
    assert_eq!(code(error), "control_credential_missing");
    let loose = host(Some(CAP), scripted);
    let cap = generation_dir(&loose.store, &loose.recipient, 1)
        .unwrap()
        .join(CTL_CAP_FILE);
    std::fs::set_permissions(&cap, std::fs::Permissions::from_mode(0o644)).unwrap();
    let error = BridgeClient::connect(&loose.store, &loose.recipient, 1).unwrap_err();
    assert_eq!(code(error), "control_credential_unsafe");
    assert!(loose.frames.try_iter().next().is_none());
}

#[test]
fn host_refusals_keep_their_code_and_a_foreign_peer_uid_is_refused() {
    let refusing = host(
        Some(CAP),
        |_| json!({"ok": false, "error": {"code": "auth_failed", "reason": "ctl_cap does not match"}}),
    );
    let mut client = BridgeClient::connect(&refusing.store, &refusing.recipient, 1).unwrap();
    assert_eq!(code(client.get_state().unwrap_err()), "auth_failed");
    assert!(peer_matches(1000, 1000).is_ok());
    assert_eq!(code(peer_matches(1001, 1000).unwrap_err()), "peer_refused");
}

#[test]
fn an_event_gap_is_recorded_as_a_reconciliation_requirement() {
    let host = host(Some(CAP), scripted);
    let mut client = BridgeClient::connect(&host.store, &host.recipient, 1).unwrap();
    let container = ContainerRef::Worker {
        task: "T-001".into(),
        run: "R1".into(),
    };
    let page = events_since_recorded(&mut client, &host.store, &container, 2).unwrap();
    assert_eq!((page.next, page.more, page.last), (7, true, 9));
    assert_eq!(
        page.gap,
        Some(EventGap {
            after: 2,
            resumed: 7,
            missing: 4
        })
    );
    let events = host.store.events().unwrap();
    let gap = events
        .iter()
        .find(|e| e.kind == "agent_events_gap")
        .unwrap();
    assert_eq!(gap.detail["container"], json!("T-001/R1"));
    assert_eq!(gap.detail["missing"], json!(4));
    assert_eq!(gap.detail["reconcile"], json!(true));
}

#[test]
fn request_ids_and_payload_digests_match_the_host_package() {
    assert_eq!(
        crate::agent::tool_request_id("session-1", "call-1"),
        "rq-eb8ccf651372cda868564a602768dbbd"
    );
    let payload = json!({"turn": 3, "kind": "turn_started", "a": [1, "x", {"c": true, "b": null}]});
    assert_eq!(
        crate::agent::payload_sha256(&payload),
        "ee3f28540ffe3d5555f1d0c02c2c45e59912380e7fd41061b776d59843c33f71"
    );
}

/// Run by the cross-lane fixture as a fresh corral process: it reconnects to a live host
/// through `ctl.cap` alone and prints one `BRIDGE_PROBE <json>` line.
#[test]
#[ignore = "driven by packages/corral-pi/test/cross-lane.test.ts"]
fn fresh_process_probe() {
    let Ok(spec) = std::env::var("CORRAL_BRIDGE_PROBE") else {
        return;
    };
    let spec: Value = serde_json::from_str(&spec).unwrap();
    let store = Store::open(std::path::Path::new(spec["root"].as_str().unwrap())).unwrap();
    let recipient = Recipient::Run {
        task: spec["task"].as_str().unwrap().into(),
        run: spec["run"].as_str().unwrap().into(),
    };
    let generation = spec["generation"].as_u64().unwrap() as u32;
    let page_all = spec["events"].as_bool().unwrap_or(false);
    let outcome = BridgeClient::connect(&store, &recipient, generation).and_then(|mut client| {
        let hello = client.hello()?;
        let state = client.get_state()?;
        if !page_all {
            return Ok(json!({"ok": true, "hello": hello, "state": state}));
        }
        let (mut cursor, mut pages, mut seqs) = (0, 0, Vec::new());
        loop {
            let page = client.events_since(cursor)?;
            pages += 1;
            seqs.extend(page.events.iter().filter_map(|event| event["seq"].as_u64()));
            cursor = page.next;
            if !page.more {
                break;
            }
        }
        let after = client.get_state()?;
        Ok(
            json!({"ok": true, "hello": hello, "state": state, "pages": pages, "seqs": seqs,
                  "last": cursor, "after": after["generation"]}),
        )
    });
    let line = match outcome {
        Ok(value) => value,
        Err(error) => match error.downcast::<BridgeError>() {
            Ok(refusal) => json!({"ok": false, "code": refusal.code, "reason": refusal.reason}),
            Err(error) => json!({"ok": false, "code": "error", "reason": format!("{error:#}")}),
        },
    };
    println!("BRIDGE_PROBE {line}");
}
