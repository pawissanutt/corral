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

#[test]
fn project_page_excludes_landed_tasks() {
    use crate::model::daemon::{Group, Step, Task};

    let task = |id: &str, state: &str| Task {
        id: Id::from(id),
        title: id.into(),
        project: Some("corral".into()),
        plan: None,
        goal: None,
        group: Group::Other,
        rail: std::array::from_fn(|_| Step {
            mark: crate::model::board::Mark::Next,
            sha7: None,
            at: None,
        }),
        now: state.into(),
        now_code: state.into(),
        age_since: None,
        since: None,
        held_reason: None,
        questions: 0,
        attention: vec![],
        agent: None,
        cost: None,
        cost_estimated: false,
        rev: 1,
    };
    let mut feed = Feed::default();
    feed.board
        .apply_batch(1, vec![task("T-1", "queued"), task("T-2", "landed")], &[]);
    let page = page(&feed, &Id::from("corral")).expect("project page");
    let ids: Vec<_> = page
        .tasks
        .iter()
        .flat_map(|group| &group.tasks)
        .map(|task| task.id.as_str())
        .collect();
    assert_eq!(ids, vec!["T-1"]);
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

#[test]
fn persisted_filters_cannot_bleed_across_daemon_stores_or_screens() {
    let mut feed = Feed::default();
    feed.capabilities
        .insert(corral_protocol::PROJECTS_V1.into());
    for id in ["corral", "all", "unassigned"] {
        feed.projects.insert(id.into(), project(id));
    }
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
