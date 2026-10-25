use std::process::Command;

use corral_core::*;
use tempfile::tempdir;

fn command(program: impl AsRef<std::ffi::OsStr>, root: &std::path::Path) -> Command {
    let mut command = Command::new(program);
    command.env("CORRAL_ROOT", root);
    for name in [
        "CORRAL_GATE_ATTEMPT",
        "CORRAL_CANDIDATE_HEAD",
        "CORRAL_INTEGRATION_BASE",
        "HERDR_PANE_ID",
        "HERDR_WORKSPACE_ID",
        "HERDR_TAB_ID",
    ] {
        command.env_remove(name);
    }
    command
}

fn git_init(p: &std::path::Path) {
    std::fs::create_dir_all(p).unwrap();
    let ok = command("git", p)
        .arg("init")
        .arg("-q")
        .arg(p)
        .status()
        .unwrap()
        .success();
    assert!(ok);
}

#[test]
fn init_is_idempotent() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    for sub in ["db", "artifacts", "prompts", "state", "lock"] {
        assert!(s.dir().join(sub).is_dir(), "{sub}");
    }
    assert!(!s.dir().join("tasks").exists());
    assert!(s.dir().join("db/corral.db").is_file());
    assert_eq!(
        std::fs::read_to_string(s.dir().join("state/schema-version")).unwrap(),
        "4\n"
    );
    assert!(s.events().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(s.dir().join(".gitignore")).unwrap(),
        "artifacts/\nprompts/\nstate/\nlock/\ndb/\n"
    );
    let mut s = s;
    s.config.limits.running = 7;
    s.save_config().unwrap();
    s.create("x", None, Status::Backlog, "").unwrap();
    let s2 = Store::init(d.path()).unwrap();
    assert_eq!(s2.config.limits.running, 7);
    assert_eq!(s2.list().unwrap().len(), 1);
}

#[test]
fn reviewer_idle_bound_defaults_round_trips_and_rejects_zero() {
    // Given an old configuration containing only the worker idle threshold.
    let config: Config = toml::from_str("[sync]\nidle_after_secs = 120\n").unwrap();
    assert_eq!(config.sync.review_idle_after_secs, 1800);
    let d = tempdir().unwrap();
    let mut store = Store::init(d.path()).unwrap();
    // When the reviewer-specific bound is configured and reopened.
    store.config.sync.review_idle_after_secs = 2400;
    store.save_config().unwrap();
    let mut reopened = Store::open(d.path()).unwrap();
    // Then it is independent, persisted, and cannot disable the bound with zero.
    assert_eq!(reopened.config.sync.review_idle_after_secs, 2400);
    assert_eq!(reopened.config.sync.idle_after_secs, 120);
    reopened.config.sync.review_idle_after_secs = 0;
    assert!(
        reopened
            .config
            .validate(d.path())
            .unwrap_err()
            .0
            .iter()
            .any(|problem| problem.starts_with("sync.review_idle_after_secs"))
    );
}

#[test]
fn stuck_threshold_defaults_and_round_trips() {
    let config: Config = toml::from_str("[sync]\nidle_after_secs = 120\n").unwrap();
    assert_eq!(config.sync.stuck_after_secs, 300);
    let d = tempdir().unwrap();
    let mut store = Store::init(d.path()).unwrap();
    store.config.sync.stuck_after_secs = 42;
    store.save_config().unwrap();
    assert_eq!(
        Store::open(d.path()).unwrap().config.sync.stuck_after_secs,
        42
    );
}

#[test]
fn startup_grace_defaults_and_round_trips() {
    // Given an older config without the new sync field.
    let config: Config = toml::from_str("[sync]\nidle_after_secs = 120\n").unwrap();
    assert_eq!(config.sync.startup_grace_secs, 30);
    let d = tempdir().unwrap();
    let mut store = Store::init(d.path()).unwrap();
    store.config.sync.startup_grace_secs = 5;
    // When a non-default startup window is saved and reopened.
    store.save_config().unwrap();
    // Then the configured deadline survives process boundaries.
    assert_eq!(
        Store::open(d.path())
            .unwrap()
            .config
            .sync
            .startup_grace_secs,
        5
    );
}

#[test]
fn zero_startup_grace_is_rejected() {
    // Given a config that would classify newly dispatched panes gone immediately.
    let d = tempdir().unwrap();
    let mut config = Config::default_for(d.path());
    config.sync.startup_grace_secs = 0;
    // When config validation runs.
    let error = config.validate(d.path()).unwrap_err();
    // Then a positive startup window is required.
    assert!(
        error
            .0
            .iter()
            .any(|problem| problem.starts_with("sync.startup_grace_secs"))
    );
}

#[test]
fn create_and_get_variants() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    let a = s
        .create("Fix the Bug!  now", None, Status::Backlog, "body\n")
        .unwrap();
    let b = s.create("Second", Some("p"), Status::Idea, "").unwrap();
    assert_eq!(a.id, "T-001");
    assert_eq!(b.id, "T-002");
    assert_eq!(a.title, "Fix the Bug!  now");
    assert!(a.path.as_os_str().is_empty(), "a stored task has no file");
    for q in ["2", "t-2", "T-002"] {
        assert_eq!(s.get(q).unwrap().id, "T-002");
    }
    assert_eq!(s.get("1").unwrap().body, "body\n");
    assert!(
        s.get("9")
            .unwrap_err()
            .to_string()
            .contains("task T-009 not found")
    );
    assert_eq!(Store::normalize_id("42").unwrap(), "T-042");
    assert_eq!(Store::normalize_id("t-1234").unwrap(), "T-1234");
    assert!(Store::normalize_id("abc").is_err());
}

#[test]
fn long_titles_are_stored_whole() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    let title = "word ".repeat(30);
    let t = s.create(&title, None, Status::Backlog, "").unwrap();
    assert_eq!(s.get(&t.id).unwrap().title, title);
    assert_eq!(
        Store::open(d.path()).unwrap().get(&t.id).unwrap().title,
        title
    );
}

#[test]
fn parse_render_round_trip() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    let body = "# Spec\n\nline two\n\n- a\n- b\n---\nnot frontmatter\n";
    let t = s.create("Round", None, Status::Ready, body).unwrap();
    let text = Store::render_task_markdown(&t);
    assert!(text.starts_with("---\n"));
    let mut back = Store::parse_task_markdown(&text).unwrap();
    back.path = t.path.clone();
    assert_eq!(back, t);
    assert_eq!(back.body, body);
}

#[test]
fn old_task_runs_keep_legacy_delivery_and_round_trip_inbox_state() {
    // Given an old task file, including an accounting child, with no delivery fields.
    let text = r#"---
id: T-001
title: Old run
status: ready
created: 2026-10-15T00:00:00Z
updated: 2026-10-15T00:00:00Z
runs:
  - id: R1
    agent: omo
    worktree: /unused
    branch: corral/T-001
    base: main
    base_sha: abc
    started: 2026-10-15T00:00:00Z
    review_runs:
      - id: R1-review
        agent: claude
        worktree: /unused
        branch: corral/T-001
        base: main
        base_sha: abc
        started: 2026-10-15T00:00:00Z
---
"#;
    // When loading a pre-inbox file.
    let mut task = Store::parse_task_markdown(text).unwrap();
    // Then both run kinds remain legacy with fresh inbox state.
    let run = &mut task.runs[0];
    for historical in [&*run, &run.review_runs[0]] {
        assert_eq!(
            historical.delivery,
            corral_core::inbox::DeliveryMode::Legacy
        );
        assert_eq!(historical.inbox, Default::default());
    }
    // Given explicit state on a subsequently saved run.
    run.delivery = corral_core::inbox::DeliveryMode::Inbox;
    run.inbox.next_seq = 7;
    run.inbox.effects_applied.insert(3);
    // When saving and reloading, then the recorded mode and counters are retained.
    let restored = Store::parse_task_markdown(&Store::render_task_markdown(&task)).unwrap();
    assert_eq!(restored, task);
}

#[test]
fn update_persists() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    let t = s.create("Upd", None, Status::Backlog, "b\n").unwrap();
    let u = s
        .update("1", |t| {
            t.status = Status::Running;
            t.queued = true;
            Ok(())
        })
        .unwrap();
    assert_eq!(u.status, Status::Running);
    let g = s.get("T-001").unwrap();
    assert_eq!(g.status, Status::Running);
    assert!(g.queued);
    assert!(g.updated >= t.updated);
    assert!(s.update("1", |_| anyhow::bail!("nope")).is_err());
    assert_eq!(s.get("1").unwrap().status, Status::Running);
}

#[test]
fn try_update_skips_write_when_declined() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    let t = s.create("Cas", None, Status::Backlog, "").unwrap();
    let before = s.get(&t.id).unwrap();
    let skipped = s
        .try_update("1", |t| {
            t.status = Status::Dropped;
            Ok(false)
        })
        .unwrap();
    assert!(skipped.is_none());
    assert_eq!(s.get(&t.id).unwrap(), before);
    let saved = s
        .try_update("1", |t| {
            t.status = Status::Ready;
            Ok(true)
        })
        .unwrap();
    assert_eq!(saved.map(|t| t.status), Some(Status::Ready));
    assert_eq!(s.get("1").unwrap().status, Status::Ready);
}

#[test]
fn garbage_documents_are_refused_and_ids_continue_after_stored_tasks() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    s.create("A", None, Status::Backlog, "").unwrap();
    let refused = s
        .fixture_put_task_text("T-050-garbage", "no frontmatter here")
        .unwrap_err();
    assert!(
        format!("{refused:#}").contains("frontmatter"),
        "{refused:#}"
    );
    let mut imported = s.get("T-001").unwrap();
    imported.id = "T-050".into();
    s.fixture_put_task(&imported).unwrap();
    s.create("B", None, Status::Backlog, "").unwrap();
    let l = s.list().unwrap();
    assert_eq!(
        l.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["T-001", "T-050", "T-051"]
    );
}

#[test]
fn default_for_detects_git_subdirs() {
    let d = tempdir().unwrap();
    git_init(&d.path().join("alpha"));
    git_init(&d.path().join("beta"));
    std::fs::create_dir(d.path().join("plain")).unwrap();
    let c = Config::default_for(d.path());
    assert_eq!(c.projects.keys().collect::<Vec<_>>(), ["alpha", "beta"]);
    assert_eq!(c.projects["alpha"].path, "alpha");
    assert_eq!(c.harnesses["generic"].command, "cat '{prompt_file}'");
    assert_eq!(c.harnesses["omp"].command, "omp {model_args} {prompt_ref}");
    assert_eq!(c.harnesses.len(), 4);
    assert_eq!(c.profiles.len(), 4);
    assert_eq!(c.profiles["claude"].harness, "claude");
    assert!(c.agents.is_empty());

    let r = tempdir().unwrap();
    git_init(r.path());
    let c = Config::default_for(r.path());
    let name = r.path().file_name().unwrap().to_str().unwrap();
    assert_eq!(c.projects.len(), 1);
    assert_eq!(c.projects[name].path, ".");
}

#[test]
fn config_toml_round_trip() {
    let d = tempdir().unwrap();
    git_init(&d.path().join("alpha"));
    let mut c = Config::default_for(d.path());
    c.herdr.session = Some("corral-test".into());
    c.limits.per_project.insert("alpha".into(), 2);
    c.projects.get_mut("alpha").unwrap().base = Some("main".into());
    let f = d.path().join("c.toml");
    std::fs::write(&f, c.to_toml()).unwrap();
    assert_eq!(Config::load(&f).unwrap(), c);
    assert!(Config::load(&d.path().join("missing.toml")).is_err());
}

#[test]
fn events_and_discover() {
    const CHILD: &str = "CORRAL_DISCOVERY_TEST_CHILD";
    if let Ok(mode) = std::env::var(CHILD) {
        let root =
            std::path::PathBuf::from(std::env::var_os("CORRAL_DISCOVERY_TEST_ROOT").unwrap());
        let sub = root.join("x/y");
        let other = tempdir().unwrap();
        assert_eq!(Store::discover(&sub).unwrap().root(), root);
        if mode == "override" {
            assert_eq!(Store::discover(other.path()).unwrap().root(), root);
        } else {
            assert!(Store::discover(other.path()).is_err());
        }
        return;
    }
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    s.log_event(Some("T-001"), "created", serde_json::json!({"a": 1}))
        .unwrap();
    s.log_event(None, "sync", serde_json::Value::Null).unwrap();
    s.fixture_append_events("garbage\n").unwrap();
    s.log_event(None, "last", serde_json::Value::Null).unwrap();
    let ev = s.events().unwrap();
    assert_eq!(
        ev.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        ["created", "sync", "last"]
    );
    assert_eq!(ev[0].task.as_deref(), Some("T-001"));
    assert_eq!(ev[0].detail["a"], 1);
    assert_eq!(ev.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2, 4]);

    let sub = d.path().join("x/y");
    std::fs::create_dir_all(&sub).unwrap();
    for mode in ["ancestry", "override"] {
        let mut child = command(std::env::current_exe().unwrap(), d.path());
        if mode == "ancestry" {
            child.env_remove("CORRAL_ROOT");
        }
        let output = child
            .args(["--exact", "events_and_discover", "--nocapture"])
            .env(CHILD, mode)
            .env("CORRAL_DISCOVERY_TEST_ROOT", s.root())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}

#[test]
fn event_fixture_stages_partial_input_until_database_commit() {
    // Given a database store and a complete JSON document without its newline.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let json = r#"{"ts":"2026-10-01T00:00:00Z","kind":"fixture","detail":{"text":"kept"}}"#;
    store.fixture_append_events(json).unwrap();
    assert!(store.events().unwrap().is_empty());
    let mut follower = store.events_since(Some(EventCursor::default())).unwrap();
    assert!(follower.next_event().unwrap().is_none());

    // When another handle completes the staged input.
    let reopened = Store::open(dir.path()).unwrap();
    reopened.fixture_append_events("\nmalformed\n").unwrap();

    // Then database readers observe one committed event and skip the malformed row.
    let (event, _) = follower.next_event().unwrap().unwrap();
    assert_eq!(event.seq, 1);
    assert_eq!(event.kind, "fixture");
    assert_eq!(event.detail["text"], "kept");
    assert_eq!(store.events().unwrap(), vec![event]);
    assert!(!store.dir().join("events.jsonl").exists());
    assert!(!store.dir().join("state/legacy-event-fixture").exists());
}

#[test]
fn event_fixture_replacement_is_atomic_on_import_failure() {
    // Given existing database history and replacement input referencing an unknown blob.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    store
        .log_event(None, "original", serde_json::json!({}))
        .unwrap();
    let original = store.events().unwrap();
    let reference = format!("blob:sha256:{}", "a".repeat(64));
    let replacement = serde_json::json!({
        "ts": "2026-10-01T00:00:00Z",
        "kind": "replacement",
        "detail": {"blob": reference}
    });

    // When the import refuses the missing reference after clearing the event tables.
    assert!(
        store
            .fixture_replace_events(&replacement.to_string())
            .is_err()
    );

    // Then the transaction restores the original history.
    assert_eq!(store.events().unwrap(), original);
}

#[test]
fn event_fixture_clear_and_corruption_change_database_authority() {
    // Given imported history with a valid EOF event.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let json = r#"{"ts":"2026-10-01T00:00:00Z","kind":"imported","detail":{}}"#;
    store.fixture_replace_events(json).unwrap();
    assert_eq!(store.events().unwrap()[0].kind, "imported");

    // When fixture hooks clear, corrupt, and restore the canonical event tables.
    store.fixture_remove_events().unwrap();
    assert!(store.events().unwrap().is_empty());
    store.fixture_break_events().unwrap();
    assert!(store.events().is_err());
    store.fixture_replace_events(json).unwrap();

    // Then production writers append normally to the restored database.
    store
        .log_event(None, "appended", serde_json::json!({}))
        .unwrap();
    let events = store.events().unwrap();
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(events[1].kind, "appended");
    assert!(!store.dir().join("events.jsonl").exists());
}

#[test]
fn event_fixture_unavailability_blocks_reads_and_writes_then_restores_history() {
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    store
        .log_event(Some("T-001"), "original", serde_json::json!({}))
        .unwrap();
    let before = store.events().unwrap();

    store.fixture_break_events().unwrap();
    assert!(store.events().is_err());
    assert!(store.events_of(Some("T-001"), &["verdict"]).is_err());
    assert!(
        store
            .log_event(Some("T-001"), "rejected", serde_json::json!({}))
            .is_err()
    );
    store.fixture_restore_events().unwrap();

    assert_eq!(store.events().unwrap(), before);
    store
        .log_event(Some("T-001"), "restored", serde_json::json!({}))
        .unwrap();
    let after = store.events().unwrap();
    assert_eq!(&after[..before.len()], &before);
    assert_eq!(after.last().unwrap().kind, "restored");
}

#[test]
fn viewers_section_parses_and_round_trips() {
    let d = tempdir().unwrap();
    let f = d.path().join("corral.toml");
    std::fs::write(&f, "[viewers]\npng = \"feh {path}\"\nhtml = \"system\"\n").unwrap();
    let c = Config::load(&f).unwrap();
    assert_eq!(c.viewers.get("png").map(String::as_str), Some("feh {path}"));
    assert_eq!(c.viewers.get("html").map(String::as_str), Some("system"));
    std::fs::write(&f, c.to_toml()).unwrap();
    assert_eq!(Config::load(&f).unwrap(), c);
    assert!(Config::default_for(d.path()).viewers.is_empty());
    assert!(
        !Config::default_for(d.path())
            .to_toml()
            .contains("[viewers]")
    );
}
