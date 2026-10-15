use super::*;

fn routing_ws(rules: &str) -> Ws {
    let ws = init_ws();
    std::fs::write(
        ws.root.join(".corral/corral.toml"),
        format!(
            r#"
[projects.app]
path = "app"
[harnesses.fixture]
command = "fixture {{model_args}}"
flags = "claude"
[profiles.omo]
harness = "fixture"
[profiles.task]
harness = "fixture"
model = "task-model"
[profiles.rule]
harness = "fixture"
model = "rule-model"
[profiles.missing]
harness = "fixture"
model = "missing-model"
{rules}
"#
        ),
    )
    .expect("config");
    let cache = serde_json::json!({
        "detected_at": "2026-01-01T00:00:00Z",
        "harnesses": [{
            "key": "fixture",
            "program": "fixture",
            "path": "/fixture/harness",
            "version": "1.0",
            "models": ["task-model", "rule-model", "cli-model"],
            "model_source": "listed",
            "efforts": [],
            "takes_effort": true
        }]
    });
    std::fs::write(
        ws.root.join(".corral/state/harnesses.json"),
        serde_json::to_vec(&cache).expect("cache json"),
    )
    .expect("cache");
    ws
}

#[test]
fn new_kind_size_roundtrip() {
    // Given an initialized workspace.
    let ws = init_ws();
    // When a classified task is filed.
    ok(
        &ws.root,
        &[
            "new",
            "Classified",
            "-p",
            "app",
            "--kind",
            "backend",
            "--size",
            "m",
        ],
    );
    // Then both metadata fields survive storage and appear in CLI views.
    let show: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["show", "1", "--json"])).expect("json");
    assert_eq!(show["kind"], "backend");
    assert_eq!(show["size"], "m");
    let list: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["list", "--json"])).expect("json");
    assert_eq!(list[0]["kind"], "backend");
    assert_eq!(list[0]["size"], "m");
    let text = ok(&ws.root, &["show", "1"]);
    assert!(text.contains("kind:     backend"), "{text}");
    assert!(text.contains("size:     m"), "{text}");
}

#[test]
fn new_rejects_invalid_kind_and_size_before_writing() {
    let ws = init_ws();
    for flag in ["--kind", "--size"] {
        // Given no tasks, when an invalid classification is supplied.
        let out = corral(&ws.root, &["new", "Invalid", flag, "bogus"]);
        // Then argument parsing rejects it and leaves storage untouched.
        assert_eq!(out.status.code(), Some(2));
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(error.contains(flag) && error.contains("bogus"), "{error}");
    }
    let list: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["list", "--json"])).expect("json");
    assert_eq!(list, serde_json::json!([]));
}

#[test]
fn route_text_includes_unset_defaults_and_source() {
    // Given a task with no routing metadata.
    let ws = routing_ws("");
    ok(&ws.root, &["new", "Unclassified", "-p", "app"]);
    // When routing is explained.
    let text = ok(&ws.root, &["route", "1"]);
    // Then the complete selection is visible, including unset values.
    for field in [
        "profile: omo",
        "harness: fixture",
        "model:   default",
        "effort:  default",
        "source:  fallback omo",
    ] {
        assert!(text.contains(field), "{text}");
    }
}

#[test]
fn route_json_explains_matching_rule() {
    // Given a classified task and a matching rule.
    let ws = routing_ws(
        r#"
[[routing.rules]]
name = "backend work"
kind = "backend"
size = "m"
profile = "rule"
"#,
    );
    ok(
        &ws.root,
        &[
            "new",
            "Classified",
            "-p",
            "app",
            "--kind",
            "backend",
            "--size",
            "m",
        ],
    );
    // When JSON routing is requested.
    let route: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["route", "1", "--json"])).expect("json");
    // Then the CLI emits the RouteDecision contract directly.
    assert_eq!(
        route,
        serde_json::json!({
            "profile": "rule", "harness": "fixture", "model": "rule-model",
            "effort": null, "route": {"source": "rule", "index": 1, "name": "backend work"},
            "warnings": []
        })
    );
}

#[test]
fn config_edits_preserve_model_routing_rules() {
    // Given a matching routing rule and a classified task.
    let ws =
        routing_ws("[[routing.rules]]\nname = 'backend'\nkind = 'backend'\nprofile = 'rule'\n");
    ok(
        &ws.root,
        &[
            "new",
            "Config integration",
            "-p",
            "app",
            "--kind",
            "backend",
            "--size",
            "s",
        ],
    );
    let before: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["route", "1", "--json"])).expect("route");

    // When an unrelated value changes through main's validated config write API.
    ok(&ws.root, &["config", "set", "limits.running", "4"]);

    // Then routing still selects the same named rule and profile.
    let after: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["route", "1", "--json"])).expect("route");
    assert_eq!(before, after);
    assert_eq!(after["route"]["name"], "backend");
}

#[test]
fn route_explicit_override_beats_task_and_rule() {
    // Given a task profile and a catch-all rule.
    let ws = routing_ws("[[routing.rules]]\nprofile = \"rule\"\n");
    ok(
        &ws.root,
        &["new", "Override", "-p", "app", "--profile", "task"],
    );
    // When an explicit profile, model and effort are supplied.
    let route: serde_json::Value = serde_json::from_str(&ok(
        &ws.root,
        &[
            "route",
            "1",
            "--profile",
            "omo",
            "--model",
            "cli-model",
            "--effort",
            "high",
            "--json",
        ],
    ))
    .expect("json");
    // Then all explicit overrides win.
    assert_eq!(route["profile"], "omo");
    assert_eq!(route["model"], "cli-model");
    assert_eq!(route["effort"], "high");
    assert_eq!(route["route"]["source"], "override");
}

#[test]
fn unavailable_rule_falls_through_with_visible_warning() {
    // Given the first matching rule names an unavailable model.
    let ws = routing_ws(
        "[[routing.rules]]\nprofile = \"missing\"\n[[routing.rules]]\nprofile = \"rule\"\n",
    );
    ok(&ws.root, &["new", "Fallthrough", "-p", "app"]);
    // When routing is explained.
    let route: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["route", "1", "--json"])).expect("json");
    // Then the second rule wins and the skipped selection is explained.
    assert_eq!(route["profile"], "rule");
    assert_eq!(route["route"]["index"], 2);
    let warnings = route["warnings"].as_array().expect("warnings");
    assert!(!warnings.is_empty());
    let text = ok(&ws.root, &["route", "1"]);
    for warning in warnings {
        assert!(
            text.contains(warning.as_str().expect("warning text")),
            "{text}"
        );
    }
}

#[test]
fn dispatch_explain_does_not_start_a_run() {
    // Given a backlog task in a workspace with no herdr.
    let ws = routing_ws("");
    ok(&ws.root, &["new", "Dry run", "-p", "app"]);
    let before = ok(&ws.root, &["show", "1", "--json"]);
    // When dispatch is explained with overrides.
    let explained = ok(
        &ws.root,
        &["dispatch", "1", "--explain", "--profile", "rule", "--json"],
    );
    // Then it prints the same decision as route, without durable task or worktree changes.
    assert_eq!(
        explained,
        ok(&ws.root, &["route", "1", "--profile", "rule", "--json"])
    );
    assert_eq!(before, ok(&ws.root, &["show", "1", "--json"]));
    assert!(!ws.root.join("app/.worktrees").exists());
    assert_eq!(
        ok(&ws.root, &["dispatch", "1", "--explain"]),
        ok(&ws.root, &["route", "1"])
    );
}

#[cfg(unix)]
fn dispatch_with_herdr(root: &Path, args: &[&str]) -> Output {
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("herdr-fixture");
    std::fs::write(&bin, r#"#!/bin/sh
case "$1 $2" in
  "workspace create")
    printf '%s\n' '{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1"}}}' ;;
  "pane run")
    printf '%s\n' "$4" > "$(dirname "$0")/launch"
    printf '%s\n' '{"result":{"type":"ok"}}' ;;
  "pane report-metadata") printf '%s\n' '{"result":{"type":"ok"}}' ;;
  *) exit 1 ;;
esac
"#).expect("write herdr fixture");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    Command::new(env!("CARGO_BIN_EXE_corral"))
        .env_remove("CORRAL_ROOT")
        .env_remove("CORRAL_HERDR_SESSION")
        .env("HERDR_BIN_PATH", &bin)
        .env("HOME", root.join("home"))
        .args(["--root", root.to_str().expect("utf8"), "dispatch", "1"])
        .args(args)
        .current_dir(root)
        .output()
        .expect("dispatch")
}

#[cfg(unix)]
#[test]
fn dispatch_records_matched_route_and_launches_resolved_preset() {
    // Given a real git project and a herdr subprocess fixture that captures launch argv.
    let ws = routing_ws(
        "[[routing.rules]]\nname = \"backend\"\nkind = \"backend\"\nprofile = \"rule\"\n",
    );
    let mut store = corral_core::Store::open(&ws.root).expect("store");
    store
        .config
        .profiles
        .get_mut("rule")
        .expect("profile")
        .effort = Some("high".into());
    store.save_config().expect("save");
    ok(
        &ws.root,
        &[
            "new",
            "Routed run",
            "-p",
            "app",
            "--ready",
            "--kind",
            "backend",
            "--size",
            "s",
        ],
    );
    // When dispatch runs through the CLI with no profile overrides.
    let out = dispatch_with_herdr(&ws.root, &[]);

    // Then the durable run and event retain the matched rule alongside the rendered model/effort.
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let task: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["show", "1", "--json"])).expect("json");
    let run = &task["runs"][0];
    assert_eq!(task["status"], "running");
    assert_eq!(run["agent"], "rule");
    assert_eq!(run["harness"], "fixture");
    assert_eq!(run["model"], "rule-model");
    assert_eq!(run["effort"], "high");
    assert_eq!(
        run["route"],
        serde_json::json!({"source":"rule","index":1,"name":"backend"})
    );
    let launch = std::fs::read_to_string(ws.root.join("launch")).expect("launch");
    assert_eq!(launch.trim(), "fixture --model rule-model --effort high");
    let events = store.events().expect("events");
    let event = events
        .iter()
        .find(|e| e.kind == "dispatched")
        .expect("dispatch event");
    assert_eq!(event.detail["route"], run["route"]);
    assert!(ws.root.join("app/.worktrees/T-001/README.md").is_file());
    let stats: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["stats", "--by", "kind", "--json"])).expect("stats");
    assert_eq!(stats[0]["group"], "backend");
    assert_eq!(stats[0]["runs"], 1);
}

#[cfg(unix)]
fn exact_model_ws(model: &str, discovered: bool) -> Ws {
    let ws = routing_ws("[[routing.rules]]\nprofile = 'rule'\n");
    let mut store = corral_core::Store::open(&ws.root).expect("store");
    store
        .config
        .harnesses
        .get_mut("fixture")
        .expect("harness")
        .flags = Some("omp".into());
    store
        .config
        .profiles
        .get_mut("rule")
        .expect("profile")
        .model = Some(model.into());
    store.save_config().expect("config");
    let path = ws.root.join(".corral/state/harnesses.json");
    let mut cache: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("cache")).expect("json");
    cache["harnesses"][0]["models"] = if discovered {
        serde_json::json!([
            "anthropic/claude-3-5-sonnet-20240620",
            "openai-codex/gpt-6.1-sol-fast"
        ])
    } else {
        serde_json::json!([])
    };
    cache["harnesses"][0]["model_source"] = if discovered { "listed" } else { "free_text" }.into();
    cache["harnesses"][0]["notes"] = if discovered {
        serde_json::json!([])
    } else {
        serde_json::json!(["models --json failed"])
    };
    std::fs::write(path, serde_json::to_vec(&cache).expect("json")).expect("cache");
    ok(&ws.root, &["new", "Exact model", "-p", "app", "--ready"]);
    ws
}

#[cfg(unix)]
#[test]
fn dispatch_rejects_fuzzy_profile_and_override_ids_with_close_matches() {
    for (model, args) in [
        ("anthropic/claude-sonnet-5-5", vec!["--profile", "rule"]),
        (
            "ignored",
            vec!["--profile", "omo", "--model", "openai-codex/gpt-6.1-sol"],
        ),
        (
            "ignored",
            vec!["--profile", "omo", "--model", "gpt-6.1-sol-fast"],
        ),
        (
            "ignored",
            vec!["--profile", "omo", "--model", "openai/gpt-6.1-sol-fast"],
        ),
    ] {
        // Given a successful catalog containing only exact provider-qualified selectors.
        let ws = exact_model_ws(model, true);
        // When an unlisted profile/override is dispatched.
        let out = dispatch_with_herdr(&ws.root, &args);
        // Then failure names close entries and happens before git or herdr launch side effects.
        let error = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success());
        assert!(
            error.contains("not an exact discovered model ID"),
            "{error}"
        );
        assert!(error.contains("close matches:"), "{error}");
        assert!(
            error.contains("anthropic/claude-3-5-sonnet-20240620"),
            "{error}"
        );
        assert!(error.contains("openai-codex/gpt-6.1-sol-fast"), "{error}");
        assert!(!ws.root.join("app/.worktrees").exists());
        assert!(!ws.root.join("launch").exists());
    }
}

#[cfg(unix)]
#[test]
fn dispatch_accepts_an_exact_provider_qualified_model() {
    // Given an exact discovered provider/model.
    let ws = exact_model_ws("openai-codex/gpt-6.1-sol-fast", true);
    // When that profile is dispatched.
    let out = dispatch_with_herdr(&ws.root, &["--profile", "rule"]);
    // Then the exact selector reaches the harness unchanged.
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(ws.root.join("launch"))
            .expect("launch")
            .trim(),
        "fixture --model openai-codex/gpt-6.1-sol-fast"
    );
}

#[cfg(unix)]
#[test]
fn dispatch_rechecks_omo_settings_against_exact_discovery() {
    // Given enabled settings retaining a stale model that the harness no longer lists.
    let model = "anthropic-subscription/claude-sonnet-5-5";
    let ws = exact_model_ws(model, true);
    let mut store = corral_core::Store::open(&ws.root).expect("store");
    store
        .config
        .harnesses
        .get_mut("fixture")
        .expect("harness")
        .flags = Some("omo".into());
    let agent = ws.root.join("agent");
    std::fs::create_dir(&agent).expect("agent");
    std::fs::write(
        agent.join("settings.json"),
        serde_json::to_vec(&serde_json::json!({"enabledModels":[model]})).expect("json"),
    )
    .expect("settings");
    store.config.sync.omo_agent_dir = Some(agent);
    store.save_config().expect("config");
    let decision: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["route", "1", "--json"])).expect("route");
    assert_eq!(decision["model"], model);
    assert_eq!(decision["warnings"], serde_json::json!([]));

    // When dispatch validates the final selection, then the known catalog rejects it.
    let out = dispatch_with_herdr(&ws.root, &[]);
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains(model) && error.contains("close matches:"),
        "{error}"
    );
    assert!(!ws.root.join("app/.worktrees").exists());
    assert!(!ws.root.join("launch").exists());
}

#[cfg(unix)]
#[test]
fn failed_discovery_warns_and_allows_routing_and_dispatch() {
    // Given a failed model probe, not a successful catalog rejecting this model.
    let ws = exact_model_ws("anthropic/claude-sonnet-5-5", false);
    let decision: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["route", "1", "--json"])).expect("route json");
    assert_eq!(decision["profile"], "rule");
    assert_eq!(decision["warnings"].as_array().expect("warnings").len(), 1);
    // When the automatic route is dispatched.
    let out = dispatch_with_herdr(&ws.root, &[]);
    // Then it launches, with a warning that the model is unverified.
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("model discovery unavailable"), "{stderr}");
    assert!(stderr.contains("allowing unverified model"), "{stderr}");
    assert_eq!(stderr.matches("allowing unverified model").count(), 1);
    assert!(ws.root.join("launch").is_file());
}
