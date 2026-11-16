use super::*;
use corral_protocol::rows::ProjectRow;

fn project(id: &str) -> ProjectRow {
    ProjectRow {
        id: id.into(),
        path: id.into(),
        open: 0,
        running: 0,
        review: 0,
        needs_me: 0,
        in_pm_scope: false,
        autopilot: false,
        rev: 1,
    }
}

fn board_task(id: &str, project: &str) -> crate::model::daemon::Task {
    crate::model::daemon::Task {
        id: Id::from(id),
        title: id.into(),
        project: Some(project.into()),
        group: crate::model::daemon::Group::Producing,
        now: "working".into(),
        now_code: "working".into(),
        ..unassigned_task()
    }
}

#[test]
fn project_page_rows_open_their_record_rather_than_select_into_a_panel_it_lacks() {
    use crate::model::work::{NowPanel, NowRow, Press, State};

    let mut feed = Feed::default();
    feed.board_received(1, vec![board_task("T-1", "corral")], &[], true);
    feed.link = crate::model::daemon::Link::Up;
    let state = State {
        selected_ask: Some(Id::from("q-1")),
        ..State::default()
    };
    let page = project_now(
        &state,
        &feed,
        crate::model::Form::Desktop,
        &Id::from("corral"),
    );
    let actions: Vec<_> = page
        .sections
        .iter()
        .flat_map(|section| &section.rows)
        .filter_map(|row| match row {
            NowRow::Task(row) => Some(row.action.clone()),
            NowRow::Ask(_) => None,
        })
        .collect();
    assert_eq!(
        actions,
        vec![Press::Open(crate::model::Live::Task(Id::from("T-1")))]
    );
    assert!(matches!(page.panel, NowPanel::Digest(_)));
}

#[test]
fn project_sessions_are_those_on_its_own_tasks_without_the_workspace_pm() {
    use corral_protocol::rows::{AgentRow, AgentStatus};

    let agent = |id: &str, role: &str, task: Option<&str>| AgentRow {
        id: id.into(),
        role: role.into(),
        task: task.map(str::to_owned),
        run: None,
        title: id.into(),
        profile: None,
        runtime: None,
        machine: None,
        pane: None,
        status: AgentStatus::Working,
        now: String::new(),
        since: None,
        tail: vec![],
        cost_usd: None,
        cost_basis: None,
        tokens: None,
        rev: 1,
        generation: None,
        turn: None,
        controls: Default::default(),
        last_output_at: None,
        activity: None,
        steps: None,
    };
    let mut feed = Feed::default();
    feed.board.apply_batch(
        1,
        vec![board_task("T-1", "corral"), board_task("T-2", "other")],
        &[],
    );
    for row in [
        agent("w1", "worker", Some("T-1")),
        agent("w2", "worker", Some("T-2")),
        agent("pm", "pm", Some("T-1")),
    ] {
        feed.agents.insert(Id::from(row.id.as_str()), row);
    }
    let page = page(&feed, &Id::from("corral")).expect("project page");
    let ids: Vec<_> = page
        .sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(ids, vec!["w1"]);
}

#[test]
fn disappeared_project_filter_cannot_hide_every_row_forever() {
    let mut feed = Feed::default();
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    feed.projects.insert("corral".into(), project("corral"));
    let filter = ProjectFilter::Project("corral".into());
    assert_eq!(filter.resolved(&feed), filter);
    feed.projects.clear();
    assert_eq!(filter.resolved(&feed), ProjectFilter::All);
}

fn unassigned_task() -> crate::model::daemon::Task {
    use crate::model::daemon::{Group, Step, Task};

    Task {
        id: "T-1".into(),
        title: "Task".into(),
        project: None,
        plan: None,
        goal: None,
        group: Group::Other,
        rail: std::array::from_fn(|_| Step {
            mark: crate::model::board::Mark::Next,
            sha7: None,
            at: None,
        }),
        now: "queued".into(),
        now_code: "queued".into(),
        age_since: None,
        since: None,
        held_reason: None,
        questions: 0,
        attention: vec![],
        agent: None,
        cost: None,
        cost_estimated: false,
        rev: 1,
    }
}

#[test]
fn unassigned_filter_remains_accessible_when_last_open_task_is_assigned() {
    let mut feed = Feed::default();
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    feed.projects.insert("corral".into(), project("corral"));
    let mut task = unassigned_task();
    feed.board.apply_batch(1, vec![task.clone()], &[]);
    let filter = ProjectFilter::Unassigned;
    assert_eq!(filter.resolved(&feed), filter);
    task.project = Some("corral".into());
    task.rev = 2;
    feed.board.apply_batch(2, vec![task], &[]);
    assert_eq!(filter.resolved(&feed), filter);
    let chips = chips(&feed, &filter, FilterScreen::Now);
    assert!(
        chips
            .iter()
            .any(|chip| chip.filter == ProjectFilter::All && !chip.selected)
    );
    assert!(
        chips
            .iter()
            .any(|chip| chip.filter == ProjectFilter::Unassigned && chip.selected)
    );
}

#[test]
fn empty_loading_or_legacy_projects_do_not_hide_board_navigation() {
    let mut feed = Feed::default();
    assert_eq!(WorkspaceMode::of(&feed), WorkspaceMode::Managed);
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    assert_eq!(WorkspaceMode::of(&feed), WorkspaceMode::Managed);
    feed.topics.insert(
        "projects".into(),
        crate::model::daemon::TopicState {
            loading: true,
            ..Default::default()
        },
    );
    assert_eq!(WorkspaceMode::of(&feed), WorkspaceMode::Managed);
    feed.topics.insert("projects".into(), Default::default());
    assert_eq!(WorkspaceMode::of(&feed), WorkspaceMode::AssistantOnly);
    feed.capabilities.clear();
    assert_eq!(WorkspaceMode::of(&feed), WorkspaceMode::Managed);
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    feed.projects.insert("corral".into(), project("corral"));
    assert_eq!(WorkspaceMode::of(&feed), WorkspaceMode::Managed);
}

#[test]
fn persisted_filters_cannot_bleed_across_daemon_stores_or_screens() {
    let mut feed = Feed::default();
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    for id in ["corral", "all", "unassigned"] {
        feed.projects.insert(id.into(), project(id));
    }
    feed.board.apply_batch(1, vec![unassigned_task()], &[]);
    let mut filters = Filters::default();
    filters.set(
        "/one/.corral",
        FilterScreen::Now,
        ProjectFilter::Project("corral".into()),
    );
    filters.set(
        "/one/.corral",
        FilterScreen::Changes,
        ProjectFilter::Unassigned,
    );
    filters.set(
        "/two/.corral",
        FilterScreen::Now,
        ProjectFilter::Project("all".into()),
    );
    filters.set(
        "/two/.corral",
        FilterScreen::Inbox,
        ProjectFilter::Project("unassigned".into()),
    );
    filters.set_now_group("/one/.corral", NowGroup::Project);
    let encoded = filters.encode().expect("filter serialization");
    let restored = Filters::parse(&encoded).expect("filter deserialization");
    assert_eq!(restored, filters);
    assert_eq!(
        restored.get("/one/.corral", FilterScreen::Now, &feed),
        ProjectFilter::Project("corral".into())
    );
    assert_eq!(
        restored.get("/one/.corral", FilterScreen::Changes, &feed),
        ProjectFilter::Unassigned
    );
    assert_eq!(
        restored.get("/two/.corral", FilterScreen::Now, &feed),
        ProjectFilter::Project("all".into())
    );
    assert_eq!(
        restored.get("/missing/.corral", FilterScreen::Now, &feed),
        ProjectFilter::All
    );
    assert_eq!(restored.now_group("/one/.corral"), NowGroup::Project);
    assert_eq!(restored.now_group("/two/.corral"), NowGroup::State);
}

#[test]
fn a_project_chip_counts_what_waits_only_when_the_daemon_reports_it() {
    let mut feed = Feed::default();
    let filed = crate::model::daemon::Task {
        id: "T-2".into(),
        project: Some("corral".into()),
        ..unassigned_task()
    };
    feed.board
        .apply_batch(1, vec![unassigned_task(), filed], &[]);
    feed.projects.insert(
        "corral".into(),
        ProjectRow {
            needs_me: 3,
            ..project("corral")
        },
    );
    feed.projects.insert("site".into(), project("site"));
    let needs = |feed: &Feed| {
        chips(feed, &ProjectFilter::All, FilterScreen::Now)
            .into_iter()
            .map(|chip| chip.needs)
            .collect::<Vec<_>>()
    };
    // Without projects.v1 the rows carry no counts: corral gets no number, never a zero.
    let legacy = needs(&feed);
    assert_eq!(legacy.len(), 3, "All, corral, Unassigned");
    assert!(legacy.iter().all(Option::is_none), "{legacy:?}");
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    // All, corral, site, Unassigned: only corral has something waiting.
    assert_eq!(needs(&feed), vec![None, Some("3".into()), None, None]);
}
