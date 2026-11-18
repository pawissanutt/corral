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
        let temp = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
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

pub(super) fn git(dir: &Path, args: &[&str]) {
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
fn review_uses_immutable_submitted_patch_and_includes_all_checks() {
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
    let engine = fixture.shared.engine().unwrap();
    let mut checks = engine.store.list_checks(&id).unwrap();
    for round in [0, 1] {
        let check = corral_engine::Check {
            kind: corral_engine::CheckKind::Review,
            key: round.to_string(),
            notes: Some(format!("review round {round}")),
            log: None,
            ..checks[0].clone()
        };
        engine.store.insert_check(&check).unwrap();
        checks.push(check);
    }
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
    assert_eq!(review.checks, checks);
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

#[test]
fn usage_detail_serves_local_day_totals_and_group_coverage_without_raw_rows() {
    use corral_types::{Usage, UsageBy};

    let fixture = Fixture::new();
    fixture.project();
    let task = fixture.named_task("document local usage", Some("demo"));
    let engine = fixture.shared.engine().unwrap();
    let mut run = fixture.dispatch(&task);
    run.started = "2026-02-01T20:00:00Z".parse().unwrap();
    run.profile = "shared-profile".into();
    engine.store.update_run(&run).unwrap();
    let mut no_usage = run.clone();
    no_usage.id = "R2".into();
    engine.store.insert_run(&no_usage).unwrap();

    for (at, input) in [
        ("2026-02-01T21:59:00Z", 19), // Previous local day; omitted.
        ("2026-02-01T22:00:00Z", 11), // Inclusive start.
        ("2026-02-02T22:00:00Z", 23), // Exclusive end.
    ] {
        engine
            .record_usage(&Usage {
                task: Some(task.clone()),
                run: Some(run.id.clone()),
                at: at.parse().unwrap(),
                model: "test-model".into(),
                profile: Some(run.profile.clone()),
                input,
                ..Usage::default()
            })
            .unwrap();
    }
    let get = |from: &str, to: &str| DetailGet {
        target: DetailRef::UsageSeries {
            from: from.into(),
            to: to.into(),
            by: Some(UsageBy::Profile),
            utc_offset_minutes: 120,
        },
        etag: None,
        page: None,
    };
    // Both runs started on the prior local day, so their cohort is outside Feb 2.
    let reply = detail(&fixture.shared, get("2026-02-02", "2026-02-03")).unwrap();
    let Some(DetailBody::UsageSeries(series)) = reply.body else {
        panic!("expected grouped usage")
    };
    let [day] = series.days.as_slice() else {
        panic!("expected one local day")
    };
    assert_eq!(day.day, "2026-02-02");
    assert_eq!(
        (day.summary.total.input, day.summary.total.requests),
        (11, 1)
    );
    assert_eq!((day.summary.covered_runs, day.summary.total_runs), (0, 0));
    let [group] = day.summary.groups.as_slice() else {
        panic!("expected one profile")
    };
    assert_eq!(group.key.as_deref(), Some("shared-profile"));
    assert_eq!((group.covered_runs, group.total_runs), (Some(0), Some(0)));

    let previous = detail(&fixture.shared, get("2026-02-01", "2026-02-02")).unwrap();
    let Some(DetailBody::UsageSeries(series)) = previous.body else {
        panic!("expected grouped usage")
    };
    let day = &series.days[0];
    assert_eq!(day.summary.total.input, 19);
    assert_eq!((day.summary.covered_runs, day.summary.total_runs), (1, 2));
    let group = &day.summary.groups[0];
    assert_eq!((group.covered_runs, group.total_runs), (Some(1), Some(2)));

    let mut request = get("2026-02-01", "2026-05-13");
    let first = detail(&fixture.shared, request.clone()).unwrap();
    request.page = first.next;
    assert!(request.page.is_some());
    let Some(DetailBody::UsageSeries(first)) = first.body else {
        panic!("expected first page")
    };
    assert_eq!(first.days.len(), 100);
    assert_eq!(first.days[0].day, "2026-02-01");
    assert_eq!(first.days[99].day, "2026-05-11");
    let last = detail(&fixture.shared, request).unwrap();
    assert!(last.next.is_none());
    let Some(DetailBody::UsageSeries(last)) = last.body else {
        panic!("expected final page")
    };
    assert_eq!(last.days.len(), 1);
    assert_eq!(last.days[0].day, "2026-05-12");
    assert_eq!(last.days[0].summary, corral_types::UsageSummary::default());
    assert!(detail(&fixture.shared, get("2026-01-01", "2027-01-03")).is_err());
}
