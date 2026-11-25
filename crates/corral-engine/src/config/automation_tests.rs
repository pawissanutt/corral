use super::*;

const EXAMPLE: &str = r#"
[automation.agents.triage]
profile = "pi"
instructions = "  Summarize what is blocking the task.  "

[automation.rules.on-blocked]
agent = "triage"
event = "blocked"
enabled = true

[automation.rules.daily_digest]
agent = "triage"
cron = "30 9 * * 1-5"
"#;

#[test]
fn defaults_and_example_round_trip() {
    let root = Path::new(".");
    let defaults = Config::check(root, "").unwrap();
    assert!(defaults.automation.is_empty());
    assert!(!toml::to_string(&defaults).unwrap().contains("[automation"));

    let config = Config::check(root, EXAMPLE).unwrap();
    assert!(config.automation.rules["on-blocked"].enabled);
    assert!(!config.automation.rules["daily_digest"].enabled);
    let encoded = toml::to_string(&config).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corral.toml");
    std::fs::write(&path, encoded).unwrap();
    assert_eq!(Config::load(&path).unwrap().automation, config.automation);
}

#[test]
fn invalid_automation_is_rejected_by_check_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corral.toml");
    for (original, replacement, key, message) in [
        (
            "agent = \"triage\"\ncron",
            "agent = \"missing\"\ncron",
            "automation.rules.daily_digest.agent",
            "unknown agent",
        ),
        (
            "\"pi\"",
            "\"missing\"",
            "automation.agents.triage.profile",
            "unknown profile",
        ),
        (
            "\"pi\"",
            "\"omp\"",
            "automation.agents.triage.profile",
            "pi harness",
        ),
        (
            "cron = \"30 9 * * 1-5\"",
            "event = \"blocked\"\ncron = \"30 9 * * 1-5\"",
            "automation.rules.daily_digest",
            "exactly one",
        ),
        (
            "cron = \"30 9 * * 1-5\"",
            "",
            "automation.rules.daily_digest",
            "exactly one",
        ),
        (
            "30 9 * * 1-5",
            "0 30 9 * * 1-5",
            "automation.rules.daily_digest.cron",
            "five",
        ),
        (
            "\"blocked\"",
            "\"done\"",
            "automation.rules.on-blocked.event",
            "blocked",
        ),
        (
            "rules.on-blocked",
            "rules.0bad",
            "automation.rules.0bad",
            "ID must match",
        ),
        (
            "  Summarize what is blocking the task.  ",
            "   ",
            "automation.agents.triage.instructions",
            "8192 bytes",
        ),
    ] {
        let invalid = EXAMPLE.replace(original, replacement);
        let problems = Config::check(dir.path(), &invalid).unwrap_err();
        assert!(
            problems.iter().any(|problem| {
                problem.key.as_deref() == Some(key) && problem.message.contains(message)
            }),
            "expected {key}: {message}, got {problems:?}"
        );
        std::fs::write(&path, invalid).unwrap();
        let error = Config::load(&path).unwrap_err().to_string();
        assert!(error.contains(key) && error.contains(message), "{error}");
    }
}
