use super::*;
use crate::model::daemon::{bare_task, task_row};
use corral_protocol::rows::ProjectRow;

fn project(id: &str) -> ProjectRow {
    ProjectRow {
        id: id.into(),
        config: serde_json::Map::new(),
        open: 0,
        ..ProjectRow::default()
    }
}

fn task(id: &str, project: Option<&str>, status: Status) -> Task {
    let mut task = bare_task(id, id, status, jiff::Timestamp::UNIX_EPOCH);
    task.project = project.map(str::to_owned);
    Task::new(task_row(task))
}

#[test]
fn project_page_rows_open_their_record_rather_than_select_into_a_panel_it_lacks() {
    use crate::model::work::{NowPanel, Press, State};

    let mut feed = Feed::default();
    feed.board_received(
        1,
        vec![task("T-1", Some("corral"), Status::Running)],
        &[],
        true,
    );
    feed.link = crate::model::daemon::Link::Up;
    let page = project_now(
        &State::default(),
        &feed,
        crate::model::Form::Desktop,
        &Id::from("corral"),
    );
    let actions: Vec<_> = page
        .sections
        .iter()
        .flat_map(|section| &section.rows)
        .map(|row| row.action.clone())
        .collect();
    assert_eq!(
        actions,
        vec![Press::Open(crate::model::Live::Task(Id::from("T-1")))]
    );
    assert!(matches!(page.panel, NowPanel::Digest(_)));
}

#[test]
fn project_sessions_are_the_live_runs_on_its_own_tasks() {
    use corral_protocol::rows::AgentRow;
    use corral_types::{Role, Run, SessionStatus};

    let agent = |task: &str| AgentRow {
        run: Run {
            id: "R1".into(),
            task: task.into(),
            role: Role::Worker,
            profile: "omp-sol".into(),
            harness: "omp".into(),
            model: None,
            effort: None,
            worktree: "/tmp/w".into(),
            branch: "corral/t".into(),
            base: "main".into(),
            base_sha: "abc".into(),
            session: None,
            started: jiff::Timestamp::UNIX_EPOCH,
            ended: None,
            outcome: None,
            summary: None,
            head_sha: None,
            quiet_since: None,
        },
        status: SessionStatus::Working,
    };
    let mut feed = Feed::default();
    feed.board.apply_batch(
        1,
        vec![
            task("T-1", Some("corral"), Status::Running),
            task("T-2", Some("other"), Status::Running),
        ],
        &[],
    );
    for task in ["T-1", "T-2"] {
        feed.agents
            .insert(Id::from(format!("{task}_R1")), agent(task));
    }
    let page = page(&feed, &Id::from("corral")).expect("project page");
    let ids: Vec<_> = page
        .sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect();
    assert_eq!(ids, vec!["T-1_R1"]);
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

#[test]
fn unassigned_filter_remains_accessible_when_last_open_task_is_assigned() {
    let mut feed = Feed::default();
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    feed.projects.insert("corral".into(), project("corral"));
    feed.board
        .apply_batch(1, vec![task("T-1", None, Status::Ready)], &[]);
    let filter = ProjectFilter::Unassigned;
    assert_eq!(filter.resolved(&feed), filter);
    feed.board
        .apply_batch(2, vec![task("T-1", Some("corral"), Status::Ready)], &[]);
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
    feed.board
        .apply_batch(1, vec![task("T-1", None, Status::Ready)], &[]);
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
        FilterScreen::Review,
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
fn a_project_chip_counts_its_blocked_tasks_only_once_the_open_tasks_arrive() {
    let mut feed = Feed::default();
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    feed.projects.insert("corral".into(), project("corral"));
    feed.projects.insert("site".into(), project("site"));
    let needs = |feed: &Feed| {
        chips(feed, &ProjectFilter::All, FilterScreen::Now)
            .into_iter()
            .map(|chip| chip.needs)
            .collect::<Vec<_>>()
    };
    // Before the open tasks arrive nothing is counted: no number, never a zero.
    assert!(needs(&feed).iter().all(Option::is_none));
    feed.board.apply_batch(
        1,
        vec![
            task("T-1", Some("corral"), Status::Blocked),
            task("T-2", Some("corral"), Status::Running),
        ],
        &[],
    );
    // All, corral, site: only corral has something waiting.
    assert_eq!(needs(&feed), vec![None, Some("1".into()), None]);
}
