use super::{Record, Verb, verb_build::command, verbs::Frozen};
use corral_protocol::{
    app::ActClass,
    cmd::{
        Command, assistant::AssistantCommand as Assistant,
        attention::AttentionCommand as Attention, mode::ModeCommand as Mode,
        plan::PlanCommand as Plan, question::QuestionCommand as Question,
        queue::QueueCommand as Queue, review::ReviewCommand as Review, task::TaskCommand as Task,
    },
};

fn at(record: Record) -> Frozen {
    Frozen {
        record,
        revision: Some("owner-rev".into()),
        question_rev: 7,
        sha: "candidate-head".into(),
        run: Some("R2".into()),
        park_revision: Some(91),
    }
}
fn build(at: &Frozen, verb: Verb, values: &[&str], class: ActClass) -> Command {
    let values: Vec<String> = values.iter().map(|value| (*value).into()).collect();
    let built = command(at, verb, &values).unwrap();
    assert_eq!(built.app_class(), class, "{verb:?}");
    let act = crate::model::act::Act::with_key("command-1".into(), verb.label(), built.clone());
    assert_eq!(
        act.confirm,
        matches!(class, ActClass::Confirm | ActClass::Admin)
    );
    assert_eq!(
        act.authority(),
        match class {
            ActClass::Reply => crate::model::act::Authority::Reply,
            ActClass::Admin => crate::model::act::Authority::Admin,
            _ => crate::model::act::Authority::Act,
        }
    );
    built
}

#[test]
fn first_action_wave_uses_canonical_commands_and_classes() {
    let at = at(Record::Task("T-9".into()));
    assert!(
        matches!(build(&at, Verb::Dispatch, &[], ActClass::Act), Command::Task(Task::Dispatch { id, profile: None, model: None, effort: None, base: None, headless: true }) if id == "T-9")
    );
    assert!(
        matches!(build(&at, Verb::QueueAdd, &[], ActClass::Act), Command::Queue(Queue::Add { id, profile: None, model: None, effort: None, base: None, headless: true }) if id == "T-9")
    );
    assert_eq!(
        build(&at, Verb::QueueRemove, &[], ActClass::Act),
        Command::Queue(Queue::Remove { id: "T-9".into() })
    );
    assert_eq!(
        build(
            &at,
            Verb::Park,
            &["waiting", "landed:T-10\nstatus:T-11=done", "resume", "3600"],
            ActClass::Act
        ),
        Command::Attention(Attention::Park {
            id: "T-9".into(),
            reason: "waiting".into(),
            resume_when: vec!["landed:T-10".into(), "status:T-11=done".into()],
            resume_action: "resume".into(),
            review_by: 3600,
            json: false
        })
    );
    assert_eq!(
        build(&at, Verb::Unpark, &[], ActClass::Act),
        Command::Attention(Attention::Unpark {
            id: "T-9".into(),
            expected_revision: 91,
            json: false
        })
    );
    assert_eq!(
        build(
            &at,
            Verb::Comment,
            &["owner's exact words\nsecond line"],
            ActClass::Reply
        ),
        Command::Task(Task::Comment {
            id: "T-9".into(),
            text: "owner's exact words\nsecond line".into(),
            author: "owner".into(),
            send: true
        })
    );
}

#[test]
fn second_action_wave_binds_task_creation_and_candidate_verdict() {
    let task = at(Record::Task("T-9".into()));
    assert_eq!(
        build(
            &at(Record::Workspace),
            Verb::NewTask,
            &["New work", "Body", "project"],
            ActClass::Act
        ),
        Command::Task(Task::New {
            title: vec!["New work".into()],
            origin: "owner".into(),
            project: Some("project".into()),
            status: "backlog".into(),
            body: "Body".into(),
            profile: None,
            depends: vec![],
            priority: None,
            kind: None,
            size: None,
            difficulty: None,
            design: false,
            labels: vec![],
        })
    );
    assert_eq!(
        build(&task, Verb::Unstick, &[], ActClass::Act),
        Command::Task(Task::Unstick { id: "T-9".into() })
    );
    assert_eq!(
        build(&task, Verb::Revert, &[], ActClass::Confirm),
        Command::Task(Task::Revert { id: "T-9".into() })
    );
    assert_eq!(
        build(
            &at(Record::Plan("T-8".into())),
            Verb::PlanCancel,
            &["No longer needed"],
            ActClass::Act
        ),
        Command::Plan(Plan::Cancel {
            id: "T-8".into(),
            owner_ok: true,
            reason: Some("No longer needed".into()),
            json: false
        })
    );
    assert_eq!(
        build(
            &at(Record::Review("T-9".into())),
            Verb::Verdict,
            &["changes", "Fix this"],
            ActClass::Act
        ),
        Command::Review(Review::Verdict {
            id: "T-9".into(),
            sha: "candidate-head".into(),
            verdict: "changes".into(),
            notes: Some("Fix this".into())
        })
    );
}

#[test]
fn third_action_wave_preserves_membership_question_and_authority_contracts() {
    let plan = at(Record::Plan("T-8".into()));
    assert_eq!(
        build(&plan, Verb::AddMember, &["T-3"], ActClass::Act),
        Command::Plan(Plan::AddMember {
            tracker: "T-8".into(),
            member: "T-3".into(),
            json: false
        })
    );
    assert_eq!(
        build(
            &plan,
            Verb::MoveMember,
            &["T-3", "T-4", "new scope"],
            ActClass::Act
        ),
        Command::Plan(Plan::MoveMember {
            tracker: "T-8".into(),
            member: "T-3".into(),
            to: "T-4".into(),
            gate_file: None,
            reason: Some("new scope".into()),
            json: false
        })
    );
    assert!(
        matches!(build(&plan, Verb::MoveMember, &["T-3", "T-4", "reviewed", "gate.json", "{\"passed\":true}"], ActClass::Act),
        Command::Plan(Plan::MoveMember { gate_file: Some(file), .. }) if file.path == "gate.json" && file.text == "{\"passed\":true}")
    );
    let question = at(Record::Question("T-9/R2/Q1".into()));
    assert_eq!(
        build(
            &question,
            Verb::Escalate,
            &["note", "owner_only"],
            ActClass::Act
        ),
        Command::Question(Question::Escalate {
            key: "T-9/R2/Q1".into(),
            rev: 7,
            note: Some("note".into()),
            kind: Some("owner_only".into()),
            json: false
        })
    );
    assert_eq!(
        build(
            &question,
            Verb::CancelQuestion,
            &["obsolete"],
            ActClass::Act
        ),
        Command::Question(Question::Cancel {
            key: "T-9/R2/Q1".into(),
            rev: 7,
            reason: "obsolete".into(),
            owner: true,
            json: false
        })
    );
    assert_eq!(
        build(
            &question,
            Verb::Deadline,
            &["2026-11-16T10:00:00Z", "escalate"],
            ActClass::Act
        ),
        Command::Question(Question::Deadline {
            key: "T-9/R2/Q1".into(),
            rev: 7,
            at: "2026-11-16T10:00:00Z".into(),
            instruction: "escalate".into(),
            json: false
        })
    );
    let task = at(Record::Task("T-9".into()));
    assert_eq!(
        build(
            &task,
            Verb::Route,
            &["worker", "reviewer", "no", "model", "high"],
            ActClass::Act
        ),
        Command::Task(Task::Route {
            id: "T-9".into(),
            set_reviewer: Some("reviewer".into()),
            clear_reviewer: false,
            profile: Some("worker".into()),
            model: Some("model".into()),
            effort: Some("high".into()),
            json: false
        })
    );
    assert_eq!(
        build(&task, Verb::Focus, &[], ActClass::Act),
        Command::Task(Task::Focus {
            id: Some("T-9".into()),
            all: false
        })
    );
    assert_eq!(
        build(&task, Verb::Move, &["done", "complete"], ActClass::Confirm),
        Command::Task(Task::Move {
            id: "T-9".into(),
            status: "done".into(),
            force: false,
            owner_ok: true,
            reason: Some("complete".into())
        })
    );
    build(&task, Verb::Move, &["ready", ""], ActClass::Act);
    assert_eq!(
        build(&at(Record::Workspace), Verb::ModeOff, &[], ActClass::Admin),
        Command::Mode(Mode::Off { json: false })
    );
    assert_eq!(
        build(
            &at(Record::Assistant("A-1".into())),
            Verb::PermissionsRevoke,
            &["grant-1"],
            ActClass::Confirm
        ),
        Command::Assistant(Assistant::PermissionsRevoke {
            id: "A-1".into(),
            grant: Some("grant-1".into()),
            all: false,
            json: false
        })
    );
    assert_eq!(
        build(
            &at(Record::Assistant("A-1".into())),
            Verb::PermissionsRevoke,
            &[""],
            ActClass::Confirm
        ),
        Command::Assistant(Assistant::PermissionsRevoke {
            id: "A-1".into(),
            grant: None,
            all: true,
            json: false
        })
    );
}

#[test]
fn action_composer_keeps_text_and_refuses_a_changed_owner_revision() {
    use super::{State, task_ref};
    use crate::model::daemon::{DetailCache, Feed, Link, detail_key};
    use corral_protocol::{
        app::{ACT_V1, AppSession, DeviceAuthority},
        detail::DetailBody,
    };
    use serde_json::json;
    let target = task_ref("T-9");
    let task = serde_json::from_value(json!({
        "id":"T-9", "owner_revision":"old", "title":"Task", "status":"ready", "body_excerpt":"",
        "stages":[{"glyph":"pending","sha7":null,"at":null},{"glyph":"pending","sha7":null,"at":null},
                  {"glyph":"pending","sha7":null,"at":null},{"glyph":"pending","sha7":null,"at":null}],
        "dependencies":[], "progress":null, "owner_next":null, "hold":null, "park":null,
        "artifacts":[], "run":null, "plan_context":null, "log_tail":[]
    })).unwrap();
    let mut feed = Feed {
        link: Link::Up,
        session: Some(AppSession {
            device: serde_json::from_value(json!({
                "id":"device", "name":"Desk", "platform":"native", "app_version":"1",
                "enrolled_at":"2026-11-15T00:00:00Z", "revoked_at":null
            }))
            .unwrap(),
            authority: DeviceAuthority {
                reply: true,
                ..DeviceAuthority::default()
            },
            seen: Default::default(),
            server_time: "2026-11-15T00:00:00Z".into(),
        }),
        ..Feed::default()
    };
    feed.capabilities.insert(ACT_V1.into());
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            target: target.clone(),
            pages: vec![],
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Task(task)),
        },
    );
    let mut state = State::default();
    assert!(
        state
            .start_verb(Record::Task("T-9".into()), Verb::Comment, &feed)
            .is_empty()
    );
    let Some(DetailBody::Task(task)) = feed
        .details
        .get_mut(&detail_key(&target))
        .unwrap()
        .merged
        .as_mut()
    else {
        panic!("task fixture")
    };
    task.owner_revision = "new".into();
    assert!(
        state
            .submit_verb(vec!["owner's words".into()], &feed)
            .is_empty()
    );
    let composer = state.composer.as_ref().unwrap();
    assert!(composer.error.is_some());
    assert_eq!(composer.fields[0].value, "owner's words");
    assert!(state.sent.is_empty());
}
