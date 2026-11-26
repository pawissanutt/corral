use corral_protocol::detail::CardKind;
use corral_protocol::session_state::SessionState;

use super::questions_tests::{SESSION, answers, card, feed_with, option, question};
use super::{Action, Body, DockAction, State};
use crate::model::Id;
use crate::model::daemon::Operation;
use crate::model::effect::Effect;
use crate::model::health::daemon_env;
use crate::model::nav::Form;

#[test]
fn side_pane_can_review_and_preparing_blocks_duplicate_approval() {
    use corral_protocol::session_state::{PlanModeBrief, PlanModePhase};
    let mut review = card(
        "plan-1",
        CardKind::PlanReview,
        vec![question(
            vec![option("execute"), option("execute_compact")],
            false,
        )],
    );
    review.body = Some("whole plan".into());
    let mut feed = feed_with(
        review,
        SessionState {
            model: Some("provider/model".into()),
            ..Default::default()
        },
    );
    feed.link = crate::model::daemon::Link::Up;
    let session = Id::from(SESSION);
    let mut state = State::default();
    state.act(
        Action::Dock(DockAction::Bring {
            session: session.clone(),
            card: "plan-1".into(),
        }),
        &feed,
    );
    let env = daemon_env(&feed, Form::Desktop);
    let pane = super::compact_in_place(&feed, &state, &session, env);
    assert!(matches!(pane.body, Body::PlanReview(_)));
    assert!(pane.dock.plan.as_ref().unwrap().options[1].action.is_some());
    feed.live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .plan_mode = Some(PlanModeBrief {
        phase: PlanModePhase::Review,
        title: None,
        revision: None,
        preparing: Some("compact".into()),
        approval_epoch: None,
        approval_host: None,
        approval_pending: None,
        approval_error: None,
        approval_failures: Vec::new(),
    });
    for page in [
        super::compact_in_place(&feed, &state, &session, env),
        super::page(&feed, &state, &session, env),
    ] {
        let review = page.dock.plan.unwrap();
        assert!(review.preparing.is_some());
        assert!(review.options.iter().all(|option| option.action.is_none()));
    }
    feed.live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .plan_mode
        .as_mut()
        .unwrap()
        .preparing = None;
    assert!(
        super::page(&feed, &state, &session, env)
            .dock
            .plan
            .unwrap()
            .options[0]
            .action
            .is_some()
    );
}

#[test]
fn preparation_ack_before_live_update_stays_visible_and_blocks_resubmission() {
    use corral_protocol::session_state::{PlanModeBrief, PlanModePhase};
    let mut review = card(
        "plan-1",
        CardKind::PlanReview,
        vec![question(
            vec![option("execute_compact"), option("revise")],
            false,
        )],
    );
    review.body = Some("whole plan".into());
    let mut feed = feed_with(
        review,
        SessionState {
            model: Some("provider/model".into()),
            ..Default::default()
        },
    );
    feed.link = crate::model::daemon::Link::Up;
    let session = Id::from(SESSION);
    feed.live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .plan_mode = Some(PlanModeBrief {
        phase: PlanModePhase::Review,
        title: None,
        revision: None,
        preparing: None,
        approval_epoch: Some("0".into()),
        approval_host: Some("initial-host".into()),
        approval_pending: None,
        approval_error: None,
        approval_failures: Vec::new(),
    });
    let mut state = State::default();
    state.act(
        Action::Dock(DockAction::Bring {
            session: session.clone(),
            card: "plan-1".into(),
        }),
        &feed,
    );
    let action = Action::Dock(DockAction::ChoosePlan {
        session: session.clone(),
        card: "plan-1".into(),
        option: "execute_compact".into(),
        expected_model: Some("provider/model".into()),
    });
    let effects = state.act(action.clone(), &feed);
    let key = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SessionCardAnswer { key, .. } => Some(key.clone()),
            _ => None,
        })
        .unwrap();
    feed.operations.insert(
        key,
        Operation {
            method: "session.card.answer".into(),
            loading: false,
            value: Some(serde_json::json!({"status":"preparing", "approval_epoch":"2", "approval_host":"host"})),
            error: None,
            refusal: None,
        },
    );
    let env = daemon_env(&feed, Form::Desktop);
    for settled in [false, true] {
        if settled {
            state.settle(&feed);
        }
        for page in [
            super::page(&feed, &state, &session, env),
            super::compact_in_place(&feed, &state, &session, env),
        ] {
            let review = page.dock.plan.unwrap();
            assert!(review.preparing.is_some());
            assert!(review.options.iter().all(|option| option.action.is_none()));
        }
        assert!(state.act(action.clone(), &feed).is_empty());
    }
    // Even a completed ACK-host snapshot cannot settle before the scheduled fresh read.
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
    plan.approval_host = Some("host".into());
    plan.approval_error = Some("The plan was not started: old snapshot".into());
    state.settle(&feed);
    assert!(state.act(action.clone(), &feed).is_empty());
    // An unrelated, older failure may arrive after this attempt's ACK.
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
    plan.approval_epoch = Some("1".into());
    plan.approval_host = Some("intermediate-host".into());
    plan.approval_error = Some("another attempt failed".into());
    state.settle(&feed);
    assert!(state.act(action.clone(), &feed).is_empty());
    feed.live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .plan_mode = Some(PlanModeBrief {
        phase: PlanModePhase::Review,
        title: None,
        revision: None,
        preparing: Some("compact".into()),
        approval_epoch: Some("2".into()),
        approval_host: Some("host".into()),
        approval_pending: Some(true),
        approval_error: None,
        approval_failures: Vec::new(),
    });
    state.settle(&feed);
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
    state.settle(&feed);
    assert!(
        state.act(action.clone(), &feed).is_empty(),
        "admission is still pending"
    );
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
    plan.approval_pending = None;
    plan.approval_error = Some("The plan was not started: offline".into());
    feed.live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .notices
        .push(corral_protocol::session_state::HostNotice {
            id: "expired".into(),
            tone: "error".into(),
            text: "The plan was not started: offline".into(),
            until: Some("1970-01-01T00:00:00Z".into()),
        });
    state.settle(&feed);
    assert!(
        state.act(action.clone(), &feed).is_empty(),
        "the post-ACK probe has not settled"
    );
    let effects = state.act(
        Action::Dock(DockAction::Probe {
            session: session.clone(),
            card: "plan-1".into(),
        }),
        &feed,
    );
    let key = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SessionAction { key, .. } => Some(key.clone()),
            _ => None,
        })
        .unwrap();
    feed.operations.insert(key, Operation { method: "session.action".into(), loading: false, refusal: None, error: None,
        value: Some(serde_json::json!({"plan_status": {"card":"plan-1","host":"host","epoch":"2","pending":false,"open":true,
            "error":"The plan was not started: offline","failures":[]}})),
    });
    state.settle(&feed);
    assert!(
        state
            .ui(SESSION)
            .notices
            .recent()
            .any(|notice| notice.text.contains("offline"))
    );
    assert!(!state.act(action, &feed).is_empty());
}

#[test]
fn duplicate_keep_planning_waits_and_surfaces_the_other_feedback_failure() {
    use corral_protocol::session_state::{PlanModeBrief, PlanModePhase};
    let mut review = card(
        "plan-1",
        CardKind::PlanReview,
        vec![question(vec![option("execute"), option("revise")], false)],
    );
    review.body = Some("whole plan".into());
    let mut feed = feed_with(
        review,
        SessionState {
            model: Some("provider/model".into()),
            plan_mode: Some(PlanModeBrief {
                phase: PlanModePhase::Review,
                title: None,
                revision: None,
                preparing: None,
                approval_epoch: Some("0".into()),
                approval_host: Some("host".into()),
                approval_pending: None,
                approval_error: None,
                approval_failures: Vec::new(),
            }),
            ..Default::default()
        },
    );
    feed.link = crate::model::daemon::Link::Up;
    let session = Id::from(SESSION);
    let mut state = State::default();
    state.act(
        Action::Dock(DockAction::Bring {
            session: session.clone(),
            card: "plan-1".into(),
        }),
        &feed,
    );
    state.act(
        Action::Dock(DockAction::Text {
            session: session.clone(),
            card: "plan-1".into(),
            question: 0,
            text: "Keep this feedback".into(),
        }),
        &feed,
    );
    let action = Action::Dock(DockAction::Submit {
        session: session.clone(),
        card: "plan-1".into(),
    });
    let sent = state.act(action.clone(), &feed);
    let key = sent
        .iter()
        .find_map(|effect| match effect {
            Effect::SessionCardAnswer { key, .. } => Some(key.clone()),
            _ => None,
        })
        .unwrap();
    feed.operations.insert(key, Operation { method: "session.card.answer".into(), loading: false,
        value: Some(serde_json::json!({"status":"waiting", "approval_epoch":"2", "approval_host":"host"})), error: None, refusal: None });
    state.settle(&feed);
    assert!(state.act(action.clone(), &feed).is_empty());
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
    plan.approval_error = Some("The session did not take the feedback".into());
    state.settle(&feed);
    assert!(
        state.act(action.clone(), &feed).is_empty(),
        "the fresh read must settle first"
    );
    let effects = state.act(
        Action::Dock(DockAction::Probe {
            session: session.clone(),
            card: "plan-1".into(),
        }),
        &feed,
    );
    let key = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SessionAction { key, .. } => Some(key.clone()),
            _ => None,
        })
        .unwrap();
    feed.operations.insert(key, Operation { method: "session.action".into(), loading: false, refusal: None, error: None,
        value: Some(serde_json::json!({"plan_status":{"card":"plan-1", "host":"host", "epoch":"2", "open":true,
            "pending":false, "error":"The session did not take the feedback", "failures":[]}})),
    });
    state.settle(&feed);
    assert!(!state.act(action, &feed).is_empty());
    assert!(
        state
            .ui(SESSION)
            .notices
            .recent()
            .any(|notice| notice.text.contains("did not take the feedback"))
    );
}

#[test]
fn a_fresh_idle_probe_supersedes_only_stale_answering_availability() {
    use corral_protocol::session_state::{PlanModeBrief, PlanModePhase};
    let mut busy = option("execute");
    busy.unavailable = Some("This review is already being answered; wait for it".into());
    let mut denied = option("execute_clean");
    denied.unavailable = Some("the model is unavailable".into());
    let mut review = card(
        "plan-1",
        CardKind::PlanReview,
        vec![question(vec![busy, denied], false)],
    );
    review.body = Some("whole plan".into());
    let plan = PlanModeBrief {
        phase: PlanModePhase::Review,
        title: None,
        revision: None,
        preparing: Some("compact".into()),
        approval_epoch: Some("3".into()),
        approval_host: Some("old-host".into()),
        approval_pending: Some(true),
        approval_error: None,
        approval_failures: Vec::new(),
    };
    let mut feed = feed_with(
        review,
        SessionState {
            model: Some("provider/model".into()),
            plan_mode: Some(plan.clone()),
            ..Default::default()
        },
    );
    feed.link = crate::model::daemon::Link::Up;
    let session = Id::from(SESSION);
    let mut state = State::default();
    state.act(
        Action::Dock(DockAction::Bring {
            session: session.clone(),
            card: "plan-1".into(),
        }),
        &feed,
    );
    state.ui_mut(&session).dock.resolved_probes.insert(
        "plan-1".into(),
        super::dock::ProbeResolution {
            baseline: Some(plan),
            host: "current-host".into(),
            epoch: "0".into(),
            open: true,
        },
    );
    let env = daemon_env(&feed, Form::Desktop);
    for page in [
        super::page(&feed, &state, &session, env),
        super::compact_in_place(&feed, &state, &session, env),
    ] {
        let review = page.dock.plan.unwrap();
        assert!(review.options[0].action.is_some());
        assert!(review.options[1].action.is_none());
    }
    let action = super::page(&feed, &state, &session, env)
        .dock
        .plan
        .unwrap()
        .options[0]
        .action
        .clone()
        .unwrap();
    assert_eq!(answers(&state.act(action, &feed)).len(), 1);
}
