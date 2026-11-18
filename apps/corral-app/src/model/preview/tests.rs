use super::*;
use crate::model::act::{self as acts, Act, ActStatus};
use crate::model::agents::{self, Control};
use crate::model::assistant::quick_id;
use crate::model::effect::Effect;
use crate::model::id::Id;
use corral_protocol::assistant::{AskTarget, AssistantSend, AssistantSessionOp, QUICK_THREAD};
use corral_protocol::rows::{AssistantKind, AssistantState};
use corral_types::{Op, Status};

#[test]
fn unfinished_dependency_refuses_dispatch_until_forced() {
    let mut feed = feed(1_800_000_000, "2027-01-15".into(), "08:00".into(), None);
    let dispatch = Op::Dispatch {
        id: "T-12".into(),
        profile: None,
        base: None,
    };
    act(
        &mut feed,
        &Act::with_key(Id::new("plain"), dispatch.clone(), false),
    )
    .unwrap();
    let Some(ActStatus::Refused(refusal)) = acts::status(&feed, "plain") else {
        panic!("dispatch with an unfinished dependency was not refused");
    };
    assert!(
        refusal
            .next
            .iter()
            .any(|next| next.force && next.op == dispatch)
    );
    assert_eq!(
        feed.board.task("T-12").map(|task| task.status()),
        Some(Status::Ready)
    );

    act(&mut feed, &Act::with_key(Id::new("forced"), dispatch, true)).unwrap();
    assert_eq!(acts::status(&feed, "forced"), Some(ActStatus::Done));
    assert_eq!(
        feed.board.task("T-12").map(|task| task.status()),
        Some(Status::Running)
    );
    assert!(feed.agents.contains_key("T-12_R1"));
    let forced =
        feed.changes.values().next_back().filter(|event| {
            event.kind == EventKind::Forced && event.task.as_deref() == Some("T-12")
        });
    assert_eq!(
        forced.map(|event| event.detail["skipped"].clone()),
        Some(serde_json::json!(["dependencies"]))
    );
}

/// The pause controls a session's Session menu offers; the turn a reply starts adds Cancel.
fn pausing(feed: &Feed, session: &str) -> Vec<Control> {
    agents::controls(feed, session)
        .into_iter()
        .filter(|control| matches!(control, Control::Pause | Control::Resume))
        .collect()
}

/// The owner's reply is input a paused worker takes: its row stops saying paused and offers
/// Pause again instead of Resume.
#[test]
fn owner_message_resumes_a_paused_session() {
    let mut feed = feed(1_800_000_000, "2027-01-15".into(), "08:00".into(), None);
    let session = "T-14_R1";
    control(&mut feed, &Id::new("pause"), session, Control::Pause);
    assert_eq!(feed.agents[session].paused, Some(true));
    assert_eq!(pausing(&feed, session), [Control::Resume]);

    let reply = Op::Reply {
        id: "T-14".into(),
        text: "Carry on".into(),
    };
    act(&mut feed, &Act::with_key(Id::new("reply"), reply, false)).unwrap();
    assert_eq!(acts::status(&feed, "reply"), Some(ActStatus::Done));
    assert_eq!(feed.agents[session].paused, Some(false));
    assert_eq!(pausing(&feed, session), [Control::Pause]);
}

/// A terminated run leaves the roster; its Session menu still offers Relaunch, which starts a
/// new run of the task. Before, the menu had no control at all once the row was gone.
#[test]
fn terminated_run_offers_relaunch() {
    let mut feed = feed(1_800_000_000, "2027-01-15".into(), "08:00".into(), None);
    let session = "T-14_R1";
    control(&mut feed, &Id::new("stop"), session, Control::Terminate);
    assert!(!feed.agents.contains_key(session));
    assert_eq!(agents::controls(&feed, session), [Control::Relaunch]);

    control(&mut feed, &Id::new("again"), session, Control::Relaunch);
    assert!(feed.operations["again"].refusal.is_none());
    assert!(feed.agents.contains_key("T-14_R2"));
}

/// Once the quick thread ended, its next message opens a new quick session that holds it.
#[test]
fn ended_quick_thread_starts_a_new_session_on_the_next_send() {
    let mut feed = feed(1_800_000_000, "2027-01-15".into(), "08:00".into(), None);
    let old = quick_id(&feed).expect("the sample has a quick session");
    let end = Effect::Session {
        key: Id::new("end"),
        op: AssistantSessionOp::End {
            session: old.clone(),
        },
    };
    effect(&mut feed, &end).unwrap();
    assert_eq!(feed.assistants[old.as_str()].state, AssistantState::Ended);

    let send = AssistantSend {
        cmd_id: "again".into(),
        thread: QUICK_THREAD.into(),
        target: AskTarget::Assistant,
        text: "Back again".into(),
        typed_at: "2027-01-15T08:00:00Z".into(),
        context: vec![],
        attachments: vec![],
    };
    effect(&mut feed, &Effect::Send(send)).unwrap();
    let new = quick_id(&feed).expect("a new quick session");
    assert_ne!(new, old);
    assert_eq!(feed.assistants[new.as_str()].kind, AssistantKind::Quick);
    assert_ne!(feed.assistants[new.as_str()].state, AssistantState::Ended);
    assert!(
        feed.threads[new.as_str()]
            .entries
            .values()
            .any(|entry| entry.text.as_deref() == Some("Back again"))
    );
}

/// The sample names each event's project, a closed task's too, as the daemon serves events.
#[test]
fn events_of_a_closed_task_name_its_project() {
    let feed = feed(1_800_000_000, "2027-01-15".into(), "08:00".into(), None);
    assert!(feed.board.task("T-6").is_none());
    let landed = feed
        .changes
        .values()
        .find(|event| event.task.as_deref() == Some("T-6") && event.kind == EventKind::Landed);
    assert_eq!(
        landed.and_then(|event| event.project.as_deref()),
        Some("site")
    );
}

/// An ended headless run's terminal is its retained log, not live; once the log is pruned the
/// answer is not found with the reason corrald gives.
#[test]
fn an_ended_headless_runs_log_is_served_until_it_is_pruned() {
    let mut feed = feed(1_800_000_000, "2027-01-15".into(), "08:00".into(), None);
    let log = DetailRef::Terminal {
        id: "T-6_R1".into(),
    };
    detail(&mut feed, &log, None).unwrap();
    assert!(matches!(
        feed.detail(&log).and_then(|cache| cache.body()),
        Some(DetailBody::Terminal(snapshot)) if !snapshot.live && !snapshot.lines.is_empty()
    ));
    let pruned = DetailRef::Terminal {
        id: "T-7_R1".into(),
    };
    let error = failure(detail(&mut feed, &pruned, None).unwrap_err());
    assert_eq!(
        (error.code.as_str(), error.message.as_str()),
        ("not_found", "session log not found")
    );
}
