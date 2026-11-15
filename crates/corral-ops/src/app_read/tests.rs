use super::*;
use corral_protocol::detail::{Burnup, BurnupPoint, DetailBody};

#[test]
fn detail_pages_reassemble_every_typed_array_item() -> Result<()> {
    let points: Vec<_> = (0..3000)
        .map(|index| BurnupPoint {
            at: "2026-11-14T00:00:00Z".to_owned(),
            total: index,
            landed: index / 2,
        })
        .collect();
    let full = DetailBody::Burnup(Burnup {
        scope: "all".to_owned(),
        points: points.clone(),
        completeness: Default::default(),
    });
    let mut offset = 0;
    let mut assembled = Vec::new();
    loop {
        let (body, next) = paging::body_page(full.clone(), offset)?;
        let reply = paging::reply(body, next, "a".repeat(64), "selector".to_owned(), now(), 1)?;
        assert!(paging::json_len(&reply)? < corral_protocol::app::APP_CONTROL_MAX);
        let Some(DetailBody::Burnup(page)) = reply.body else {
            panic!("wrong detail body");
        };
        assembled.extend(page.points);
        let Some(next) = next else {
            break;
        };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(assembled, points);
    Ok(())
}

#[test]
fn continuation_rejects_another_selector_or_content_revision() -> Result<()> {
    let reply = paging::reply(
        DetailBody::Burnup(Burnup {
            scope: "all".to_owned(),
            points: vec![],
            completeness: Default::default(),
        }),
        Some(1),
        "a".repeat(64),
        "task-one".to_owned(),
        now(),
        1,
    )?;
    let cursor = paging::decode(reply.next.as_deref(), "task-one")?;
    assert!(paging::check(cursor.as_ref(), &"b".repeat(64)).is_err());
    assert!(paging::decode(reply.next.as_deref(), "task-two").is_err());
    assert!(paging::decode(Some("{}"), "task-one").is_err());
    Ok(())
}

#[test]
fn utf8_bounds_and_oversized_metadata_fail_explicitly() -> Result<()> {
    assert_eq!(short("aéz", 2), "a");
    assert_eq!(short("aéz", 3), "aé");
    let body = DetailBody::Burnup(Burnup {
        scope: "x".repeat(70 * 1024),
        points: vec![],
        completeness: Default::default(),
    });
    assert!(paging::body_page(body, 0).is_err());
    Ok(())
}

#[test]
fn fixed_task_rails_remain_valid_on_every_page() -> Result<()> {
    use corral_protocol::{
        board::{RailGlyph, RailStep},
        detail::{ArtifactRef, TaskDetail},
    };
    let full = DetailBody::Task(TaskDetail {
        id: "T-1".into(),
        owner_revision: "canonical".into(),
        title: "Task".into(),
        status: "in_progress".into(),
        body_excerpt: String::new(),
        stages: std::array::from_fn(|_| RailStep {
            glyph: RailGlyph::Unavailable,
            sha7: None,
            at: None,
        }),
        dependencies: vec![],
        progress: None,
        owner_next: None,
        hold: None,
        park: None,
        artifacts: (0..1000)
            .map(|index| ArtifactRef {
                source_ref: format!("T-1:R1:{index}"),
                title: "x".repeat(100),
                media_type: None,
            })
            .collect(),
        run: None,
        plan_context: None,
        log_tail: vec![],
    });
    let mut offset = 0;
    let mut count = 0;
    loop {
        let (body, next) = paging::body_page(full.clone(), offset)?;
        let DetailBody::Task(page) = body else {
            panic!("wrong body");
        };
        assert_eq!(page.stages.len(), 4);
        count += page.artifacts.len();
        let Some(next) = next else {
            break;
        };
        offset = next;
    }
    assert_eq!(count, 1000);
    Ok(())
}

#[test]
fn large_annotation_preserves_all_messages_with_same_canonical_identity() -> Result<()> {
    use corral_protocol::{
        cmd::annotation::AnnotationSide,
        detail::{AnnotationMessage, ReviewAnnotation, ReviewDetail},
    };
    let full = DetailBody::Review(ReviewDetail {
        task: "T-1".into(),
        candidate_sha: None,
        base: None,
        state: "review".into(),
        verdicts: vec![],
        files: vec![],
        report: None,
        acceptance: vec![],
        gate_log_tail: vec![],
        artifacts: vec![],
        approvable: false,
        annotations: vec![ReviewAnnotation {
            id: "thread-1".into(),
            revision: 1,
            run: "R1".into(),
            base: "base".into(),
            head: "head".into(),
            path: "file.rs".into(),
            side: AnnotationSide::New,
            range: None,
            resolved: false,
            stale: false,
            messages: (0..64)
                .map(|index| AnnotationMessage {
                    by: "owner".into(),
                    at: format!("2026-11-14T00:00:{index:02}Z"),
                    text: "x".repeat(4096),
                })
                .collect(),
        }],
    });
    let mut offset = 0;
    let mut messages = 0;
    loop {
        let (body, next) = paging::body_page(full.clone(), offset)?;
        assert!(paging::json_len(&body)? < corral_protocol::app::APP_CONTROL_MAX - 2048);
        let DetailBody::Review(page) = body else {
            panic!("wrong body");
        };
        for annotation in page.annotations {
            assert_eq!(annotation.id, "thread-1");
            messages += annotation.messages.len();
        }
        let Some(next) = next else {
            break;
        };
        offset = next;
    }
    assert_eq!(messages, 64);
    Ok(())
}

#[test]
fn assistant_feed_usage_reads_bound_transcripts_without_a_stored_snapshot() -> Result<()> {
    use corral_core::Store;
    use corral_protocol::board::CostBasis;

    let tmp = tempfile::tempdir()?;
    let store = Store::init(tmp.path())?;
    let session = store.create_assistant("pi", None, None, None)?;
    let transcript = tmp.path().join("session.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            "{\"type\":\"session\",\"id\":\"sid\"}\n",
            "{\"type\":\"message\",\"id\":\"m1\",",
            "\"message\":{\"role\":\"assistant\",\"provider\":\"llama.cpp\",",
            "\"usage\":{\"input\":23,\"output\":7,\"cost\":{\"total\":0}}}}\n"
        ),
    )?;
    store.update_assistant_container(&session.id, |container| {
        for n in 1..=2 {
            container
                .state
                .generations
                .push(serde_json::from_value(serde_json::json!({
                    "n": n, "bind_nonce_sha256": "nonce", "pane": "pane",
                    "created": session.created,
                    "bound": {
                        "host_cap_sha256": "host", "ctl_cap_sha256": "ctl", "session_id": "sid",
                        "session_file": transcript, "pid": 123, "start_time": "start",
                        "request_id": "hello", "at": session.created
                    }
                }))?);
        }
        container.state.binding.generation = 2;
        Ok(())
    })?;
    let unknown = store.create_assistant("pi", None, None, None)?;
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    for ended in [false, true] {
        if ended {
            ctx.store.end_assistant(&session.id)?;
        }
        assert!(ctx.store.get_assistant(&session.id)?.usage.is_none());
        let assistants = assistant_rows(&ctx)?;
        let assistant = assistants.iter().find(|row| row.id == session.id).unwrap();
        assert_eq!(assistant.cost_usd, Some(0.0));
        assert_eq!(assistant.cost_basis, Some(CostBasis::Reported));
    }
    // The agents feed lists ended sessions inside its window, with the same accounting.
    ctx.store.end_assistant(&unknown.id)?;
    let agents = agents_rows(&ctx)?;
    let agent = agents.iter().find(|row| row.id == session.id).unwrap();
    assert_eq!(
        agent.tokens,
        Some(30),
        "resumed generations must not duplicate usage"
    );
    assert_eq!(agent.cost_usd, Some(0.0));
    assert_eq!(agent.cost_basis, Some(CostBasis::Reported));
    let missing = agents.iter().find(|row| row.id == unknown.id).unwrap();
    assert_eq!(missing.tokens, None);
    assert_eq!(missing.cost_usd, None);
    assert_eq!(missing.cost_basis, None);
    Ok(())
}

/// A task with one run `R1`; returns the run's session id on the agents feed.
fn task_with_run(
    store: &corral_core::Store,
    status: corral_core::Status,
    ended: Option<jiff::Timestamp>,
    transcript: Option<&std::path::Path>,
) -> Result<String> {
    let mut task = store.create("fixture", None, status, "")?;
    let started = corral_core::clock::now() - jiff::SignedDuration::from_hours(5);
    task.runs.push(serde_json::from_value(serde_json::json!({
        "id": "R1", "agent": "fixture", "worktree": "/unused", "branch": "fixture",
        "base": "main", "base_sha": "fixture", "started": started, "ended": ended,
        "session": transcript.map(|path| serde_json::json!({
            "id": "sid", "policy": "reuse", "directory": "/unused", "harness": "pi",
            "action": "new", "transcript": path,
        })),
    }))?);
    store.fixture_put_task(&task)?;
    Ok(format!("{}/run/R1", task.id))
}

#[test]
fn agents_feed_lists_running_and_recently_ended_sessions_only() -> Result<()> {
    use corral_core::{Status, Store};
    use corral_protocol::rows::AgentStatus;

    let tmp = tempfile::tempdir()?;
    let store = Store::init(tmp.path())?;
    let ago = |minutes| corral_core::clock::now() - jiff::SignedDuration::from_mins(minutes);
    let running = task_with_run(&store, Status::Running, None, None)?;
    let just_ended = task_with_run(&store, Status::Backlog, Some(ago(10)), None)?;
    let landed = task_with_run(&store, Status::Done, Some(ago(10)), None)?;
    let long_ended = task_with_run(&store, Status::Backlog, Some(ago(120)), None)?;
    // A resumable assistant session with no host runs nowhere.
    let cold = store.create_assistant("pi", None, None, None)?;
    let mut ctx = crate::resources::FixedResourceProbe::ctx(store);
    let status = |ctx: &Ctx, id: &str| -> Result<Option<AgentStatus>> {
        let rows = agents_rows(ctx)?;
        Ok(rows.iter().find(|row| row.id == id).map(|row| row.status))
    };
    assert_eq!(status(&ctx, &running)?, Some(AgentStatus::Working));
    assert_eq!(status(&ctx, &just_ended)?, Some(AgentStatus::Ended));
    assert_eq!(status(&ctx, &landed)?, Some(AgentStatus::Ended));
    assert_eq!(status(&ctx, &long_ended)?, None);
    assert_eq!(status(&ctx, &cold.id)?, None);

    ctx.store.config.ui.agents_ended_window = "3h".into();
    assert_eq!(status(&ctx, &long_ended)?, Some(AgentStatus::Ended));
    Ok(())
}

#[test]
fn a_session_page_is_one_window_whose_older_cursor_outlives_later_writes() -> Result<()> {
    use corral_core::{Status, Store};

    let tmp = tempfile::tempdir()?;
    let store = Store::init(tmp.path())?;
    let transcript = tmp.path().join("session.jsonl");
    let mut text = String::new();
    for index in 0..40 {
        text.push_str(&serde_json::to_string(&serde_json::json!({
            "message": { "role": "user", "content": format!("message-{index}") }
        }))?);
        text.push('\n');
    }
    std::fs::write(&transcript, text)?;
    let id = task_with_run(&store, Status::Running, None, Some(&transcript))?;
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let get =
        |page: Option<String>| -> Result<(DetailReply, corral_protocol::detail::SessionPage)> {
            let mut reply = read(
                &ctx,
                &DetailGet {
                    target: DetailRef::Session { id: id.clone() },
                    etag: None,
                    page,
                },
            )?;
            let Some(DetailBody::Session(page)) = reply.body.take() else {
                panic!("expected a session page");
            };
            Ok((reply, page))
        };
    let (reply, latest) = get(None)?;
    // A continuation would have the client fetch and assemble the whole transcript.
    assert!(reply.next.is_none());
    assert_eq!(latest.messages.len(), 32);

    ctx.store
        .create("a later write", None, Status::Backlog, "")?;
    let (reply, older) = get(latest.older)?;
    assert!(reply.next.is_none());
    assert_eq!(older.messages.len(), 8);
    assert!(older.older.is_none());
    Ok(())
}

#[cfg(unix)]
fn live_terminal_fixture() -> Result<(tempfile::TempDir, Ctx, String)> {
    let tmp = tempfile::tempdir()?;
    let store = corral_core::Store::init(tmp.path())?;
    let id = task_with_run(&store, corral_core::Status::Running, None, None)?;
    let bin = tmp.path().join("herdr");
    crate::agent::test_support::write_executable(
        &bin,
        concat!(
            "#!/bin/sh\n",
            "[ \"$*\" = '--session terminal-fixture pane read pane-live --source visible --format ansi' ] || exit 70\n",
            "dir=$(dirname \"$0\")\n",
            "printf called > \"$dir/probed\"\n",
            "cat \"$dir/screen\"\n",
        ),
    );
    let herdr = corral_herdr::Herdr::with_bin(bin, Some("terminal-fixture".into()));
    let mut task = store.get(id.split('/').next().unwrap())?;
    task.runs[0].herdr = Some(corral_core::HerdrRef {
        workspace_id: "workspace".into(),
        pane_id: "pane-live".into(),
        socket: Some(herdr.socket_path()?),
    });
    store.fixture_put_task(&task)?;
    let mut ctx = crate::resources::FixedResourceProbe::ctx(store);
    ctx.herdr = herdr;
    Ok((tmp, ctx, id))
}

#[test]
#[cfg(unix)]
fn live_terminal_preserves_screen_columns_and_reuses_unchanged_etag() -> Result<()> {
    let (tmp, ctx, id) = live_terminal_fixture()?;
    std::fs::write(
        tmp.path().join("screen"),
        "  \x1b[38;2;10;20;30;1mheading\x1b[0m   \n  ┌─┐  \n\n  │ x │\u{7}   \n   \n\n",
    )?;
    let rows = agents_rows(&ctx)?;
    assert_eq!(
        rows.iter()
            .find(|row| row.id == id)
            .unwrap()
            .pane
            .as_deref(),
        Some("pane-live")
    );
    assert!(!tmp.path().join("probed").exists());
    let mut request = DetailGet {
        target: DetailRef::Terminal { id },
        etag: None,
        page: None,
    };
    let first = read(&ctx, &request)?;
    let Some(DetailBody::Terminal(snapshot)) = &first.body else {
        panic!("expected a terminal snapshot");
    };
    assert!(snapshot.live);
    assert_eq!(snapshot.lines, ["  heading", "  ┌─┐", "", "  │ x │"]);
    assert_eq!(snapshot.lines.len(), snapshot.rows.len());
    assert_eq!(
        snapshot.rows[0][1].fg,
        Some(corral_protocol::detail::session::TerminalColor::Rgb([
            10, 20, 30
        ]))
    );
    assert!(snapshot.rows[0][1].bold);
    snapshot.captured_at.parse::<jiff::Timestamp>()?;
    assert!(first.next.is_none());
    request.etag = Some(first.etag.clone());
    let unchanged = read(&ctx, &request)?;
    assert_eq!(unchanged.etag, first.etag);
    assert!(unchanged.body.is_none());

    std::fs::write(
        tmp.path().join("screen"),
        "  \x1b[38;2;30;20;10;1mheading\x1b[0m   \n  ┌─┐  \n\n  │ x │\u{7}   \n   \n\n",
    )?;
    let recoloured = read(&ctx, &request)?;
    let Some(DetailBody::Terminal(recoloured_screen)) = &recoloured.body else {
        panic!("expected a colour-only terminal change");
    };
    assert_eq!(recoloured_screen.lines, snapshot.lines);
    assert_ne!(recoloured_screen.rows, snapshot.rows);
    assert_ne!(recoloured.etag, first.etag);
    request.etag = Some(recoloured.etag);

    let large = (0..220)
        .map(|n| format!("{n:03} {}é\n", "\"\\".repeat(1020)))
        .collect::<String>();
    std::fs::write(tmp.path().join("screen"), large)?;
    let changed = read(&ctx, &request)?;
    let Some(DetailBody::Terminal(snapshot)) = changed.body else {
        panic!("expected a changed terminal snapshot");
    };
    assert_ne!(changed.etag, first.etag);
    assert!(snapshot.lines.first().unwrap().starts_with("210 "));
    assert!(snapshot.lines.last().unwrap().starts_with("219 "));
    assert!(
        snapshot
            .lines
            .iter()
            .all(|row| row.len() <= 2048 && row.ends_with('é'))
    );
    assert!(serde_json::to_vec(&snapshot.lines)?.len() <= 40 * 1024);
    Ok(())
}

#[test]
#[cfg(unix)]
fn foreign_and_unowned_terminal_panes_are_refused_before_herdr_reads() -> Result<()> {
    let (tmp, ctx, id) = live_terminal_fixture()?;
    let mut task = ctx.store.get(id.split('/').next().unwrap())?;
    let foreign = corral_herdr::Herdr::with_bin(
        tmp.path().join("herdr"),
        Some("foreign-terminal-fixture".into()),
    )
    .socket_path()?;
    for socket in [Some(foreign), None] {
        task.runs[0].herdr.as_mut().unwrap().socket = socket;
        ctx.store.fixture_put_task(&task)?;
        let error = read(
            &ctx,
            &DetailGet {
                target: DetailRef::Terminal { id: id.clone() },
                etag: None,
                page: None,
            },
        )
        .unwrap_err();
        assert!(!error.to_string().starts_with("not_found"));
        assert!(!tmp.path().join("probed").exists());
    }
    task.runs[0].herdr.as_mut().unwrap().socket = Some(ctx.herdr.socket_path()?);
    ctx.store.fixture_put_task(&task)?;
    std::fs::write(tmp.path().join("screen"), "\x1b[42;3mowned\x1b[0m\n")?;
    let reply = read(
        &ctx,
        &DetailGet {
            target: DetailRef::Terminal { id },
            etag: None,
            page: None,
        },
    )?;
    let Some(DetailBody::Terminal(snapshot)) = reply.body else {
        panic!("expected an owned terminal snapshot");
    };
    assert_eq!(snapshot.lines, ["owned"]);
    assert_eq!(
        snapshot.rows[0][0].bg,
        Some(corral_protocol::detail::session::TerminalColor::Indexed(2))
    );
    assert!(snapshot.rows[0][0].italic);
    Ok(())
}
