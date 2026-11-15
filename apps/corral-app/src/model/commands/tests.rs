use super::*;

#[test]
fn free_text_ask_survives_even_when_a_command_matches() {
    let feed = Feed::default();
    let rows = filtered(
        "plan",
        false,
        DataState::Fixture,
        &feed,
        None,
        Form::Desktop,
    );
    assert!(
        rows.iter()
            .any(|row| row.always && matches!(row.action, Action::Ask(Some(_))))
    );
    assert!(rows.iter().any(|row| !row.always));
    for query in ["t-894", "go now", "open task"] {
        assert!(
            !filtered(query, false, DataState::Fixture, &feed, None, Form::Desktop)
                .iter()
                .any(|row| row.always)
        );
    }
}

#[test]
fn task_id_search_does_not_turn_navigation_into_a_question() {
    let rows = filtered(
        "t-894",
        false,
        DataState::Fixture,
        &crate::model::preview::feed(1_794_700_000, "2026-11-15".into(), "12:00".into(), None),
        None,
        Form::Desktop,
    );
    assert!(
        rows.iter()
            .any(|row| row.action == Action::OpenLive(Live::Task("T-894".into())))
    );
    assert!(!rows.iter().any(|row| matches!(row.action, Action::Ask(_))));
}

#[test]
fn daemon_palette_lists_served_records_only() {
    use corral_protocol::rows::PlanRow;
    let mut feed = Feed::default();
    feed.plans.insert(
        "T-31".into(),
        PlanRow {
            id: "T-31".into(),
            title: "Queue lifecycle".into(),
            state: PlanState::Awaiting,
            revision: Some("P1".into()),
            delivery: None,
            members_total: 2,
            members_landed: 0,
            blockers: Vec::new(),
            accepted_at: None,
            role: None,
            linked: None,
            rev: 1,
        },
    );
    let rows = filtered("", true, DataState::Daemon, &feed, None, Form::Desktop);
    assert!(rows.iter().any(
        |row| row.action == Action::OpenLive(Live::Plan("T-31".into()))
            && row.id == "live:plan:T-31"
    ));
    assert!(
        !rows.iter().any(|row| row.group == "Demo"
            || matches!(row.action, Action::Ask(_) | Action::ToggleSession))
    );
    assert_eq!(
        run(
            "live:plan:T-31",
            "",
            true,
            DataState::Daemon,
            &feed,
            None,
            Form::Desktop
        ),
        Some(Action::OpenLive(Live::Plan("T-31".into())))
    );
    feed.capabilities
        .insert(corral_protocol::app::ASSISTANT_V1.into());
    assert!(
        filtered("why", true, DataState::Daemon, &feed, None, Form::Desktop)
            .iter()
            .any(|row| matches!(row.action, Action::Ask(Some(_))))
    );
}

#[test]
fn desktop_and_terminal_never_offer_phone_only_demos() {
    for form in [Form::Desktop, Form::Terminal] {
        assert!(
            !commands("", false, DataState::Fixture, &Feed::default(), None, form)
                .iter()
                .any(|row| matches!(row.action, Action::Share | Action::Lock))
        );
    }
}
