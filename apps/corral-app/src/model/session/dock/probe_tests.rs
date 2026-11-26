use super::*;

#[test]
fn a_live_completion_cannot_preempt_an_inflight_probe_and_unconfirmed_pending_can_retry() {
    for (live_epoch, status_host, status_epoch) in
        [("2", "new-host", "0"), ("1", "answer-host", "2")]
    {
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
        plan.approval_host = Some("answer-host".into());
        plan.approval_epoch = Some(live_epoch.into());
        plan.approval_pending = None;
        let baseline = feed.live[SESSION].state.as_ref().unwrap().plan_mode.clone();
        let mut state = State::default();
        let key = Id::from("probe");
        let ui = state.ui_mut(&Id::from(SESSION));
        ui.dock.plan_attempts.insert(
            CARD.into(),
            super::super::super::PlanAttempt {
                context: "compact".into(),
                acknowledged: true,
                observed: false,
                initial_epoch: Some("0".into()),
                ack_epoch: Some("2".into()),
                ack_host: Some("answer-host".into()),
                duplicate: false,
                probe_needed: false,
                probe_failed: false,
                probe_unconfirmed: false,
                last_probe_plan: baseline.clone(),
                probe_id: Some(key.clone()),
                prior_failures: Default::default(),
            },
        );
        ui.dock.pending.insert(
            key.clone(),
            Pending::Probe {
                card: CARD.into(),
                baseline,
            },
        );
        state.settle(&feed);
        assert!(state.ui(SESSION).dock.plan_attempts.contains_key(CARD));
        feed.operations.insert(key, Operation { method: "session.action".into(), loading: false, refusal: None, error: None,
            value: Some(serde_json::json!({"plan_status":{"card":CARD,"host":status_host,"epoch":status_epoch,
                "pending":true,"open":true,"failures":[]}})),
        });
        state.settle(&feed);
        let attempt = &state.ui(SESSION).dock.plan_attempts[CARD];
        assert!(
            attempt.probe_unconfirmed,
            "an older or different live snapshot cannot confirm pending"
        );
        assert!(super::super::super::sending(state.ui(SESSION), &feed, CARD));
    }
}

#[test]
fn a_fresh_idle_probe_supersedes_a_stale_busy_live_snapshot() {
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
    plan.approval_host = Some("stale-host".into());
    plan.approval_epoch = Some("1".into());
    plan.approval_pending = Some(true);
    let mut state = State::default();
    state.ui_mut(&Id::from(SESSION)).dock.plan_attempts.insert(
        CARD.into(),
        super::super::super::PlanAttempt {
            context: "compact".into(),
            acknowledged: true,
            observed: false,
            initial_epoch: Some("0".into()),
            ack_epoch: Some("2".into()),
            ack_host: Some("answer-host".into()),
            duplicate: false,
            probe_needed: true,
            probe_failed: false,
            probe_unconfirmed: false,
            last_probe_plan: None,
            probe_id: None,
            prior_failures: Default::default(),
        },
    );
    let effects = state.act(
        Action::Dock(super::super::DockAction::Probe {
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
        value: Some(serde_json::json!({"plan_status": {"card":CARD,"host":"current-host","epoch":"0","pending":false,"open":true,"failures":[]}})),
    });
    state.settle(&feed);
    assert!(!super::super::super::sending(
        state.ui(SESSION),
        &feed,
        CARD
    ));
    assert!(!super::super::super::live_busy(
        &state.ui(SESSION).dock,
        feed.live[SESSION].state.as_ref(),
        CARD
    ));
    assert!(
        shown(&state, &feed)
            .unwrap()
            .text
            .contains("outcome is unavailable")
    );
}

#[test]
fn a_probe_reports_original_failure_while_the_next_answer_stays_guarded() {
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
    plan.approval_host = Some("host".into());
    plan.approval_epoch = Some("3".into());
    plan.approval_pending = Some(true);
    let mut state = State::default();
    state.ui_mut(&Id::from(SESSION)).dock.plan_attempts.insert(
        CARD.into(),
        super::super::super::PlanAttempt {
            context: "compact".into(),
            acknowledged: true,
            observed: false,
            initial_epoch: Some("0".into()),
            ack_epoch: Some("2".into()),
            ack_host: Some("host".into()),
            duplicate: true,
            probe_needed: true,
            probe_failed: false,
            probe_unconfirmed: false,
            last_probe_plan: None,
            probe_id: None,
            prior_failures: Default::default(),
        },
    );
    let effects = state.act(
        Action::Dock(super::super::DockAction::Probe {
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
        value: Some(serde_json::json!({"plan_status": {"card":CARD,"host":"host","epoch":"3","pending":true,"open":true,
            "failures":[{"epoch":"2","error":"The plan was not started: original"}]}})),
    });
    state.settle(&feed);
    assert!(shown(&state, &feed).unwrap().text.contains("original"));
    assert!(state.ui(SESSION).dock.plan_attempts.contains_key(CARD));
    assert!(super::super::super::sending(state.ui(SESSION), &feed, CARD));
}

fn waiting_on_probe() -> (Feed, State, Id) {
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
    plan.approval_host = Some("answer-host".into());
    plan.approval_epoch = Some("2".into());
    plan.approval_pending = None;
    let mut state = State::default();
    state.ui_mut(&Id::from(SESSION)).dock.plan_attempts.insert(
        CARD.into(),
        super::super::super::PlanAttempt {
            context: "compact".into(),
            acknowledged: true,
            observed: false,
            initial_epoch: Some("1".into()),
            ack_epoch: Some("2".into()),
            ack_host: Some("answer-host".into()),
            duplicate: false,
            probe_needed: true,
            probe_failed: false,
            probe_unconfirmed: false,
            last_probe_plan: None,
            probe_id: None,
            prior_failures: Default::default(),
        },
    );
    let effects = state.act(
        Action::Dock(super::super::DockAction::Probe {
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
    (feed, state, key)
}

#[test]
fn a_failed_probe_holds_the_guard_and_its_warning_clears_after_retry() {
    let (mut feed, mut state, key) = waiting_on_probe();
    feed.operations.insert(
        key,
        Operation {
            method: "session.action".into(),
            loading: false,
            value: None,
            error: Some(ClientError {
                code: "unavailable".into(),
                message: "offline".into(),
            }),
            refusal: None,
        },
    );
    state.settle(&feed);
    assert!(state.ui(SESSION).dock.plan_attempts[CARD].probe_failed);
    assert!(super::super::super::sending(state.ui(SESSION), &feed, CARD));
    assert!(shown(&state, &feed).unwrap().text.contains("Retry status"));
    let effects = state.act(
        Action::Dock(super::super::DockAction::Probe {
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
    feed.operations.insert(key, Operation { method: "session.action".into(), loading: false, error: None, refusal: None,
        value: Some(serde_json::json!({"plan_status":{"card":CARD,"host":"new-host","epoch":"0","pending":false,"open":true,"failures":[]}})) });
    state.settle(&feed);
    assert!(!state.ui(SESSION).dock.plan_attempts.contains_key(CARD));
    let outcome = shown(&state, &feed).unwrap();
    assert!(outcome.text.contains("outcome is unavailable"));
    state
        .ui_mut(&Id::from(SESSION))
        .notices
        .dismiss(&outcome.id);
    assert!(
        shown(&state, &feed).is_none(),
        "the old status warning must not return"
    );
}

#[test]
fn an_expired_unseen_host_note_does_not_hide_an_unknown_probe_outcome() {
    let (mut feed, mut state, key) = waiting_on_probe();
    feed.live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .notices
        .push(HostNotice {
            id: "plan-failure-answer-host-2".into(),
            tone: "error".into(),
            text: "The plan was not started: old".into(),
            until: Some("1970-01-01T00:00:00Z".into()),
        });
    feed.operations.insert(key, Operation { method: "session.action".into(), loading: false, error: None, refusal: None,
        value: Some(serde_json::json!({"plan_status":{"card":CARD,"host":"new-host","epoch":"0","pending":false,"open":true,"failures":[]}})) });
    state.settle(&feed);
    assert!(
        shown(&state, &feed)
            .unwrap()
            .text
            .contains("outcome is unavailable")
    );
}

#[test]
fn a_superseded_probe_still_records_its_correlated_failure() {
    let (mut feed, mut state, key) = waiting_on_probe();
    let live = feed
        .live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .plan_mode
        .as_mut()
        .unwrap();
    live.approval_host = Some("new-host".into());
    live.approval_epoch = Some("0".into());
    live.approval_pending = Some(true);
    feed.operations.insert(key, Operation { method: "session.action".into(), loading: false, error: None, refusal: None,
        value: Some(serde_json::json!({"plan_status":{"card":CARD,"host":"answer-host","epoch":"2","pending":false,"open":true,
            "error":"The plan was not started: provider offline","failures":[]}})) });
    state.settle(&feed);
    assert!(
        shown(&state, &feed)
            .unwrap()
            .text
            .contains("provider offline")
    );
    assert!(state.ui(SESSION).dock.plan_attempts[CARD].probe_needed);
}

#[test]
fn a_returned_stale_frame_rechecks_and_retires_the_cross_host_wait() {
    let (mut feed, mut state, key) = waiting_on_probe();
    let stale = feed.live[SESSION].state.as_ref().unwrap().plan_mode.clone();
    feed.operations.insert(key, Operation { method: "session.action".into(), loading: false, error: None, refusal: None,
        value: Some(serde_json::json!({"plan_status":{"card":CARD,"host":"new-host","epoch":"0","pending":true,"open":true,"failures":[]}})) });
    state.settle(&feed);
    assert!(
        shown(&state, &feed)
            .unwrap()
            .text
            .contains("waiting for the current answer")
    );
    let live = feed
        .live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .plan_mode
        .as_mut()
        .unwrap();
    live.approval_host = Some("new-host".into());
    live.approval_epoch = Some("0".into());
    live.approval_pending = Some(true);
    state.settle(&feed);
    assert!(!state.ui(SESSION).dock.plan_attempts[CARD].probe_unconfirmed);
    feed.live
        .get_mut(SESSION)
        .unwrap()
        .state
        .as_mut()
        .unwrap()
        .plan_mode = stale;
    state.settle(&feed);
    assert!(state.ui(SESSION).dock.plan_attempts[CARD].probe_needed);
    let effects = state.act(
        Action::Dock(super::super::DockAction::Probe {
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
    feed.operations.insert(key, Operation { method: "session.action".into(), loading: false, error: None, refusal: None,
        value: Some(serde_json::json!({"plan_status":{"card":CARD,"host":"new-host","epoch":"0","pending":false,"open":true,"failures":[]}})) });
    state.settle(&feed);
    let outcome = shown(&state, &feed).unwrap();
    assert!(outcome.text.contains("outcome is unavailable"));
    state
        .ui_mut(&Id::from(SESSION))
        .notices
        .dismiss(&outcome.id);
    assert!(
        shown(&state, &feed).is_none(),
        "the settled cross-host wait must not reappear"
    );
}
