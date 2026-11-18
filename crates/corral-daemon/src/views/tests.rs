use std::{fs, path::Path, process::Command, sync::Arc};

use corral_engine::{Applied, NewTask, Op, Run, testing::FakeRunner};
use corral_protocol::detail::{ArtifactEncoding, DetailBody, DetailRef};

use super::*;

pub(crate) struct Fixture {
    _temp: tempfile::TempDir,
    pub shared: Shared,
    pub runner: FakeRunner,
}

impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        Engine::init(temp.path(), None).unwrap();
        let runner = FakeRunner::new();
        let shared = Shared::new(
            temp.path().to_owned(),
            Some(Arc::new({
                let runner = runner.clone();
                move || -> Box<dyn corral_engine::Runner> { Box::new(runner.clone()) }
            })),
        )
        .unwrap();
        Self {
            _temp: temp,
            shared,
            runner,
        }
    }

    pub fn project(&self) {
        let repo = self.shared.root().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["config", "user.email", "test@example.invalid"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        git(&repo, &["config", "core.hooksPath", "/dev/null"]);
        fs::write(repo.join(".gitignore"), "/.worktrees/\n").unwrap();
        git(&repo, &["add", ".gitignore"]);
        git(&repo, &["commit", "-qm", "base"]);
        fs::write(
            self.shared.root().join(".corral-next/corral.toml"),
            "[runner]\nmode = \"process\"\n[projects.demo]\npath = \"repo\"\n",
        )
        .unwrap();
        self.shared.publish();
    }

    pub fn apply(&self, op: Op) -> Applied {
        let result = self.shared.engine().unwrap().apply(op, false).unwrap();
        self.shared.publish();
        result
    }

    pub fn task(&self, task: NewTask) -> String {
        self.apply(Op::New { task }).task.unwrap().id
    }

    pub fn named_task(&self, title: &str, project: Option<&str>) -> String {
        self.task(NewTask {
            title: title.into(),
            project: project.map(str::to_owned),
            ..Default::default()
        })
    }

    pub fn dispatch(&self, id: &str) -> Run {
        self.apply(Op::Dispatch {
            id: id.into(),
            profile: None,
            base: None,
        })
        .run
        .unwrap()
    }
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn changes_pages_raw_event_sequences_and_device_seen_independently() {
    let fixture = Fixture::new();
    let id = fixture.named_task("first task", None);
    fixture.apply(Op::Comment {
        id: id.clone(),
        text: "comment".into(),
    });
    fixture.apply(Op::Drop { id });
    let seen = SeenCursor {
        through: 1,
        at: Some(jiff::Timestamp::now().to_string()),
    };
    let page = |after, before| {
        changes(
            &fixture.shared,
            ChangesRequest {
                query: ChangesQuery {
                    after,
                    before,
                    limit: 2,
                },
                seen: seen.clone(),
            },
        )
        .unwrap()
    };
    let head = page(None, None);
    assert_eq!(
        head.events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        [3, 2]
    );
    assert_eq!(
        (head.head, head.older, head.total, head.unseen),
        (3, Some(2), Some(3), Some(2))
    );
    assert_eq!(head.seen, seen);
    assert_eq!(page(None, head.older).events[0].seq, 1);
    assert_eq!(
        page(Some(1), None)
            .events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        [2, 3]
    );
}

#[test]
fn artifact_chunks_preserve_utf8_boundaries_and_are_selector_bound() {
    let fixture = Fixture::new();
    let id = fixture.named_task("large artifact", None);
    let engine = fixture.shared.engine().unwrap();
    let text = format!("{}🦀end", "a".repeat(8191));
    let artifact = engine
        .add_artifact_bytes(&id, text.as_bytes(), None, "artifact.md")
        .unwrap();
    let get = |target, page, etag| detail(&fixture.shared, DetailGet { target, page, etag });
    let target = DetailRef::Artifact {
        reference: artifact.reference(),
    };
    let first = get(target.clone(), None, None).unwrap();
    let second = get(target.clone(), first.next.clone(), None).unwrap();
    assert_eq!(first.etag, second.etag);
    let (Some(DetailBody::Artifact(a)), Some(DetailBody::Artifact(b))) = (first.body, second.body)
    else {
        panic!("expected a text artifact");
    };
    assert_eq!(a.encoding, ArtifactEncoding::Text);
    assert!(a.truncated);
    assert!(!b.truncated);
    assert_eq!(a.content + &b.content, text);
    assert!(get(target, None, Some(first.etag)).unwrap().body.is_none());
    assert!(get(DetailRef::Artifacts { filter: None }, first.next, None).is_err());
}

#[test]
fn review_uses_immutable_submitted_patch_and_current_checks() {
    let fixture = Fixture::new();
    fixture.project();
    let id = fixture.named_task("review me", Some("demo"));
    let run = fixture.dispatch(&id);
    let file = run.worktree.join("with space.txt");
    fs::write(&file, "submitted\n").unwrap();
    fixture.apply(Op::Done {
        id: id.clone(),
        summary: "summary".into(),
    });
    fixture.apply(Op::Gate { id: id.clone() });
    fs::write(&file, "not submitted\n").unwrap();
    let get = |target| {
        detail(
            &fixture.shared,
            DetailGet {
                target,
                page: None,
                etag: None,
            },
        )
        .unwrap()
        .body
        .unwrap()
    };
    let DetailBody::Review(review) = get(DetailRef::Review { task: id.clone() }) else {
        panic!("expected review");
    };
    assert_eq!(review.run.unwrap().summary.as_deref(), Some("summary"));
    assert_eq!(review.checks[0].kind, corral_engine::CheckKind::Gate);
    assert_eq!(review.files[0].path, "with space.txt");
    assert_eq!(review.files[0].added, 1);
    let DetailBody::ReviewFile(file) = get(DetailRef::ReviewFile {
        task: id,
        path: "with space.txt".into(),
    }) else {
        panic!("expected patch");
    };
    assert!(file.patch.contains("+submitted"));
    assert!(!file.patch.contains("not submitted"));
}
