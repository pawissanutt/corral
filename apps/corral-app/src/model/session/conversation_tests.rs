use corral_protocol::detail::{
    ChildSteer, ChildUsage, DetailBody, DetailRef, HostPresence, SessionChildDetail, SessionItem,
    SessionMessage, SessionPage,
};
use corral_protocol::subscription::{SessionAgentMessage, SessionChild, SessionFleet};

use super::conversation;
use crate::model::Id;
use crate::model::daemon::{self, DetailCache, Feed, Link};
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::session::transcript::{LiveSession, MessageKind};
use crate::model::session::{Cx, Facts, SessionRole, Ui};

const ID: &str = "B-k3x9";
const SCOPE: &str = "B-k3x9/child/a1";

fn ms(at: &str) -> u64 {
    let at: jiff::Timestamp = at.parse().expect("a test time parses");
    u64::try_from(at.as_millisecond()).expect("after the epoch")
}

fn cache(target: DetailRef, body: DetailBody) -> (String, DetailCache) {
    (
        daemon::detail_key(&target),
        DetailCache {
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(body),
        },
    )
}

fn reply(id: &str, at: &str, text: &str) -> SessionMessage {
    SessionMessage {
        id: id.into(),
        author: "assistant".into(),
        at: Some(at.into()),
        items: vec![SessionItem::Text { text: text.into() }],
        model: None,
        stop_reason: None,
        usage: None,
        input: None,
        attachments: Vec::new(),
    }
}

fn row() -> SessionChild {
    SessionChild {
        id: "a1".into(),
        kind: "slice".into(),
        title: "token store".into(),
        state: "running".into(),
        model: "provider/model".into(),
        started_at: Some(ms("2026-11-19T09:59:55Z")),
        ended_at: None,
        activity: None,
        tokens: 0,
        cost_usd: None,
        verdict: None,
        question: None,
        profile: None,
        effort: None,
        pending_model: None,
        turns: 2,
        context_percent: None,
        files: 0,
        outside: 0,
        owns: 0,
        after: Vec::new(),
        steerable: true,
        error: None,
    }
}

fn detail() -> SessionChildDetail {
    SessionChildDetail {
        id: "a1".into(),
        kind: "slice".into(),
        title: "token store".into(),
        brief: "Store tokens.".into(),
        brief_more: false,
        owns: Vec::new(),
        after: Vec::new(),
        profile: "slice".into(),
        model: "provider/model".into(),
        pending_model: None,
        effort: None,
        state: "running".into(),
        created_at: ms("2026-11-19T09:59:50Z"),
        started_at: None,
        ended_at: None,
        activity: None,
        edits: Vec::new(),
        usage: ChildUsage {
            input: 0,
            output: 0,
            cost_usd: None,
            context_percent: None,
            turns: 2,
        },
        steers: vec![ChildSteer {
            at: Some("2026-11-19T10:00:10Z".into()),
            text: "use the existing cache".into(),
        }],
        question: None,
        report: None,
        report_more: false,
        error: None,
        verdict: None,
        pulse: Vec::new(),
        history: String::new(),
        dropped: None,
        transcript: true,
        models: Vec::new(),
        profiles: Vec::new(),
    }
}

#[test]
fn steers_and_agent_messages_interleave_by_time() {
    let mut feed = Feed {
        link: Link::Up,
        ..Feed::default()
    };
    let mut live = LiveSession::default();
    live.replace_fleet(Some(SessionFleet {
        children: vec![row()],
        messages: vec![SessionAgentMessage {
            seq: 7,
            at: ms("2026-11-19T10:00:30Z"),
            from: "parent".into(),
            to: "a1".into(),
            text: "a0 landed the schema".into(),
            delivery: "delivered".into(),
        }],
        messages_total: 1,
        max_parallel: 2,
    }));
    feed.live.insert(Id::from(ID), live);
    feed.details.extend([
        cache(
            DetailRef::SessionChild {
                id: ID.into(),
                child: "a1".into(),
            },
            DetailBody::SessionChild(detail()),
        ),
        cache(
            DetailRef::Session { id: SCOPE.into() },
            DetailBody::Session(SessionPage {
                id: SCOPE.into(),
                role: "child".into(),
                task: None,
                run: None,
                machine: None,
                messages: vec![
                    reply("first", "2026-11-19T10:00:00Z", "Reading the token code."),
                    reply("second", "2026-11-19T10:00:20Z", "Using the cache now."),
                ],
                older: None,
                replyable: false,
                reply_refusal: None,
                host: HostPresence::Running,
            }),
        ),
    ]);
    let id = Id::from(ID);
    let ui = Ui::default();
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
    let shown = conversation(&cx, &Ui::default(), "a1", Some(&row()));
    let order: Vec<&str> = shown
        .transcript
        .messages
        .iter()
        .map(|message| {
            if message.id.ends_with("/steer/0") {
                "steer"
            } else if message.id.ends_with("/msg/7") {
                "message"
            } else if message.kind == MessageKind::Agent {
                "reply"
            } else {
                "other"
            }
        })
        .collect();
    assert_eq!(order, ["reply", "steer", "reply", "message"]);
}
