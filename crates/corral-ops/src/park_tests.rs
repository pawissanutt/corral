use super::*;
use corral_core::{Store, parse_park_check};

fn ctx() -> (tempfile::TempDir, Ctx) {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(tmp.path()).unwrap());
    (tmp, ctx)
}

fn now() -> Timestamp {
    "2099-01-01T00:00:00Z".parse().unwrap()
}

#[test]
fn park_and_unpark_preserve_hold_and_refuse_stale_or_owner_revision() {
    // Given an owner-held Ready task and a PM park.
    let (_tmp, ctx) = ctx();
    let task = ctx.store.create("Park", None, Status::Ready, "").unwrap();
    ctx.store
        .update(&task.id, |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let park = park_as(
        &ctx,
        &task.id,
        "wait",
        &[],
        "resume",
        86400,
        (now(), Responsible::Pm),
    )
    .unwrap();
    let before = ctx.store.get(&task.id).unwrap();
    // When a stale revision is used, then neither the park nor the hold changes.
    assert!(unpark_as(&ctx, &task.id, park.revision - 1, Responsible::Pm).is_err());
    assert_eq!(ctx.store.get(&task.id).unwrap(), before);
    unpark_as(&ctx, &task.id, park.revision, Responsible::Pm).unwrap();
    assert!(ctx.store.get(&task.id).unwrap().dispatch_hold);
    let owner = park_as(
        &ctx,
        &task.id,
        "owner",
        &[],
        "",
        86400,
        (now(), Responsible::Owner),
    )
    .unwrap();
    let before = ctx.store.get(&task.id).unwrap();
    assert!(unpark_as(&ctx, &task.id, owner.revision, Responsible::Pm).is_err());
    assert!(
        park_as(
            &ctx,
            &task.id,
            "replace",
            &[],
            "",
            86400,
            (now(), Responsible::Pm)
        )
        .is_err()
    );
    assert_eq!(ctx.store.get(&task.id).unwrap(), before);
}

#[test]
fn invalid_parks_are_refused_without_writes() {
    // Given tasks in each non-parkable lifecycle.
    let (_tmp, ctx) = ctx();
    for status in [
        Status::Running,
        Status::Idea,
        Status::Backlog,
        Status::Done,
        Status::Dropped,
    ] {
        let task = ctx.store.create("No park", None, status, "").unwrap();
        let before = ctx.store.events().unwrap();
        // When parking is requested, then the task and event stream are unchanged.
        assert!(
            park_as(
                &ctx,
                &task.id,
                "wait",
                &[],
                "",
                86400,
                (now(), Responsible::Pm)
            )
            .is_err()
        );
        assert_eq!(ctx.store.get(&task.id).unwrap(), task);
        assert_eq!(ctx.store.events().unwrap(), before);
    }
    let task = ctx.store.create("Ready", None, Status::Ready, "").unwrap();
    for seconds in [3599, 604801] {
        assert!(
            park_as(
                &ctx,
                &task.id,
                "wait",
                &[],
                "",
                seconds,
                (now(), Responsible::Pm)
            )
            .is_err()
        );
    }
    for expression in [
        "manual:anything",
        "unknown:T-1",
        "status:T-1=unknown",
        "answered:Q1@0",
    ] {
        assert!(parse_park_check(expression).is_err(), "{expression}");
    }
    let mut manual = parse_park_check("landed:T-1").unwrap();
    manual.machine_checkable = false;
    assert!(
        park_as(
            &ctx,
            &task.id,
            "wait",
            &[manual],
            "",
            86400,
            (now(), Responsible::Pm)
        )
        .is_err()
    );
    assert_eq!(ctx.store.get(&task.id).unwrap(), task);
}

#[test]
fn every_status_exit_clears_park_after_persistence_once() {
    // Given each parkable status and each possible lifecycle destination.
    let (_tmp, ctx) = ctx();
    for from in [Status::Ready, Status::Review, Status::NeedsMe] {
        for to in [
            Status::Idea,
            Status::Backlog,
            Status::Running,
            Status::Done,
            Status::Dropped,
        ] {
            let task = ctx.store.create("Exit", None, from, "").unwrap();
            park_as(
                &ctx,
                &task.id,
                "wait",
                &[],
                "",
                86400,
                (now(), Responsible::Owner),
            )
            .unwrap();
            // When a normal owner/store transition leaves the allowed statuses.
            ctx.store
                .update(&task.id, |task| {
                    task.status = to;
                    Ok(())
                })
                .unwrap();
            ctx.store.update(&task.id, |_| Ok(())).unwrap();
            // Then the same saved record has no park and exactly one system event exists.
            assert!(ctx.store.get(&task.id).unwrap().park.is_none());
            let events: Vec<_> = ctx
                .store
                .events()
                .unwrap()
                .into_iter()
                .filter(|event| {
                    event.task.as_deref() == Some(task.id.as_str()) && event.kind == "unparked"
                })
                .collect();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].detail["by"], "system");
            assert_eq!(events[0].detail["reason"], "status");
        }
    }
}

#[test]
fn park_record_enforces_byte_check_and_actor_bounds() {
    // Given the maximum supported record, independently of the CLI parser.
    let mut record = Park {
        version: 1,
        revision: 1,
        by: Responsible::Owner,
        reason: "r".repeat(2048),
        at: now(),
        resume_when: vec![parse_park_check("status:T-1=done").unwrap(); 8],
        resume_action: "a".repeat(1024),
        review_by: now()
            .checked_add(SignedDuration::from_secs(604800))
            .unwrap(),
        ready_at: None,
    };
    assert!(record.validate().is_ok());
    let bytes = serde_json::to_vec(&record).unwrap();
    assert_eq!(serde_json::from_slice::<Park>(&bytes).unwrap(), record);
    // When one boundary is exceeded, then validation refuses that record.
    record.reason.push('x');
    assert!(record.validate().is_err());
    record.reason.pop();
    record.resume_action.push('x');
    assert!(record.validate().is_err());
    record.resume_action.pop();
    record
        .resume_when
        .push(parse_park_check("landed:T-2").unwrap());
    assert!(record.validate().is_err());
    record.resume_when.pop();
    record.by = Responsible::Executor;
    assert!(record.validate().is_err());
}

#[test]
fn resume_checks_require_exact_canonical_answers_landing_and_hold_evidence() {
    use corral_core::agent::{AgentBinding, AgentRole, AgentRunState, HostHealth};
    use serde_json::json;
    // Given an answered question/decision, an owner-held task, and Done without a landing.
    let (_tmp, mut ctx) = ctx();
    ctx.store
        .edit_config(|doc| doc.set_str("projects.app.path", "."))
        .unwrap();
    ctx.store.reload_config_if_changed().unwrap();
    let dependency = ctx
        .store
        .create("Prerequisite", None, Status::Done, "")
        .unwrap();
    let task = ctx.store.create("Park", None, Status::NeedsMe, "").unwrap();
    let key = format!("{}/R1/Q1", task.id);
    ctx.store.update(&task.id, |task| {
        let mut run: corral_core::Run = serde_json::from_value(json!({
            "id":"R1","agent":"fixture","worktree":"/missing-park-question",
            "branch":"corral/T","base":"main","base_sha":"base","started":now()
        }))?;
        let mut state = AgentRunState::new(AgentBinding {
            role: AgentRole::Worker, store: "fixture".into(), project: "app".into(),
            generation: 1, health: HostHealth::Healthy, callback: None, closing: false,
        });
        state.questions.push(serde_json::from_value(json!({
            "version":1,"key":key,"revision":2,"kind":"clarification","blocking":false,
            "text":"Constraint?","state":"answered",
            "asked":{"request_id":"ask","payload_sha256":"sha","session_id":"s","generation":1,"at":now()},
            "answer":{"by":"owner","text":"Use it","at":now()},
            "wake":{"route":"none","key":"wake","state":"pending"}
        }))?);
        run.agent_host = Some(Box::new(state));
        run.obligations.decisions.push(serde_json::from_value(json!({
            "request":{"version":1,"id":"park-decision","run":"R1","question":"Continue?",
                "requested_action":"Answer","owner":"owner","source":"worker"},
            "registered_at":now(),"observation":{"at":now(),"state":"ready","detail":"Answered"},
            "receipt":{"version":1,"id":"answer","run":"R1","outcome":"answered",
                "delivery":"acknowledged","observed_at":now(),"answer":"Yes","data":null}
        }))?);
        task.runs.push(run);
        task.dispatch_hold = true;
        Ok(())
    }).unwrap();
    let expressions = [
        format!("status:{}=done", dependency.id),
        format!("landed:{}", dependency.id),
        format!("answered:{key}@2"),
        format!("answered:{key}@1"),
        "decision:park-decision".into(),
        format!("hold-lifted:{}", task.id),
        "hold-lifted:app".into(),
    ];
    let requested: Vec<_> = expressions
        .iter()
        .map(|value| parse_park_check(value).unwrap())
        .collect();
    let record = park_as(
        &ctx,
        &task.id,
        "wait",
        &requested,
        "",
        86400,
        (now(), Responsible::Pm),
    )
    .unwrap();
    // When checking the records without event history or Git.
    let tasks = ctx.store.list_shared().unwrap();
    let policy = ctx.store.effective_operating_policy().unwrap();
    let n = Neighborhood::new(&tasks, &ctx.store.config, &policy);
    let result: Vec<_> = checks(&record, &n)
        .into_iter()
        .map(|check| check.satisfied)
        .collect();
    // Then Done alone is not a landing, stale question revisions stay unknown, and holds remain.
    assert_eq!(
        result,
        [
            Some(true),
            Some(false),
            Some(true),
            None,
            Some(true),
            Some(false),
            Some(true)
        ]
    );
}
