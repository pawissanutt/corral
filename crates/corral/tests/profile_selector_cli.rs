//! Selector commands run with private files, null stdin and no executable providers/engine.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use corral_core::{Status, Store};
use serde_json::{Value, json};

struct Fixture {
    temp: tempfile::TempDir,
    id: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        for dir in [
            ".corral", "home", "config", "data", "state", "cache", "runtime", "bin",
        ] {
            std::fs::create_dir(temp.path().join(dir)).unwrap();
        }
        std::fs::create_dir(temp.path().join(".corral/tasks")).unwrap();
        std::fs::write(
            temp.path().join(".corral/corral.toml"),
            r#"
[harnesses.fixture]
command = "must-not-run {model_args} {prompt_file}"
flags = "omo"

[profiles.database]
harness = "fixture"
model = "openai/fixture-v1"
effort = "high"

[profiles.database.metadata]
display_name = "Database"
purpose = "Transaction correctness"
revision = "database-r1"
instructions = "Trace rollback boundaries"

[profiles.database.metadata.guidance]
use_when = ["Persistence correctness dominates"]
avoid_when = ["Prose only"]
positive_examples = ["Repair rollback"]
negative_examples = ["Document SQL"]
distinctions = ["General follows established patterns"]

[profiles.database.metadata.eligibility]
enabled = true
automatic = true
roles = ["implementation", "planning"]
projects = []

[pm]
agent = "database"

[artifacts]
max_bytes = 4096
"#,
        )
        .unwrap();
        let store = Store::open_unobserved(temp.path()).unwrap();
        let id = store
            .create(
                "Repair rollback",
                None,
                Status::Backlog,
                "Preserve atomicity",
            )
            .unwrap()
            .id;
        Self { temp, id }
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_corral"))
            .env_clear()
            .env("HOME", self.root().join("home"))
            .env("XDG_CONFIG_HOME", self.root().join("config"))
            .env("XDG_DATA_HOME", self.root().join("data"))
            .env("XDG_STATE_HOME", self.root().join("state"))
            .env("XDG_CACHE_HOME", self.root().join("cache"))
            .env("XDG_RUNTIME_DIR", self.root().join("runtime"))
            .env("PATH", self.root().join("bin"))
            .current_dir(self.root())
            .arg("--root")
            .arg(self.root())
            .arg("selection")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.cli(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{args:?}: {:?}", output.stderr);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn reject(&self, args: &[&str]) -> Value {
        let output = self.cli(args);
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {:?}", output.stdout);
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert!(error["error"].is_string(), "{error}");
        error
    }

    fn receipt(&self) -> Value {
        let catalog = self.ok(&["catalog", &self.id, "--role", "implementation"]);
        assert_eq!(catalog["task"]["id"], self.id);
        assert_eq!(
            catalog["eligible"]["database"]["metadata"]["guidance"]["avoid_when"],
            json!(["Prose only"])
        );
        json!({
            "version":catalog["version"], "id":"selection-1", "task":self.id,
            "actor":"proposer", "selected_at":"2026-10-19T00:00:00Z",
            "role":catalog["role"], "profile":"database",
            "profile_revision":catalog["eligible"]["database"]["profile_revision"],
            "spec_revision":catalog["spec_revision"], "catalog_revision":catalog["catalog_revision"],
            "reason":"Transaction correctness dominates", "closest_alternative":null,
            "distinction":null, "review":null
        })
    }

    fn write(&self, input: &Value) -> PathBuf {
        let file = self.root().join("selection.json");
        std::fs::write(&file, serde_json::to_vec(input).unwrap()).unwrap();
        file
    }
}

#[test]
fn catalog_and_save_round_trip_without_pins_authority_or_external_commands() {
    let fixture = Fixture::new();
    let input = fixture.receipt();
    let store = Store::open_unobserved(fixture.root()).unwrap();
    let task_before = store.get(&fixture.id).unwrap();
    let config_before = std::fs::read(fixture.root().join(".corral/corral.toml")).unwrap();
    let events_before = std::fs::read(fixture.root().join(".corral/events.jsonl")).unwrap();
    let file = fixture.write(&input);
    for _ in 0..2 {
        assert_eq!(
            fixture.ok(&["save", "--file", file.to_str().unwrap(), "--json"]),
            input
        );
    }
    let receipts = store.auto_profile_selections().unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(serde_json::to_value(&receipts[0]).unwrap(), input);
    assert_eq!(store.get(&fixture.id).unwrap(), task_before);
    assert_eq!(
        std::fs::read(fixture.root().join(".corral/corral.toml")).unwrap(),
        config_before
    );
    assert_eq!(
        std::fs::read(fixture.root().join(".corral/events.jsonl")).unwrap(),
        events_before
    );
    let planning = fixture.ok(&["catalog", &fixture.id, "--role", "planning", "--json"]);
    assert_eq!(planning["role"], "planning");
}

#[test]
fn structured_failures_reject_stale_ineligible_review_and_malformed_inputs() {
    let fixture = Fixture::new();
    let input = fixture.receipt();
    for (field, value) in [
        ("spec_revision", json!("old")),
        ("catalog_revision", json!("old")),
        ("profile", json!("unknown")),
        ("profile_revision", json!("old")),
        ("actor", json!("owner")),
        ("role", json!("implementation_review")),
        ("role", json!("plan_review")),
        ("role", json!("pm")),
        ("extra", json!("unknown field")),
    ] {
        let mut invalid = input.clone();
        invalid[field] = value;
        let file = fixture.write(&invalid);
        assert_eq!(
            fixture.reject(&["save", "--file", file.to_str().unwrap()])["command"],
            "selection save"
        );
    }
    let file = fixture.write(&input);
    let store = Store::open_unobserved(fixture.root()).unwrap();
    let mut config = store.config.clone();
    config
        .profiles
        .get_mut("database")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap()
        .eligibility
        .automatic = false;
    // A raw valid config change must be checked without generating config-observation events.
    let path = fixture.root().join(".corral/corral.toml");
    std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
    let catalog = fixture.ok(&["catalog", &fixture.id, "--role", "implementation"]);
    assert!(catalog["eligible"].get("database").is_none());
    assert!(catalog["excluded"]["database"][0].is_string());
    fixture.reject(&["save", "--file", file.to_str().unwrap()]);
    assert!(store.auto_profile_selections().unwrap().is_empty());
    assert_eq!(
        fixture.reject(&["catalog", &fixture.id, "--role", "plan-review"])["command"],
        "selection"
    );
    fixture.reject(&["save"]);
}

#[test]
fn input_is_bounded_and_regular_and_never_reads_stdin() {
    let fixture = Fixture::new();
    let file = fixture.root().join("oversized.json");
    std::fs::write(&file, vec![b' '; 4097]).unwrap();
    assert!(
        fixture.reject(&["save", "--file", file.to_str().unwrap()])["error"]
            .as_str()
            .unwrap()
            .contains("exceeds")
    );
    fixture.reject(&["save", "--file", fixture.root().to_str().unwrap()]);
    std::fs::write(&file, b"{not json").unwrap();
    fixture.reject(&["save", "--file", file.to_str().unwrap()]);
    #[cfg(unix)]
    {
        let link = fixture.root().join("link.json");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        fixture.reject(&["save", "--file", link.to_str().unwrap()]);
        fixture.reject(&["save", "--file", "/dev/null"]);
    }
}
