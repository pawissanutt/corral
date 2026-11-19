use super::*;

#[test]
fn destination_keys_round_trip() {
    for destination in Destination::ALL {
        assert_eq!(Destination::from_key(destination.key()), Some(destination));
    }
}
#[test]
fn keys_are_ignored_while_overlay_is_open() {
    let mut nav = NavState::new(Form::Desktop);
    nav.open(Overlay::Ask);
    let before = nav.clone();
    assert!(!nav.press_key('i'));
    assert!(!nav.press_key('/'));
    assert_eq!(nav, before);
}
#[test]
fn newest_overlay_closes_first() {
    let mut nav = NavState::new(Form::Desktop);
    nav.open(Overlay::Ask);
    nav.open(Overlay::Palette);
    nav.open(Overlay::Ask);
    assert_eq!(nav.close_newest(), Some(Overlay::Ask));
    assert_eq!(nav.close_newest(), Some(Overlay::Palette));
    assert_eq!(nav.close_newest(), None);
}

#[test]
fn explicitly_opening_hidden_plans_and_reviews_reveals_their_requested_records() {
    use crate::model::daemon::{Task, bare_task, task_row};
    use crate::model::projects::{FilterScreen, Filters, ProjectFilter};
    use corral_protocol::rows::{PlanRow, ProjectRow};
    use corral_types::Status;

    let mut feed = crate::model::daemon::Feed {
        store: Some("store".into()),
        ..Default::default()
    };
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    for project in ["A", "B"] {
        feed.projects.insert(
            project.into(),
            ProjectRow {
                id: project.into(),
                config: serde_json::Map::new(),
                open: 1,
                ..ProjectRow::default()
            },
        );
    }
    let mut rows = Vec::new();
    for (id, project, status) in [("T-1", "A", Status::Backlog), ("T-2", "B", Status::Review)] {
        let mut task = bare_task(id, id, status, jiff::Timestamp::UNIX_EPOCH);
        task.project = Some(project.into());
        feed.plans.insert(
            id.into(),
            PlanRow {
                parent: task.clone(),
                child_counts: Vec::new(),
            },
        );
        rows.push(Task::new(task_row(task)));
    }
    feed.board.apply_batch(1, rows, &[]);
    for form in [Form::Desktop, Form::Terminal, Form::Phone] {
        let mut filters = Filters::default();
        filters.set(
            "store",
            FilterScreen::Plans,
            ProjectFilter::Project("A".into()),
        );
        let mut nav = NavState::new(form);
        let live = Live::Plan("T-2".into());
        let (screen, filter) = explicit_record_filter(&feed, &filters, &live).unwrap();
        filters.set("store", screen, filter);
        nav.open_live(live.clone(), form);
        let filter = filters.get("store", FilterScreen::Plans, &feed);
        assert_eq!(filter, ProjectFilter::Project("B".into()));
        assert_eq!(
            crate::model::plans::current_plan_filtered(&nav.selection, &feed, &filter),
            Some("T-2".into()),
        );
        assert_eq!(explicit_record_filter(&feed, &filters, &live), None);
        filters.set(
            "store",
            FilterScreen::Review,
            ProjectFilter::Project("A".into()),
        );
        let live = Live::Review("T-2".into());
        let (screen, filter) = explicit_record_filter(&feed, &filters, &live).unwrap();
        assert_eq!(screen, FilterScreen::Review);
        filters.set("store", screen, filter);
        nav.open_live(live.clone(), form);
        assert_eq!(
            filters.get("store", FilterScreen::Review, &feed),
            ProjectFilter::Project("B".into()),
        );
        assert_eq!(nav.selection.review.as_deref(), Some("T-2"));
        assert_eq!(explicit_record_filter(&feed, &filters, &live), None);
    }
}

#[test]
fn live_records_select_everywhere_and_push_only_where_the_form_stacks_them() {
    use crate::model::id::Id;
    for form in [Form::Desktop, Form::Terminal, Form::Phone] {
        let mut nav = NavState::new(form);
        let task = Live::Task(Id::from("T-9"));
        nav.open_live(task.clone(), form);
        assert_eq!(nav.destination, Destination::Now);
        assert_eq!(nav.selection.task.as_deref(), Some("T-9"));
        assert_eq!(nav.live(), (form == Form::Phone).then_some(&task));
        let session = Live::Session(Id::from("T-9_R1"));
        nav.open_live(session.clone(), form);
        nav.open_live(session.clone(), form);
        assert_eq!(nav.live(), Some(&session));
        assert_eq!(nav.pages.len(), if form == Form::Phone { 2 } else { 1 });
        assert!(nav.back());
        assert_eq!(nav.selection.session.as_deref(), Some("T-9_R1"));
    }
}

#[test]
fn repeated_settings_section_opens_remain_observable() {
    let mut nav = NavState::new(Form::Phone);
    nav.go_settings(SettingsSection::Devices);
    let first = nav.clone();
    nav.go_settings(SettingsSection::Devices);
    assert_ne!(nav, first);
    assert_eq!(nav.section_opens, first.section_opens.wrapping_add(1));
    let opens = nav.section_opens;
    nav.go(Destination::Settings);
    assert_eq!(nav.section_opens, opens);
    nav.section_opens = u32::MAX;
    nav.go_settings(SettingsSection::Devices);
    assert_eq!(nav.section_opens, 0);
}

#[test]
fn navigating_to_the_current_destination_remains_observable() {
    let mut nav = NavState::new(Form::Phone);
    let first = nav.clone();
    nav.go(nav.destination);
    assert_ne!(nav, first);
    assert_eq!(nav.goes, first.goes.wrapping_add(1));
    let goes = nav.goes;
    nav.go_settings(SettingsSection::Devices);
    assert_eq!(nav.goes, goes.wrapping_add(1));
    nav.goes = u32::MAX;
    nav.go(nav.destination);
    assert_eq!(nav.goes, 0);
}

#[test]
fn a_wide_choice_never_unfolds_a_narrow_sidebar_and_survives_the_return_to_wide() {
    let mut pref = SidebarPref::Auto;
    pref = pref.toggled(false).toggled(false);
    assert_eq!(pref, SidebarPref::Expanded);
    // Narrow: folded whatever was chosen wide, and the toggle keeps the wide choice.
    assert!(pref.collapsed(true));
    assert_eq!(pref.toggled(true), SidebarPref::Expanded);
    // Wide again: the choice made wide shows again.
    assert!(!pref.collapsed(false));
    assert!(SidebarPref::Collapsed.collapsed(false));
    for pref in [
        SidebarPref::Auto,
        SidebarPref::Expanded,
        SidebarPref::Collapsed,
    ] {
        assert_eq!(SidebarPref::from_slug(pref.slug()), Some(pref));
    }
}
