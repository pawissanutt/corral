//! `corral assistant permissions` (T-656 grant-store): the listing's grant states, the running
//! generation's settings beside the saved ones with each allow entry's resolution, and
//! revocation from the owner's shell, the session's own shell and another assistant's shell.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use corral_core::Store;
use corral_core::agent::CapabilitySnapshot;
use corral_core::agent::write_permissions::{
    Invocation, NewPermission, OperationKind, PermissionChoice, PermissionPath, WriteOperation,
    WriteTool,
};
use corral_ops::agent::{BindTarget, open_generation};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    proj: PathBuf,
}

impl Fixture {
    /// Sessions A-1 (generation 1 open) and A-2, a home with an allow-listed skills directory
    /// and a link to another directory, and a project outside the home.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let (root, home, proj) = (base.join("ws"), base.join("home"), base.join("work/proj"));
        for path in [
            &root,
            &proj,
            &home.join(".agents/skills"),
            &home.join("real"),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        std::os::unix::fs::symlink(home.join("real"), home.join("link")).unwrap();
        let fx = Self {
            _dir: dir,
            root,
            home,
            proj,
        };
        fx.ok(None, &["init"]);
        fx.ok(None, &["assistant", "create"]);
        fx.ok(None, &["assistant", "create"]);
        fx
    }

    fn run(&self, assistant: Option<&str>, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("HERDR_BIN_PATH", "/nonexistent/herdr");
        if let Some(session) = assistant {
            cmd.env("CORRAL_ASSISTANT", session);
        }
        cmd.args(args)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, assistant: Option<&str>, args: &[&str]) -> String {
        let out = self.run(assistant, args);
        assert!(
            out.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn json(&self, assistant: Option<&str>, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(assistant, args)).unwrap()
    }

    fn store(&self) -> Store {
        Store::open(&self.root).unwrap()
    }

    fn listing(&self) -> Value {
        self.json(None, &["assistant", "permissions", "A-1", "--json"])
    }

    fn state(&self, grant: &str) -> Value {
        let listing = self.listing();
        let rows = listing["grants"].as_array().unwrap();
        rows.iter().find(|row| row["id"] == grant).unwrap().clone()
    }

    fn events(&self, kind: &str) -> Vec<Value> {
        self.store()
            .events()
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == kind)
            .map(|event| event.detail)
            .collect()
    }
}

fn decision(n: u32, choice: PermissionChoice, scope: &Path) -> NewPermission {
    let path = |p: &Path| PermissionPath {
        shown: p.display().to_string(),
        canonical: p.display().to_string(),
    };
    NewPermission {
        generation: 1,
        turn: None,
        request: format!("rq-{n:032x}"),
        tool: WriteTool::Write,
        operation: WriteOperation {
            kind: OperationKind::NewFile,
            replacements: None,
        },
        target: path(&scope.join("a.txt")),
        digest: "d".repeat(64),
        invocation: Invocation {
            generation: 1,
            id: format!("{n:032x}"),
            tool_call: format!("call-{n}"),
        },
        choice,
        scope: (choice != PermissionChoice::Once).then(|| path(scope)),
    }
}

/// A-1 generation 1 runs with write_permissions settings and holds one grant of each state:
/// G-1 hour (active), G-2 once (consumed), G-3 hour made two hours ago (expired), G-4 session
/// (revoked) and G-5 and G-6 session (active).
fn seed(fx: &Fixture) {
    let store = fx.store();
    let target = BindTarget::Assistant {
        session: "A-1".into(),
    };
    open_generation(&store, &target, "w1:p1").unwrap();
    let now = Timestamp::now();
    let earlier = now.checked_sub(SignedDuration::from_hours(2)).unwrap();
    store
        .update_assistant_container("A-1", |c| {
            let generation = c.state.generations.last_mut().unwrap();
            let s = generation.capabilities.clone().unwrap();
            let mut features = s.features.clone();
            features.insert(
                "write_permissions".into(),
                serde_json::from_value(json!({"enabled": true, "source": "preset",
                    "settings": {"allow": ["~/.agents/skills", "~/link", "~/missing"],
                        "deny": ["~/.agents/skills/corral"]}}))
                .unwrap(),
            );
            generation.capabilities = Some(CapabilitySnapshot::with_settings(
                s.role,
                s.preset,
                features,
                s.web,
                s.mcp,
                s.project_text,
                s.llama_cpp,
            ));
            let grants = [
                (PermissionChoice::Hour, now),
                (PermissionChoice::Once, now),
                (PermissionChoice::Hour, earlier),
                (PermissionChoice::Session, now),
                (PermissionChoice::Session, now),
                (PermissionChoice::Session, now),
            ];
            for (n, (choice, at)) in (1..).zip(grants) {
                c.record_permission("A-1", decision(n, choice, &fx.proj), at)
                    .unwrap();
            }
            c.revoke_write_grants("A-1", Some("G-4"), "owner", now, false)
                .unwrap();
            c.permission_audit.clear();
            Ok(())
        })
        .unwrap();
}

#[test]
fn the_listing_shows_every_grant_state_and_each_allow_entry_resolution() {
    // Given A-1 with one grant in each state and recorded write_permissions settings.
    let fx = Fixture::new();
    seed(&fx);
    // When the owner lists A-1.
    let listing = fx.listing();
    // Then each record carries its state.
    let states: Vec<(&str, &str)> = listing["grants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["id"].as_str().unwrap(), row["state"].as_str().unwrap()))
        .collect();
    assert_eq!(
        states,
        [
            ("G-1", "active"),
            ("G-2", "consumed"),
            ("G-3", "expired"),
            ("G-4", "revoked"),
            ("G-5", "active"),
            ("G-6", "active"),
        ]
    );
    // And the generation's allow entries resolve, or say why they are ignored.
    let settings = &listing["generation_settings"];
    assert_eq!(settings["enabled"], true);
    assert_eq!(settings["deny"], json!(["~/.agents/skills/corral"]));
    let allow = settings["allow"].as_array().unwrap();
    assert_eq!(allow[0]["canonical"], json!(fx.home.join(".agents/skills")));
    assert_eq!(allow[1]["canonical"], json!(fx.home.join("real")));
    assert_eq!(
        allow[2]["ignored"],
        "does not resolve to an existing directory"
    );
    // And the saved configuration, which does not register the key yet, differs.
    assert_eq!(listing["saved_settings"], Value::Null);
    assert_eq!(listing["saved_differs"], true);
    // When ~/link is relinked to /, then the entry is flagged as ignored.
    std::fs::remove_file(fx.home.join("link")).unwrap();
    std::os::unix::fs::symlink("/", fx.home.join("link")).unwrap();
    let relinked = fx.listing();
    let entry = &relinked["generation_settings"]["allow"][1];
    assert_eq!(
        entry["ignored"],
        "resolves to /, which is the root directory"
    );
    assert!(entry.get("canonical").is_none());
    let text = fx.ok(None, &["assistant", "permissions", "A-1"]);
    assert!(text.contains("G-3 expired hour"), "{text}");
    assert!(
        text.contains("allow ~/link ignored: resolves to /"),
        "{text}"
    );
}

#[test]
fn the_owner_and_the_session_revoke_while_another_assistant_is_refused() {
    let fx = Fixture::new();
    seed(&fx);
    // When the owner revokes G-1 from their shell.
    let revoked = fx.json(
        None,
        &["assistant", "permissions", "revoke", "A-1", "G-1", "--json"],
    );
    assert_eq!(revoked["revoked"], json!(["G-1"]));
    // Then the record says when and by whom, and the audit line is logged under its key.
    let g1 = fx.state("G-1");
    assert_eq!(
        (&g1["state"], &g1["revoked_by"]),
        (&json!("revoked"), &json!("owner"))
    );
    assert!(g1["revoked_at"].is_string());
    let lines = fx.events("assistant_write_permission_revoked");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["key"], "G-1:revoked");
    // When A-1's own shell revokes G-5, then it is recorded as revoked by A-1.
    fx.ok(
        Some("A-1"),
        &["assistant", "permissions", "revoke", "A-1", "G-5"],
    );
    assert_eq!(fx.state("G-5")["revoked_by"], "A-1");
    // When A-2's shell tries to revoke A-1's grants, then it is refused and nothing changes.
    let before = fx.store().get_assistant_container("A-1").unwrap();
    let refused = fx.run(
        Some("A-2"),
        &["assistant", "permissions", "revoke", "A-1", "--all"],
    );
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("assistant_refused"));
    assert_eq!(fx.store().get_assistant_container("A-1").unwrap(), before);
    // And an inactive grant cannot be revoked again.
    let again = fx.run(None, &["assistant", "permissions", "revoke", "A-1", "G-1"]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("G-1 is revoked"));
    // When the owner ends A-1, then its remaining active grant shows as ended.
    fx.ok(None, &["assistant", "end", "A-1"]);
    assert_eq!(fx.state("G-6")["state"], "ended");
    assert_eq!(fx.listing()["ended"], true);
}
