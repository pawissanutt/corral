use super::*;
use corral_core::Store;
use serde_json::json;

fn reviewer_ws() -> Ws {
    let ws = init_ws();
    std::fs::write(
        ws.root.join(".corral/corral.toml"),
        r#"
[projects.app]
path = "app"
[harnesses.fixture]
command = "fixture"
[profiles.omo]
harness = "fixture"
vendor = "anthropic"
[profiles.peer]
harness = "fixture"
vendor = "openai"
"#,
    )
    .expect("config");
    ok(
        &ws.root,
        &["new", "Reviewer override", "-p", "app", "--profile", "omo"],
    );
    ws
}

#[test]
fn route_sets_reviewer_and_logs_old_and_new_values() {
    for name in ["peer", "pm", "cross-vendor", "none"] {
        // Given a task whose implementer profile is explicitly pinned.
        let ws = reviewer_ws();
        let store = Store::open(&ws.root).unwrap();
        let before = store.get("1").unwrap();
        assert_eq!(before.reviewer, None);
        assert!(
            serde_json::to_value(&before)
                .unwrap()
                .get("reviewer")
                .is_none()
        );
        // When a supported reviewer override is set through the CLI.
        let text = ok(&ws.root, &["route", "1", "--set-reviewer", name, "--json"]);
        // Then explanation still runs, while storage and the event retain the override.
        let explanation: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(explanation["profile"], "omo");
        let task = store.get("1").unwrap();
        assert_eq!(task.reviewer.as_deref(), Some(name));
        assert_eq!(task.profile, before.profile);
        let events: Vec<_> = store
            .events()
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "reviewer_override")
            .collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].task.as_deref(), Some("T-001"));
        assert_eq!(events[0].detail, json!({"old": null, "new": name}));
    }
}

#[test]
fn route_clears_reviewer_and_logs_previous_value() {
    // Given an existing override.
    let ws = reviewer_ws();
    let store = Store::open(&ws.root).unwrap();
    store
        .update("1", |task| {
            task.reviewer = Some("peer".into());
            Ok(())
        })
        .unwrap();
    // When the CLI clears it.
    let text = ok(&ws.root, &["route", "1", "--clear-reviewer", "--json"]);
    // Then the default is restored, omitted from storage, and explained.
    let explanation: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(explanation["profile"], "omo");
    let task = store.get("1").unwrap();
    assert_eq!(task.reviewer, None);
    assert_eq!(task.profile.as_deref(), Some("omo"));
    assert!(
        !std::fs::read_to_string(&task.path)
            .unwrap()
            .contains("\nreviewer:")
    );
    let event = store
        .events()
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "reviewer_override")
        .unwrap();
    assert_eq!(event.detail, json!({"old": "peer", "new": null}));
}

#[test]
fn route_rejects_unknown_reviewer_without_mutating_task_or_events() {
    // Given a persisted reviewer override and its exact task/event bytes.
    let ws = reviewer_ws();
    let store = Store::open(&ws.root).unwrap();
    let task = store
        .update("1", |task| {
            task.reviewer = Some("peer".into());
            Ok(())
        })
        .unwrap();
    let before = std::fs::read(&task.path).unwrap();
    let event_path = ws.root.join(".corral/events.jsonl");
    let events = std::fs::read(&event_path).unwrap();
    // When an unknown reviewer is supplied.
    let out = corral(
        &ws.root,
        &["route", "1", "--set-reviewer", "unknown-reviewer"],
    );
    // Then resolution fails before any mutation.
    assert!(!out.status.success());
    let error = String::from_utf8_lossy(&out.stderr);
    assert!(
        error.contains("unknown profile 'unknown-reviewer'"),
        "{error}"
    );
    assert_eq!(std::fs::read(&task.path).unwrap(), before);
    assert_eq!(std::fs::read(event_path).unwrap(), events);
}

#[test]
fn route_reviewer_flags_conflict_without_mutating_storage() {
    // Given a task with no override.
    let ws = reviewer_ws();
    let store = Store::open(&ws.root).unwrap();
    let task = store.get("1").unwrap();
    let before = std::fs::read(&task.path).unwrap();
    let events = store.events().unwrap();
    // When both mutation flags are supplied.
    let out = corral(
        &ws.root,
        &["route", "1", "--set-reviewer", "peer", "--clear-reviewer"],
    );
    // Then clap rejects the conflict before writing.
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(std::fs::read(&task.path).unwrap(), before);
    assert_eq!(store.events().unwrap(), events);
}

#[test]
fn reviewer_set_rejects_unresolvable_profile_without_mutation() {
    // Given an existing profile whose harness cannot be resolved.
    let ws = reviewer_ws();
    let mut ctx = corral_ops::Ctx::open(&ws.root).unwrap();
    let mut broken = ctx.store.config.profiles["peer"].clone();
    broken.harness = "missing-harness".into();
    ctx.store.config.profiles.insert("broken".into(), broken);
    let task = ctx.store.get("1").unwrap();
    let before = std::fs::read(&task.path).unwrap();
    let events = ctx.store.events().unwrap();
    // When the mutation service is asked to use that profile.
    let error = corral_ops::reviewer_set(&ctx, "1", Some("broken")).unwrap_err();
    // Then profile resolution fails before the task or events are changed.
    assert!(
        format!("{error:#}").contains("missing-harness"),
        "{error:#}"
    );
    assert_eq!(std::fs::read(&task.path).unwrap(), before);
    assert_eq!(ctx.store.events().unwrap(), events);
}
