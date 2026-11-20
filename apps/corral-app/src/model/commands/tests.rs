use super::*;

#[test]
fn free_text_ask_survives_even_when_a_command_matches() {
    let feed = Feed::default();
    let rows = filtered(
        "tasks",
        false,
        DataState::Fixture,
        &feed,
        None,
        Form::Desktop,
        None,
    );
    assert!(
        rows.iter()
            .any(|row| row.always && matches!(row.action, Action::Ask(Some(_))))
    );
    assert!(rows.iter().any(|row| !row.always));
    for query in ["t-894", "go now", "open task"] {
        assert!(
            !filtered(
                query,
                false,
                DataState::Fixture,
                &feed,
                None,
                Form::Desktop,
                None
            )
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
        None,
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
            parent: crate::model::daemon::bare_task(
                "T-31",
                "Queue lifecycle",
                corral_types::Status::Backlog,
                jiff::Timestamp::UNIX_EPOCH,
            ),
            child_counts: Vec::new(),
        },
    );
    let rows = filtered(
        "",
        true,
        DataState::Daemon,
        &feed,
        None,
        Form::Desktop,
        None,
    );
    assert!(rows.iter().any(
        |row| row.action == Action::OpenLive(Live::Plan("T-31".into()))
            && row.id == "live:plan:T-31"
    ));
    assert!(
        !rows
            .iter()
            .any(|row| row.group == "Demo" || matches!(row.action, Action::Ask(_)))
    );
    assert_eq!(
        run(
            "live:plan:T-31",
            "",
            true,
            DataState::Daemon,
            &feed,
            None,
            Form::Desktop,
            None,
        ),
        Some(Action::OpenLive(Live::Plan("T-31".into())))
    );
    feed.capabilities
        .insert(corral_protocol::app::ASSISTANT_V1.into());
    assert!(
        filtered(
            "why",
            true,
            DataState::Daemon,
            &feed,
            None,
            Form::Desktop,
            None
        )
        .iter()
        .any(|row| matches!(row.action, Action::Ask(Some(_))))
    );
}

#[test]
fn the_palette_lists_every_operation_of_the_selected_task() {
    let feed = Feed::default();
    let task = crate::model::work::Record::Task("T-12".into());
    let rows = filtered(
        "",
        true,
        DataState::Daemon,
        &feed,
        Some(&task),
        Form::Desktop,
        None,
    );
    let actions: Vec<&str> = rows
        .iter()
        .filter(|row| row.group == "Actions")
        .map(|row| row.label.as_str())
        .collect();
    assert_eq!(actions.len(), 15);
    assert!(actions.contains(&"Dispatch… T-12"));
}

#[test]
fn desktop_and_terminal_never_offer_phone_only_demos() {
    for form in [Form::Desktop, Form::Terminal] {
        assert!(
            !commands(
                "",
                false,
                DataState::Fixture,
                &Feed::default(),
                None,
                form,
                None
            )
            .iter()
            .any(|row| matches!(row.action, Action::Share | Action::Lock))
        );
    }
}

#[test]
fn palette_offers_a_task_id_the_board_does_not_list() {
    let feed = Feed::default();
    let opens = Action::OpenLive(Live::Task("T-404".into()));
    for data in [DataState::Daemon, DataState::Fixture] {
        let rows = filtered(" t-404 ", true, data, &feed, None, Form::Desktop, None);
        let row = rows.iter().find(|row| row.action == opens).unwrap();
        assert_eq!(
            run(
                &row.id,
                " t-404 ",
                true,
                data,
                &feed,
                None,
                Form::Desktop,
                None
            ),
            Some(opens.clone())
        );
    }
    assert!(
        !filtered(
            "t-404",
            true,
            DataState::Disconnected,
            &feed,
            None,
            Form::Desktop,
            None,
        )
        .iter()
        .any(|row| row.action == opens)
    );
}
