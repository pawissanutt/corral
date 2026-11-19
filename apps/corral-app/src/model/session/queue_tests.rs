use corral_protocol::session_host::{QueueWithdrawn, SESSION_QUEUE_WITHDRAW, WithdrawOutcome};
use corral_protocol::session_state::{InputOrigin, QueuedInput, SendPolicy, SessionState};

use super::transcript::LiveSession;
use super::{Action, DockAction, State};
use crate::model::Id;
use crate::model::daemon::{Feed, Operation};
use crate::model::effect::Effect;

/// Edit takes a queued input back to rework it: the whole text the host returns (not the
/// clipped line the queue lists) must land in the draft, above what the owner was typing.
#[test]
fn withdrawing_puts_the_text_back_in_the_draft() {
    let session = Id::from("B-k3x9");
    let mut feed = Feed::default();
    let mut live = LiveSession::default();
    live.replace_state(Some(Box::new(SessionState {
        queue: vec![QueuedInput {
            key: "app-1".into(),
            policy: SendPolicy::FollowUp,
            origin: InputOrigin::Owner,
            text: "also update the docs".into(),
            bytes: 300,
            to: None,
            actions: vec!["withdraw".into()],
        }],
        queue_total: 1,
        ..SessionState::default()
    })));
    feed.live.insert(session.clone(), live);
    let mut state = State::default();
    state.act(
        Action::Draft {
            session: session.clone(),
            text: "and the changelog".into(),
        },
        &feed,
    );

    let effects = state.act(
        Action::Dock(DockAction::Withdraw {
            session: session.clone(),
            key: "app-1".into(),
            edit: true,
        }),
        &feed,
    );
    let Some(Effect::SessionQueue { key, .. }) = effects.first() else {
        panic!("Edit sent no withdraw: {effects:?}");
    };
    let whole = "also update the docs, the README and every example under docs/";
    feed.operations.insert(
        key.clone(),
        Operation {
            method: SESSION_QUEUE_WITHDRAW.into(),
            loading: false,
            value: Some(
                serde_json::to_value(QueueWithdrawn {
                    outcome: WithdrawOutcome::Withdrawn,
                    turn: None,
                    text: Some(whole.into()),
                })
                .unwrap(),
            ),
            error: None,
            refusal: None,
        },
    );
    state.settle(&feed);

    let draft = &state.ui(session.as_str()).draft;
    assert!(draft.starts_with(whole), "draft: {draft}");
    assert!(draft.ends_with("and the changelog"), "draft: {draft}");
}
