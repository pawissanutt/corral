use std::process::{Command, Output, Stdio};

use corral_core::{Status, Store};

struct Fixture {
    root: tempfile::TempDir,
    store: Store,
    id: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("app/.git")).unwrap();
        let store = Store::init(root.path()).unwrap();
        let id = store
            .create("Finite work", Some("app"), Status::Backlog, "Agreed work")
            .unwrap()
            .id;
        Self { root, store, id }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        command
            .env_clear()
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("XDG_CACHE_HOME", self.root.path().join("cache"))
            .current_dir(self.root.path())
            .stdin(Stdio::null())
            .arg("--root")
            .arg(self.root.path())
            .arg("automation");
        command
    }

    fn preview(&self) -> serde_json::Value {
        let output = self
            .command()
            .args(["preview", "--task", &self.id, "--dispatch"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        let json = text.split("\nExact confirmation digest:").next().unwrap();
        serde_json::from_str(json).unwrap()
    }

    fn confirm(&self, digest: &str, marker: Option<(&str, &str)>) -> Output {
        let mut command = self.command();
        if let Some((key, value)) = marker {
            command.env(key, value);
        }
        command
            .args([
                "confirm",
                "--task",
                &self.id,
                "--dispatch",
                "--digest",
                digest,
            ])
            .output()
            .unwrap()
    }
}

#[test]
fn automation_preview_is_read_only_and_exposes_exact_digest() {
    // Given an existing task and no owner grant.
    let fixture = Fixture::new();
    let config = std::fs::read(fixture.store.config_path()).unwrap();
    // When the finite selection is previewed through the CLI.
    let preview = fixture.preview();
    // Then the exact identity and authority are visible without mutation.
    assert_eq!(preview["id"].as_str().unwrap().len(), 64);
    assert_eq!(preview["policy"]["authority"]["dispatch"], true);
    assert!(
        preview["policy"]["scope"]["tasks"]
            .get(&fixture.id)
            .is_some()
    );
    assert_eq!(std::fs::read(fixture.store.config_path()).unwrap(), config);
    assert!(fixture.store.operating_state().unwrap().approval.is_none());
}

#[test]
fn automation_confirmation_rejects_noninteractive_self_grant() {
    // Given the correct preview digest but no terminal owner input.
    let fixture = Fixture::new();
    let preview = fixture.preview();
    // When an automated caller tries to confirm.
    let output = fixture.confirm(preview["id"].as_str().unwrap(), None);
    // Then passing a digest is not consent.
    assert!(!output.status.success());
    assert!(fixture.store.operating_state().unwrap().approval.is_none());
}

#[test]
fn automation_pm_and_agent_markers_cannot_claim_owner() {
    // Given an exact preview and a PM or agent process.
    let fixture = Fixture::new();
    let preview = fixture.preview();
    // When each process attempts owner confirmation.
    for marker in [
        ("CORRAL_PM", "1"),
        ("AI_AGENT", "senpi"),
        ("PI_CODING_AGENT", "true"),
    ] {
        let output = fixture.confirm(preview["id"].as_str().unwrap(), Some(marker));
        // Then authority remains absent.
        assert!(!output.status.success());
        assert!(fixture.store.operating_state().unwrap().approval.is_none());
    }
}

#[test]
fn automation_confirmation_rejects_changed_selection_digest() {
    // Given a digest from a different selection.
    let fixture = Fixture::new();
    // When confirm receives that digest.
    let output = fixture.confirm(&"0".repeat(64), None);
    // Then no authority is published.
    assert!(!output.status.success());
    assert!(fixture.store.operating_state().unwrap().approval.is_none());
}

#[test]
fn automation_has_no_owner_override_flag() {
    // Given an agent seeking a caller-controlled owner classification.
    let fixture = Fixture::new();
    // When an owner override is supplied.
    let output = fixture
        .command()
        .args(["confirm", "--author", "owner"])
        .output()
        .unwrap();
    // Then the parser refuses the unsupported override.
    assert!(!output.status.success());
    assert!(fixture.store.operating_state().unwrap().approval.is_none());
}
