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

/// One daemon's scope must not scope another's pages, and a project named like the menu's own
/// words ("all") is still that project after a restart.
#[test]
fn a_persisted_scope_cannot_bleed_across_daemon_stores() {
    let mut feed = Feed::default();
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    for id in ["corral", "all"] {
        feed.projects.insert(id.into(), project(id));
    }
    let mut scopes = Scopes::default();
    scopes.set("/one/.corral", Scope::Project("corral".into()));
    scopes.set("/two/.corral", Scope::Project("all".into()));
    let encoded = scopes.encode().expect("scope serialization");
    let restored = Scopes::parse(&encoded).expect("scope deserialization");
    assert_eq!(restored, scopes);
    assert_eq!(
        restored.get("/one/.corral", &feed),
        Scope::Project("corral".into())
    );
    assert_eq!(
        restored.get("/two/.corral", &feed),
        Scope::Project("all".into())
    );
    assert_eq!(restored.get("/missing/.corral", &feed), Scope::All);
    // The project left the daemon's list: every page would stay empty under it.
    feed.projects.remove("corral");
    assert_eq!(restored.get("/one/.corral", &feed), Scope::All);
}

#[test]
fn the_scope_menu_counts_what_waits_only_once_the_open_tasks_arrive() {
    let mut feed = Feed::default();
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    feed.projects.insert("corral".into(), project("corral"));
    feed.projects.insert("site".into(), project("site"));
    let needs = |feed: &Feed| {
        scope_menu(feed, &Scope::Project("site".into()))
            .into_iter()
            .map(|entry| (entry.needs, entry.selected))
            .collect::<Vec<_>>()
    };
    // Before the open tasks arrive nothing is counted: no number, never a zero.
    assert!(needs(&feed).iter().all(|(needs, _)| needs.is_none()));
    feed.board.apply_batch(
        1,
        vec![
            task("T-1", Some("corral"), Status::Blocked),
            task("T-2", Some("corral"), Status::Running),
        ],
        &[],
    );
    // All projects, corral, site: only corral has something waiting; site is in scope.
    assert_eq!(
        needs(&feed),
        vec![(None, false), (Some(1), false), (None, true)]
    );
}
