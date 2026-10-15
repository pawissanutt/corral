use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

static NEXT_FAKE: AtomicU64 = AtomicU64::new(0);

// Commands, not elapsed time, drive this fake's composer. It preserves literal argv
// text on paste and only clears it when the configured Enter is accepted.
const FAKE: &str = r#"#!/usr/bin/env bash
set -eu
cd "$(dirname "$0")"
mode=$(<mode)
ok() { printf '%s\n' '{"result":{}}'; }
fail() { printf '{"error":{"code":"%s","message":"fake failure"}}\n' "$1"; exit 1; }
bump() {
  local n=0
  if [[ -f "$1" ]]; then n=$(<"$1"); fi
  printf '%s' "$((n + 1))" > "$1"
}
case "$1 $2" in
  "agent prompt")
    bump prompts
    case "$mode" in
      agent_ok) ok ;;
      agent_error) fail failed ;;
      native_pasted) printf '%s' "$4" >> composer; bump pastes; fail agent_not_ready ;;
      no_agent) fail no_agent ;;
      *) fail agent_not_ready ;;
    esac ;;
  "pane read")
    bump reads
    [[ "$mode" != read_error ]] || fail transport
    if [[ "$mode" == verify_error && -f enters ]]; then fail transport; fi
    printf 'history: previous reply\n❯ '
    if [[ -f composer ]]; then
      if [[ "$mode" == delayed ]]; then
        bump visibility_reads
        if (( $(<visibility_reads) >= 3 )); then printf '%s' "$(<composer)"; fi
      else
        printf '%s' "$(<composer)"
      fi
    fi
    printf '\n' ;;
  "pane send-text")
    bump pastes
    [[ "$mode" != paste_error ]] || fail transport
    if [[ "$mode" != never ]]; then printf '%s' "$4" >> composer; fi
    ok ;;
  "pane send-keys")
    [[ "$4" == enter ]] || fail unexpected_key
    bump enters
    [[ "$mode" != enter_error ]] || fail transport
    case "$mode" in
      exhausted) ;;
      lost_first) if (( $(<enters) >= 2 )); then rm -f composer; fi ;;
      *) rm -f composer ;;
    esac
    ok ;;
  *) fail unexpected_command ;;
esac
"#;

struct Fake {
    dir: PathBuf,
    herdr: Herdr,
}

impl Fake {
    fn new(mode: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "corral-delivery-{}-{}",
            std::process::id(),
            NEXT_FAKE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&dir).unwrap();
        let bin = dir.join("herdr");
        std::fs::write(&bin, FAKE).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(dir.join("mode"), mode).unwrap();
        Self {
            dir,
            herdr: Herdr::with_bin(bin, None),
        }
    }

    fn count(&self, name: &str) -> u32 {
        match std::fs::read_to_string(self.dir.join(name)) {
            Ok(n) => n.parse().unwrap(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => panic!("{e}"),
        }
    }

    fn send(&self, text: &str, waits: &mut Vec<Duration>) -> Delivery {
        self.herdr
            .send_text_with_wait("w1:p1", text, true, &mut |d| waits.push(d))
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}

#[test]
fn lost_first_enter_retries_without_repasting() {
    // Given a composer that ignores the first Enter.
    let fake = Fake::new("lost_first");
    let mut waits = Vec::new();
    // When delivery runs using a virtual wait.
    let delivery = fake.send("please fix", &mut waits);
    // Then the second Enter submits, with only one literal paste.
    assert_eq!(
        delivery,
        Delivery {
            via: DeliveryVia::SendText,
            submitted: true,
            attempts: 2,
            error: None,
        }
    );
    assert_eq!(fake.count("pastes"), 1);
    assert_eq!(fake.count("enters"), 2);
    assert_eq!(waits.len(), POLLS);
}

#[test]
fn never_visible_text_cannot_trigger_enter() {
    // Given a transport that accepts a paste but never displays it.
    let fake = Fake::new("never");
    // When the bounded visibility poll completes.
    let delivery = fake.send("please fix", &mut Vec::new());
    // Then there is no submission attempt or false success.
    assert!(!delivery.submitted);
    assert_eq!(delivery.attempts, 0);
    assert!(delivery.error.is_some());
    assert_eq!(fake.count("enters"), 0);
    assert_eq!(fake.count("reads"), 1 + u32::try_from(POLLS).unwrap());
}

#[test]
fn public_delivery_apis_use_builder_wait_for_never_visible_text() {
    // Given a fake whose input never appears and a wait that records instead of sleeping.
    static WAITS: AtomicU64 = AtomicU64::new(0);
    fn record_wait(_: Duration) {
        WAITS.fetch_add(1, Ordering::Relaxed);
    }
    for native in [false, true] {
        let fake = Fake::new("never");
        let herdr = fake.herdr.clone().with_poll_wait(record_wait);
        let before = WAITS.load(Ordering::Relaxed);
        // When either public entry point exhausts its visibility budget.
        let delivery = if native {
            herdr.agent_prompt("w1:p1", "please fix")
        } else {
            herdr.pane_send_text("w1:p1", "please fix", true)
        }
        .unwrap();
        // Then failure is recordable with no Enter, and every wait used the injected function.
        assert!(!delivery.submitted);
        assert_eq!(delivery.via, DeliveryVia::SendText);
        assert_eq!(delivery.attempts, 0);
        assert!(delivery.error.is_some());
        assert_eq!(fake.count("enters"), 0);
        assert_eq!(
            WAITS.load(Ordering::Relaxed) - before,
            u64::try_from(POLLS - 1).unwrap()
        );
    }
}

#[test]
fn immediate_send_success_has_transport_metadata() {
    // Given an input that accepts the first Enter.
    let fake = Fake::new("immediate");
    let mut waits = Vec::new();
    // When the public production entry point is used (no waits are needed).
    let delivery = fake.herdr.pane_send_text("w1:p1", "hello", true).unwrap();
    // Then success counts Enter, not the paste or reads.
    assert_eq!(delivery.via, DeliveryVia::SendText);
    assert!(delivery.submitted);
    assert_eq!(delivery.attempts, 1);
    assert_eq!(delivery.error, None);
    // The injected entry point must agree on the same immediate behavior.
    assert!(fake.send("hello again", &mut waits).submitted);
    assert!(waits.is_empty());
}

#[test]
fn agent_prompt_success_serializes_exact_metadata() {
    // Given a named agent that accepts a semantic prompt.
    let fake = Fake::new("agent_ok");
    // When the public prompt API is used.
    let delivery = fake.herdr.agent_prompt("w1:p1", "hello").unwrap();
    // Then there is no terminal fallback and the wire enum is snake_case.
    assert_eq!(
        serde_json::to_value(delivery).unwrap(),
        serde_json::json!({
            "via": "agent_prompt", "submitted": true, "attempts": 1, "error": null
        })
    );
    assert_eq!(fake.count("pastes"), 0);
    assert_eq!(fake.count("reads"), 1);
}

#[test]
fn agent_prompt_falls_back_when_no_agent_or_not_ready() {
    // Given either supported refusal from the semantic transport.
    for mode in ["no_agent", "immediate"] {
        let fake = Fake::new(mode);
        let mut waits = Vec::new();
        // When a prompt is delivered through the production poller.
        let delivery = fake
            .herdr
            .agent_prompt_with_wait("w1:p1", "please fix", &mut |d| waits.push(d));
        // Then terminal fallback verifies the composer and records its own attempts.
        assert!(delivery.submitted);
        assert_eq!(delivery.via, DeliveryVia::SendText);
        assert_eq!(delivery.attempts, 1);
        assert_eq!(fake.count("prompts"), 1);
        assert_eq!(fake.count("pastes"), 1);
        assert_eq!(fake.count("reads"), 4);
        assert!(waits.is_empty());
    }
}

#[test]
fn exhausted_enters_leave_pending_text_and_bounded_backoff() {
    // Given an input that ignores every Enter.
    let fake = Fake::new("exhausted");
    let mut waits = Vec::new();
    // When the retry budget is exhausted.
    let delivery = fake.send("please fix", &mut waits);
    // Then the exact text is retained, no duplicate paste occurs, and waits are bounded.
    assert!(!delivery.submitted);
    assert_eq!(delivery.attempts, ENTER_ATTEMPTS);
    assert!(delivery.error.is_some());
    assert_eq!(fake.count("enters"), ENTER_ATTEMPTS);
    assert_eq!(fake.count("pastes"), 1);
    assert_eq!(
        std::fs::read_to_string(fake.dir.join("composer")).unwrap(),
        "please fix"
    );
    assert_eq!(waits.len(), 3 * (POLLS - 1) + 2);
    assert_eq!(waits[2 * (POLLS - 1) + 1], Duration::from_millis(200));
}

#[test]
fn resend_reuses_existing_composer_instead_of_duplicate_paste() {
    // Given a previously failed delivery still sitting in the composer.
    let fake = Fake::new("exhausted");
    assert!(!fake.send("please fix", &mut Vec::new()).submitted);
    std::fs::write(fake.dir.join("mode"), "immediate").unwrap();
    // When the caller resends the same pending reply.
    let delivery = fake.send("please fix", &mut Vec::new());
    // Then only Enter is sent this time.
    assert!(delivery.submitted);
    assert_eq!(delivery.attempts, 1);
    assert_eq!(fake.count("pastes"), 1);
}

#[test]
fn resend_checks_composer_before_native_prompt() {
    // Given matching pending input and a native transport that would accept another prompt.
    let fake = Fake::new("agent_ok");
    std::fs::write(fake.dir.join("composer"), "please fix").unwrap();
    // When the same pending prompt is resent through the public agent API.
    let delivery = fake.herdr.agent_prompt("w1:p1", "please fix").unwrap();
    // Then submission uses only Enter, never a native prompt or another paste.
    assert!(delivery.submitted);
    assert_eq!(delivery.via, DeliveryVia::SendText);
    assert_eq!(delivery.attempts, 1);
    assert_eq!(fake.count("prompts"), 0);
    assert_eq!(fake.count("pastes"), 0);
    assert_eq!(fake.count("enters"), 1);
}

#[test]
fn fallback_reuses_text_pasted_by_refused_native_attempt() {
    // Given a native prompt that pastes before reporting it is not ready.
    let fake = Fake::new("native_pasted");
    // When fallback begins after that refused native attempt.
    let delivery = fake.herdr.agent_prompt("w1:p1", "please fix").unwrap();
    // Then fallback observes the existing text and sends only Enter.
    assert!(delivery.submitted);
    assert_eq!(delivery.via, DeliveryVia::SendText);
    assert_eq!(delivery.attempts, 1);
    assert_eq!(fake.count("prompts"), 1);
    assert_eq!(fake.count("pastes"), 1);
    assert_eq!(fake.count("enters"), 1);
}

#[test]
fn collapsed_pending_paste_refuses_duplicate_native_or_terminal_write() {
    // Given a composer with an opaque multiline paste whose identity cannot be verified.
    let fake = Fake::new("agent_ok");
    std::fs::write(fake.dir.join("composer"), "[Pasted text #1 +2 lines]").unwrap();
    // When a pending multiline prompt is resent.
    let delivery = fake.herdr.agent_prompt("w1:p1", "first\nsecond").unwrap();
    // Then it remains pending without appending text or claiming success.
    assert!(!delivery.submitted);
    assert_eq!(delivery.attempts, 0);
    assert!(delivery.error.is_some());
    assert_eq!(fake.count("prompts"), 0);
    assert_eq!(fake.count("pastes"), 0);
    assert_eq!(fake.count("enters"), 0);
}

#[test]
fn submission_during_backoff_does_not_send_extra_enter() {
    // Given delayed acceptance after the first Enter's verification window.
    let fake = Fake::new("exhausted");
    let mut waits = 0;
    // When acceptance occurs at the exact backoff signal, not a wall-clock deadline.
    let delivery = fake
        .herdr
        .send_text_with_wait("w1:p1", "hello", true, &mut |_| {
            waits += 1;
            if waits == POLLS {
                std::fs::remove_file(fake.dir.join("composer")).unwrap();
            }
        });
    // Then the final read notices it and avoids Enter into a fresh composer.
    assert!(delivery.submitted);
    assert_eq!(delivery.attempts, 1);
    assert_eq!(fake.count("enters"), 1);
}

#[test]
fn delayed_multiline_paste_is_fully_visible_before_enter() {
    // Given a paste that becomes visible only on its third read.
    let fake = Fake::new("delayed");
    let mut waits = Vec::new();
    // When multiline text is delivered.
    let delivery = fake.send("first line\nsecond line", &mut waits);
    // Then delivery waits on deterministic reads, not elapsed time.
    assert!(delivery.submitted);
    assert_eq!(fake.count("visibility_reads"), 3);
    assert_eq!(waits, vec![POLL_INTERVAL; 2]);
    assert_eq!(fake.count("enters"), 1);
}

#[test]
fn transport_errors_preserve_failure_metadata() {
    // Given errors before paste, during paste, on Enter, or while verifying Enter.
    for (mode, attempts) in [
        ("read_error", 0),
        ("paste_error", 0),
        ("enter_error", 1),
        ("verify_error", 1),
    ] {
        let fake = Fake::new(mode);
        // When delivery encounters the failed subprocess.
        let delivery = fake.send("please fix", &mut Vec::new());
        // Then the failure remains recordable rather than masquerading as submission.
        assert!(!delivery.submitted, "{mode}");
        assert_eq!(delivery.attempts, attempts, "{mode}");
        assert!(delivery.error.as_deref().unwrap().contains("transport"));
    }
}

#[test]
fn semantic_transport_errors_return_delivery_without_fallback() {
    // Given a prompt transport error unrelated to agent readiness.
    let fake = Fake::new("agent_error");
    // When the public prompt API is called.
    let delivery = fake.herdr.agent_prompt("w1:p1", "please fix").unwrap();
    // Then callers can record the failed attempt, with no risky terminal fallback.
    assert!(!delivery.submitted);
    assert_eq!(delivery.via, DeliveryVia::AgentPrompt);
    assert_eq!(delivery.attempts, 1);
    assert!(delivery.error.is_some());
    assert_eq!(fake.count("pastes"), 0);
}

#[test]
fn paste_without_submit_has_no_enter_attempts() {
    // Given an empty composer.
    let fake = Fake::new("immediate");
    // When the public paste-only API is called.
    let delivery = fake.herdr.pane_send_text("w1:p1", "hello", false).unwrap();
    // Then visibility is verified but submission is explicitly false.
    assert!(!delivery.submitted);
    assert_eq!(delivery.attempts, 0);
    assert_eq!(delivery.error, None);
    assert_eq!(fake.count("enters"), 0);
}

#[test]
fn last_marker_excludes_history_and_accepts_multiline_collapsing() {
    // Given terminal snapshots containing old prompts and different input styles.
    let cases = [
        (
            "❯ old\nanswer\n❯ first line\nsecond line\n",
            "first line\nsecond line",
            true,
        ),
        (
            "> old\nanswer\n> first line second line\n",
            "first line\nsecond line",
            true,
        ),
        ("❯ hello\nanswer\n❯ \n", "hello", false),
        ("> hello\nanswer\n> \n", "hello", false),
        ("❯ first line\n", "first line\nsecond line", false),
        ("history hello\nstatus\n", "hello", false),
        ("history\nhello\n\n", "hello", true),
        ("❯something else\n", "hello", false),
    ];
    for (screen, text, expected) in cases {
        // When the input heuristic inspects the snapshot.
        let present = input_contains(screen, text);
        // Then only full normalized text in the last composer counts.
        assert_eq!(present, expected, "{screen:?}");
    }
}
