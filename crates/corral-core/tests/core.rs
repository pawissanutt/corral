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
    for sub in ["tasks", "artifacts", "prompts", "state", "lock"] {
        assert!(s.dir().join(sub).is_dir(), "{sub}");
    }
    assert!(s.dir().join("events.jsonl").is_file());
    assert_eq!(
        std::fs::read_to_string(s.dir().join(".gitignore")).unwrap(),
        "artifacts/\nprompts/\nstate/\nlock/\n"
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
    assert!(a.path.ends_with("T-001-fix-the-bug-now.md"));
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
fn slug_limited() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    let t = s
        .create(&"word ".repeat(30), None, Status::Backlog, "")
        .unwrap();
    let stem = t.path.file_stem().unwrap().to_str().unwrap().to_string();
    assert!(stem.len() <= "T-001-".len() + 48);
    assert!(!stem.ends_with('-'));
}

#[test]
fn parse_render_round_trip() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    let body = "# Spec\n\nline two\n\n- a\n- b\n---\nnot frontmatter\n";
    let t = s.create("Round", None, Status::Ready, body).unwrap();
    let text = render_task(&t);
    assert!(text.starts_with("---\n"));
    let back = parse_task(&text, &t.path).unwrap();
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
    let mut task = parse_task(text, std::path::Path::new("T-001.md")).unwrap();
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
    let restored = parse_task(&render_task(&task), &task.path).unwrap();
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
    let before = std::fs::read_to_string(&t.path).unwrap();
    let skipped = s
        .try_update("1", |t| {
            t.status = Status::Dropped;
            Ok(false)
        })
        .unwrap();
    assert!(skipped.is_none());
    assert_eq!(std::fs::read_to_string(&t.path).unwrap(), before);
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
fn list_skips_garbage() {
    let d = tempdir().unwrap();
    let s = Store::init(d.path()).unwrap();
    s.create("A", None, Status::Backlog, "").unwrap();
    std::fs::write(
        s.dir().join("tasks/T-050-garbage.md"),
        "no frontmatter here",
    )
    .unwrap();
    s.create("B", None, Status::Backlog, "").unwrap();
    let l = s.list().unwrap();
    assert_eq!(
        l.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["T-001", "T-051"]
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
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(s.dir().join("events.jsonl"))
        .unwrap();
    std::io::Write::write_all(&mut f, b"garbage\n").unwrap();
    s.log_event(None, "last", serde_json::Value::Null).unwrap();
    let ev = s.events().unwrap();
    assert_eq!(
        ev.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        ["created", "sync", "last"]
    );
    assert_eq!(ev[0].task.as_deref(), Some("T-001"));
    assert_eq!(ev[0].detail["a"], 1);

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
