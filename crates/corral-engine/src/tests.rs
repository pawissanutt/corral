pub(crate) fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap()
}

use std::path::PathBuf;

use jiff::Timestamp;

use crate::{Engine, Outcome, Priority, Role, Run, Status, Task, discover_root};

pub(crate) fn task(id: &str) -> Task {
    let now = Timestamp::now();
    Task {
        id: id.to_owned(),
        title: "small change".to_owned(),
        body: "Goal\nAcceptance\nNotes".to_owned(),
        status: Status::Backlog,
        project: Some("project".to_owned()),
        priority: Priority::P2,
        kind: None,
        size: None,
        profile: None,
        parent: None,
        base: None,
        depends_on: Vec::new(),
        blocked: None,
        round: 0,
        created: now,
        updated: now,
    }
}

pub(crate) fn run(task: &str, id: &str) -> Run {
    Run {
        id: id.to_owned(),
        task: task.to_owned(),
        role: Role::Worker,
        profile: "generic".to_owned(),
        harness: "generic".to_owned(),
        model: None,
        effort: None,
        worktree: PathBuf::from("project/.worktrees").join(task),
        branch: format!("corral/{task}"),
        base: "main".to_owned(),
        base_sha: "abc".to_owned(),
        session: None,
        started: Timestamp::now(),
        ended: None,
        outcome: None,
        summary: None,
        head_sha: None,
        quiet_since: None,
    }
}

#[test]
fn init_first_id_open_and_explicit_discovery() {
    let root = tempdir();
    let engine = Engine::init(root.path(), Some(42)).unwrap();
    let task = engine
        .new_task(crate::tasks::NewTask {
            title: "first".to_owned(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(task.id, "T-042");
    assert!(engine.state_dir.join("runs").is_dir());
    assert!(engine.state_dir.join("locks").is_dir());
    assert!(engine.state_dir.join("blobs/sha256").is_dir());
    assert_eq!(
        Engine::open(root.path())
            .unwrap()
            .store
            .require_task(&task.id)
            .unwrap(),
        task
    );
    assert_eq!(
        discover_root(root.path(), Some(root.path())).unwrap(),
        engine.root
    );
    assert!(Engine::init(root.path(), None).is_err());
    assert_eq!(run(&task.id, "R1").outcome, None::<Outcome>);
}

#[test]
fn typed_refusal_display_and_downcast_survive_context() {
    let error = crate::refuse(
        "not ready",
        vec![
            crate::Next::run(crate::Op::Sync),
            crate::Next::force(crate::Op::Move {
                id: "T-001".into(),
                status: Status::Done,
            }),
        ],
    )
    .context("operation");
    let refusal = crate::as_refusal(&error).unwrap();
    assert_eq!(
        refusal.to_string(),
        "not ready\nnext: corral sync\nnext: corral move T-001 done --force"
    );
    assert!(!refusal.next[0].force);
    assert!(refusal.next[1].force);
}
