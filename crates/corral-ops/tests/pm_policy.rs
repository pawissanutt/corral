//! Policy delivery through the real Herdr adapter, never an owner-configured engine.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use corral_core::{AgentCmd, OperatingAuthority, OperatingSelection, Status, Store};
use corral_herdr::Herdr;
use corral_ops::{Ctx, open_pm, pm_policy_health, reconcile_pm_policy};
use serde_json::{Value, json};

const ENGINE: &str = r#"#!/usr/bin/env python3
import json, pathlib, sys
root = pathlib.Path(__file__).parent
args = sys.argv[1:]
if args[:1] == ["--session"]:
    args = args[2:]
with (root / "calls.jsonl").open("a") as log:
    log.write(json.dumps(args) + "\n")
def result(value):
    print(json.dumps({"result": value}))
def fail(code):
    print(json.dumps({"error": {"code": code, "message": "fixture refusal"}}))
    sys.exit(1)
verb = args[:2]
if verb == ["workspace", "create"]:
    if (root / "pi-check").exists():
        assert not (root / ".corral/state/agent-pm.json").exists()
    result({"workspace": {"workspace_id": "pm"}, "root_pane":
        {"workspace_id": "pm", "tab_id": "pm:t1", "pane_id": "pm:p1"}})
elif verb == ["pane", "get"]:
    result({"pane": {"pane_id": args[2], "workspace_id": "pm"}})
elif verb == ["pane", "read"]:
    if (root / "read-fail").exists(): fail("read_failed")
    if (root / "collapsed").exists():
        print("> [paste #1 1200 chars]")
        sys.exit(0)
    print("> ")
elif verb == ["agent", "prompt"]:
    if (root / "collapse-on-prompt").exists():
        (root / "collapsed").touch()
        fail("agent_not_ready")
    if (root / "no-agent").exists(): fail("agent_not_ready")
    if (root / "ambiguous").exists(): sys.exit(1)
    with (root / "prompts.jsonl").open("a") as log:
        log.write(json.dumps({"pane": args[2], "text": args[3]}) + "\n")
    result({})
elif verb == ["pane", "run"]:
    if (root / "pi-check").exists():
        state = root / ".corral/state"
        pm = json.loads((state / "agent-pm.json").read_text())
        saved = json.loads((state / "pm.json").read_text())
        policy = json.loads((state / "pm-policy.json").read_text())
        assert pm["pane"] == args[2] == saved["pane_id"]
        assert policy["panes"][args[2]]["disposition"] == "submitting"
        (root / "launch-observed.json").write_text(json.dumps(pm))
    result({})
elif verb in [["pane", "report-metadata"],
              ["workspace", "focus"], ["agent", "focus"]]:
    result({})
else:
    fail("unexpected_call")
"#;

struct Fx {
    tmp: tempfile::TempDir,
    ctx: Ctx,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("app/.git")).unwrap();
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.agents.insert(
            "fake".into(),
            AgentCmd {
                command: "true {prompt_file}".into(),
            },
        );
        store.config.pm.agent = "fake".into();
        store.config.pm.push_events = false;
        store.save_config().unwrap();
        let bin = tmp.path().join("herdr");
        std::fs::write(&bin, ENGINE).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ctx = Ctx {
            store,
            herdr: Herdr::with_bin(bin, Some("fixture-only".into())),
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        };
        Self { tmp, ctx }
    }

    fn fresh(&self) -> Ctx {
        Ctx {
            store: Store::open_unobserved(self.tmp.path()).unwrap(),
            herdr: self.ctx.herdr.clone(),
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        }
    }

    fn pi(&mut self) {
        let assets = self.tmp.path().join("assets");
        for module in ["roles/pm", "delivery", "auth"] {
            let dir = assets.join("src").join(module);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("index.ts"), "export {}").unwrap();
        }
        std::fs::write(assets.join("compat.json"), r#"{"verdict":"GO"}"#).unwrap();
        self.ctx.store.config.agent.assets_dir = Some(assets);
        self.ctx.store.config.profiles.insert(
            "pm-pi".into(),
            serde_json::from_value(json!({
                "harness":"pi-native", "model":"gpt-fixture", "vendor":"openai"
            }))
            .unwrap(),
        );
        self.ctx.store.config.pm.agent = "pm-pi".into();
        self.ctx.store.save_config().unwrap();
        self.flag("pi-check");
    }

    fn remember(&self, pane: &str) {
        std::fs::write(
            self.ctx.store.state_dir().join("pm.json"),
            serde_json::to_vec(&json!({"pane_id":pane})).unwrap(),
        )
        .unwrap();
    }

    fn flag(&self, name: &str) {
        std::fs::write(self.tmp.path().join(name), "").unwrap();
    }

    fn lines(&self, name: &str) -> Vec<Value> {
        std::fs::read_to_string(self.tmp.path().join(name))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn calls(&self, verb: &[&str]) -> Vec<Value> {
        self.lines("calls.jsonl")
            .into_iter()
            .filter(|call| call[0] == verb[0] && call[1] == verb[1])
            .collect()
    }

    fn saved(&self) -> Value {
        parse_policy(&std::fs::read_to_string(self.ctx.store.prompts_dir().join("pm.md")).unwrap())
    }

    fn receipt(&self, pane: &str) -> Value {
        pm_policy_health(&self.ctx).unwrap()["panes"][pane].clone()
    }

    fn grant(&self) -> String {
        let task = self
            .ctx
            .store
            .create("included", Some("app"), Status::Ready, "finite")
            .unwrap();
        let selection = OperatingSelection {
            tasks: [task.id.clone()].into(),
            authority: OperatingAuthority {
                dispatch: true,
                accept: false,
                plan_accept: true,
                aggregate_accept: false,
            },
            ..Default::default()
        };
        let preview = self.ctx.store.preview_operating_policy(&selection).unwrap();
        self.ctx
            .store
            .confirm_operating_policy(
                &preview,
                "fixture owner",
                "2026-10-18T12:00:00Z".parse().unwrap(),
            )
            .unwrap();
        task.id
    }
}

fn parse_policy(text: &str) -> Value {
    let block = text.rsplit_once("```json\n").unwrap().1;
    serde_json::from_str(block.split_once("\n```").unwrap().0).unwrap()
}

#[test]
fn live_reuse_saves_and_submits_current_scoped_policy_without_events() {
    // Given a remembered pane, disabled event pushes, and a stale cached Assist config.
    let fx = Fx::new();
    fx.remember("pm:p1");
    let id = fx.grant();
    fx.ctx
        .store
        .create("future", Some("app"), Status::Ready, "not delegated")
        .unwrap();
    std::fs::write(fx.ctx.store.prompts_dir().join("pm.md"), "obsolete").unwrap();
    // When opening the existing PM.
    assert_eq!(open_pm(&fx.ctx).unwrap(), "pm:p1");
    // Then saved instructions and actual transport agree on exact finite authority.
    let policy = fx.saved();
    assert_eq!(policy["mode"], "autopilot");
    assert_eq!(policy["scope"]["tasks"].as_object().unwrap().len(), 1);
    assert!(policy["scope"]["tasks"].get(&id).is_some());
    assert_eq!(
        policy["authority"],
        json!({"dispatch":true,"accept":false,"plan_accept":true})
    );
    assert_eq!(policy["paused"], false);
    assert!(!policy["revision"].as_str().unwrap().is_empty());
    assert_eq!(
        parse_policy(fx.lines("prompts.jsonl")[0]["text"].as_str().unwrap()),
        policy
    );
    assert_eq!(fx.receipt("pm:p1")["current"], true);
    assert!(fx.calls(&["pane", "run"]).is_empty());
    assert!(!fx.ctx.store.state_dir().join("pm-events.json").exists());
}

#[test]
fn confirmed_policy_is_not_repeated_after_reopen() {
    // Given one confirmed delivery.
    let fx = Fx::new();
    fx.remember("pm:p1");
    open_pm(&fx.ctx).unwrap();
    // When another process opens and reconciles the same pane/revision.
    let fresh = fx.fresh();
    open_pm(&fresh).unwrap();
    reconcile_pm_policy(&fresh, true).unwrap();
    // Then there was only one submission and no worker or shell restart.
    assert_eq!(fx.calls(&["agent", "prompt"]).len(), 1);
    assert!(fx.calls(&["pane", "run"]).is_empty());
}

#[test]
fn scoped_lifecycle_changes_do_not_redeliver_policy() {
    // Given a confirmed finite grant.
    let fx = Fx::new();
    fx.remember("pm:p1");
    let id = fx.grant();
    open_pm(&fx.ctx).unwrap();
    let before = fx.saved();
    // When lifecycle changes the scoped task's permissions without changing policy.
    for status in [Status::Running, Status::Review, Status::Done] {
        fx.ctx
            .store
            .update(&id, |task| {
                task.status = status;
                Ok(())
            })
            .unwrap();
        reconcile_pm_policy(&fx.fresh(), true).unwrap();
    }
    // Then fresh instructions reflect progress without another PM wake.
    let after = fx.saved();
    assert_eq!(before["revision"], after["revision"]);
    assert_ne!(before["permissions"], after["permissions"]);
    assert_ne!(before["refusals"], after["refusals"]);
    assert_eq!(fx.calls(&["agent", "prompt"]).len(), 1);
    assert_eq!(fx.receipt("pm:p1")["current"], true);
}

#[test]
fn policy_revision_change_delivers_once_across_repeated_syncs() {
    // Given a confirmed Assist policy and then a real finite grant.
    let fx = Fx::new();
    fx.remember("pm:p1");
    open_pm(&fx.ctx).unwrap();
    let before = fx.saved();
    fx.grant();
    // When several fresh processes reconcile the changed policy.
    for _ in 0..3 {
        reconcile_pm_policy(&fx.fresh(), true).unwrap();
    }
    // Then the new revision caused exactly one further delivery.
    assert_ne!(before["revision"], fx.saved()["revision"]);
    assert_eq!(fx.calls(&["agent", "prompt"]).len(), 2);
    assert_eq!(fx.receipt("pm:p1")["current"], true);
}

#[test]
fn legacy_snapshot_fingerprint_does_not_trigger_an_upgrade_wake() {
    // Given a confirmed receipt written using the old whole-snapshot identity.
    use sha2::{Digest, Sha256};
    let fx = Fx::new();
    fx.remember("pm:p1");
    open_pm(&fx.ctx).unwrap();
    let path = fx.ctx.store.state_dir().join("pm-policy.json");
    let mut journal: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let policy = &journal["panes"]["pm:p1"]["snapshot"];
    journal["panes"]["pm:p1"]["fingerprint"] = json!(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(policy).unwrap())
    ));
    std::fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
    // When a fresh process reconciles the unchanged policy.
    reconcile_pm_policy(&fx.fresh(), true).unwrap();
    // Then compatibility uses the saved snapshot, not the obsolete digest.
    assert_eq!(fx.calls(&["agent", "prompt"]).len(), 1);
    assert_eq!(fx.receipt("pm:p1")["current"], true);
}

#[test]
fn legacy_collapsed_not_submitted_receipt_blocks_repeated_syncs() {
    // Given a collapsed paste recorded as NotSubmitted by an older process.
    let fx = Fx::new();
    fx.remember("pm:p1");
    open_pm(&fx.ctx).unwrap();
    let path = fx.ctx.store.state_dir().join("pm-policy.json");
    let mut journal: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    journal["panes"]["pm:p1"]["disposition"] = json!("not_submitted");
    journal["panes"]["pm:p1"]["delivery"] = json!({
        "via":"send_text", "submitted":false, "attempts":0,
        "error":"cannot verify collapsed paste in pane input; refusing another paste"
    });
    std::fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
    // When the composer has cleared and several processes sync, even a newer policy.
    fx.ctx.store.pause_operating_policy(true).unwrap();
    for _ in 0..3 {
        reconcile_pm_policy(&fx.fresh(), true).unwrap();
    }
    // Then the one original delivery is never replayed and uncertainty remains visible.
    assert_eq!(fx.calls(&["agent", "prompt"]).len(), 1);
    assert_eq!(fx.receipt("pm:p1")["uncertain"], true);
    assert_eq!(fx.receipt("pm:p1")["current"], false);
}

#[test]
fn collapsed_delivery_is_uncertain_even_without_enter_attempts() {
    // Given an opaque composer that the real Herdr adapter cannot verify.
    let fx = Fx::new();
    fx.remember("pm:p1");
    fx.flag("collapse-on-prompt");
    open_pm(&fx.ctx).unwrap();
    // When that composer clears and subsequent processes sync.
    std::fs::remove_file(fx.tmp.path().join("collapsed")).unwrap();
    std::fs::remove_file(fx.tmp.path().join("collapse-on-prompt")).unwrap();
    for _ in 0..3 {
        reconcile_pm_policy(&fx.fresh(), true).unwrap();
    }
    // Then zero Enter attempts are not treated as permission to paste again.
    let receipt = fx.receipt("pm:p1");
    assert_eq!(receipt["record"]["delivery"]["attempts"], 0);
    assert_eq!(receipt["uncertain"], true);
    assert_eq!(fx.calls(&["agent", "prompt"]).len(), 1);
    assert!(fx.calls(&["pane", "send-text"]).is_empty());
}

#[test]
fn owning_reconcile_delivers_pause_from_strict_current_bytes() {
    // Given a live PM that already received its grant.
    let fx = Fx::new();
    fx.remember("pm:p1");
    let id = fx.grant();
    open_pm(&fx.ctx).unwrap();
    fx.ctx.store.pause_operating_policy(true).unwrap();
    // When sync's policy hook runs with the old context and notifications disabled.
    reconcile_pm_policy(&fx.ctx, true).unwrap();
    // Then both instructions and update contain the current paused permissions.
    let policy = fx.saved();
    assert_eq!(policy["paused"], true);
    assert_eq!(policy["permissions"][&id], json!([]));
    assert_eq!(fx.lines("prompts.jsonl").len(), 2);
    assert_eq!(
        parse_policy(fx.lines("prompts.jsonl")[1]["text"].as_str().unwrap()),
        policy
    );
}

#[test]
fn unconfirmed_zero_attempt_delivery_is_not_retried_after_reopen() {
    // Given a fallback transport that could not read input and made zero Enter attempts.
    let fx = Fx::new();
    fx.remember("pm:p1");
    fx.flag("no-agent");
    fx.flag("read-fail");
    open_pm(&fx.ctx).unwrap();
    assert_eq!(fx.receipt("pm:p1")["record"]["disposition"], "uncertain");
    assert_eq!(fx.receipt("pm:p1")["record"]["delivery"]["attempts"], 0);
    std::fs::remove_file(fx.tmp.path().join("no-agent")).unwrap();
    std::fs::remove_file(fx.tmp.path().join("read-fail")).unwrap();
    // When a new process reconciles.
    reconcile_pm_policy(&fx.fresh(), true).unwrap();
    // Then the incomplete transport evidence never authorizes an automatic replay.
    assert_eq!(fx.calls(&["agent", "prompt"]).len(), 1);
    assert!(fx.lines("prompts.jsonl").is_empty());
    assert_eq!(fx.receipt("pm:p1")["uncertain"], true);
    assert_eq!(fx.receipt("pm:p1")["current"], false);
}

#[test]
fn ambiguous_native_delivery_is_not_blindly_replayed() {
    // Given a native prompt attempt whose result was lost.
    let fx = Fx::new();
    fx.remember("pm:p1");
    fx.flag("ambiguous");
    open_pm(&fx.ctx).unwrap();
    std::fs::remove_file(fx.tmp.path().join("ambiguous")).unwrap();
    fx.ctx.store.pause_operating_policy(true).unwrap();
    // When a fresh process opens and reconciles a newer policy.
    open_pm(&fx.fresh()).unwrap();
    reconcile_pm_policy(&fx.fresh(), true).unwrap();
    // Then uncertainty remains visible and the new saved prompt does not trigger a replay.
    assert_eq!(fx.calls(&["agent", "prompt"]).len(), 1);
    assert_eq!(fx.saved()["paused"], true);
    assert_eq!(fx.receipt("pm:p1")["uncertain"], true);
    assert_eq!(fx.receipt("pm:p1")["current"], false);
}

#[test]
fn crash_submission_marker_survives_reopen_without_replay() {
    // Given the durable journal bytes that a crash during IPC leaves behind.
    let fx = Fx::new();
    fx.remember("pm:p1");
    open_pm(&fx.ctx).unwrap();
    let path = fx.ctx.store.state_dir().join("pm-policy.json");
    let mut journal: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    journal["panes"]["pm:p1"]["disposition"] = json!("submitting");
    journal["panes"]["pm:p1"]["delivery"] = Value::Null;
    std::fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
    // When another process reopens and runs the owning hook.
    reconcile_pm_policy(&fx.fresh(), true).unwrap();
    // Then the crash is uncertain, not evidence that a second submission is safe.
    assert_eq!(fx.calls(&["agent", "prompt"]).len(), 1);
    assert_eq!(fx.receipt("pm:p1")["uncertain"], true);
}

#[test]
fn fresh_pm_starts_current_prompt_without_redundant_update() {
    // Given no remembered PM and a current scoped grant.
    let fx = Fx::new();
    let id = fx.grant();
    // When opening and then reconciling the newly started PM.
    open_pm(&fx.ctx).unwrap();
    reconcile_pm_policy(&fx.fresh(), true).unwrap();
    // Then startup carries current instructions and no separate update is sent.
    assert_eq!(fx.calls(&["pane", "run"]).len(), 1);
    assert!(fx.calls(&["agent", "prompt"]).is_empty());
    assert!(fx.saved()["scope"]["tasks"].get(&id).is_some());
    assert_eq!(fx.receipt("pm:p1")["current"], true);
}

#[test]
fn delivery_confirmation_is_per_pane() {
    // Given confirmation for one pane.
    let fx = Fx::new();
    fx.remember("pm:p1");
    open_pm(&fx.ctx).unwrap();
    fx.remember("pm:p2");
    // When a replacement pane is reused at the same revision.
    open_pm(&fx.fresh()).unwrap();
    // Then that pane receives its own update and both receipts remain durable.
    assert_eq!(fx.lines("prompts.jsonl").len(), 2);
    assert_eq!(fx.lines("prompts.jsonl")[1]["pane"], "pm:p2");
    assert_eq!(fx.receipt("pm:p1")["current"], true);
    assert_eq!(fx.receipt("pm:p2")["current"], true);
}

#[test]
fn unavailable_transport_keeps_policy_due_until_reopen() {
    // Given a remembered pane while the engine is unavailable.
    let fx = Fx::new();
    fx.remember("pm:p1");
    reconcile_pm_policy(&fx.ctx, false).unwrap();
    // When availability returns in another process.
    reconcile_pm_policy(&fx.fresh(), true).unwrap();
    // Then it receives current policy rather than a checkpoint that skipped delivery.
    assert_eq!(fx.lines("prompts.jsonl").len(), 1);
    assert_eq!(fx.receipt("pm:p1")["current"], true);
}

#[test]
fn delivery_health_is_read_only_and_transport_free() {
    // Given a confirmed delivery journal.
    let fx = Fx::new();
    fx.remember("pm:p1");
    open_pm(&fx.ctx).unwrap();
    let path = fx.ctx.store.state_dir().join("pm-policy.json");
    let before = std::fs::read(&path).unwrap();
    let calls = fx.lines("calls.jsonl");
    // When brief reads health repeatedly.
    let first = pm_policy_health(&fx.ctx).unwrap();
    let second = pm_policy_health(&fx.ctx).unwrap();
    // Then neither state nor transport was touched.
    assert_eq!(first, second);
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(fx.lines("calls.jsonl"), calls);
}

#[test]
fn malformed_current_policy_never_sends_cached_authority() {
    // Given a cached valid grant and then malformed current config bytes.
    let fx = Fx::new();
    fx.grant();
    fx.remember("pm:p1");
    std::fs::write(fx.ctx.store.config_path(), "[automation\n").unwrap();
    // When opening or syncing from the stale context.
    assert!(open_pm(&fx.ctx).is_err());
    assert!(reconcile_pm_policy(&fx.ctx, true).is_err());
    // Then no cached grant is sent through either transport.
    assert!(fx.calls(&["agent", "prompt"]).is_empty());
    assert!(fx.calls(&["pane", "run"]).is_empty());
}

#[test]
fn pi_pm_launch_binds_the_actual_pane_before_policy_startup_and_run() {
    // Given an approved single-project scope and complete offline Pi assets.
    let mut fx = Fx::new();
    fx.pi();
    fx.grant();
    // When opening the Pi PM through the real launch and policy services.
    assert_eq!(open_pm(&fx.ctx).unwrap(), "pm:p1");
    // Then the fake engine observed binding, startup policy and saved identity before run.
    let observed: Value =
        serde_json::from_slice(&std::fs::read(fx.tmp.path().join("launch-observed.json")).unwrap())
            .unwrap();
    assert_eq!(observed["state"]["binding"]["project"], "app");
    assert_eq!(observed["state"]["binding"]["role"], "pm");
    assert_eq!(observed["state"]["binding"]["generation"], 1);
    let dir = corral_ops::agent::launch::generation_dir(
        &fx.ctx.store,
        &corral_core::inbox::Recipient::Pm,
        1,
    )
    .unwrap();
    let bootstrap: Value =
        serde_json::from_slice(&std::fs::read(dir.join("bootstrap.json")).unwrap()).unwrap();
    assert_eq!(bootstrap["pane"], "pm:p1");
    assert_eq!(bootstrap["model"], "gpt-fixture");
    assert!(bootstrap["task"].is_null() && bootstrap["run"].is_null());
    assert_eq!(fx.receipt("pm:p1")["current"], true);
    assert_eq!(fx.calls(&["pane", "run"]).len(), 1);
    assert!(fx.calls(&["agent", "prompt"]).is_empty());
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

#[test]
fn pi_pm_preflight_refuses_missing_assets_unapproved_and_ambiguous_scopes() {
    // Given complete assets but no approval, or an approved multi-project scope.
    for variant in ["assets", "unapproved", "ambiguous"] {
        let mut fx = Fx::new();
        fx.pi();
        if variant == "assets" {
            fx.grant();
            std::fs::remove_file(fx.tmp.path().join("assets/src/roles/pm/index.ts")).unwrap();
        } else if variant == "ambiguous" {
            std::fs::create_dir_all(fx.tmp.path().join("other/.git")).unwrap();
            fx.ctx.store.config.projects.insert(
                "other".into(),
                serde_json::from_value(json!({"path":"other"})).unwrap(),
            );
            fx.ctx.store.save_config().unwrap();
            let first = fx.grant();
            let other = fx
                .ctx
                .store
                .create("other", Some("other"), Status::Ready, "")
                .unwrap();
            let selection = OperatingSelection {
                tasks: [first, other.id].into(),
                ..Default::default()
            };
            let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
            fx.ctx
                .store
                .confirm_operating_policy(&preview, "fixture owner", jiff::Timestamp::now())
                .unwrap();
        }
        // When launch preflight runs, then it refuses before any pane or policy startup.
        let error = open_pm(&fx.fresh()).unwrap_err().to_string();
        let code = match variant {
            "assets" => "role_unavailable",
            "unapproved" => "pm_scope_unapproved",
            _ => "pm_scope_ambiguous",
        };
        assert!(error.contains(code), "{variant}: {error}");
        assert!(fx.calls(&["workspace", "create"]).is_empty());
        assert!(fx.calls(&["pane", "run"]).is_empty());
        assert!(!fx.ctx.store.state_dir().join("agent-pm.json").exists());
    }
}

#[test]
fn omo_rollback_preserves_pi_pm_records_and_current_policy() {
    // Given a Pi PM with durable binding and existing task state.
    let mut fx = Fx::new();
    fx.pi();
    let task = fx.grant();
    open_pm(&fx.ctx).unwrap();
    use corral_core::agent::{ContainerRef, paths, pm_state};
    use corral_ops::agent::{self, CallResponse};
    let dir = agent::launch::generation_dir(&fx.ctx.store, &corral_core::inbox::Recipient::Pm, 1)
        .unwrap();
    let bootstrap: Value =
        serde_json::from_slice(&std::fs::read(dir.join("bootstrap.json")).unwrap()).unwrap();
    let store_id = paths::store_id(&fx.ctx.store);
    let envelope = |op: &str, request: String, payload: Value, hello: bool| {
        let mut auth = json!({"host_cap":"e".repeat(64)});
        if hello {
            auth["bind_nonce"] = bootstrap["bind_nonce"].clone();
        }
        serde_json::to_vec(&json!({
            "version":1,"op":op,"request_id":request,
            "payload_sha256":agent::payload_sha256(&payload),"payload":payload,"auth":auth,
            "identity":{"store":store_id,"project":"app","role":"pm",
                "session_id":"rollback-session","generation":1},
        }))
        .unwrap()
    };
    let hello = envelope(
        "hello",
        agent::event_request_id(1, 0),
        json!({
            "ctl_cap_sha256":paths::sha256_hex("d".repeat(64).as_bytes()),
            "session_file":paths::container_sessions_dir(&fx.ctx.store, &ContainerRef::Pm).join("session.jsonl"),
            "pid":4242,"start_time":"123456","pane":"pm:p1"
        }),
        true,
    );
    assert!(matches!(
        agent::call(&fx.ctx, "hello", &hello).unwrap(),
        CallResponse::Receipt { .. }
    ));
    let comment = envelope(
        "comment",
        agent::tool_request_id("rollback-session", "before"),
        json!({"task":task,"text":"retained"}),
        false,
    );
    assert!(matches!(
        agent::call(&fx.ctx, "comment", &comment).unwrap(),
        CallResponse::Receipt { .. }
    ));
    let before = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    let inbox =
        corral_core::inbox::Inbox::new(&fx.ctx.store, corral_core::inbox::Recipient::Pm).unwrap();
    inbox
        .post(corral_core::inbox::Post {
            kind: "question_wake",
            urgency: corral_core::inbox::Urgency::Wake,
            author: "corral",
            body: "pending",
            binding: Default::default(),
            subject: None,
            max_payload: corral_core::inbox::MAX_PAYLOAD,
        })
        .unwrap();
    let queued = inbox.messages().unwrap();
    drop(inbox);
    let task_before = std::fs::read(fx.ctx.store.get(&task).unwrap().path).unwrap();
    std::fs::remove_file(fx.ctx.store.state_dir().join("pm.json")).unwrap();
    std::fs::remove_file(fx.tmp.path().join("pi-check")).unwrap();
    fx.ctx.store.config.pm.agent = "fake".into();
    fx.ctx.store.save_config().unwrap();
    // When the owner switches back to the legacy PM.
    open_pm(&fx.fresh()).unwrap();
    // Then records survive and OmO retains the approved policy startup path.
    let after = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    assert_eq!(after.state.receipts, before.state.receipts);
    assert_eq!(after.state.questions, before.state.questions);
    assert!(after.state.generation(1).unwrap().fenced.is_some());
    assert_eq!(
        corral_core::inbox::Inbox::new(&fx.ctx.store, corral_core::inbox::Recipient::Pm)
            .unwrap()
            .messages()
            .unwrap(),
        queued
    );
    let comment = envelope(
        "comment",
        agent::tool_request_id("rollback-session", "after"),
        json!({"task":task,"text":"forbidden"}),
        false,
    );
    let refused = agent::call(&fx.fresh(), "comment", &comment).unwrap();
    assert_eq!(refused.rejection().unwrap().code.as_str(), "agent_fenced");
    assert_eq!(
        std::fs::read(fx.ctx.store.get(&task).unwrap().path).unwrap(),
        task_before
    );
    assert_eq!(fx.receipt("pm:p1")["current"], true);
    assert_eq!(fx.calls(&["pane", "run"]).len(), 2);
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}
