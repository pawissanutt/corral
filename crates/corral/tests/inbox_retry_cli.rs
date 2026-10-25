//! `corral inbox retry` and `recover` caller contexts and refusals through the real binary.
use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::inbox::{
    Inbox, MAX_PAYLOAD, Message, MessageBinding, Post, Recipient, SendOutcome, SessionBinding,
    TerminalState, Urgency,
};
use corral_core::{Run, Status, Store};
use serde_json::json;

fn invoke(root: &Path, cwd: &Path, pm: bool, args: &[&str]) -> Output {
    invoke_with_marker(root, cwd, pm, args, None)
}

fn invoke_with_marker(
    root: &Path,
    cwd: &Path,
    pm: bool,
    args: &[&str],
    marker: Option<&str>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
    if pm {
        command.env("CORRAL_PM", "1");
    }
    command.env_remove("AI_AGENT").env_remove("PI_CODING_AGENT");
    if let Some(marker) = marker {
        command.env(marker, "1");
    }
    command
        .current_dir(cwd)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("HERDR_BIN_PATH", root.join("no-herdr"))
        .arg("--root")
        .arg(root)
        .arg("inbox")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn post(inbox: &Inbox<'_>, body: &str) -> Message {
    inbox
        .post(Post {
            kind: "owner_note",
            urgency: Urgency::Wake,
            author: "owner",
            body,
            binding: MessageBinding::default(),
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap()
}

fn uncertain_envelope(inbox: &Inbox<'_>) {
    let _deliver = inbox.delivery_lock().unwrap();
    let binding = SessionBinding {
        session: "fixture".into(),
        endpoint: None,
        transcript: None,
    };
    let mut envelope = inbox
        .publish(&binding, "fixture", MAX_PAYLOAD, false)
        .unwrap()
        .unwrap();
    inbox
        .begin_attempt(&mut envelope, jiff::Timestamp::now())
        .unwrap();
    inbox
        .finish_attempt(
            &mut envelope,
            SendOutcome::Uncertain {
                reason: "fixture timeout".into(),
            },
        )
        .unwrap();
}

/// A store with one live inbox run whose worktree exists inside the root.
fn fixture() -> (tempfile::TempDir, Store, Recipient, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let worktree = dir.path().join(".worktrees/T-1");
    std::fs::create_dir_all(&worktree).unwrap();
    let task = store.create("Worker", None, Status::Running, "").unwrap();
    let run: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "fixture", "worktree": worktree, "branch": "corral/T-1",
        "base": "main", "base_sha": "abc", "started": "2026-10-25T00:00:00Z",
        "delivery": "inbox"
    }))
    .unwrap();
    store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    let recipient = Recipient::Run {
        task: task.id,
        run: "R1".into(),
    };
    (dir, store, recipient, worktree)
}

#[test]
fn workers_and_reviewers_cannot_retry_or_recover_from_a_run_worktree() {
    // Given a safe never-attempted cancelled message and an agent shell in the run worktree.
    let (dir, store, recipient, worktree) = fixture();
    let inbox = Inbox::new(&store, recipient).unwrap();
    let message = post(&inbox, "cancelled");
    inbox
        .mark_terminal(message.seq, TerminalState::Cancelled, "fixture", "test")
        .unwrap();
    let before = inbox.messages().unwrap();

    // When the worker or reviewer retries or recovers.
    let retry = invoke(dir.path(), &worktree, false, &["retry", &message.id]);
    let inspect = invoke(
        dir.path(),
        &worktree,
        false,
        &["inspect-retry", &message.id],
    );
    let recover = invoke(
        dir.path(),
        &worktree,
        false,
        &["recover", inbox.key(), "--resend"],
    );

    // Then both refuse with their reason and nothing is posted.
    assert_eq!(retry.status.code(), Some(1), "{retry:?}");
    assert!(
        stderr(&inspect).contains("inspect_refused: disallowed_context"),
        "{inspect:?}"
    );
    assert!(
        stderr(&retry).contains("retry_refused: disallowed_context"),
        "{retry:?}"
    );
    assert_eq!(recover.status.code(), Some(1), "{recover:?}");
    assert!(
        stderr(&recover).contains("recover_refused: owner_only"),
        "{recover:?}"
    );
    assert_eq!(inbox.messages().unwrap(), before);
}

#[test]
fn the_pm_retries_a_safe_message_but_cannot_recover_an_envelope() {
    // Given a never-attempted cancelled message and an open uncertain envelope for one run.
    let (dir, store, recipient, _) = fixture();
    let inbox = Inbox::new(&store, recipient).unwrap();
    let cancelled = post(&inbox, "cancelled");
    inbox
        .mark_terminal(cancelled.seq, TerminalState::Cancelled, "fixture", "test")
        .unwrap();

    // When the PM retries the exact message.
    let retry = invoke(dir.path(), dir.path(), true, &["retry", &cancelled.id]);

    // Then the re-post lands on the same recipient with a new id.
    assert!(retry.status.success(), "{retry:?}");
    let id = String::from_utf8(retry.stdout).unwrap();
    assert_eq!(id.trim(), format!("{}#2", inbox.key()));
    let retried = inbox.messages().unwrap().pop().unwrap();
    assert_eq!(retried.recipient, inbox.recipient);

    // And the PM cannot record an owner recovery decision.
    uncertain_envelope(&inbox);
    let state = inbox.state().unwrap();
    let recover = invoke(
        dir.path(),
        dir.path(),
        true,
        &["recover", inbox.key(), "--abandon"],
    );
    assert_eq!(recover.status.code(), Some(1), "{recover:?}");
    assert!(
        stderr(&recover).contains("recover_refused: owner_only"),
        "{recover:?}"
    );
    assert_eq!(inbox.state().unwrap(), state);
    assert_eq!(inbox.terminal(retried.seq).unwrap(), None);
}

#[test]
fn owner_terminal_recovery_flags_refuse_as_unavailable_and_change_nothing() {
    for action in ["--submit", "--discard"] {
        // Given an open uncertain envelope on a live run.
        let (dir, store, recipient, _) = fixture();
        let inbox = Inbox::new(&store, recipient).unwrap();
        post(&inbox, "draft");
        uncertain_envelope(&inbox);
        let state = inbox.state().unwrap();
        let events = std::fs::read(store.dir().join("events.jsonl")).unwrap();

        // When the owner runs the uninstalled terminal adapter's recovery.
        let output = invoke(
            dir.path(),
            dir.path(),
            false,
            &["recover", inbox.key(), action],
        );

        // Then it refuses as unavailable and the envelope, state and events are unchanged.
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(
            stderr(&output).contains("terminal_adapter_unavailable"),
            "{output:?}"
        );
        assert_eq!(inbox.state().unwrap(), state);
        assert_eq!(
            std::fs::read(store.dir().join("events.jsonl")).unwrap(),
            events
        );
    }
}

#[test]
fn an_owner_retry_of_an_ambiguous_message_names_an_inspect_disposition() {
    // Given an uncertain attempt the owner abandoned.
    let (dir, store, recipient, _) = fixture();
    let inbox = Inbox::new(&store, recipient).unwrap();
    let message = post(&inbox, "maybe delivered");
    uncertain_envelope(&inbox);
    let abandon = invoke(
        dir.path(),
        dir.path(),
        false,
        &["recover", inbox.key(), "--abandon"],
    );
    assert!(abandon.status.success(), "{abandon:?}");
    let before = inbox.messages().unwrap();

    // When the owner retries it.
    let output = invoke(dir.path(), dir.path(), false, &["retry", &message.id]);

    // Then the refusal names the earlier attempt and an inspect disposition, not a recover command.
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let text = stderr(&output);
    assert!(
        text.contains("retry_refused: uncertain_prior_attempt"),
        "{text}"
    );
    assert!(
        text.contains(&format!("corral inbox show {}", message.id)),
        "{text}"
    );
    assert!(!text.contains("inbox recover"), "{text}");
    assert_eq!(inbox.messages().unwrap(), before);
}

#[test]
fn agent_marked_planners_and_reviewers_cannot_retry_outside_coding_worktrees() {
    for marker in ["AI_AGENT", "PI_CODING_AGENT"] {
        // Given a project-root planning/review shell with no coding worktrees.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
        let message = post(&inbox, "cancelled");
        inbox
            .mark_terminal(message.seq, TerminalState::Cancelled, "fixture", "test")
            .unwrap();
        let before = inbox.state().unwrap();
        let events = std::fs::read(store.dir().join("events.jsonl")).unwrap();

        // When either supported agent marker identifies the caller.
        let output = invoke_with_marker(
            dir.path(),
            dir.path(),
            false,
            &["retry", &message.id],
            Some(marker),
        );

        // Then retry refuses before posting or claiming a retry.
        assert_eq!(output.status.code(), Some(1), "{marker}: {output:?}");
        assert!(
            stderr(&output).contains("retry_refused: disallowed_context"),
            "{output:?}"
        );
        assert_eq!(inbox.state().unwrap(), before);
        assert_eq!(inbox.messages().unwrap(), vec![message.clone()]);
        assert!(!inbox.record_path(message.seq, "retried").exists());
        assert_eq!(
            std::fs::read(store.dir().join("events.jsonl")).unwrap(),
            events
        );
    }
}

#[test]
fn agent_marked_planners_and_reviewers_cannot_recover_outside_coding_worktrees() {
    for marker in ["AI_AGENT", "PI_CODING_AGENT"] {
        for action in ["--resend", "--abandon"] {
            // Given an uncertain envelope and a project-root shell without coding worktrees.
            let dir = tempfile::tempdir().unwrap();
            let store = Store::init(dir.path()).unwrap();
            let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
            let message = post(&inbox, "uncertain");
            uncertain_envelope(&inbox);
            let before = inbox.state().unwrap();
            let envelope = inbox.envelope(1).unwrap();
            let events = std::fs::read(store.dir().join("events.jsonl")).unwrap();

            // When either supported agent marker requests an owner recovery.
            let output = invoke_with_marker(
                dir.path(),
                dir.path(),
                false,
                &["recover", inbox.key(), action],
                Some(marker),
            );

            // Then both recovery choices refuse without settling, posting or authorizing resend.
            assert_eq!(
                output.status.code(),
                Some(1),
                "{marker} {action}: {output:?}"
            );
            assert!(
                stderr(&output).contains("recover_refused: owner_only"),
                "{output:?}"
            );
            assert_eq!(inbox.state().unwrap(), before);
            assert_eq!(inbox.envelope(1).unwrap(), envelope);
            assert_eq!(inbox.terminal(message.seq).unwrap(), None);
            assert_eq!(inbox.messages().unwrap(), vec![message]);
            assert!(!inbox.flush_requested());
            assert!(!inbox.dir().join("envelopes/000001.settled.json").exists());
            assert_eq!(
                std::fs::read(store.dir().join("events.jsonl")).unwrap(),
                events
            );
        }
    }
}

#[test]
fn agent_marked_pm_keeps_safe_retry_authority_but_not_owner_recovery() {
    for marker in ["AI_AGENT", "PI_CODING_AGENT"] {
        // Given a PM that also runs inside an agent harness.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
        let message = post(&inbox, "cancelled");
        inbox
            .mark_terminal(message.seq, TerminalState::Cancelled, "fixture", "test")
            .unwrap();

        // When the PM retries an eligible message.
        let output = invoke_with_marker(
            dir.path(),
            dir.path(),
            true,
            &["retry", &message.id],
            Some(marker),
        );

        // Then safe retry still succeeds, while recovery remains owner-only.
        assert!(output.status.success(), "{marker}: {output:?}");
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "pm#2");
        uncertain_envelope(&inbox);
        let before = inbox.state().unwrap();
        let output = invoke_with_marker(
            dir.path(),
            dir.path(),
            true,
            &["recover", inbox.key(), "--abandon"],
            Some(marker),
        );
        assert_eq!(output.status.code(), Some(1), "{marker}: {output:?}");
        assert!(
            stderr(&output).contains("recover_refused: owner_only"),
            "{output:?}"
        );
        assert_eq!(inbox.state().unwrap(), before);
        assert_eq!(inbox.terminal(2).unwrap(), None);
    }
}

#[test]
fn inspect_retry_is_read_only_and_retry_rechecks_a_new_admission() {
    // Given a legacy-safe failed message and no evidence projection certificate.
    let (dir, store, recipient, _) = fixture();
    let inbox = Inbox::new(&store, recipient).unwrap();
    let message = post(&inbox, "failed");
    inbox
        .mark_terminal(message.seq, TerminalState::Failed, "fixture", "test")
        .unwrap();
    inbox
        .update_state(|state| {
            state.owner_header.as_mut().unwrap().entries[0].retry_evidence = Default::default();
            Ok(())
        })
        .unwrap();
    let before = inbox.state().unwrap();
    let events = std::fs::read(store.dir().join("events.jsonl")).unwrap();
    // When the PM inspects the exact ID through the real command.
    let output = invoke(
        dir.path(),
        dir.path(),
        true,
        &["inspect-retry", &message.id, "--json"],
    );
    assert!(output.status.success(), "{output:?}");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["eligibility"], true);
    assert_eq!(
        result["next_action"],
        format!("corral inbox retry {}", message.id)
    );
    assert_eq!(inbox.state().unwrap(), before);
    assert_eq!(
        std::fs::read(store.dir().join("events.jsonl")).unwrap(),
        events
    );
    // Then a newly admitted receipt invalidates that suggestion without a post.
    inbox
        .record_receipt(
            message.seq,
            corral_core::inbox::ReceiptLevel::Admitted,
            &corral_core::inbox::Receipt {
                envelope: None,
                binding: None,
                observed_at: "2026-10-25T00:00:00Z".parse().unwrap(),
                via: "fixture".into(),
                detail: json!({}),
            },
        )
        .unwrap();
    let retry = invoke(dir.path(), dir.path(), true, &["retry", &message.id]);
    assert!(!retry.status.success());
    assert!(
        stderr(&retry).contains("retry_refused: admitted"),
        "{retry:?}"
    );
    assert_eq!(inbox.state().unwrap().next_seq, 2);
}

#[test]
fn explicit_index_owner_is_budgeted_context_guarded_and_keeps_evidence_unknown() {
    // Given a legacy recipient with three retained messages.
    let (dir, store, recipient, worktree) = fixture();
    let inbox = Inbox::new(&store, recipient).unwrap();
    for _ in 0..3 {
        post(&inbox, "legacy");
    }
    inbox
        .update_state(|state| {
            state.owner_header = None;
            Ok(())
        })
        .unwrap();
    // When a worker asks for indexing, or the owner asks for an invalid page size.
    let denied = invoke(dir.path(), &worktree, false, &["index-owner", inbox.key()]);
    assert!(
        stderr(&denied).contains("index_refused: disallowed_context"),
        "{denied:?}"
    );
    for limit in ["0", "65"] {
        let output = invoke(
            dir.path(),
            dir.path(),
            false,
            &["index-owner", inbox.key(), "--limit", limit],
        );
        assert_eq!(output.status.code(), Some(2));
    }
    assert!(inbox.state().unwrap().owner_header.is_none());
    // Then the authorized command indexes identities only, with no transport or retry certificate.
    let output = invoke(
        dir.path(),
        dir.path(),
        true,
        &["index-owner", inbox.key(), "--limit", "64", "--json"],
    );
    assert!(output.status.success(), "{output:?}");
    let header: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(header["legacy_debt"].is_null());
    assert_eq!(header["entries"].as_array().unwrap().len(), 3);
    for entry in header["entries"].as_array().unwrap() {
        assert_eq!(entry["retry_evidence"]["coverage"], "unknown");
    }
}

fn evidence_bytes(path: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
    let mut snapshot = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            snapshot.insert(path.clone(), None);
            snapshot.extend(evidence_bytes(&path));
        } else {
            snapshot.insert(path.clone(), Some(std::fs::read(path).unwrap()));
        }
    }
    snapshot
}

#[test]
fn inspection_previews_full_header_without_writing_unrelated_retry_proof() {
    for position in [1, 9] {
        for pending in [false, true] {
            for blocked_handoff in [false, true] {
                // Given a full header with superseded barred/pending proof in either array.
                let (dir, store, recipient, _) = fixture();
                let inbox = Inbox::new(&store, recipient.clone()).unwrap();
                for _ in 1..position {
                    let earlier = post(&inbox, "settled");
                    inbox
                        .mark_terminal(earlier.seq, TerminalState::Cancelled, "fixture", "test")
                        .unwrap();
                }
                let unsafe_message = post(&inbox, "may have reached agent");
                let mut envelope = inbox
                    .publish(
                        &SessionBinding {
                            session: "fixture".into(),
                            endpoint: None,
                            transcript: None,
                        },
                        "fixture",
                        MAX_PAYLOAD,
                        true,
                    )
                    .unwrap()
                    .unwrap();
                let now = "2026-10-25T00:00:00Z".parse().unwrap();
                inbox.begin_attempt(&mut envelope, now).unwrap();
                if !pending {
                    inbox
                        .finish_attempt(&mut envelope, SendOutcome::Accepted)
                        .unwrap();
                }
                inbox
                    .mark_terminal(unsafe_message.seq, TerminalState::Failed, "fixture", "test")
                    .unwrap();
                let target = post(&inbox, "safe terminal target");
                inbox
                    .mark_terminal(target.seq, TerminalState::Cancelled, "fixture", "test")
                    .unwrap();
                let later = post(&inbox, "superseding instruction");
                while inbox.state().unwrap().next_seq <= 16 {
                    post(&inbox, "still retained");
                }
                inbox
                    .record_receipt(
                        later.seq,
                        corral_core::inbox::ReceiptLevel::Observed,
                        &corral_core::inbox::Receipt {
                            envelope: None,
                            binding: None,
                            observed_at: now,
                            via: "fixture".into(),
                            detail: json!({}),
                        },
                    )
                    .unwrap();
                let proof = inbox.record_path(unsafe_message.seq, TerminalState::Failed.suffix());
                let temporary = proof.with_extension("json.tmp");
                if blocked_handoff {
                    std::fs::create_dir(&temporary).unwrap();
                }
                let state = inbox.state().unwrap();
                assert_eq!(state.owner_header.as_ref().unwrap().len(), 16);
                let task_bytes =
                    std::fs::read(store.get(recipient.task().unwrap()).unwrap().path).unwrap();
                let events = std::fs::read(store.dir().join("events.jsonl")).unwrap();
                let evidence = evidence_bytes(&inbox.dir());
                // When the real CLI previews capacity without committing those handoffs.
                let output = invoke(
                    dir.path(),
                    dir.path(),
                    true,
                    &["inspect-retry", &target.id, "--json"],
                );
                assert!(output.status.success(), "{output:?}");
                let inspection: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
                // Then writability never affects eligibility and every evidence byte stays unchanged.
                assert_eq!(inspection["eligibility"], true, "{inspection}");
                assert_eq!(
                    inspection["next_action"],
                    format!("corral inbox retry {}", target.id)
                );
                assert_eq!(evidence_bytes(&inbox.dir()), evidence);
                assert_eq!(inbox.state().unwrap(), state);
                assert_eq!(
                    std::fs::read(store.get(recipient.task().unwrap()).unwrap().path).unwrap(),
                    task_bytes
                );
                assert_eq!(
                    std::fs::read(store.dir().join("events.jsonl")).unwrap(),
                    events
                );
                // Actual maintenance still must commit proof before removing the identity.
                if blocked_handoff {
                    assert!(inbox.maintain_owner_header(now).is_err());
                    assert_eq!(inbox.state().unwrap(), state);
                    std::fs::remove_dir(&temporary).unwrap();
                }
                inbox.maintain_owner_header(now).unwrap();
                assert!(
                    !inbox
                        .state()
                        .unwrap()
                        .owner_header
                        .unwrap()
                        .iter()
                        .any(|entry| entry.seq == unsafe_message.seq)
                );
                assert_ne!(
                    std::fs::read(&proof).unwrap(),
                    evidence[&proof].as_ref().unwrap().clone()
                );
                assert!(!inbox.inspect_retry(unsafe_message.seq).unwrap().eligible);
                assert!(inbox.retry(unsafe_message.seq).is_err());
            }
        }
    }
}

#[test]
fn inspect_retry_reports_unknown_publication_and_corrupt_envelope_without_writes() {
    let (dir, store, recipient, _) = fixture();
    let inbox = Inbox::new(&store, recipient).unwrap();
    let message = post(&inbox, "publication");
    uncertain_envelope(&inbox);
    std::fs::write(inbox.envelope_path(1), "{corrupt").unwrap();
    let before = inbox.state().unwrap();
    // When exact inspection sees corrupt evidence it is an incomplete result, not replay permission.
    let output = invoke(
        dir.path(),
        dir.path(),
        true,
        &["inspect-retry", &message.id, "--json"],
    );
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["evidence_complete"], false);
    assert_eq!(value["eligibility"], false);
    assert_eq!(inbox.state().unwrap(), before);
}
