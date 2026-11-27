use corral_protocol::detail::{
    ChildSteer, ChildUsage, DetailBody, DetailRef, HostPresence, SessionChildDetail, SessionItem,
    SessionMessage, SessionPage,
};
use corral_protocol::session_state::{Activity, SessionState};
use corral_protocol::subscription::{SessionAgentMessage, SessionChild, SessionFleet};

use super::conversation;
use crate::model::Id;
use crate::model::daemon::{self, DetailCache, Feed, Link};
use crate::model::effect::{Conn, Env};
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::session::transcript::{LiveSession, MessageKind};
use crate::model::session::{Action, Cx, Facts, FleetAction, SessionRole, State, Tab, Ui, page};

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
            transcript_seams: Default::default(),
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

#[test]
fn an_opened_conversation_gives_the_composer_and_status_bar_its_agent() {
    let mut feed = Feed {
        link: Link::Up,
        ..Feed::default()
    };
    let mut live = LiveSession {
        state: Some(SessionState {
            model: Some("parent/model".into()),
            activity: Some(Activity {
                phase: "compacting".into(),
                since: "2026-11-19T09:59:55Z".into(),
                retry_at: None,
            }),
            queue_total: 2,
            ..SessionState::default()
        }),
        ..LiveSession::default()
    };
    let mut fleet = SessionFleet {
        children: vec![SessionChild {
            model: "child/model".into(),
            effort: Some("low".into()),
            cost_usd: Some(0.21),
            context_percent: Some(42.0),
            activity: Some("running child-check".into()),
            ..row()
        }],
        messages: Vec::new(),
        messages_total: 0,
        max_parallel: 2,
    };
    live.replace_fleet(Some(fleet.clone()));
    let id = Id::from(ID);
    feed.live.insert(id.clone(), live);
    let mut state = State::default();
    let open = |child: &str| {
        Action::Fleet(FleetAction::Open {
            session: id.clone(),
            child: child.into(),
        })
    };
    let env = Env {
        conn: Conn::Healthy,
        form: Form::Terminal,
    };
    let shown_page = |feed: &Feed, state: &State, id: &Id| page(feed, state, id, env);
    state.act(open("a1"), &feed);
    let inline = shown_page(&feed, &state, &id);
    let shown = inline.composer.expect("a build takes messages");
    assert_eq!(shown.title.to, "→ a1");
    assert_eq!(shown.send.label, "Steer");
    assert!(shown.send.unavailable.is_none());
    assert!(shown.queue.is_none() && shown.stop.is_none());
    assert_eq!(
        shown.activity.expect("child is working").short,
        "child-check"
    );
    let model = shown.model.expect("the agent's model");
    assert_eq!(model.label, "child/model");
    assert!(model.opens.is_some());
    let effort = shown.effort.expect("the agent's effort");
    assert_eq!((effort.level.as_deref(), effort.fixed), (Some("low"), true));
    let bar = inline.status;
    assert_eq!(bar.role.full, "AGENT a1");
    assert_eq!(bar.cost.text, "$0.21");
    assert_eq!(bar.meter.short, "ctx 42%");
    assert_eq!(
        bar.model_effort.as_deref(),
        Some("child/model · effort low")
    );
    assert!(bar.queued.is_none());

    let scope = Id::from(SCOPE);
    let standalone = shown_page(&feed, &state, &scope);
    assert_eq!(standalone.status.role, bar.role);
    assert_eq!(standalone.status.cost.text, bar.cost.text);
    assert_eq!(standalone.status.meter.short, bar.meter.short);
    assert_eq!(standalone.status.model_effort, bar.model_effort);
    assert!(standalone.composer.is_none() && standalone.watch.is_none());
    let target = DetailRef::SessionChild {
        id: ID.into(),
        child: "a1".into(),
    };
    assert!(
        standalone
            .fetches
            .iter()
            .any(|action| matches!(action, Action::Refresh(asked) if *asked == target))
    );

    fleet.children[0].state = "needs".into();
    let host = feed.live.get_mut(ID).expect("parent is live");
    host.replace_fleet(Some(fleet));
    let waiting = shown_page(&feed, &state, &id)
        .composer
        .expect("inline composer");
    assert_eq!(waiting.activity.expect("child is asking").glyph, Some("▲"));

    state.act(
        Action::Draft {
            session: id.clone(),
            text: "draft stays".into(),
        },
        &feed,
    );
    state.act(
        Action::Tab {
            session: id.clone(),
            tab: Tab::Chat,
        },
        &feed,
    );
    let chat = shown_page(&feed, &state, &id)
        .composer
        .expect("parent composer");
    assert_eq!(chat.title.to, "→ parent");
    assert_eq!(chat.send.label, "Send");
    assert_eq!(chat.draft, "draft stays");

    state.act(open("a1"), &feed);
    state.act(open("unlisted"), &feed);
    let unlisted = shown_page(&feed, &state, &id);
    let shown = unlisted.composer.expect("inline composer");
    assert_eq!(shown.title.to, "→ parent");
    assert!(shown.activity.is_none());
    assert_eq!(
        shown.model.expect("placeholder model").label,
        "Model unavailable"
    );
    assert_eq!(unlisted.status.cost.text, "$–");

    feed.live
        .get_mut(ID)
        .expect("parent is live")
        .replace_fleet(None);
    feed.details
        .extend([cache(target, DetailBody::SessionChild(detail()))]);
    let archived = shown_page(&feed, &state, &scope);
    assert_eq!(
        archived.status.model_effort.as_deref(),
        Some("provider/model")
    );
    assert_eq!(archived.status.cost.text, "$–");
    assert_eq!(archived.status.meter.short, "ctx –");

    state.act(Action::Fleet(FleetAction::Leave(id.clone())), &feed);
    let parent = shown_page(&feed, &state, &id);
    assert_eq!(parent.status.role.full, "BUILD");
    assert_eq!(parent.status.queued.as_deref(), Some("2 queued"));
    assert_eq!(
        parent
            .composer
            .expect("parent composer")
            .model
            .expect("parent model")
            .label,
        "parent/model"
    );
}
