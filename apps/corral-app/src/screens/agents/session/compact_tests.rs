use corral_protocol::detail::{DetailBody, DetailRef, HostPresence, SessionItem, SessionMessage};

use crate::model::daemon::{DetailCache, Feed, detail_key, empty_quick_page};
use crate::model::effect::{Conn, Env};
use crate::model::session::{self, Body, SubView, Tab};
use crate::model::{Form, Id};

/// The chat's own page was left on its Usage view: Ask, which is always the chat, drew an empty
/// transcript area until the owner went back to Chat there. It must draw the chat, and the
/// page must keep the view it was left on.
#[test]
fn ask_shows_the_chat_when_its_page_was_left_on_another_view() {
    let id = Id::from("A-7");
    let mut feed = Feed::default();
    let target = DetailRef::Session { id: "A-7".into() };
    let mut page = empty_quick_page();
    page.id = "A-7".into();
    page.host = HostPresence::Running;
    page.messages.push(SessionMessage {
        id: "A-7/entry/1".into(),
        author: "user".into(),
        at: Some("2026-11-19T10:00:00Z".into()),
        items: vec![SessionItem::Text {
            text: "what blocks T-12?".into(),
        }],
        model: None,
        stop_reason: None,
        usage: None,
        input: None,
        attachments: Vec::new(),
    });
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Session(page)),
        },
    );
    let mut state = session::State::default();
    let left = session::Ui {
        tab: Tab::Chat,
        sub: Some(SubView::Usage),
        ..session::Ui::default()
    };
    state.sessions.insert(id.clone(), left);
    let env = Env {
        conn: Conn::Healthy,
        form: Form::Desktop,
    };
    assert!(
        matches!(session::page(&feed, &state, &id, env).body, Body::Usage(_)),
        "the session's own page shows the view it was left on"
    );
    let Body::Chat(chat) = session::compact(&feed, &state, &id, env).body else {
        panic!("Ask draws the chat");
    };
    assert!(!chat.transcript.messages.is_empty());
    assert_eq!(state.ui("A-7").sub, Some(SubView::Usage));
}
