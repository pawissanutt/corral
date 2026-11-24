use std::{fs, path::Path, process::Command, sync::Barrier, thread};

use super::*;
use crate::{Op, as_refusal, events::EventKind, testing::FakeRunner, tests::tempdir};

fn configure(root: &Path) {
    let engine = Engine::init(root, None).unwrap();
    fs::write(
        engine.state_dir.join("corral.toml"),
        r#"
[projects.sample]
path = "repo"
base = "main"
lenses = "docs/lenses"

[extensions.fixture]
projects = ["sample"]
facts = "sh provider.sh"
"#,
    )
    .unwrap();
}

fn draft(kind: &str, about: &str, ready: bool) -> Op {
    Op::LensDraft {
        project: "sample".into(),
        kind: kind.into(),
        about: about.into(),
        ready,
    }
}

fn repair(id: &str, ready: bool) -> Op {
    Op::LensRepair {
        project: "sample".into(),
        id: id.into(),
        ready,
    }
}

fn apply(engine: &Engine, op: Op) -> Task {
    let applied = engine.apply(op, false).unwrap();
    assert!(applied.run.is_none());
    applied.task.unwrap()
}

#[test]
fn drafts_share_format_but_use_kind_specific_schema_without_dispatching() {
    let root = tempdir();
    configure(root.path());
    let fake = FakeRunner::new();
    let engine = Engine::open_with_runner(root.path(), Box::new(fake.clone())).unwrap();
    let state = apply(&engine, draft("state", "submit job", false));
    let sequence = apply(&engine, draft("sequence", "submit job", true));
    assert_ne!(state.id, sequence.id);
    assert_eq!(
        (state.status, sequence.status),
        (Status::Backlog, Status::Ready)
    );
    for task in [&state, &sequence] {
        assert_eq!(task.project.as_deref(), Some("sample"));
        for expected in [
            "submit job",
            "docs/lenses",
            "read_at",
            "lens check --project",
        ] {
            assert!(task.body.contains(expected), "missing {expected}");
        }
        let events = engine.store.list_events(&task.id).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Created);
        assert!(
            engine
                .store
                .live_runs_for_task(&task.id)
                .unwrap()
                .is_empty()
        );
    }
    for expected in [
        "states_from",
        "[[states]]",
        "[[transitions]]",
        "enum variant",
    ] {
        assert!(state.body.contains(expected), "missing {expected}");
    }
    for expected in ["[[participants]]", "[[messages]]", "entry = {"] {
        assert!(sequence.body.contains(expected), "missing {expected}");
    }
    assert!(!sequence.body.contains("states_from"));
    assert!(fake.starts().is_empty());
    assert_eq!(apply(&engine, draft("state", " submit job ", true)), state);
    assert!(engine.apply(draft("state", " \n ", false), false).is_err());
    assert!(
        engine
            .apply(draft("graph", "submit job", false), false)
            .is_err()
    );
    assert_eq!(engine.store.next_task_number().unwrap(), 3);
}

#[test]
fn concurrent_drafts_use_one_open_task_and_one_created_event() {
    let root = tempdir();
    configure(root.path());
    let first = Engine::open_with_runner(root.path(), Box::new(FakeRunner::new())).unwrap();
    let second = Engine::open_with_runner(root.path(), Box::new(FakeRunner::new())).unwrap();
    let barrier = Barrier::new(2);
    thread::scope(|scope| {
        let shared_barrier = &barrier;
        let other = scope.spawn(move || {
            shared_barrier.wait();
            apply(&second, draft("state", "concurrent", false))
        });
        barrier.wait();
        let task = apply(&first, draft("state", "concurrent", false));
        assert_eq!(other.join().unwrap(), task);
        assert_eq!(first.store.next_task_number().unwrap(), 2);
        assert_eq!(first.store.list_events(&task.id).unwrap().len(), 1);
    });
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .current_dir(repo)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit_lenses(repo: &Path) -> String {
    fs::create_dir_all(repo.join("docs/lenses")).unwrap();
    git(repo, &["init", "-b", "main"]);
    fs::write(repo.join("code.rs"), "enum JobStatus { Queued, Running }\n").unwrap();
    git(repo, &["add", "code.rs"]);
    git(repo, &["commit", "-m", "code"]);
    let read_at = git(repo, &["rev-parse", "HEAD"]);
    fs::write(
        repo.join("docs/lenses/job.toml"),
        format!(
            r#"
id = "job-lifecycle"
title = "Lifecycle for jobs"
kind = "state"
read_at = "{read_at}"
states_from = {{ path = "code.rs", name = "JobStatus" }}

[[states]]
id = "queued"
label = "Queued"
anchor = {{ path = "code.rs", text = "missing in committed source" }}

[[states]]
id = "obsolete"
label = "Obsolete"
anchor = {{ path = "code.rs", name = "removed_definition" }}
"#
        ),
    )
    .unwrap();
    fs::write(repo.join("docs/lenses/broken.toml"), "kind = [").unwrap();
    fs::write(
        repo.join("docs/lenses/current.toml"),
        format!(
            r#"
id = "current"
title = "Current sequence"
kind = "sequence"
read_at = "{read_at}"

[[participants]]
id = "worker"
label = "Worker"
anchor = {{ path = "code.rs", name = "missing_definition" }}
"#
        ),
    )
    .unwrap();
    git(repo, &["add", "docs/lenses"]);
    git(repo, &["commit", "-m", "lenses"]);
    let checked_at = git(repo, &["rev-parse", "HEAD"]);
    // Checks must not be fooled by an anchor that only exists in the checkout.
    fs::write(repo.join("code.rs"), "// missing in committed source\n").unwrap();
    checked_at
}

#[test]
fn repairs_use_committed_diagnostics_and_dedup_with_lens_drafts() {
    let root = tempdir();
    configure(root.path());
    let repo = root.path().join("repo");
    let checked_at = commit_lenses(&repo);
    let fake = FakeRunner::new();
    let engine = Engine::open_with_runner(root.path(), Box::new(fake.clone())).unwrap();

    let drafted = apply(&engine, draft("state", "job-lifecycle", false));
    let task = apply(&engine, repair("job-lifecycle", true));
    assert_eq!(task.id, drafted.id);
    assert_eq!(task.status, drafted.status);
    assert!(task.title.starts_with("Repair lens:"));
    assert!(task.body.contains("Repair the existing state lens"));
    assert!(!task.body.contains("Choose a new ID and path"));
    assert_eq!(engine.store.next_task_number().unwrap(), 2);
    assert_eq!(
        engine.store.list_events(&task.id).unwrap()[1].kind,
        EventKind::Edited
    );
    for expected in [
        checked_at.as_str(),
        "docs/lenses/job.toml",
        "state queued:",
        "missing in committed source",
        "Missing state: Running",
        "Extra state: obsolete",
        "Not checked",
        "removed_definition",
        "No cached facts provider answer",
    ] {
        assert!(task.body.contains(expected), "missing {expected}");
    }
    let edited = engine
        .edit_task(
            &task.id,
            crate::tasks::TaskEdit {
                title: Some("owner title".into()),
                body: Some("owner text".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(apply(&engine, repair("job.toml", false)), edited);
    assert_eq!(
        apply(&engine, draft("state", "Lifecycle for jobs", false)),
        edited
    );
    engine.drop_task(&task.id).unwrap();
    let fresh = apply(&engine, repair("job-lifecycle", true));
    assert_ne!(fresh.id, task.id);
    assert_eq!(fresh.status, Status::Ready);
    assert!(fake.starts().is_empty());

    for (selector, expected) in [
        ("broken.toml", "malformed lens"),
        ("current", "no failed checks"),
    ] {
        let error = engine.apply(repair(selector, false), false).unwrap_err();
        assert!(as_refusal(&error).unwrap().to_string().contains(expected));
    }
    assert_eq!(engine.store.next_task_number().unwrap(), 3);
}

#[test]
fn repair_does_not_retarget_a_draft_with_an_existing_run() {
    let root = tempdir();
    configure(root.path());
    commit_lenses(&root.path().join("repo"));
    let engine = Engine::open_with_runner(root.path(), Box::new(FakeRunner::new())).unwrap();
    let original = apply(&engine, draft("state", "job-lifecycle", true));
    let run = crate::tests::run(&original.id, "R1");
    engine.store.insert_run(&run).unwrap();
    let error = engine
        .apply(repair("job-lifecycle", true), false)
        .unwrap_err();
    let reason = &as_refusal(&error).unwrap().reason;
    assert!(reason.contains(&original.id) && reason.contains("started"));
    assert_eq!(engine.store.require_task(&original.id).unwrap(), original);
    assert_eq!(
        engine.store.live_runs_for_task(&original.id).unwrap(),
        vec![run]
    );
    assert_eq!(engine.store.list_events(&original.id).unwrap().len(), 1);
    assert_eq!(engine.store.next_task_number().unwrap(), 2);
}

#[test]
fn repair_recovers_identity_from_schema_invalid_committed_toml() {
    let root = tempdir();
    configure(root.path());
    let repo = root.path().join("repo");
    let read_at = commit_lenses(&repo);
    let file = repo.join("docs/lenses/bad-schema.toml");
    fs::write(
        &file,
        format!(
            r#"
id = "bad-schema"
kind = "state"
title = false
read_at = "{read_at}"
"#
        ),
    )
    .unwrap();
    git(&repo, &["add", "docs/lenses/bad-schema.toml"]);
    git(&repo, &["commit", "-m", "malformed lens"]);
    let checked_at = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(file, "id = 'checkout-only'").unwrap();
    let engine = Engine::open_with_runner(root.path(), Box::new(FakeRunner::new())).unwrap();
    let task = apply(&engine, repair("bad-schema", false));
    assert_eq!(
        apply(&engine, repair("docs/lenses/bad-schema.toml", false)),
        task
    );
    for expected in [
        "Repair the existing state lens",
        "bad-schema.toml",
        &checked_at,
        "(malformed)",
        "file: invalid lens TOML",
    ] {
        assert!(task.body.contains(expected), "missing {expected}");
    }
}

#[test]
#[cfg(unix)]
fn repair_uses_cached_provider_checks_without_starting_the_provider() {
    let root = tempdir();
    configure(root.path());
    let repo = root.path().join("repo");
    let checked_at = commit_lenses(&repo);
    let engine = Engine::open_with_runner(root.path(), Box::new(FakeRunner::new())).unwrap();
    let marker = repo.join("provider-started");
    fs::write(
        repo.join("provider.sh"),
        r#"
set -eu
cat > /dev/null
: > provider-started
printf '%s\n' '{"type":"done","provided":["definitions"],"skipped":{},"omitted":{}}'
"#,
    )
    .unwrap();
    let error = engine.apply(repair("current", false), false).unwrap_err();
    let reason = &as_refusal(&error).unwrap().reason;
    assert!(reason.contains(&format!(
        "corral-next lens check --project sample --at {checked_at} docs/lenses/current.toml"
    )));
    assert!(reason.contains("before retrying lens repair"));
    let task = apply(&engine, repair("job-lifecycle", false));
    assert!(!marker.exists());
    assert!(task.body.contains("Not checked"));
    engine.drop_task(&task.id).unwrap();

    let reader = FactsReader::new(root.path(), &engine.config, "sample").unwrap();
    let rev = reader.resolve_revision(&checked_at).unwrap();
    let provider = reader.check_lenses(&rev).unwrap().1.unwrap();
    assert_eq!(provider.error, None);
    fs::remove_file(&marker).unwrap();
    let task = apply(&engine, repair("job-lifecycle", false));
    assert!(
        task.body
            .contains("unknown definition \"removed_definition\"")
    );
    assert!(!task.body.contains("No cached facts provider answer"));
    let task = apply(&engine, repair("current", false));
    assert!(
        task.body
            .contains("unknown definition \"missing_definition\"")
    );
    assert!(!marker.exists());
}
