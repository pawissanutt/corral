use super::*;
use crate::model::daemon::{bare_task, task_row};
use crate::model::effect::Conn;

/// The unscoped workspace: no project chosen, grouped by state.
const ALL: ProjectFilter = ProjectFilter::All;
const BY_STATE: NowGroup = NowGroup::State;
const SCOPE: NowScope<'_> = NowScope {
    filter: &ALL,
    grouping: BY_STATE,
};

fn task(id: &str, status: Status) -> daemon::Task {
    daemon::Task::new(task_row(bare_task(
        id,
        &format!("task {id}"),
        status,
        jiff::Timestamp::UNIX_EPOCH,
    )))
}

fn ids(list: &GroupedList, key: GroupKey) -> Vec<String> {
    list.groups
        .iter()
        .filter(|group| group.key == key)
        .flat_map(|group| group.rows.iter())
        .filter_map(|row| match row {
            Row::Task(task) => Some(task.line.id.clone()),
            Row::More(_) => None,
        })
        .collect()
}

fn selected_after(effects: Option<Vec<Effect>>, current: &mut Option<Id>) {
    for effect in effects.unwrap_or_default() {
        if let Effect::Select(Live::Task(id)) = effect {
            *current = Some(id);
        }
    }
}

#[test]
fn unenrolled_or_revoked_device_hides_board_rows() {
    use crate::model::devices::{DeviceLink, ENROL_FIRST, REVOKED};

    let state = State::default();
    let mut feed = daemon::Feed::default();
    feed.board_received(
        1,
        vec![task("T-1", Status::Ready), task("T-2", Status::Blocked)],
        &[],
        true,
    );
    for (device, notice) in [
        (DeviceLink::Unenrolled, ENROL_FIRST),
        (DeviceLink::Revoked { id: "dev-1".into() }, REVOKED),
    ] {
        feed.device = device;
        let list = daemon_list_filtered(
            &state,
            &feed,
            &Selection::default(),
            Form::Desktop,
            &ALL,
            BY_STATE,
        );
        assert_eq!(list.notice, Some(notice));
        assert!(list.groups.is_empty());
    }
}

#[test]
fn rows_group_by_who_moves_next_in_the_plans_order() {
    let mut ready = task_row(bare_task(
        "T-7",
        "reviewed",
        Status::Review,
        jiff::Timestamp::UNIX_EPOCH,
    ));
    ready.ready = true;
    let tasks = [
        task("T-1", Status::Idea),
        task("T-2", Status::Running),
        task("T-3", Status::Blocked),
        task("T-4", Status::Ready),
        task("T-5", Status::Backlog),
        task("T-6", Status::Review),
        daemon::Task::new(ready),
    ];
    let refs: Vec<&daemon::Task> = tasks.iter().collect();
    let list = list(
        &State::default(),
        &refs,
        None,
        &daemon::Feed::default(),
        Form::Desktop,
        &ProjectFilter::All,
        NowGroup::State,
    );
    let keys: Vec<GroupKey> = list.groups.iter().map(|group| group.key).collect();
    assert_eq!(
        keys,
        [
            GroupKey::You,
            GroupKey::Corral,
            GroupKey::Worker,
            GroupKey::Queued,
            GroupKey::Backlog,
            GroupKey::Ideas
        ]
    );
    assert_eq!(ids(&list, GroupKey::You), ["T-3", "T-7"]);
    assert_eq!(ids(&list, GroupKey::Corral), ["T-6"]);
}

#[test]
fn collapsed_queued_rows_stay_out_of_keyboard_reach_until_expanded() {
    let tasks: Vec<daemon::Task> = (1..=COLLAPSED + 2)
        .map(|n| task(&format!("T-{n}"), Status::Ready))
        .collect();
    let refs: Vec<&daemon::Task> = tasks.iter().collect();
    let feed = daemon::Feed::default();
    let mut state = State::default();
    let mut current = Some(tasks[0].id.clone());
    for _ in 0..tasks.len() {
        let effects = state.key_tasks("j", current.as_ref(), &refs, &feed, Form::Desktop, SCOPE);
        selected_after(effects, &mut current);
    }
    assert_eq!(current, Some(tasks[COLLAPSED - 1].id.clone()));
    state.act(Action::ShowAll(GroupKey::Queued));
    for _ in 0..tasks.len() {
        let effects = state.key_tasks("j", current.as_ref(), &refs, &feed, Form::Desktop, SCOPE);
        selected_after(effects, &mut current);
    }
    assert_eq!(current, Some(tasks[tasks.len() - 1].id.clone()));
}

#[test]
fn an_off_board_selection_shows_its_own_detail_and_never_aliases_the_first_row() {
    use corral_protocol::detail::{DetailBody, TaskDetail};
    let mut feed = daemon::Feed::default();
    feed.board_received(
        1,
        vec![task("T-5", Status::Ready), task("T-6", Status::Review)],
        &[],
        true,
    );
    let target = super::super::task_ref("T-404");
    feed.details.insert(
        daemon::detail_key(&target),
        daemon::DetailCache {
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Task(TaskDetail {
                task: bare_task("T-404", "landed", Status::Done, jiff::Timestamp::UNIX_EPOCH),
                runs: Vec::new(),
                checks: Vec::new(),
                artifacts: Vec::new(),
                events: Vec::new(),
            })),
        },
    );
    let closed = Id::from("T-404");
    let selected = Selection {
        task: Some(closed.clone()),
        ..Selection::default()
    };
    let mut state = State::default();
    let list = daemon_list_filtered(&state, &feed, &selected, Form::Desktop, &ALL, BY_STATE);
    assert!(
        !list
            .groups
            .iter()
            .flat_map(|group| group.rows.iter())
            .any(|row| matches!(row, Row::Task(row) if row.selected))
    );
    let inspector = super::super::daemon_inspector_filtered(
        &state,
        &feed,
        &selected,
        Form::Desktop,
        &ALL,
        BY_STATE,
    );
    assert!(matches!(
        &inspector,
        super::super::Inspector::LiveStory(story) if story.id == "T-404" && story.who == "Done"
    ));
    // The list keys enter the list from its top.
    let mut current = Some(closed);
    let effects = state.key_live_filtered(
        "j",
        None,
        &selected,
        &feed,
        Env {
            conn: Conn::Healthy,
            form: Form::Desktop,
        },
        SCOPE,
    );
    selected_after(effects, &mut current);
    assert_eq!(current.as_deref(), Some("T-6"));
}

#[test]
fn tasks_topic_failure_stops_loading_and_marks_retained_rows_last_known() {
    use crate::model::daemon::{ClientError, TopicState};
    let mut feed = daemon::Feed::default();
    feed.link = daemon::Link::Up;
    feed.topics.insert(
        "tasks".into(),
        TopicState {
            loading: false,
            stale: true,
            error: Some(ClientError {
                code: "internal".into(),
                message: "projection failed".into(),
            }),
        },
    );
    let list = daemon_list_filtered(
        &State::default(),
        &feed,
        &Selection::default(),
        Form::Desktop,
        &ALL,
        BY_STATE,
    );
    assert_eq!(list.notice, Some(BOARD_UNAVAILABLE));
    feed.board_received(1, vec![task("T-1", Status::Ready)], &[], true);
    let status = super::super::daemon_status(&feed);
    assert!(status.dimmed);
    let retry = status
        .banner
        .and_then(|banner| banner.actions.into_iter().next())
        .and_then(|button| button.action);
    assert_eq!(retry, Some(Press::RetryBoard));
}
