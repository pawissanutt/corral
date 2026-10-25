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
    let recover = invoke(
        dir.path(),
        &worktree,
        false,
        &["recover", inbox.key(), "--resend"],
    );

    // Then both refuse with their reason and nothing is posted.
    assert_eq!(retry.status.code(), Some(1), "{retry:?}");
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
