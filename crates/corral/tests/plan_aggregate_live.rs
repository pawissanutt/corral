//! Aggregate Plan review on a real isolated herdr server: the policy-selected reviewer runs in
//! its own pane, inspects the combined candidate and evidence, and answers via its callback.
//! Run inside `scripts/test-herdr.sh`; skips when `corral-test` is unavailable.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use corral_core::{AgentCmd, AggregateAttempt, Origin, ReviewContractKind, Status, Store};
use corral_herdr::Herdr;
use notify::{RecursiveMode, Watcher};
use serde_json::{Value, json};

/// Waits for the test's go signal, inspects the checkout and its prompt, then calls back.
const REVIEWER: &str = r#"#!/bin/sh
set -eu
prompt="$1"
out="$2"
read go < "$out/go.fifo"
cmd=$(grep 'plan aggregate-verdict' "$prompt" | tail -n 1)
notes=$(printf '%s\n' "$cmd" | sed "s/.*--notes-file '\(.*\)'$/\1/")
head=$(git rev-parse HEAD)
verdict=approve
{
  echo "Inspected checkout $PWD at $head."
  for file in first.txt second.txt; do
    if test -f "$file"; then echo "- $file present"; else echo "- $file missing"; verdict=changes; fi
  done
  if grep -q "Combined candidate $head" "$prompt"; then echo "- prompt candidate matches HEAD"; else echo "- prompt candidate differs from HEAD"; verdict=changes; fi
  if grep -q 'Aggregate gate: pass' "$prompt"; then echo "- aggregate gate passed"; else echo "- aggregate gate did not pass"; verdict=changes; fi
  for criterion in C1 C2; do
    if grep -q "\"criterion\": \"$criterion\"" "$prompt"; then echo "- aggregate evidence for $criterion recorded"; else echo "- no aggregate evidence for $criterion"; verdict=changes; fi
  done
} > "$notes"
cmd=$(printf '%s\n' "$cmd" | sed "s/approve|changes|escalate/$verdict/")
export HERDR_BIN_PATH="$out/herdr-fence"
eval "$cmd" > "$out/verdict.out" 2>&1
"#;

/// Hold only this callback's real pane-close request after its verdict has committed.
const CLEANUP_FENCE: &str = r#"#!/usr/bin/env python3
import os
import socket
import sys

if sys.argv[1:5] == ["--session", "corral-test", "pane", "close"]:
    out = os.path.dirname(__file__)
    with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as signal:
        signal.bind(os.path.join(out, "cleanup-release.sock"))
        signal.settimeout(120)
        signal.sendto(b"cleanup-ready", os.path.join(out, "cleanup-ready.sock"))
        assert signal.recv(64) == b"close"
os.execvp("herdr", ["herdr", *sys.argv[1:]])
"#;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn commit(repo: &Path, message: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", message]);
    git(repo, &["rev-parse", "HEAD"])
}

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corral"))
        .env("CORRAL_ROOT", root)
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_BIN_PATH")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn json_ok(root: &Path, args: &[&str]) -> Value {
    let output = cli(root, args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// A direct-to-main Plan with two members accepted on main and an `auto` reviewer mapping.
fn fixture(root: &Path, reviewer: &Path, out: &Path) -> (Store, String) {
    let repo = root.join("app");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-qb", "main"]);
    git(&repo, &["config", "user.name", "Live fixture"]);
    git(&repo, &["config", "user.email", "live@example.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join(".gitignore"), ".worktrees/\n").unwrap();
    let mut base = commit(&repo, "initial");
    let mut store = Store::init(root).unwrap();
    store.config.herdr.session = Some("corral-test".into());
    store.config.review.gate = Some(vec![
        "test -f first.txt".into(),
        "test -f second.txt".into(),
    ]);
    store.config.review.reviewer = Some("auto".into());
    store.config.agents.insert(
        "aggregate-reviewer".into(),
        AgentCmd {
            command: format!(
                "sh {} {{prompt_file}} {}",
                reviewer.display(),
                out.display()
            ),
        },
    );
    for kind in [ReviewContractKind::Implementation, ReviewContractKind::Plan] {
        store
            .config
            .routing
            .review_profiles
            .insert(kind, "aggregate-reviewer".into());
    }
    store.config.validate_review().unwrap();
    store.save_config().unwrap();
    let source = root.join("plan.md");
    std::fs::write(
        &source,
        "---\ntitle: Live Plan\nproject: app\napproved_scope:\n  goal: Combined change\n  acceptance:\n    - {criterion: C1, scenario: first works, expected: pass}\n    - {criterion: C2, scenario: both work together, expected: pass together}\ntasks:\n  - {key: first, title: First}\n  - {key: second, title: Second}\n---\n",
    )
    .unwrap();
    let ctx = corral_ops::Ctx::from_store(store.clone());
    let imported = corral_ops::import_plan(&ctx, &source, false, Origin::Agent).unwrap();
    for (member, file) in imported.tasks.iter().zip(["first.txt", "second.txt"]) {
        std::fs::write(repo.join(file), "accepted work\n").unwrap();
        let sha = commit(&repo, &member.id);
        store
            .update(&member.id, |task| {
                task.status = Status::Done;
                task.runs = vec![
                    serde_json::from_value(json!({
                        "id":"R1", "agent":"fixture", "worktree":"/unused",
                        "branch":format!("corral/{}", member.id), "base":"main", "base_sha":base,
                        "started":"2026-10-19T00:00:00Z", "outcome":"done",
                    }))
                    .unwrap(),
                ];
                Ok(())
            })
            .unwrap();
        store
            .log_event(
                Some(&member.id),
                "approved",
                json!({"run":"R1", "sha":sha, "strategy":"fast_forward", "base":"main"}),
            )
            .unwrap();
        base = sha;
    }
    (store, imported.tracker.id)
}

fn observe(root: &Path, plan: &str, attempt: &str, criterion: &str, expected: &str) {
    let file: PathBuf = root.join(format!("{criterion}.json"));
    std::fs::write(
        &file,
        serde_json::to_vec(&json!({
            "version":1, "key":format!("{attempt}-{criterion}"), "criterion":criterion,
            "run":"aggregate", "attempt":attempt, "status":"pass",
            "action":"run the combined scenario", "environment":"isolated corral-test herdr",
            "expected":expected, "observed":"observed", "artifacts":[],
            "captured":jiff::Timestamp::now(), "producer":"live test", "live":true,
        }))
        .unwrap(),
    )
    .unwrap();
    json_ok(
        root,
        &[
            "plan",
            "evidence",
            plan,
            "--file",
            file.to_str().unwrap(),
            "--json",
        ],
    );
}

fn current(store: &Store, plan: &str) -> AggregateAttempt {
    store
        .get(plan)
        .unwrap()
        .aggregate_attempts
        .last()
        .cloned()
        .unwrap()
}

fn herdr_event(events: &mut BufReader<UnixStream>, deadline: Instant) -> Value {
    events
        .get_ref()
        .set_read_timeout(Some(
            deadline.checked_duration_since(Instant::now()).unwrap(),
        ))
        .unwrap();
    let mut line = String::new();
    assert_ne!(events.read_line(&mut line).unwrap(), 0, "herdr event EOF");
    serde_json::from_str(&line).unwrap()
}

#[test]
fn isolated_aggregate_reviewer_answers_its_callback_before_owner_acceptance() {
    let herdr = Herdr::with_bin("herdr".into(), Some("corral-test".into()));
    if !herdr.available() {
        println!("SKIP: herdr session corral-test unavailable");
        return;
    }
    // Given a verified Plan with passing aggregate scenarios and an `auto` reviewer mapping.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let out = root.join("reviewer-out");
    std::fs::create_dir(&out).unwrap();
    let cleanup_ready = UnixDatagram::bind(out.join("cleanup-ready.sock")).unwrap();
    cleanup_ready
        .set_read_timeout(Some(Duration::from_secs(120)))
        .unwrap();
    let fence = out.join("herdr-fence");
    std::fs::write(&fence, CLEANUP_FENCE).unwrap();
    std::fs::set_permissions(&fence, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mkfifo = Command::new("mkfifo")
        .arg(out.join("go.fifo"))
        .status()
        .unwrap();
    assert!(mkfifo.success());
    let reviewer = root.join("reviewer.sh");
    std::fs::write(&reviewer, REVIEWER).unwrap();
    let (store, plan) = fixture(&root, &reviewer, &out);
    let attempt = json_ok(&root, &["plan", "verify", &plan, "--json"]);
    assert_eq!(attempt["gate"]["result"], "pass", "{attempt}");
    let attempt = attempt["id"].as_str().unwrap().to_owned();
    observe(&root, &plan, &attempt, "C1", "pass");
    observe(&root, &plan, &attempt, "C2", "pass together");

    // When the owner accepts before any aggregate review, it is refused.
    let refused = cli(&root, &["plan", "accept", &plan, "--attempt", &attempt]);
    assert!(!refused.status.success(), "{refused:?}");
    let missing = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        missing.contains("aggregate review missing or pending"),
        "{missing}"
    );

    // When the configured reviewer is launched in a real pane, subscribed before launch.
    let (tx, events) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event
            .as_ref()
            .is_ok_and(|event| matches!(event.kind, notify::EventKind::Access(_)))
        {
            return;
        }
        let _ = tx.send(event);
    })
    .unwrap();
    // The recorded verdict signals approval, not completion of the subsequent pane cleanup.
    watcher
        .watch(store.dir(), RecursiveMode::Recursive)
        .unwrap();
    let launched = json_ok(
        &root,
        &[
            "plan",
            "aggregate-review",
            &plan,
            "--attempt",
            &attempt,
            "--json",
        ],
    );
    let pane = launched["herdr"]["pane_id"].as_str().unwrap().to_owned();
    let socket = launched["herdr"]["socket"].as_str().unwrap().to_owned();
    assert_eq!(launched["reviewer"], "aggregate-reviewer", "{launched}");
    assert_eq!(launched["route"]["profile_source"], "auto_mapping");
    assert!(launched["verdict"].is_null(), "{launched}");
    assert!(
        socket.ends_with("sessions/corral-test/herdr.sock"),
        "{socket}"
    );
    assert!(
        herdr.pane_get(&pane).unwrap().is_some(),
        "reviewer pane {pane}"
    );

    // Then the pending review still cannot be accepted.
    let pending = cli(&root, &["plan", "accept", &plan, "--attempt", &attempt]);
    assert!(!pending.status.success(), "{pending:?}");
    assert!(
        String::from_utf8_lossy(&pending.stderr).contains("aggregate review missing or pending"),
        "{pending:?}"
    );

    // Subscribe and observe the acknowledgment before triggering the callback's cleanup.
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut connection = UnixStream::connect(&socket).unwrap();
    writeln!(
        connection,
        "{}",
        json!({"id":"cleanup", "method":"events.subscribe",
            "params":{"subscriptions":[{"type":"pane.closed"}]}}),
    )
    .unwrap();
    let mut cleanup_events = BufReader::new(connection);
    let subscribed = herdr_event(&mut cleanup_events, deadline);
    assert_eq!(
        subscribed["result"]["type"], "subscription_started",
        "{subscribed}"
    );

    // When the reviewer is released, it inspects and answers through its exact callback.
    let fifo = out.join("go.fifo");
    let released = std::thread::spawn(move || std::fs::write(fifo, "go\n"));
    let answered = || {
        current(&store, &plan)
            .reviews
            .last()
            .is_some_and(|review| review.verdict.is_some())
    };
    while !answered() {
        let received = deadline
            .checked_duration_since(Instant::now())
            .map(|remaining| events.recv_timeout(remaining));
        if !matches!(received, Some(Ok(Ok(_)))) {
            let files: Vec<_> = std::fs::read_dir(&out)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            panic!(
                "isolated aggregate reviewer did not finish ({received:?}); files {files:?}; verdict output {:?}; pane:\n{}",
                std::fs::read_to_string(out.join("verdict.out")).ok(),
                herdr
                    .pane_read(&pane, 80)
                    .unwrap_or_else(|error| format!("{error:#}"))
            );
        }
    }
    released.join().unwrap().unwrap();
    cleanup_ready
        .set_read_timeout(Some(
            deadline.checked_duration_since(Instant::now()).unwrap(),
        ))
        .unwrap();
    let mut signal = [0; 64];
    let (length, cleanup_sender) = cleanup_ready.recv_from(&mut signal).unwrap();
    assert_eq!(&signal[..length], b"cleanup-ready");
    let callback = std::fs::read_to_string(out.join("verdict.out")).unwrap_or_default();
    let review = current(&store, &plan).reviews.last().cloned().unwrap();
    assert_eq!(review.verdict, Some(corral_core::Verdict::Approve));
    let notes = review.notes.clone().unwrap();
    assert!(
        notes.contains("aggregate evidence for C2 recorded"),
        "{notes}"
    );

    // Then owner acceptance succeeds against that current completed review.
    let acceptance = json_ok(
        &root,
        &["plan", "accept", &plan, "--attempt", &attempt, "--json"],
    );
    assert_eq!(acceptance["by"], "owner", "{acceptance}");
    assert_eq!(acceptance["state"], "landed", "{acceptance}");
    assert_eq!(store.get(&plan).unwrap().status, Status::Done);
    // The fence proves acceptance can land before the real cleanup request has run.
    let pane_before_cleanup = herdr.pane_get(&pane).unwrap().is_some();
    assert!(
        pane_before_cleanup,
        "callback must still be at its cleanup fence"
    );
    println!(
        "CLEANUP_RACE_EVIDENCE {}",
        json!({"verdict": review.verdict, "acceptance": acceptance, "pane_still_open": pane_before_cleanup,
            "cleanup_sender": cleanup_sender.as_pathname()}),
    );
    cleanup_ready
        .send_to(b"close", cleanup_sender.as_pathname().unwrap())
        .unwrap();
    let closed = loop {
        let event = herdr_event(&mut cleanup_events, deadline);
        assert_ne!(event["event"], "events_lost", "{event}");
        if event["event"] == "pane_closed" && event["data"]["pane_id"] == pane {
            break event;
        }
    };
    let pane_after = herdr.pane_get(&pane).unwrap().is_some();
    assert!(
        !pane_after,
        "the reviewer workspace is closed after its verdict"
    );
    println!(
        "LIVE_AGGREGATE_EVIDENCE {}",
        json!({
            "plan": plan, "attempt": attempt, "candidate": review.identity,
            "review": review.id, "reviewer": review.reviewer, "route": review.route,
            "launch": review.herdr, "socket": socket, "pane_open_after_verdict": pane_after,
            "pane_open_before_cleanup": pane_before_cleanup, "cleanup_event": closed,
            "owner_refused_without_review": missing.trim(),
            "verdict": review.verdict, "notes": notes, "callback_output": callback.trim(),
            "acceptance": acceptance,
        })
    );
}
