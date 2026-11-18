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
fn landed_task_events_include_project_in_all_views() {
    use corral_engine::{Actor, EventKind, Status, events::NewEvent};
    use corral_protocol::detail::TimelineBy;

    let fixture = Fixture::new();
    fixture.project();
    let id = fixture.named_task("Landed", Some("demo"));
    let engine = fixture.shared.engine().unwrap();
    engine
        .store
        .transaction(|store| {
            let mut task = store.require_task(&id)?;
            task.status = Status::Done;
            store.update_task(&task)?;
            store.append_event(&NewEvent {
                task: Some(id.clone()),
                run: None,
                kind: EventKind::Landed,
                actor: Actor::Owner,
                detail: serde_json::Value::Null,
            })?;
            Ok(())
        })
        .unwrap();
    let page = changes(
        &fixture.shared,
        ChangesRequest {
            query: ChangesQuery {
                after: None,
                before: None,
                limit: 50,
            },
            seen: SeenCursor {
                through: 0,
                at: None,
            },
        },
    )
    .unwrap();
    let check = |events: &[corral_types::Event]| {
        assert!(events.iter().any(|event| event.kind == EventKind::Landed));
        assert!(
            events
                .iter()
                .all(|event| event.project.as_deref() == Some("demo"))
        );
    };
    check(&page.events);
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
    let DetailBody::Task(task) = get(DetailRef::Task { id }) else {
        panic!("expected task")
    };
    check(&task.events);
    let DetailBody::Burnup(burnup) = get(DetailRef::Burnup {
        scope: "project:demo".into(),
    }) else {
        panic!("expected burnup")
    };
    check(&burnup.events);
    let DetailBody::Timeline(timeline) = get(DetailRef::Timeline {
        from: "2000-01-01T00:00:00Z".into(),
        to: "2100-01-01T00:00:00Z".into(),
        by: TimelineBy::Task,
    }) else {
        panic!("expected timeline")
    };
    check(&timeline.events);
}

#[test]
fn ended_process_terminal_detail_keeps_log_and_reports_missing_file() {
    let fixture = Fixture::new();
    fixture.project();
    let id = fixture.named_task("Process", Some("demo"));
    let mut run = fixture.dispatch(&id);
    let dir = fixture.runner.starts().pop().unwrap().dir;
    let log = dir.join("session.log");
    fs::write(&log, "retained output\n").unwrap();
    run.session = Some(SessionRef::Process { pid: 1, dir });
    run.ended = Some(jiff::Timestamp::now());
    fixture
        .shared
        .engine()
        .unwrap()
        .store
        .update_run(&run)
        .unwrap();
    let query = DetailGet {
        target: DetailRef::Terminal {
            id: format!("{}_{}", run.task, run.id),
        },
        page: None,
        etag: None,
    };
    let reply = detail(&fixture.shared, query.clone()).unwrap();
    let Some(DetailBody::Terminal(snapshot)) = reply.body else {
        panic!("expected terminal")
    };
    assert_eq!(snapshot.lines, ["retained output"]);
    assert!(!snapshot.live);
    fs::remove_file(log).unwrap();
    let error = detail(&fixture.shared, query).unwrap_err();
    assert!(error.is::<NotFound>());
    assert_eq!(error.to_string(), "session log not found");
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
    let finder = DetailRef::Artifacts {
        filter: None,
        narrowing: None,
    };
    assert!(get(finder, first.next, None).is_err());
}

#[test]
fn artifact_finder_orders_and_filters_before_stable_paging() {
    use corral_engine::{Actor, Artifact, EventKind, events::NewEvent};
    use corral_protocol::detail::ArtifactNarrowing;

    let fixture = Fixture::new();
    let site = fixture.named_task("site", Some("site-web"));
    let other = fixture.named_task("docs", Some("site-workers"));
    let engine = fixture.shared.engine().unwrap();
    let insert = |task: &str, id, at: &str, kind: &str, title: String, run: Option<&str>| {
        let artifact = Artifact {
            task: task.to_owned(),
            id,
            at: at.parse().unwrap(),
            kind: Some(kind.to_owned()),
            title,
            sha256: "unused".into(),
            bytes: 0,
            by: Actor::Agent,
            // Older catalog rows also lack these fields.
            project: None,
            run: None,
        };
        let reference = artifact.reference();
        engine
            .store
            .transaction(|store| {
                store.insert_artifact(&artifact)?;
                store.append_event(&NewEvent {
                    task: Some(task.to_owned()),
                    run: run.map(str::to_owned),
                    kind: EventKind::Artifact,
                    actor: Actor::Agent,
                    detail: serde_json::json!({"reference": reference}),
                })
            })
            .unwrap();
        reference
    };
    let large = format!("Deploy {}", "x".repeat(31 * 1024));
    let old = insert(
        &site,
        1,
        "2026-04-01T00:00:00Z",
        "nightly",
        large.clone(),
        Some("R1"),
    );
    let new = insert(
        &site,
        2,
        "2026-04-01T00:00:00.1Z",
        "Nightly",
        large,
        Some("R2"),
    );
    let tied = insert(
        &other,
        1,
        "2026-04-01T00:00:00.1Z",
        "nightly",
        "Deploy".into(),
        None,
    );
    let wrong_kind = insert(
        &site,
        3,
        "2026-04-01T00:00:00.2Z",
        "manual",
        "Deploy nightly".into(),
        None,
    );
    let wrong_word = insert(
        &site,
        4,
        "2026-04-01T00:00:00.3Z",
        "nightly",
        "Review".into(),
        None,
    );

    let get =
        |target, page, etag| detail(&fixture.shared, DetailGet { target, page, etag }).unwrap();
    let artifacts = |reply: &DetailReply| match &reply.body {
        Some(DetailBody::Artifacts(page)) => page.rows.clone(),
        _ => panic!("expected artifacts"),
    };
    let refs = |reply: &DetailReply| {
        artifacts(reply)
            .iter()
            .map(Artifact::reference)
            .collect::<Vec<_>>()
    };

    let all = DetailRef::Artifacts {
        filter: None,
        narrowing: None,
    };
    let first = get(all.clone(), None, None);
    assert_eq!(refs(&first), [wrong_word, wrong_kind, tied, new.clone()]);
    assert_eq!(
        refs(&get(all, first.next, None)).as_slice(),
        std::slice::from_ref(&old)
    );

    let narrowing = ArtifactNarrowing {
        kind: Some("nig".into()),
        task: Some(site.clone()),
        project: Some("SITE-W".into()),
    };
    let search = DetailRef::Artifacts {
        filter: Some("DEPLOY nightly".into()),
        narrowing: Some(narrowing.clone()),
    };
    let first = get(search.clone(), None, None);
    assert_eq!(refs(&first), [new]);
    let page = first.next.unwrap();
    let second = get(search.clone(), Some(page.clone()), None);
    let rows = artifacts(&second);
    assert_eq!(refs(&second), [old]);
    assert_eq!(rows[0].project.as_deref(), Some("site-web"));
    assert_eq!(rows[0].run.as_deref(), Some("R1"));
    assert!(second.next.is_none());

    let mismatched_project = DetailRef::Artifacts {
        filter: Some("DEPLOY nightly".into()),
        narrowing: Some(ArtifactNarrowing {
            project: Some("docs".into()),
            ..narrowing
        }),
    };
    assert!(refs(&get(mismatched_project, None, None)).is_empty());

    // A matching row added before the first page's anchor does not invalidate
    // a client holding the second page or its cached etag.
    insert(
        &site,
        5,
        "2026-04-01T00:00:01Z",
        "nightly",
        "Deploy".into(),
        None,
    );
    let unchanged = get(search, Some(page), Some(second.etag.clone()));
    assert_eq!(unchanged.etag, second.etag);
    assert!(unchanged.body.is_none());
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
            round: Some(round),
            run: None,
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
            time_zone: None,
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

#[test]
fn named_usage_zone_buckets_and_bounds_follow_the_clock_change() {
    let fixture = Fixture::new();
    let engine = fixture.shared.engine().unwrap();
    for (at, input) in [
        ("2026-03-08T05:00:00Z", 1), // First midnight, still EST.
        ("2026-03-09T03:59:59Z", 2),
        ("2026-03-09T04:00:00Z", 4), // Next midnight, now EDT.
        ("2026-03-10T04:00:00Z", 8), // Exclusive upper bound.
    ] {
        engine
            .record_usage(&corral_types::Usage {
                at: at.parse().unwrap(),
                model: "test-model".into(),
                input,
                ..Default::default()
            })
            .unwrap();
    }
    let query = serde_json::from_value(serde_json::json!({
        "kind": "usage_series", "from": "2026-03-08", "to": "2026-03-10",
        "utc_offset_minutes": -300, "time_zone": "America/New_York"
    }))
    .unwrap();
    let reply = detail(&fixture.shared, query).unwrap();
    let Some(DetailBody::UsageSeries(series)) = reply.body else {
        panic!("expected usage series");
    };
    let totals: Vec<_> = series
        .days
        .iter()
        .map(|day| (day.day.as_str(), day.summary.total.input))
        .collect();
    assert_eq!(totals, [("2026-03-08", 3), ("2026-03-09", 4)]);
}

#[test]
fn burnup_includes_drops_and_reopens_in_event_order() {
    use corral_engine::EventKind;

    let fixture = Fixture::new();
    let id = fixture.named_task("Scope changes", None);
    fixture.apply(Op::Drop { id: id.clone() });
    fixture.apply(Op::Reopen { id: id.clone() });
    fixture.apply(Op::Comment {
        id,
        text: "Unrelated to scope".into(),
    });
    let reply = detail(
        &fixture.shared,
        DetailGet {
            target: DetailRef::Burnup {
                scope: "workspace".into(),
            },
            page: None,
            etag: None,
        },
    )
    .unwrap();
    let Some(DetailBody::Burnup(series)) = reply.body else {
        panic!("expected burnup");
    };
    let kinds: Vec<_> = series.events.iter().map(|event| event.kind).collect();
    assert_eq!(
        kinds,
        [EventKind::Created, EventKind::Dropped, EventKind::Reopened]
    );
}
