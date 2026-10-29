//! Compiled stand-in for Pi: consume the real inherited pipe and call the real CLI.
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn sha256(bytes: &[u8]) -> String {
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(bytes).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

fn hello(message: &Value, envelope: &Value) -> Value {
    let mut child = Command::new(message["corral"].as_str().unwrap())
        .args([
            "--root",
            message["root"].as_str().unwrap(),
            "agent",
            "call",
            "--op",
            "hello",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(envelope).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.stdout.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args[1], "--launch-fd");
    let mut message: Value =
        serde_json::from_slice(&std::fs::read(format!("/proc/self/fd/{}", args[2])).unwrap())
            .unwrap();
    let root = message["root"].as_str().unwrap().to_owned();
    let role = message["role"].as_str().unwrap().to_owned();
    let session = format!("fixture-{role}");
    let session_file = Path::new(message["session_dir"].as_str().unwrap()).join("fixture.jsonl");
    let mut identity = json!({
        "store": message["store"], "project": message["project"], "role": message["role"],
        "session_id": session, "generation": message["generation"],
    });
    for key in ["task", "run", "review", "callback"] {
        if !message[key].is_null() {
            identity[key] = message[key].clone();
        }
    }
    let start = Command::new(message["corral"].as_str().unwrap())
        .args(["runtime-probe", "--pid", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(start.status.success(), "{start:?}");
    let payload = json!({
        "ctl_cap_sha256": sha256(message["secrets"]["ctl_cap"].as_str().unwrap().as_bytes()),
        "session_file": session_file, "pid": std::process::id(),
        "start_time": String::from_utf8(start.stdout).unwrap().trim(),
        "pane": message["pane"],
    });
    // Sort explicitly even if Cargo enabled serde_json's preserve_order feature.
    let canonical: std::collections::BTreeMap<_, _> = payload.as_object().unwrap().iter().collect();
    let mut envelope = json!({
        "version": 1, "op": "hello", "request_id": "ev-1-0", "identity": identity,
        "payload_sha256": sha256(&serde_json::to_vec(&canonical).unwrap()),
        "auth": {"host_cap": message["secrets"]["host_cap"],
                 "bind_nonce": message["secrets"]["bind_nonce"]},
        "payload": payload,
    });
    let bound = hello(&message, &envelope);
    assert_eq!(bound["state"], "committed", "{bound}");
    // A new host cannot reuse the nonce to take over this generation.
    envelope["request_id"] = json!("ev-1-1");
    envelope["auth"]["host_cap"] = json!("f".repeat(64));
    let reused = hello(&message, &envelope);
    assert_eq!(reused["result"]["code"], "nonce_consumed", "{reused}");
    let prompt = Path::new(message["prompt_file"].as_str().unwrap());
    assert!(
        prompt.is_file(),
        "the launch must carry the actual rendered prompt"
    );
    // Planner launches already promise an enforcing sandbox. Do not introduce a
    // source mutation into the separate reviewer launch just to probe that promise.
    let read_only = role == "planner" && {
        let protected = Path::new(message["cwd"].as_str().unwrap()).join("protected.txt");
        std::fs::write(protected, "must not write").is_err()
    };
    message.as_object_mut().unwrap().remove("secrets");
    let proof = json!({
        "launch": message, "hello": bound, "nonce_reuse": reused, "read_only": read_only,
    });
    std::fs::write(
        Path::new(&root).join(format!(".corral/state/planner-launch-{role}.json")),
        serde_json::to_vec(&proof).unwrap(),
    )
    .unwrap();
}
