use super::*;

/// A place's key that did not come back to its place would open the wrong page, and a stored
/// slug that no longer parsed would drop the owner on Home after every restart.
#[test]
fn place_keys_and_slugs_round_trip() {
    for area in Area::ALL {
        assert_eq!(Area::from_key(area.key()), Some(area));
    }
    for destination in Destination::ALL {
        assert_eq!(
            Destination::from_slug(destination.slug()),
            Some(destination)
        );
    }
    let keys: Vec<String> = Area::ALL.map(|area| area.key().to_string()).to_vec();
    assert!(keys.iter().all(|key| PLACE_KEYS.contains(&key.as_str())));
}

/// A destination stored by the nine-destination app, an old link or an old key has to land on
/// the place that took its content over, never nowhere.
#[test]
fn the_former_destinations_open_their_new_homes() {
    for (slug, home) in [
        ("now", Destination::Home),
        ("inbox", Destination::Home),
        ("changes", Destination::Feed),
        ("plans", Destination::Tasks),
        ("review", Destination::Tasks),
        ("agents", Destination::Sessions),
        ("builds", Destination::Sessions),
        ("timeline", Destination::Timeline),
        ("usage", Destination::Usage),
        ("artifacts", Destination::Artifacts),
        ("system", Destination::Structure),
    ] {
        assert_eq!(Destination::from_slug(slug), Some(home), "{slug}");
    }
    let nav = NavState::new(Form::Desktop);
    for (key, home) in [
        ('n', Destination::Home),
        ('p', Destination::Tasks),
        ('r', Destination::Tasks),
        ('u', Destination::Usage),
        ('f', Destination::Artifacts),
        ('m', Destination::Structure),
    ] {
        assert_eq!(nav.key_page(key), Some(home), "{key}");
    }
}

/// `a` opened Feed every time, so reading Usage, leaving and coming back meant choosing the
/// page again: Activity opens at the page it last showed.
#[test]
fn activity_opens_at_its_last_page() {
    let mut nav = NavState::new(Form::Desktop);
    assert!(nav.press_key('a'));
    assert_eq!(nav.destination, Destination::Feed);
    nav.go(Destination::Usage);
    nav.go(Destination::Tasks);
    assert!(nav.press_key('a'));
    assert_eq!(nav.destination, Destination::Usage);
    assert_eq!(nav.destination.area(), Area::Activity);
}

/// System's views are Insight's pages: each its own page under Insight, none under Activity,
/// and `m` reopens the one last shown, as `a` does Activity's.
#[test]
fn insight_holds_the_system_views_and_opens_at_its_last_page() {
    use crate::model::system::View;
    for view in View::ALL {
        let page = Destination::of_view(view);
        assert_eq!(page.system_view(), Some(view));
        assert_eq!(page.area(), Area::Insight);
        assert!(!Destination::ACTIVITY.contains(&page));
    }
    // Git history is Insight's page too, though no System map view draws it.
    assert_eq!(Destination::History.area(), Area::Insight);
    assert!(Destination::INSIGHT.contains(&Destination::History));
    assert_eq!(Destination::History.system_view(), None);
    let mut nav = NavState::new(Form::Desktop);
    assert!(nav.press_key('m'));
    assert_eq!(nav.destination, Destination::Structure);
    nav.go(Destination::Lenses);
    nav.go(Destination::Usage);
    assert!(nav.press_key('m'));
    assert_eq!(nav.destination, Destination::Lenses);
    assert!(nav.press_key('a'));
    assert_eq!(nav.destination, Destination::Usage);
}

#[test]
fn keys_are_ignored_while_overlay_is_open() {
    let mut nav = NavState::new(Form::Desktop);
    nav.open(Overlay::Ask);
    let before = nav.clone();
    assert!(!nav.press_key('t'));
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

/// A task had a page only on the phone, and a plan and a review each another screen: every
/// form opens one page per task, a plan's and a review's being the task's, and leaving it
/// returns to Tasks, a session's to Sessions.
#[test]
fn every_record_opens_its_one_page_and_leaves_to_its_place() {
    use crate::model::id::Id;
    let task = Live::Task(Id::from("T-9"));
    for live in [
        task.clone(),
        Live::Plan(Id::from("T-9")),
        Live::Review(Id::from("T-9")),
    ] {
        let mut nav = NavState::new(Form::Desktop);
        nav.go(Destination::Home);
        nav.task_tab = TaskTab::History;
        nav.open_live(live);
        assert_eq!(nav.live(), Some(&task));
        assert_eq!(nav.selection.task.as_deref(), Some("T-9"));
        assert_eq!(nav.task_tab, TaskTab::Overview);
        assert!(nav.back());
        assert_eq!((nav.destination, nav.pages.len()), (Destination::Tasks, 0));
    }
    let mut nav = NavState::new(Form::Desktop);
    nav.open_live(task.clone());
    let session = Live::Session(Id::from("T-9_R1"));
    nav.open_live(session.clone());
    nav.open_live(session.clone());
    assert_eq!(nav.live(), Some(&session));
    assert_eq!(nav.pages.len(), 2);
    assert!(nav.back());
    assert_eq!(nav.live(), Some(&task));
    assert_eq!(nav.selection.session.as_deref(), Some("T-9_R1"));
    nav.pages.clear();
    nav.open_live(session);
    assert!(nav.back());
    assert_eq!(nav.destination, Destination::Sessions);
}

/// A workspace without projects has no board: its board places must not be offered at all,
/// on the sidebar, the phone's tabs or the palette.
#[test]
fn an_assistant_only_workspace_offers_home_sessions_usage_host_and_settings() {
    let mut feed = crate::model::daemon::Feed::default();
    assert_eq!(places(&feed), Area::PLACES);
    assert_eq!(phone_tabs(&feed), PhoneTab::ALL);
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    feed.topics.insert("projects".into(), Default::default());
    assert_eq!(places(&feed), [Area::Home, Area::Sessions, Area::Activity]);
    assert_eq!(
        activity_pages(&feed),
        [Destination::Usage, Destination::Host]
    );
    assert_eq!(
        destinations(&feed),
        [
            Destination::Home,
            Destination::Sessions,
            Destination::Usage,
            Destination::Host,
            Destination::Settings
        ]
    );
    assert_eq!(
        phone_tabs(&feed),
        [PhoneTab::Home, PhoneTab::Sessions, PhoneTab::Activity]
    );
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
