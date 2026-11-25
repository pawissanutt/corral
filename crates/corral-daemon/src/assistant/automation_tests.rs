use super::*;
use corral_engine::{Engine, testing::FakeRunner};

fn fixture() -> (tempfile::TempDir, Shared) {
    let dir = tempfile::Builder::new()
        .prefix("auto-")
        .tempdir_in("/tmp")
        .unwrap();
    Engine::init(dir.path(), None).unwrap();
    let shared = Shared::new(
        dir.path().to_path_buf(),
        Some(std::sync::Arc::new(|| Box::new(FakeRunner::new()))),
    )
    .unwrap();
    (dir, shared)
}

fn save_rule(shared: &Shared, enabled: bool, instructions: &str) {
    fs::write(
        shared.root().join(crate::STATE_DIR).join("corral.toml"),
        format!(
            r#"
[automation.agents.agent]
profile = "pi"
instructions = "{instructions}"
[automation.rules.rule]
agent = "agent"
enabled = {enabled}
event = "blocked"
"#
        ),
    )
    .unwrap();
}

fn fire(shared: &Shared, id: Option<&str>) -> Result<String> {
    deliver(shared, "rule", "pi", id, "fire-1", "untrusted trigger")
}

#[test]
fn allocation_survives_failed_launch_and_never_claims_owner_chat() {
    let (_dir, shared) = fixture();
    save_rule(&shared, true, "saved policy");
    let manual = create(&shared, false, None, "pi".into(), None).unwrap();
    assert!(fire(&shared, Some(&manual.id())).is_err());
    assert_eq!(allocated_chat(&shared, "rule").unwrap(), None);

    // FakeRunner does not support process launch. Retry must keep the allocation.
    assert!(fire(&shared, None).is_err());
    let id = allocated_chat(&shared, "rule").unwrap().unwrap();
    assert_ne!(id, manual.id());
    assert!(fire(&shared, None).is_err());
    assert_eq!(
        allocated_chat(&shared, "rule").unwrap().as_deref(),
        Some(id.as_str())
    );
    let chat = require(&shared, &id).unwrap();
    assert_eq!(
        launch_policy(&chat, &load_config(&shared).unwrap()).unwrap(),
        Some(("rule", "saved policy"))
    );
    assert_eq!(chat.row(&shared).unwrap().title, "Automation: rule");
}

#[test]
fn stopped_chat_stays_stopped_and_changed_policy_rotates_cold_chat() {
    let (_dir, shared) = fixture();
    save_rule(&shared, true, "first policy");
    assert!(fire(&shared, None).is_err());
    let first = allocated_chat(&shared, "rule").unwrap().unwrap();
    terminate(&shared, &first).unwrap();
    assert!(fire(&shared, Some(&first)).unwrap_err().is::<Skip>());
    save_rule(&shared, true, "second policy");
    assert!(fire(&shared, Some(&first)).unwrap_err().is::<Skip>());
    assert!(relaunch(&shared, &first).is_err());
    assert!(
        require(&shared, &first)
            .unwrap()
            .meta
            .automation
            .unwrap()
            .stopped
    );

    // Simulate the persisted owner-resumed flag without requiring a real host.
    set_stopped(&shared, &mut require(&shared, &first).unwrap(), false).unwrap();
    assert!(fire(&shared, Some(&first)).is_err());
    let second = allocated_chat(&shared, "rule").unwrap().unwrap();
    assert_ne!(first, second);
    assert!(require(&shared, &first).unwrap().meta.ended);
    let chat = require(&shared, &second).unwrap();
    assert_eq!(
        launch_policy(&chat, &load_config(&shared).unwrap()).unwrap(),
        Some(("rule", "second policy"))
    );
    save_rule(&shared, false, "second policy");
    assert!(!fire(&shared, None).unwrap_err().is::<Skip>());
    assert!(launch_policy(&chat, &load_config(&shared).unwrap()).is_err());
}
