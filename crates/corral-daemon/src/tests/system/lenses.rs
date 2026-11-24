use corral_protocol::app::SYSTEM_LENSES_V1;
use corral_types::lenses::LensStatus;

use super::*;

#[test]
fn socket_lenses_follow_the_landed_head_and_review_rows_are_opt_in() {
    let fixture = Fixture::with_repository();
    let repo = fixture.root().join("repo");
    let read_at = corral_facts::resolve_revision(&repo, "HEAD").unwrap();
    fs::create_dir_all(repo.join("diagrams")).unwrap();
    let diagram = |id: &str, anchor: &str, note: &str| {
        format!(
            r#"
id = "{id}"
title = "Worker run"
kind = "sequence"
read_at = "{}"

[[participants]]
id = "worker"
label = "Worker"
anchor = {anchor}

[[messages]]
from = "worker"
to = "worker"
label = "Runs"
note = "{note}"
anchor = {anchor}
"#,
            read_at.commit(),
        )
    };
    fs::write(
        repo.join("diagrams/run.toml"),
        diagram("run", r#"{path = "note", text = "base"}"#, ""),
    )
    .unwrap();
    fs::write(
        repo.join("diagrams/oversized.toml"),
        diagram(
            "oversized",
            r#"{path = ".gitignore"}"#,
            &"x".repeat(corral_protocol::app::APP_CONTROL_MAX),
        ),
    )
    .unwrap();
    const MALFORMED: &str = "diagrams/broken.toml";
    fs::write(repo.join(MALFORMED), "kind = 'nonsense'\n").unwrap();
    git(&repo, &["add", "diagrams"]);
    git(&repo, &["commit", "-m", "add lens examples"]);
    let base = corral_facts::resolve_revision(&repo, "HEAD").unwrap();
    let config = fixture.root().join(".corral-next/corral.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str("lenses = 'diagrams'\n");
    fs::write(config, text).unwrap();

    let engine = fixture.shared().engine().unwrap();
    let task = engine
        .apply(
            Op::New {
                task: NewTask {
                    title: "change the anchored line".into(),
                    project: Some("demo".into()),
                    ..NewTask::default()
                },
            },
            false,
        )
        .unwrap()
        .task
        .unwrap()
        .id;
    let run = engine.apply(dispatch(&task), false).unwrap().run.unwrap();
    fs::write(run.worktree.join("note"), "changed\n").unwrap();
    let submitted = engine
        .apply(
            Op::Done {
                id: task.clone(),
                summary: "Change the note".into(),
            },
            false,
        )
        .unwrap()
        .run
        .unwrap();
    let head = submitted.head_sha.unwrap();

    let _poll = PollGuard::start(fixture.shared());
    let path = fixture.unix_server();
    let project = "demo".to_owned();
    let list = DetailRef::SystemLenses {
        project: project.clone(),
    };
    let lens = |id: &str, at: Option<&str>| DetailRef::SystemLens {
        project: project.clone(),
        id: id.into(),
        at: at.map(str::to_owned),
    };
    let (mut unsupported, welcome) = connect(&path, vec![SYSTEM_LENSES_V1.into()]);
    assert!(!welcome.caps.iter().any(|cap| cap == SYSTEM_LENSES_V1));
    for target in [list.clone(), lens("run", None)] {
        let error = unsupported.call(DETAIL_GET, query(target)).unwrap_err();
        assert_eq!(error.code, rpc::ErrorCode::InvalidParams);
    }
    let (mut system, _) = connect(&path, vec![SYSTEM_V1.into()]);
    let (mut opted_in, welcome) = connect(&path, vec![SYSTEM_V1.into(), SYSTEM_LENSES_V1.into()]);
    assert!(welcome.caps.iter().any(|cap| cap == SYSTEM_LENSES_V1));
    let initial = await_detail(&mut system, list.clone(), |body| match body {
        DetailBody::SystemLenses(list) if list.status == SystemStatus::Ready => Some(list),
        _ => None,
    });
    assert_eq!(initial.head.as_deref(), Some(base.commit()));
    assert!(
        initial
            .rows
            .iter()
            .any(|row| row.id.as_deref() == Some("run") && row.status == LensStatus::Current)
    );
    assert!(
        initial
            .rows
            .iter()
            .any(|row| row.file == MALFORMED && row.id.is_none())
    );
    let error = system
        .call(DETAIL_GET, query(lens("oversized", None)))
        .unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::TooLarge);
    let malformed = lens_detail(&mut system, lens(MALFORMED, None));
    assert_eq!(malformed.lens.unwrap().file, MALFORMED);
    let historical = lens_detail(&mut system, lens("run", Some(base.commit())));
    assert_eq!(historical.commit.as_deref(), Some(base.commit()));
    assert_eq!(historical.lens.unwrap().status, LensStatus::Current);

    // A symbolic selector that failed must recover after a Git change with
    // no Corral event. Observe the recovered revision, not a fixed sleep.
    let late = lens("run", Some("late-tag"));
    let unavailable = lens_detail(&mut system, late.clone());
    assert!(unavailable.error.is_some());
    git(&repo, &["tag", "late-tag", base.commit()]);
    let recovered = await_detail(&mut system, late, |body| match body {
        DetailBody::SystemLens(lens) if lens.commit.as_deref() == Some(base.commit()) => Some(lens),
        _ => None,
    });
    assert_eq!(recovered.lens.unwrap().status, LensStatus::Current);

    let review = DetailRef::SystemReview {
        task: task.clone(),
        at: None,
    };
    let accepted = await_detail(&mut opted_in, review.clone(), |body| match body {
        DetailBody::SystemReview(review) if review.status == SystemStatus::Ready => Some(review),
        _ => None,
    });
    assert!(
        accepted
            .categories
            .iter()
            .any(|row| row.category == SystemImpactCategory::Lenses
                && row.provided
                && row.total == 1)
    );
    assert!(accepted.rows.iter().any(|row| matches!(row,
        SystemReviewRow::Lens { impact } if impact.id == "run" && !impact.touched.is_empty()
    )));
    let ordinary: DetailReply = system.ok(DETAIL_GET, query(review));
    let Some(DetailBody::SystemReview(ordinary)) = ordinary.body else {
        panic!("expected review detail")
    };
    assert!(
        !ordinary
            .categories
            .iter()
            .any(|row| row.category == SystemImpactCategory::Lenses)
    );
    assert!(
        !ordinary
            .rows
            .iter()
            .any(|row| matches!(row, SystemReviewRow::Lens { .. }))
    );

    engine
        .apply(
            Op::Approve {
                id: task,
                onto: None,
            },
            false,
        )
        .unwrap();
    let landed = await_detail(&mut system, list, |body| match body {
        DetailBody::SystemLenses(list) if list.head.as_deref() == Some(head.as_str()) => Some(list),
        _ => None,
    });
    assert!(
        landed
            .rows
            .iter()
            .any(|row| row.id.as_deref() == Some("run") && row.status == LensStatus::Stale)
    );
    let current = lens_detail(&mut system, lens("run", None));
    assert_eq!(current.commit.as_deref(), Some(head.as_str()));
    assert_eq!(current.lens.unwrap().status, LensStatus::Stale);
    let historical = lens_detail(&mut system, lens("run", Some(base.commit())));
    assert_eq!(historical.lens.unwrap().status, LensStatus::Current);
}

fn query(target: DetailRef) -> DetailGet {
    DetailGet {
        target,
        etag: None,
        page: None,
    }
}

fn lens_detail(client: &mut Client, target: DetailRef) -> corral_types::SystemLens {
    await_detail(client, target, |body| match body {
        DetailBody::SystemLens(lens) if lens.status == SystemStatus::Ready => Some(lens),
        _ => None,
    })
}

fn await_detail<T>(
    client: &mut Client,
    target: DetailRef,
    mut select: impl FnMut(DetailBody) -> Option<T>,
) -> T {
    let deadline = Instant::now() + WAIT;
    let query = query(target);
    loop {
        let reply: DetailReply = client.ok(DETAIL_GET, &query);
        if let Some(result) = reply.body.and_then(&mut select) {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "detail did not become ready: {:?}",
            query.target
        );
        thread::sleep(Duration::from_millis(10));
    }
}
