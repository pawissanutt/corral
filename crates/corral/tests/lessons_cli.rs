use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use corral_core::{Lesson, LessonEvidence, LessonScope, Status, Store};
use corral_ops::{LessonProposal, LessonRequest};
use serde_json::{Value, json};

const OBSERVED: &str = "2026-10-01T10:00:00Z";

struct Fixture {
    root: tempfile::TempDir,
    store: Store,
}

fn isolate(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
            command.env_remove(name);
        }
    }
    command.stdin(Stdio::null());
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        for project in ["p", "q"] {
            let path = root.path().join(project);
            std::fs::create_dir(&path).unwrap();
            let mut git = Command::new("git");
            isolate(&mut git);
            let output = git
                .args(["init", "-q", "-b", "main"])
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let mut store = Store::init(root.path()).unwrap();
        store.config.defaults.profile = Some("generic".into());
        store.config.lessons.every = None;
        store.config.lessons.after_landings = None;
        store.config.lessons.after_failures = Some(2);
        store.config.lessons.profile = Some("generic".into());
        store.save_config().unwrap();
        Self { root, store }
    }

    fn cli(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        isolate(&mut command);
        command
            .env("CORRAL_ROOT", self.root.path())
            .env("CORRAL_HERDR_SESSION", "corral-test")
            // No test may contact a live terminal engine.
            .env("HERDR_BIN_PATH", self.root.path().join("no-herdr"))
            .arg("--root")
            .arg(self.root.path())
            .current_dir(self.root.path())
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.cli(args);
        assert!(
            output.status.success(),
            "corral {args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }

    fn lessons(&self) -> Value {
        self.json(&["lessons", "--project", "p", "--json"])
    }

    fn history(&self) -> String {
        let task = self
            .store
            .create("Historical change", Some("p"), Status::Done, "")
            .unwrap();
        self.store
            .log_event(
                Some(&task.id),
                "gate",
                json!({"result":"fail", "failed_cmd":"cargo test -p sample"}),
            )
            .unwrap();
        self.store
            .log_event(
                Some(&task.id),
                "changes_requested",
                json!({"notes":"[P1] Retain evidence before applying updates"}),
            )
            .unwrap();
        task.id
    }

    fn distill(&self) -> Value {
        self.json(&[
            "lessons",
            "distill",
            "--project",
            "p",
            "--profile",
            "generic",
            "--json",
        ])
    }

    fn request(&self, task: &Value) -> (PathBuf, LessonRequest) {
        let body = task["body"].as_str().unwrap();
        let path = body
            .lines()
            .find_map(|line| line.strip_prefix("Request: "))
            .map(PathBuf::from)
            .unwrap();
        let request = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        (path, request)
    }
}

fn lesson(id: &str) -> Lesson {
    Lesson {
        id: id.into(),
        rule: "Retain supporting evidence".into(),
        why: "Updates need an auditable source".into(),
        evidence: vec![LessonEvidence {
            task: "T-001".into(),
            ts: OBSERVED.parse().unwrap(),
            source: "event:gate".into(),
        }],
        scope: LessonScope::default(),
        confidence: 0.75,
        pinned: false,
        contradicted: false,
        last_seen: OBSERVED.parse().unwrap(),
    }
}

fn ids(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect()
}

#[test]
fn listing_json_keeps_project_collections_separate() {
    // Given two configured projects with distinct stored lesson IDs.
    let fx = Fixture::new();
    fx.store.save_lessons("p", &[lesson("L-p")]).unwrap();
    fx.store.save_lessons("q", &[lesson("L-q")]).unwrap();

    // When listing one project and the workspace through the real CLI.
    let project = fx.lessons();
    let all = fx.json(&["lessons", "--json"]);

    // Then the project list is scoped and the global list labels its lessons.
    assert_eq!(ids(&project), ["L-p"]);
    assert_eq!(project[0]["evidence"][0]["task"], "T-001");
    assert_eq!(project[0]["scope"]["files"], json!([]));
    assert_eq!(all.as_array().unwrap().len(), 2);
    assert_eq!(all[0]["project"], "p");
    assert_eq!(all[0]["lesson"]["id"], "L-p");
    assert_eq!(all[1]["project"], "q");
    assert_eq!(all[1]["lesson"]["id"], "L-q");
}

#[test]
fn task_preview_matches_scope_without_recording_dispatch_exposure() {
    // Given one matching rule and rules rejected by each declared dimension.
    let fx = Fixture::new();
    let task = fx
        .store
        .create("Repair evidence", Some("p"), Status::Backlog, "src/lib.rs")
        .unwrap();
    fx.store
        .update(&task.id, |t| {
            t.kind = Some("fix".into());
            t.profile = Some("generic".into());
            Ok(())
        })
        .unwrap();
    let mut matching = lesson("L-match");
    matching.scope = LessonScope {
        files: vec!["src/**".into()],
        kinds: vec!["fix".into()],
        profiles: vec!["generic".into()],
        harnesses: vec!["generic".into()],
        tags: vec!["evidence".into()],
    };
    let mut rules = vec![matching.clone()];
    for (id, dimension) in [
        ("L-file", "files"),
        ("L-kind", "kinds"),
        ("L-profile", "profiles"),
        ("L-harness", "harnesses"),
        ("L-tag", "tags"),
    ] {
        let mut excluded = matching.clone();
        excluded.id = id.into();
        match dimension {
            "files" => excluded.scope.files = vec!["docs/**".into()],
            "kinds" => excluded.scope.kinds = vec!["spike".into()],
            "profiles" => excluded.scope.profiles = vec!["claude".into()],
            "harnesses" => excluded.scope.harnesses = vec!["claude".into()],
            "tags" => excluded.scope.tags = vec!["unrelated".into()],
            _ => unreachable!(),
        }
        rules.push(excluded);
    }
    let mut contradicted = matching;
    contradicted.id = "L-contradicted".into();
    contradicted.contradicted = true;
    rules.push(contradicted);
    fx.store.save_lessons("p", &rules).unwrap();
    fx.store.save_lessons("q", &[lesson("L-other")]).unwrap();
    let events = fx.store.events().unwrap().len();

    // When previewing relevant lessons for the task.
    let output = fx.json(&["lessons", "--for", &task.id, "--json"]);

    // Then only the matching project rule appears and no exposure is recorded.
    assert_eq!(ids(&output), ["L-match"]);
    assert_eq!(fx.store.events().unwrap().len(), events);
    assert!(fx.store.get(&task.id).unwrap().runs.is_empty());
}

#[test]
fn pin_persists_in_project_json_and_store() {
    // Given an unpinned project lesson.
    let fx = Fixture::new();
    fx.store.save_lessons("p", &[lesson("L-1")]).unwrap();

    // When pinning the lesson through the CLI.
    fx.ok(&["lessons", "pin", "L-1", "--project", "p"]);

    // Then fresh CLI and store reads agree on the pin.
    assert_eq!(fx.lessons()[0]["pinned"], true);
    assert!(Store::open(fx.root.path()).unwrap().lessons("p").unwrap()[0].pinned);
}

#[test]
fn unpin_persists_in_project_json() {
    // Given a pinned lesson.
    let fx = Fixture::new();
    let mut pinned = lesson("L-1");
    pinned.pinned = true;
    fx.store.save_lessons("p", &[pinned]).unwrap();

    // When explicitly unpinning it.
    fx.ok(&["lessons", "pin", "L-1", "--project", "p", "--unpin"]);

    // Then its stored JSON pin is false.
    assert_eq!(fx.lessons()[0]["pinned"], false);
}

#[test]
fn delete_removes_only_the_selected_project_lesson() {
    // Given identical IDs in two separate projects.
    let fx = Fixture::new();
    fx.store
        .save_lessons("p", &[lesson("L-1"), lesson("L-keep")])
        .unwrap();
    fx.store.save_lessons("q", &[lesson("L-1")]).unwrap();

    // When deleting one project's rule.
    fx.ok(&["lessons", "delete", "L-1", "--project", "p"]);

    // Then the remaining IDs are correct in both projects.
    assert_eq!(ids(&fx.lessons()), ["L-keep"]);
    let other = fx.json(&["lessons", "--project", "q", "--json"]);
    assert_eq!(ids(&other), ["L-1"]);
}

#[test]
fn invalid_projects_and_task_project_mismatches_fail_without_mutations() {
    // Given configured projects, an existing task and lesson.
    let fx = Fixture::new();
    let task = fx
        .store
        .create("Preview", Some("p"), Status::Backlog, "")
        .unwrap();
    fx.store.save_lessons("p", &[lesson("L-1")]).unwrap();
    let events = fx.store.events().unwrap().len();

    // When selecting unknown, unsafe or mismatched project keys.
    for args in [
        vec!["lessons", "--project", "missing", "--json"],
        vec!["lessons", "--project", "../p", "--json"],
        vec![
            "lessons",
            "distill",
            "--project",
            "missing",
            "--profile",
            "generic",
        ],
        vec!["lessons", "pin", "L-1", "--project", "missing"],
        vec!["lessons", "delete", "L-1", "--project", "missing"],
        vec!["lessons", "--project", "q", "--for", &task.id, "--json"],
    ] {
        let output = fx.cli(&args);
        assert!(!output.status.success(), "{args:?}");
    }

    // Then all commands fail and leave the authoritative collection untouched.
    assert_eq!(ids(&fx.lessons()), ["L-1"]);
    assert_eq!(fx.store.events().unwrap().len(), events);
    assert_eq!(fx.store.list().unwrap().len(), 1);
}

#[test]
fn due_and_pm_brief_expose_failure_schedule_without_writing() {
    // Given exactly two real failure events, with no time-based schedule.
    let fx = Fixture::new();
    fx.history();
    fx.store.save_lessons("p", &[lesson("L-1")]).unwrap();
    let events = fx.store.events().unwrap().len();

    // When reading the due list and PM brief JSON.
    let due = fx.json(&["lessons", "due", "--json"]);
    let brief = fx.json(&["pm", "brief", "--json"]);

    // Then due state and PM routing expose structured project information only.
    assert_eq!(due.as_array().unwrap().len(), 1);
    assert_eq!(due[0]["project"], "p");
    assert_eq!(due[0]["profile"], "generic");
    assert!(due[0]["pending"].is_null());
    assert_eq!(due[0]["reasons"].as_array().unwrap().len(), 1);
    assert_eq!(brief["lessons_due"], due);
    assert_eq!(ids(&brief["lessons"]["p"]), ["L-1"]);
    assert_eq!(brief["next"]["kind"], "lessons_distill");
    assert_eq!(brief["next"]["recommended"], "distill");
    assert_eq!(brief["next"]["requires_owner"], false);
    assert_eq!(fx.store.events().unwrap().len(), events);
    assert_eq!(fx.store.list().unwrap().len(), 1);
}

#[test]
fn repeated_distill_returns_one_pending_backlog_spike_without_dispatch() {
    // Given real gate and changes-requested history, with herdr unavailable.
    let fx = Fixture::new();
    let historical = fx.history();

    // When filing the same explicit-profile distillation twice.
    let first = fx.distill();
    let second = fx.distill();

    // Then the same pending task is returned and exactly one request is filed.
    assert_eq!(first["id"], second["id"]);
    assert_eq!(second["status"], "backlog");
    assert_eq!(second["kind"], "spike");
    assert_eq!(second["profile"], "generic");
    assert_eq!(second["project"], "p");
    assert!(second["runs"].as_array().unwrap().is_empty());
    assert_eq!(fx.store.list().unwrap().len(), 2);
    let events = fx.store.events().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "lessons_distill_requested")
            .count(),
        1
    );
    assert!(!events.iter().any(|e| e.kind == "dispatched"));
    let (path, request) = fx.request(&second);
    assert!(path.starts_with(fx.root.path()));
    assert_eq!(request.version, 1);
    assert_eq!(request.task, second["id"].as_str().unwrap());
    assert_eq!(request.project, "p");
    for source in ["event:gate", "event:changes_requested"] {
        assert!(
            request
                .signals
                .iter()
                .flat_map(|s| &s.evidence)
                .any(|e| { e.task == historical && e.source == source })
        );
    }
    let due = fx.json(&["lessons", "due", "--json"]);
    assert_eq!(due[0]["pending"], second["id"]);
    assert_eq!(due[0]["reasons"], json!([]));
}

fn proposal(fx: &Fixture) -> LessonProposal {
    fx.history();
    let task = fx.distill();
    let (_, request) = fx.request(&task);
    let evidence = request
        .signals
        .iter()
        .flat_map(|s| &s.evidence)
        .find(|e| e.source == "event:gate")
        .unwrap()
        .clone();
    let mut distilled = lesson("L-distilled");
    distilled.last_seen = evidence.ts;
    distilled.evidence = vec![evidence];
    LessonProposal {
        version: request.version,
        project: request.project,
        task: request.task,
        source_digest: request.source_digest,
        lessons: vec![distilled],
    }
}

fn write_proposal(root: &Path, proposal: &LessonProposal) -> PathBuf {
    let path = root.join("proposal.json");
    std::fs::write(&path, serde_json::to_vec(proposal).unwrap()).unwrap();
    path
}

#[test]
fn apply_persists_a_proposal_bound_to_the_request_evidence() {
    // Given a proposal using the new spike's actual request digest and evidence.
    let fx = Fixture::new();
    let proposal = proposal(&fx);
    let path = write_proposal(fx.root.path(), &proposal);

    // When applying the proposal through the real CLI.
    fx.ok(&[
        "lessons",
        "apply",
        "--project",
        "p",
        "--file",
        path.to_str().unwrap(),
    ]);

    // Then the store, listing and applied event contain the proposed machine data.
    let store = Store::open(fx.root.path()).unwrap();
    let saved = store.lessons("p").unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, "L-distilled");
    assert_eq!(saved[0].evidence, proposal.lessons[0].evidence);
    assert_eq!(saved[0].last_seen, proposal.lessons[0].last_seen);
    assert_eq!(ids(&fx.lessons()), ["L-distilled"]);
    let events = store.events().unwrap();
    let applied: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "lessons_distilled")
        .collect();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].task.as_deref(), Some(proposal.task.as_str()));
    assert_eq!(applied[0].detail["source_digest"], proposal.source_digest);
    assert_eq!(applied[0].detail["lessons"], json!(["L-distilled"]));
}

#[test]
fn apply_rejects_invented_evidence_without_updating_lessons() {
    // Given a valid request but an evidence source absent from its signals.
    let fx = Fixture::new();
    let mut proposal = proposal(&fx);
    proposal.lessons[0].evidence[0].source = "event:invented".into();
    let path = write_proposal(fx.root.path(), &proposal);
    let events = fx.store.events().unwrap().len();

    // When applying the ungrounded proposal.
    let output = fx.cli(&[
        "lessons",
        "apply",
        "--project",
        "p",
        "--file",
        path.to_str().unwrap(),
    ]);

    // Then it fails without writing a lesson or successful-apply event.
    assert!(!output.status.success());
    assert_eq!(fx.lessons(), json!([]));
    assert_eq!(fx.store.events().unwrap().len(), events);
}

#[test]
fn sync_wakes_project_waiters_once_without_launching_a_spike() {
    // Given an offline engine and enough completed failure history.
    let fx = Fixture::new();
    fx.history();
    // When sync evaluates the schedule twice and a project subscriber reads retained events.
    fx.ok(&["sync"]);
    fx.ok(&["sync"]);
    let event = fx.json(&[
        "wait",
        "--for",
        "lessons_due",
        "--project",
        "p",
        "--since",
        "2020-01-01T00:00:00Z",
        "--timeout",
        "1s",
        "--json",
    ]);
    // Then the actionable event is project-scoped and only one announcement exists.
    assert_eq!(event["kind"], "lessons_due");
    assert_eq!(event["detail"]["project"], "p");
    assert_eq!(
        fx.store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "lessons_due")
            .count(),
        1
    );
    assert_eq!(fx.store.list().unwrap().len(), 1);
}
