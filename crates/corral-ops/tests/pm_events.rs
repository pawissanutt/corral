//! PM delivery through the real sync and herdr adapters, with an isolated fake CLI.
#![cfg(unix)] // Fixtures execute a POSIX-shebang engine.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use corral_core::Store;
use corral_herdr::Herdr;
use corral_ops::Ctx;
use serde_json::json;

const HERDR: &str = r#"#!/usr/bin/env bash
export CORRAL_ROOT="$(cd "$(dirname "$0")" && pwd)"
unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID
dir="$(dirname "$0")"
if [ "${1:-}" = "--session" ]; then shift 2; fi
case "$1 $2" in
  "workspace list")
    if [ -f "$dir/offline" ]; then exit 1; fi
    echo '{"result":{"workspaces":[]}}' ;;
  "pane list") echo '{"result":{"panes":[{"pane_id":"worker:p1","workspace_id":"worker"}]}}' ;;
  "pane get")
    printf '%s\n' "$3" >> "$dir/lookups"
    if [ -f "$dir/gone" ]; then
      echo '{"error":{"code":"pane_not_found","message":"gone"}}'; exit 1
    fi
    printf '{"result":{"pane":{"pane_id":"%s","workspace_id":"pm"}}}\n' "$3" ;;
  "agent prompt")
    # Policy replacement is mandatory and has its own delivery record. These
    # event-cursor fixtures exercise only the optional [corral] event channel.
    if [[ "$4" == "[corral-policy]"* ]]; then
      printf '%s\n' "$3" >> "$dir/policy-lookups"
      echo '{"result":{}}'; exit 0
    fi
    if [ -f "$dir/not-ready" ]; then
      echo '{"error":{"code":"agent_not_ready","message":"not an active named agent"}}'; exit 1
    fi
    if [ -f "$dir/fail" ]; then
      echo '{"error":{"code":"failed","message":"delivery failed"}}'; exit 1
    fi
    printf '%s|%s\n' "$3" "$4" >> "$dir/prompts"
    echo '{"result":{}}' ;;
  "pane send-text")
    printf '%s|%s\n' "$3" "$4" >> "$dir/text"
    printf '%s' "$4" > "$dir/composer"
    echo '{"result":{}}' ;;
  "pane read")
    printf '> '
    if [ -f "$dir/composer" ]; then printf '%s' "$(< "$dir/composer")"; fi
    printf '\n' ;;
  "pane send-keys")
    printf '%s|%s\n' "$3" "$4" >> "$dir/keys"
    if [ -f "$dir/reject-enter" ]; then
      echo '{"error":{"code":"failed","message":"Enter rejected"}}'; exit 1
    fi
    rm -f "$dir/composer"
    echo '{"result":{}}' ;;
  *) printf '{"error":{"code":"unexpected","message":"%s"}}\n' "$*"; exit 1 ;;
esac
"#;

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    bin: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("herdr");
        std::fs::write(&bin, HERDR).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.pm.push_events = true;
        store.save_config().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        // The initial configuration is fixture setup, not an event under delivery test.
        std::fs::write(store.dir().join("events.jsonl"), "").unwrap();
        Self {
            ctx: Ctx {
                resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
                store,
                herdr: Herdr::with_bin(bin.clone(), None),
            },
            bin,
            _tmp: tmp,
        }
    }

    fn pm(&self) {
        std::fs::write(
            self.ctx.store.state_dir().join("pm.json"),
            r#"{"pane_id":"pm:p1"}"#,
        )
        .unwrap();
    }

    fn flag(&self, name: &str) {
        std::fs::write(self.ctx.store.root().join(name), "").unwrap();
    }

    fn log(&self, name: &str) -> String {
        std::fs::read_to_string(self.ctx.store.root().join(name)).unwrap_or_default()
    }

    fn done(&self, id: &str) {
        self.ctx
            .store
            .log_event(Some(id), "done", json!({"run":"R1","added":3,"removed":1}))
            .unwrap();
    }

    fn sync(&self) {
        corral_ops::sync(&self.ctx).unwrap();
    }
}

#[test]
fn due_lessons_reach_the_pm_once_with_project_identity() {
    // Given an active PM and one actionable project schedule event.
    let fx = Fx::new();
    fx.ctx
        .store
        .set_attribution(corral_core::Actor::System, "hook")
        .unwrap();
    fx.pm();
    fx.ctx
        .store
        .log_event(
            None,
            "lessons_due",
            json!({"project":"app","reason":"interval"}),
        )
        .unwrap();
    // When two sync callers deliver retained events.
    fx.sync();
    fx.sync();
    // Then the event delivery receipt proves one submission to the PM.
    let deliveries: Vec<_> = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "pm_event_delivery" && e.detail["event_kind"] == "lessons_due")
        .collect();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].detail["pane"], "pm:p1");
    assert_eq!(deliveries[0].detail["submitted"], true);
    assert_eq!(deliveries[0].actor, Some(corral_core::Actor::Pm));
    assert_eq!(deliveries[0].via.as_deref(), Some("hook"));
}

#[test]
fn sync_observes_file_edits_and_only_pushes_pm_relevant_config_changes() {
    let fx = Fx::new();
    fx.pm();
    let mut doc = corral_core::ConfigDoc::load(&fx.ctx.store.config_path()).unwrap();
    doc.set_str("limits.running", "8").unwrap();
    std::fs::write(fx.ctx.store.config_path(), doc.to_string()).unwrap();
    fx.sync();
    fx.sync();
    let prompts = fx.log("prompts");
    assert_eq!(prompts.lines().count(), 1);
    assert!(
        prompts.contains("config_changed: limits.running"),
        "{prompts}"
    );
    assert!(prompts.contains("running agents unchanged"), "{prompts}");
    doc.set_str("ui.mouse", "false").unwrap();
    std::fs::write(fx.ctx.store.config_path(), doc.to_string()).unwrap();
    fx.sync();
    assert_eq!(fx.log("prompts"), prompts);
    let changes: Vec<_> = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "config_changed")
        .collect();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].detail["pm_relevant"], true);
    assert_eq!(changes[1].detail["pm_relevant"], false);
}

#[test]
fn offline_sync_observes_config_before_herdr_availability_check() {
    let fx = Fx::new();
    fx.flag("offline");
    let mut doc = corral_core::ConfigDoc::load(&fx.ctx.store.config_path()).unwrap();
    doc.set_str("limits.running", "9").unwrap();
    std::fs::write(fx.ctx.store.config_path(), doc.to_string()).unwrap();
    fx.sync();
    let changes: Vec<_> = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "config_changed")
        .collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].detail["source"], "file_edit");
    assert_eq!(changes[0].detail["changes"][0]["new"], 9);
}

#[test]
fn sync_pushes_each_event_once_only_to_the_remembered_pm() {
    // Given two actionable events and an unrelated worker pane.
    let fx = Fx::new();
    fx.pm();
    fx.done("T-001");
    fx.done("T-002");
    // When multiple sync entry points reconcile the same log.
    fx.sync();
    fx.sync();
    // Then each event is delivered once to the PM, not the worker.
    assert_eq!(
        fx.log("prompts"),
        "pm:p1|[corral] T-001 done R1 +3/-1\npm:p1|[corral] T-002 done R1 +3/-1\n"
    );
    assert!(fx.log("lookups").lines().all(|id| id == "pm:p1"));
}

#[test]
fn restarted_sync_resumes_after_the_last_successful_push() {
    // Given a successful push persisted by an earlier context.
    let fx = Fx::new();
    fx.pm();
    fx.done("T-001");
    fx.sync();
    fx.done("T-002");
    let restarted = Ctx {
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        store: Store::open(fx.ctx.store.root()).unwrap(),
        herdr: Herdr::with_bin(fx.bin.clone(), None),
    };
    // When a fresh sync process resumes.
    corral_ops::sync(&restarted).unwrap();
    // Then it delivers only the new event.
    assert_eq!(fx.log("prompts").lines().count(), 2);
    assert!(
        fx.log("prompts")
            .ends_with("pm:p1|[corral] T-002 done R1 +3/-1\n")
    );
}

#[test]
fn disabled_push_skips_events_but_not_mandatory_policy_refresh() {
    // Given push disabled with a remembered PM.
    let mut fx = Fx::new();
    fx.pm();
    fx.ctx.store.config.pm.push_events = false;
    fx.done("T-001");
    // When sync runs.
    fx.sync();
    // Then no optional event is sent, while policy replacement still targets the PM.
    assert_eq!(fx.log("prompts"), "");
    assert_eq!(fx.log("policy-lookups"), "pm:p1\n");
}

#[test]
fn enabling_push_does_not_replay_events_skipped_while_disabled() {
    // Given an event skipped while push was disabled.
    let mut fx = Fx::new();
    fx.pm();
    fx.ctx.store.config.pm.push_events = false;
    fx.done("T-001");
    fx.sync();
    fx.ctx.store.config.pm.push_events = true;
    fx.done("T-002");
    // When push is enabled.
    fx.sync();
    // Then only the new event is delivered.
    assert_eq!(fx.log("prompts"), "pm:p1|[corral] T-002 done R1 +3/-1\n");
}

#[test]
fn missing_pm_skips_events_instead_of_using_another_pane() {
    // Given no remembered PM, but a worker returned by pane list.
    let fx = Fx::new();
    fx.done("T-001");
    // When sync runs.
    fx.sync();
    // Then no pane is prompted or looked up.
    assert_eq!(fx.log("prompts"), "");
    assert_eq!(fx.log("lookups"), "");
}

#[test]
fn newly_opened_pm_does_not_receive_events_skipped_while_absent() {
    // Given an event skipped before the PM exists.
    let fx = Fx::new();
    fx.done("T-001");
    fx.sync();
    fx.pm();
    fx.done("T-002");
    // When the PM becomes live.
    fx.sync();
    // Then only the new event is delivered.
    assert_eq!(fx.log("prompts"), "pm:p1|[corral] T-002 done R1 +3/-1\n");
}

#[test]
fn stale_pm_pane_is_skipped_silently() {
    // Given a PM pane that herdr no longer has.
    let fx = Fx::new();
    fx.pm();
    fx.flag("gone");
    fx.done("T-001");
    // When sync runs.
    fx.sync();
    // Then it succeeds without prompting any pane.
    assert_eq!(fx.log("prompts"), "");
}

#[test]
fn omo_not_ready_uses_send_text_and_enter_on_the_pm_only() {
    // Given the same not-ready error that herdr returns for OmO.
    let fx = Fx::new();
    fx.pm();
    fx.flag("not-ready");
    fx.done("T-001");
    // When sync pushes and then reconciles again.
    fx.sync();
    fx.sync();
    // Then the reply fallback submits exactly once to the PM.
    assert_eq!(fx.log("text"), "pm:p1|[corral] T-001 done R1 +3/-1\n");
    assert_eq!(fx.log("keys"), "pm:p1|enter\n");
    assert_eq!(fx.log("prompts"), "");
    let events = fx.ctx.store.events().unwrap();
    let deliveries: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "pm_event_delivery")
        .collect();
    assert_eq!(
        deliveries.len(),
        1,
        "delivery accounting must not trigger a push"
    );
    assert_eq!(deliveries[0].detail["via"], "send_text");
    assert_eq!(deliveries[0].detail["submitted"], true);
    assert_eq!(deliveries[0].detail["attempts"], 1);
}

#[test]
fn unsubmitted_fallback_retains_cursor_and_retry_does_not_duplicate_composer() {
    // Given a fallback paste whose Enter is rejected, followed by another actionable event.
    let fx = Fx::new();
    fx.pm();
    fx.flag("not-ready");
    fx.flag("reject-enter");
    fx.done("T-001");
    fx.done("T-002");
    let error = corral_ops::sync(&fx.ctx).unwrap_err();
    assert!(format!("{error:#}").contains("Enter rejected"), "{error:#}");
    let events = fx.ctx.store.events().unwrap();
    let failed = events
        .iter()
        .find(|e| e.kind == "pm_event_delivery")
        .unwrap();
    assert_eq!(failed.detail["submitted"], false);
    assert_eq!(failed.detail["via"], "send_text");
    assert_eq!(failed.detail["attempts"], 1);
    assert!(!fx.ctx.store.state_dir().join("pm-events.json").exists());
    std::fs::remove_file(fx.ctx.store.root().join("reject-enter")).unwrap();

    // When the next sync resumes after Enter is restored.
    fx.sync();
    fx.sync();

    // Then both events submit in order, with no second paste of the retained first event.
    assert_eq!(fx.log("text").lines().count(), 2);
    assert_eq!(fx.log("keys").lines().count(), 3);
    let deliveries: Vec<_> = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "pm_event_delivery")
        .collect();
    assert_eq!(deliveries.len(), 3);
    assert!(
        deliveries[1..]
            .iter()
            .all(|e| e.detail["submitted"] == true)
    );
}

#[test]
fn configured_kinds_include_autopilot_blocked_and_escalated() {
    // Given a restricted push filter and raw autopilot transition events.
    let mut fx = Fx::new();
    fx.pm();
    fx.ctx.store.config.pm.push_kinds = vec!["blocked".into(), "escalated".into()];
    fx.done("T-001");
    for (id, stage) in [("T-002", "blocked"), ("T-003", "escalated")] {
        fx.ctx
            .store
            .log_event(Some(id), "autopilot", json!({"to":stage,"run":"R1"}))
            .unwrap();
    }
    // When sync runs.
    fx.sync();
    // Then only matching transitions reach the PM.
    let prompts = fx.log("prompts");
    assert_eq!(prompts.lines().count(), 2);
    assert!(prompts.contains("[corral] T-002 blocked"));
    assert!(prompts.contains("[corral] T-003 escalated"));
}

#[test]
fn concurrent_sync_owner_prevents_duplicate_delivery() {
    // Given another sync holding the shared push cursor lock.
    let fx = Fx::new();
    fx.pm();
    fx.done("T-001");
    let lock = fx.ctx.store.try_lock("pm-events").unwrap().unwrap();
    fx.sync();
    assert_eq!(fx.log("prompts"), "");
    drop(lock);
    // When the next sync owns the lock.
    fx.sync();
    // Then it pushes the pending event once.
    assert_eq!(fx.log("prompts").lines().count(), 1);
}

#[test]
fn failed_delivery_keeps_the_event_pending_for_next_sync() {
    // Given a failed transport.
    let fx = Fx::new();
    fx.pm();
    fx.flag("fail");
    fx.done("T-001");
    assert!(corral_ops::sync(&fx.ctx).is_err());
    std::fs::remove_file(fx.ctx.store.root().join("fail")).unwrap();
    // When transport is restored.
    fx.sync();
    // Then the unsent event is delivered.
    assert_eq!(fx.log("prompts").lines().count(), 1);
}

#[test]
fn offline_sync_skips_events_without_later_replaying_them() {
    // Given an unavailable herdr server.
    let fx = Fx::new();
    fx.pm();
    fx.flag("offline");
    fx.done("T-001");
    fx.sync();
    std::fs::remove_file(fx.ctx.store.root().join("offline")).unwrap();
    fx.done("T-002");
    // When herdr is available again.
    fx.sync();
    // Then only the new event is pushed.
    assert_eq!(fx.log("prompts"), "pm:p1|[corral] T-002 done R1 +3/-1\n");
}
