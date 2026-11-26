use corral_protocol::detail::CardKind;
use corral_protocol::session_state::{HostNotice, PlanModeBrief, PlanModePhase, SessionState};

use crate::model::Id;
use crate::model::daemon::{ClientError, Feed, Operation};
use crate::model::hue::Hue;
use crate::model::session::notices::{Notice, current};
use crate::model::session::questions_tests::{SESSION, card, feed_with};
use crate::model::session::{Action, CommandAction, State};

use super::Pending;

const CARD: &str = "plan-review";
const DUPLICATE: &str = "This review is already being answered; wait for it";
const FAILURE: &str = "Context could not be prepared";

fn review() -> Feed {
    feed_with(
        card(CARD, CardKind::PlanReview, Vec::new()),
        SessionState {
            plan_mode: Some(PlanModeBrief {
                phase: PlanModePhase::Review,
                title: None,
                revision: None,
                preparing: Some("compact".into()),
                approval_epoch: None,
                approval_host: None,
                approval_pending: None,
                approval_error: None,
                approval_failures: Vec::new(),
            }),
            ..SessionState::default()
        },
    )
}

fn request(state: &mut State, key: &str) {
    state
        .ui_mut(&Id::from(SESSION))
        .dock
        .pending
        .insert(Id::from(key), Pending::Card { card: CARD.into() });
}

fn refusal(feed: &mut Feed, key: &str, reason: &str) {
    feed.operations.insert(
        Id::from(key),
        Operation {
            method: "session.card.answer".into(),
            loading: false,
            value: None,
            error: Some(ClientError {
                code: "refused".into(),
                message: reason.into(),
            }),
            refusal: None,
        },
    );
}

fn shown(state: &State, feed: &Feed) -> Option<Notice> {
    current(
        &state.ui(SESSION).notices,
        feed.live[SESSION].state.as_ref(),
        feed.now,
    )
}

#[test]
fn unique_host_notices_remain_dismissed_across_plan_phases() {
    let mut feed = review();
    let live = feed.live.get_mut(SESSION).unwrap().state.as_mut().unwrap();
    live.plan_mode = None;
    live.notices.push(HostNotice {
        id: "note-old-1".into(),
        tone: "error".into(),
        text: "The plan was not started: old".into(),
        until: Some("2026-11-26T00:00:00Z".into()),
    });
    let mut state = State::default();
    state.ui_mut(&Id::from(SESSION));
    state.settle(&feed);
    let notice = shown(&state, &feed).unwrap();
    state.ui_mut(&Id::from(SESSION)).notices.dismiss(&notice.id);
    let live = feed.live.get_mut(SESSION).unwrap().state.as_mut().unwrap();
    live.plan_mode = review().live[SESSION]
        .state
        .as_ref()
        .unwrap()
        .plan_mode
        .clone();
    live.plan_mode.as_mut().unwrap().approval_host = Some("old".into());
    state.settle(&feed);
    assert!(shown(&state, &feed).is_none());
    let live = feed.live.get_mut(SESSION).unwrap().state.as_mut().unwrap();
    live.plan_mode = None;
    live.notices[0].id = "note-new-1".into();
    live.notices[0].text = "The plan was not started: new".into();
    state.settle(&feed);
    assert!(shown(&state, &feed).unwrap().text.contains("new"));
}

#[test]
fn a_dismissed_host_failure_does_not_return_after_snapshot_clipping() {
    let mut feed = review();
    let note = HostNotice {
        id: "plan-failure-host-2".into(),
        tone: "error".into(),
        text: "The plan was not started: offline".into(),
        until: Some("2026-11-26T00:00:00Z".into()),
    };
    let mut state = State::default();
    state.ui_mut(&Id::from(SESSION));
    let live = feed.live.get_mut(SESSION).unwrap();
    live.state.as_mut().unwrap().notices.push(note.clone());
    state.settle(&feed);
    let notice = shown(&state, &feed).unwrap();
    state.ui_mut(&Id::from(SESSION)).notices.dismiss(&notice.id);
    assert!(shown(&state, &feed).is_none());

    // The host can omit notices to fit its live-state byte budget, then advertise the
    // same still-active note again once the snapshot shrinks.
    let live = feed.live.get_mut(SESSION).unwrap();
    live.state.as_mut().unwrap().notices.clear();
    state.settle(&feed);
    let live = feed.live.get_mut(SESSION).unwrap();
    live.state.as_mut().unwrap().notices.push(note);
    state.settle(&feed);
    assert!(shown(&state, &feed).is_none());
    assert_eq!(state.ui(SESSION).notices.recent().count(), 1);
}

#[test]
fn independent_host_failures_with_the_same_reason_are_not_suppressed() {
    let mut feed = review();
    let live = feed.live.get_mut(SESSION).unwrap().state.as_mut().unwrap();
    for id in ["first", "second"] {
        live.notices.push(HostNotice {
            id: id.into(),
            tone: "error".into(),
            text: "The plan was not started: offline".into(),
            until: Some("2026-11-26T00:00:00Z".into()),
        });
    }
    let mut state = State::default();
    state.ui_mut(&Id::from(SESSION));
    state.settle(&feed);
    assert_eq!(
        state
            .ui(SESSION)
            .notices
            .recent()
            .filter(|notice| notice.text.contains("offline"))
            .count(),
        2
    );
}

#[test]
fn an_independent_failure_is_recorded_while_a_review_is_answering() {
    let mut feed = review();
    feed.live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .notices
        .push(HostNotice {
            id: "other".into(),
            tone: "error".into(),
            text: "The plan was not started: another failure".into(),
            until: Some("2026-11-26T00:00:00Z".into()),
        });
    let mut state = State::default();
    state.ui_mut(&Id::from(SESSION)).dock.plan_attempts.insert(
        CARD.into(),
        super::super::PlanAttempt {
            context: "compact".into(),
            acknowledged: false,
            observed: false,
            initial_epoch: Some("1".into()),
            ack_epoch: None,
            ack_host: None,
            duplicate: false,
            probe_needed: false,
            probe_failed: false,
            probe_unconfirmed: false,
            last_probe_plan: None,
            probe_id: None,
            prior_failures: Default::default(),
        },
    );
    state.settle(&feed);
    assert!(
        shown(&state, &feed)
            .unwrap()
            .text
            .contains("another failure")
    );
}

#[test]
fn a_failed_probe_remains_retryable_when_the_card_is_missing() {
    let mut feed = review();
    feed.live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .cards
        .clear();
    let mut state = State::default();
    state.ui_mut(&Id::from(SESSION)).dock.plan_attempts.insert(
        CARD.into(),
        super::super::PlanAttempt {
            context: "compact".into(),
            acknowledged: true,
            observed: false,
            initial_epoch: Some("1".into()),
            ack_epoch: Some("2".into()),
            ack_host: Some("host".into()),
            duplicate: false,
            probe_needed: false,
            probe_failed: true,
            probe_unconfirmed: false,
            last_probe_plan: None,
            probe_id: None,
            prior_failures: Default::default(),
        },
    );
    let dock = crate::model::session::questions_tests::dock_of(&feed, &state);
    assert!(matches!(
        dock.read.and_then(|read| read.retry),
        Some(Action::Dock(super::DockAction::Probe { .. }))
    ));
}

#[test]
fn a_settled_answer_is_not_orphaned_when_its_old_card_disappears() {
    let mut feed = review();
    feed.live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .cards
        .clear();
    feed.details.clear();
    let mut state = State::default();
    state.ui_mut(&Id::from(SESSION)).dock.plan_attempts.insert(
        CARD.into(),
        super::super::PlanAttempt {
            context: "compact".into(),
            acknowledged: false,
            observed: false,
            initial_epoch: Some("1".into()),
            ack_epoch: None,
            ack_host: None,
            duplicate: false,
            probe_needed: false,
            probe_failed: false,
            probe_unconfirmed: false,
            last_probe_plan: None,
            probe_id: None,
            prior_failures: Default::default(),
        },
    );
    request(&mut state, "old-answer");
    refusal(&mut feed, "old-answer", "review no longer exists");
    state.settle(&feed);
    assert!(!state.ui(SESSION).dock.plan_attempts.contains_key(CARD));
}

#[test]
fn a_missed_preparation_reconciles_a_failure_or_host_restart() {
    for error in [Some("The plan was not started: offline"), None] {
        let mut feed = review();
        let plan = feed
            .live
            .get_mut(SESSION)
            .unwrap()
            .state
            .as_mut()
            .unwrap()
            .plan_mode
            .as_mut()
            .unwrap();
        plan.preparing = None;
        plan.approval_epoch = Some("2".into());
        plan.approval_host = Some(
            if error.is_some() {
                "old-host"
            } else {
                "new-host"
            }
            .into(),
        );
        plan.approval_error = error.map(str::to_owned);
        let mut state = State::default();
        state.ui_mut(&Id::from(SESSION)).dock.plan_attempts.insert(
            CARD.into(),
            super::super::PlanAttempt {
                context: "compact".into(),
                acknowledged: true,
                observed: false,
                initial_epoch: Some("1".into()),
                ack_epoch: Some("2".into()),
                ack_host: Some("old-host".into()),
                duplicate: false,
                probe_needed: false,
                probe_failed: false,
                probe_unconfirmed: false,
                last_probe_plan: None,
                probe_id: None,
                prior_failures: Default::default(),
            },
        );
        state.settle(&feed);
        assert!(
            state.ui(SESSION).dock.plan_attempts.contains_key(CARD),
            "the fresh read must settle first"
        );
        let effects = state.act(
            Action::Dock(super::DockAction::Probe {
                session: Id::from(SESSION),
                card: CARD.into(),
            }),
            &feed,
        );
        let key = effects
            .iter()
            .find_map(|effect| match effect {
                crate::model::effect::Effect::SessionAction { key, .. } => Some(key.clone()),
                _ => None,
            })
            .unwrap();
        let host = if error.is_some() {
            "old-host"
        } else {
            "new-host"
        };
        let epoch = if error.is_some() { "2" } else { "0" };
        feed.operations.insert(
            key,
            Operation {
                method: "session.action".into(),
                loading: false,
                refusal: None,
                error: None,
                value: Some(
                    serde_json::json!({"plan_status": {"card":CARD,"host":host,"epoch":epoch,
                "pending":false,"open":true,"error":error,"failures":[]}}),
                ),
            },
        );
        state.settle(&feed);
        assert!(!state.ui(SESSION).dock.plan_attempts.contains_key(CARD));
        let notice = shown(&state, &feed).unwrap();
        assert!(
            notice
                .text
                .contains(error.unwrap_or("outcome is unavailable"))
        );
    }
}

fn superseded_failure_is_one_notice(host_note_delayed: bool) {
    let reason = format!(
        "The plan was not started: {} original failure",
        "x".repeat(250)
    );
    let mut feed = review();
    let plan = feed
        .live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .plan_mode
        .as_mut()
        .unwrap();
    plan.preparing = None;
    plan.approval_epoch = Some("3".into());
    plan.approval_host = Some("host".into());
    plan.approval_pending = Some(true);
    plan.approval_failures
        .push(corral_protocol::session_state::PlanApprovalFailure {
            epoch: "2".into(),
            error: reason.clone(),
        });
    let mut state = State::default();
    state.ui_mut(&Id::from(SESSION)).dock.plan_attempts.insert(
        CARD.into(),
        super::super::PlanAttempt {
            context: "compact".into(),
            acknowledged: false,
            observed: false,
            initial_epoch: Some("0".into()),
            ack_epoch: None,
            ack_host: None,
            duplicate: false,
            probe_needed: false,
            probe_failed: false,
            probe_unconfirmed: false,
            last_probe_plan: None,
            probe_id: None,
            prior_failures: Default::default(),
        },
    );
    state
        .ui_mut(&Id::from(SESSION))
        .notices
        .push_approval_wait(CARD.into(), "old wait", feed.now);
    request(&mut state, "duplicate");
    feed.operations.insert(Id::from("duplicate"), Operation {
        method: "session.card.answer".into(), loading: false,
        value: Some(serde_json::json!({"status":"waiting", "approval_epoch":"2", "approval_host":"host"})),
        error: None, refusal: None,
    });
    let add_host_note = |feed: &mut Feed| {
        feed.live
            .get_mut(SESSION)
            .unwrap()
            .state
            .as_mut()
            .unwrap()
            .notices
            .push(HostNotice {
                id: "plan-failure-host-2".into(),
                tone: "error".into(),
                text: reason[..240].into(),
                until: Some("2026-11-26T00:00:00Z".into()),
            });
    };
    if !host_note_delayed {
        add_host_note(&mut feed);
    }
    state.settle(&feed);
    let notice = shown(&state, &feed).unwrap();
    assert_eq!(notice.text, reason);
    assert!(
        state.ui(SESSION).dock.plan_attempts.contains_key(CARD),
        "the newer answer remains guarded"
    );
    state.ui_mut(&Id::from(SESSION)).notices.dismiss(&notice.id);
    if host_note_delayed {
        add_host_note(&mut feed);
        state.settle(&feed);
    }
    assert!(
        shown(&state, &feed).is_none(),
        "neither the timed note nor the obsolete wait should follow dismissal"
    );
    assert_eq!(
        state.ui(SESSION).notices.recent().count(),
        2,
        "the failure and the retired wait"
    );
}

#[path = "probe_tests.rs"]
mod probe_tests;

#[test]
fn a_superseded_failure_and_host_note_share_one_dismissal() {
    superseded_failure_is_one_notice(false);
}

#[test]
fn a_host_note_arriving_after_its_correlated_failure_was_dismissed_stays_hidden() {
    superseded_failure_is_one_notice(true);
}

#[test]
fn a_duplicate_while_other_feedback_fails_rearms_from_the_fresh_epoch() {
    let mut feed = review();
    let plan = feed
        .live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .plan_mode
        .as_mut()
        .unwrap();
    plan.preparing = None;
    plan.approval_epoch = Some("1".into());
    plan.approval_host = Some("host".into());
    let mut state = State::default();
    state.ui_mut(&Id::from(SESSION)).dock.plan_attempts.insert(
        CARD.into(),
        super::super::PlanAttempt {
            context: "feedback".into(),
            acknowledged: false,
            observed: false,
            initial_epoch: Some("1".into()),
            ack_epoch: None,
            ack_host: None,
            duplicate: false,
            probe_needed: false,
            probe_failed: false,
            probe_unconfirmed: false,
            last_probe_plan: None,
            probe_id: None,
            prior_failures: Default::default(),
        },
    );
    request(&mut state, "duplicate");
    feed.operations.insert(Id::from("duplicate"), Operation {
        method: "session.card.answer".into(), loading: false,
        value: Some(serde_json::json!({"status":"waiting", "approval_epoch":"2", "approval_host":"host"})),
        error: None, refusal: None,
    });
    state.settle(&feed);
    assert!(state.ui(SESSION).dock.plan_attempts.contains_key(CARD));
    let plan = feed
        .live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .plan_mode
        .as_mut()
        .unwrap();
    plan.approval_epoch = Some("2".into());
    plan.approval_pending = None;
    plan.approval_error = Some("The session did not take the feedback".into());
    state.settle(&feed);
    assert!(state.ui(SESSION).dock.plan_attempts.contains_key(CARD));
    let effects = state.act(
        Action::Dock(super::DockAction::Probe {
            session: Id::from(SESSION),
            card: CARD.into(),
        }),
        &feed,
    );
    let key = effects
        .iter()
        .find_map(|effect| match effect {
            crate::model::effect::Effect::SessionAction { key, .. } => Some(key.clone()),
            _ => None,
        })
        .unwrap();
    feed.operations.insert(key, Operation { method: "session.action".into(), loading: false, refusal: None, error: None,
        value: Some(serde_json::json!({"plan_status":{"card":CARD,"host":"host","epoch":"2","pending":false,
            "open":true,"error":"The session did not take the feedback","failures":[]}})),
    });
    state.settle(&feed);
    assert!(!state.ui(SESSION).dock.plan_attempts.contains_key(CARD));
    assert!(
        shown(&state, &feed)
            .unwrap()
            .text
            .contains("did not take the feedback")
    );
}

#[test]
fn dismissing_a_duplicate_approval_wait_leaves_the_original_in_flight() {
    let mut feed = review();
    let mut state = State::default();
    request(&mut state, "original");
    request(&mut state, "duplicate");
    refusal(&mut feed, "duplicate", DUPLICATE);
    state.settle(&feed);
    let wait = shown(&state, &feed).expect("the original approval is still pending");
    let pending = state.ui(SESSION).dock.pending.clone();
    assert_eq!(wait.hue, Hue::Held);
    assert!(pending.contains_key("original"));

    assert!(
        state
            .act(
                Action::Command(CommandAction::DismissNotice {
                    session: Id::from(SESSION),
                    notice: wait.id,
                }),
                &feed,
            )
            .is_empty()
    );
    assert!(shown(&state, &feed).is_none());
    state.settle(&feed);
    assert!(shown(&state, &feed).is_none());
    assert_eq!(state.ui(SESSION).dock.pending, pending);

    // Even with a stale live snapshot, a refusal of the original wins over a duplicate
    // reply observed in the same settlement pass, regardless of their request-key order.
    request(&mut state, "z-late-duplicate");
    refusal(&mut feed, "z-late-duplicate", DUPLICATE);
    refusal(&mut feed, "original", FAILURE);
    state.settle(&feed);
    let failure = shown(&state, &feed).expect("the original's failure is visible");
    assert_eq!(failure.hue, Hue::Fail);
    assert!(failure.text.contains(FAILURE));
}

#[test]
fn a_duplicate_wait_yields_to_live_preparation_failure_after_acknowledgment() {
    let mut feed = review();
    let mut state = State::default();
    request(&mut state, "compact");
    feed.operations.insert(
        Id::from("compact"),
        Operation {
            method: "session.card.answer".into(),
            loading: false,
            value: Some(serde_json::json!({ "status": "preparing" })),
            error: None,
            refusal: None,
        },
    );
    request(&mut state, "duplicate");
    refusal(&mut feed, "duplicate", DUPLICATE);
    state.settle(&feed);
    assert_eq!(
        shown(&state, &feed).map(|notice| notice.hue),
        Some(Hue::Held)
    );

    // A compaction failure leaves the same review waiting. Its reported reason must not be
    // hidden by this device's duplicate reply, even one arriving after that live update.
    state
        .ui_mut(&Id::from(SESSION))
        .notices
        .push(Hue::Fail, "older refusal", feed.now);
    let live = feed.live.get_mut(SESSION).unwrap().state.as_mut().unwrap();
    live.plan_mode.as_mut().unwrap().preparing = None;
    live.notices.push(HostNotice {
        id: "failure".into(),
        tone: "error".into(),
        text: format!("The plan was not started: {FAILURE}"),
        until: Some("2026-11-26T00:00:00Z".into()),
    });
    request(&mut state, "late-duplicate");
    refusal(&mut feed, "late-duplicate", DUPLICATE);
    state.settle(&feed);
    let failure = shown(&state, &feed).expect("the host's failure is visible");
    assert_eq!(failure.hue, Hue::Fail);
    assert!(failure.text.contains(FAILURE));
    state
        .ui_mut(&Id::from(SESSION))
        .notices
        .dismiss(&failure.id);
    let older = shown(&state, &feed).unwrap();
    state.ui_mut(&Id::from(SESSION)).notices.dismiss(&older.id);
    assert!(
        shown(&state, &feed).is_none(),
        "the host failure must not reappear"
    );

    // Once that failure expires, the obsolete, previously visible wait cannot come back.
    feed.live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .notices
        .clear();
    state.settle(&feed);
    assert!(shown(&state, &feed).is_none());

    // A reply can also arrive after the review has gone from both parts of the feed.
    feed.live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .cards
        .clear();
    feed.details.clear();
    request(&mut state, "closed-review");
    refusal(&mut feed, "closed-review", DUPLICATE);
    state.settle(&feed);
    assert!(shown(&state, &feed).is_none());
}
