use super::*;

#[test]
fn only_the_chat_hosts_profiles_are_models_and_the_exact_one_is_selected() {
    let mut feed = Feed::default();
    let snapshot: SettingsSnapshot = serde_json::from_value(serde_json::json!({
        "config": {"profiles": {
            "api": {"harness": "pi", "model": "claude-sonnet-4-6"},
            "subscription": {"harness": "pi", "model": "claude-sonnet-4-6", "effort": "high"},
            "worker": {"harness": "omp", "model": "opus"}
        }},
        "listener": null,
        "devices": []
    }))
    .unwrap();
    feed.settings = Some(snapshot);
    let list = model_list(&feed, "A-1", Some("subscription"));
    let names: Vec<_> = list
        .options
        .iter()
        .map(|option| option.name.as_str())
        .collect();
    assert_eq!(names, ["api", "subscription"]);
    assert!(!list.options[0].selected);
    assert!(list.options[1].selected);
    assert_eq!(
        list.options[0].action,
        Action::PickProfile {
            thread: "A-1".into(),
            profile: "api".into(),
        }
    );
}

/// A named session whose host is not running (after a daemon restart) lost its model pill, so
/// the owner could not change the profile the host starts with next.
#[test]
fn a_named_session_without_a_running_host_still_offers_its_model_pill() {
    use crate::model::Effect;
    use crate::model::effect::Conn;
    use corral_protocol::assistant::AssistantSessionOp;
    let mut feed = Feed {
        transport_ready: true,
        projection_ready: true,
        link: Link::Up,
        session: Some(serde_json::from_value(serde_json::json!({
            "device":{"id":"dev","name":"Desk","platform":"native","app_version":"0","enrolled_at":"2026-11-15T09:00:00Z","revoked_at":null},
            "authority":{"reply":true,"act":true,"assistant":true,"assistant_act":true,"admin":true},
            "seen":{"through":0,"at":null},"server_time":"2026-11-15T09:00:00Z"
        })).unwrap()),
        settings: serde_json::from_value(serde_json::json!({
            "config": {"profiles": {"api": {"harness": "pi", "model": "claude-sonnet-4-6"}}},
            "listener": null,
            "devices": []
        }))
        .unwrap(),
        ..Feed::default()
    };
    feed.assistants.insert(
        "A-1".into(),
        AssistantRow {
            id: "A-1".into(),
            kind: AssistantKind::Session,
            title: "Session".into(),
            state: AssistantState::Cold,
            live_in: AssistantLocation::None,
            profile: None,
            connection: None,
            focus: None,
            pending: 0,
            turn: None,
            cost_usd: None,
            cost_basis: None,
            wait: None,
            question_id: None,
            rev: 1,
            model: None,
            effort: None,
            created: None,
            updated: None,
            build: None,
        },
    );
    let env = Env {
        conn: Conn::Healthy,
        form: Form::Desktop,
    };
    let draft = Draft {
        scope: "A-1",
        text: "",
        context: None,
        staged: &[],
    };
    let mut state = State::default();
    assert!(
        live_composer(&state, &feed, draft, env)
            .model_pill
            .is_some()
    );
    state.act_live(Action::ToggleModelMenu, &feed);
    let composer = live_composer(&state, &feed, draft, env);
    let Some(menu) = composer.model_menu else {
        panic!("the open model menu is offered");
    };
    let effects = state.act_live(menu.options[0].action.clone(), &feed);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Session { op: AssistantSessionOp::SetProfile { thread, profile }, .. }]
            if thread == "A-1" && profile == "api"
    ));
}
