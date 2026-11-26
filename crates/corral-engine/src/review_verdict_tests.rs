use super::*;
use crate::{SessionRef, events::NewEvent};
use serde_json::json;

// Keep the fake runner's placement; record the Pi directory that a real host
// would use. The package's daemon journey exercises actual launch and transport.
fn authorize(cx: &Fixture, reviewer: &mut Run, socket: &Path) -> String {
    let task = cx.task(&reviewer.task);
    reviewer.harness = "pi".into();
    if !matches!(reviewer.session, Some(SessionRef::Pi { .. })) {
        let dir = cx
            .engine
            .state_dir
            .join("runs")
            .join(&task.id)
            .join("reviewer");
        fs::create_dir_all(&dir).unwrap();
        reviewer.session = Some(SessionRef::Pi {
            inner: Box::new(reviewer.session.take().unwrap()),
            socket: socket.to_owned(),
            dir,
        });
    }
    cx.engine
        .store
        .transaction(|store| {
            store.update_run(reviewer)?;
            let request = store.pi_review_request(&task)?.id;
            store.append_event(&NewEvent {
                task: Some(task.id),
                run: Some(reviewer.id.clone()),
                kind: EventKind::ReviewStarted,
                actor: Actor::Auto,
                detail: json!({"round": task.round, "request": request}),
            })?;
            Ok(request)
        })
        .unwrap()
}

fn save(reviewer: &Run, request: &str, verdict: &str) {
    let Some(SessionRef::Pi { dir, .. }) = &reviewer.session else {
        panic!("expected Pi session");
    };
    let file = dir.join(format!("verdict-{}-{request}", reviewer.id));
    fs::write(
        file.with_extension("json"),
        serde_json::to_vec(&json!({"verdict": verdict, "notes": "Retained\nnotes"})).unwrap(),
    )
    .unwrap();
    fs::write(file.with_extension("error"), "ENOENT: executable missing").unwrap();
}

#[test]
fn saved_verdict_retries_after_reviewer_loss_and_rejects_superseded_request() {
    let mut cx = Fixture::new(&[]);
    let worktree = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap();
    let runtime = tempfile::Builder::new()
        .prefix("pi-")
        .tempdir_in(worktree)
        .unwrap();
    let socket = runtime.path().join("absent.sock");
    cx.project().auto_land = Some(false);
    let id = cx.backlog("review with a saved verdict");
    let worker = cx.dispatch(&id);
    fs::write(worker.worktree.join("file.txt"), "candidate\n").unwrap();
    let sha = cx.done(&id).run.unwrap().head_sha.unwrap();
    let mut reviewer = cx.engine.review(&id, Some("generic")).unwrap();
    let request = authorize(&cx, &mut reviewer, &socket);
    let pending = || cx.engine.pending_review_verdict(&cx.task(&id)).unwrap();
    let retry = |request| cx.engine.record_review_verdict(&id, &reviewer.id, request);

    save(&reviewer, &request, "changes");
    let shown = pending().unwrap();
    assert_eq!(shown.run, reviewer.id);
    assert_eq!(shown.verdict, "changes");
    assert!(shown.reason.contains("ENOENT"));
    cx.engine.advance(&id).unwrap();
    assert!(cx.events(&id, EventKind::Verdict).is_empty());
    let Some(SessionRef::Pi { inner, .. }) = &reviewer.session else {
        panic!("expected Pi session");
    };
    cx.runner.set_status(inner, SessionStatus::Gone);
    cx.engine.reconcile(&reviewer).unwrap();
    assert_eq!(cx.task(&id).status, Status::Blocked);
    assert_eq!(pending().unwrap().request, request);
    retry(&request).unwrap();
    assert_eq!(
        (cx.task(&id).status, cx.task(&id).round),
        (Status::Running, 1)
    );
    assert!(pending().is_none());
    let [event]: [Event; 1] = cx.events(&id, EventKind::Verdict).try_into().unwrap();
    assert_eq!(event.detail["notes"], "Retained\nnotes");
    assert_eq!(event.detail["request"], request);

    cx.done(&id);
    let mut replacement = cx.engine.review(&id, Some("generic")).unwrap();
    let old_request = authorize(&cx, &mut replacement, &socket);
    save(&replacement, &old_request, "changes");
    assert_eq!(pending().unwrap().request, old_request);
    cx.engine
        .changes(&id, "owner requests another review")
        .unwrap();
    assert_eq!(
        cx.done(&id).run.unwrap().head_sha.as_deref(),
        Some(sha.as_str())
    );
    let new_request = authorize(&cx, &mut replacement, &socket);
    assert_ne!(old_request, new_request);
    assert!(pending().is_none());
    assert!(
        cx.engine
            .record_review_verdict(&id, &replacement.id, &old_request)
            .is_err()
    );
    retry(&request).unwrap(); // Lost response to the original, committed verdict.
    assert_eq!(cx.task(&id).status, Status::Review);
    assert_eq!(cx.events(&id, EventKind::Verdict).len(), 1);

    // Separate receipt from pipeline advancement as a detached CLI does.
    cx.project().auto_land = Some(true);
    cx.engine.pipeline = PipelineMode::Off;
    save(&replacement, &new_request, "approve");
    cx.engine
        .record_review_verdict(&id, &replacement.id, &new_request)
        .unwrap();
    fs::write(worker.worktree.join("file.txt"), "unreviewed follow-up\n").unwrap();
    git(&worker.worktree, &["commit", "-am", "unreviewed"]);
    assert!(as_refusal(&cx.engine.advance(&id).unwrap_err()).is_some());
    assert!(
        as_refusal(
            &cx.engine
                .approve_as(&id, None, false, Actor::Auto)
                .unwrap_err()
        )
        .is_some()
    );
    assert_eq!(cx.task(&id).status, Status::Review);
    assert!(cx.branch_exists(&worker.branch));
}
