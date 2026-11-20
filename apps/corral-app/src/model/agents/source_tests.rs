use super::{Control, controls, session_lines};
use crate::model::Id;
use crate::model::daemon::{Feed, Link, Task, TopicState, bare_task, task_row};
use crate::model::projects::ProjectFilter;
use crate::model::status::{SessionState, status_unavailable};
use corral_protocol::rows::{
    AgentRow, AssistantKind, AssistantLocation, AssistantRow, AssistantState, BuildFacts,
    WaitReason,
};
use corral_protocol::session_state::Counts;
use corral_types::{Role, Run, SessionRef, SessionStatus, Status};

/// A live run of `task`, working in a herdr pane.
pub(crate) fn agent(task: &str, run: &str, role: Role) -> AgentRow {
    AgentRow {
        run: Run {
            id: run.into(),
            task: task.into(),
            role,
            profile: "claude".into(),
            harness: "omp".into(),
            model: Some("opus".into()),
            effort: None,
            worktree: "/work/tree".into(),
            branch: format!("corral/{task}"),
            base: "main".into(),
            base_sha: "0".repeat(40),
            session: Some(SessionRef::Herdr {
                workspace: "w1".into(),
                pane: "w1:p1".into(),
            }),
            started: jiff::Timestamp::UNIX_EPOCH,
            ended: None,
            outcome: None,
            summary: None,
            head_sha: None,
            quiet_since: None,
        },
        status: SessionStatus::Working,
        paused: None,
    }
}

/// A chat or build row, ready and served by corrald.
pub(crate) fn assistant(id: &str, kind: AssistantKind) -> AssistantRow {
    AssistantRow {
        id: id.into(),
        kind,
        title: id.into(),
        state: AssistantState::Ready,
        live_in: AssistantLocation::Daemon,
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
    }
}

fn key(row: &AgentRow) -> Id {
    Id::from(format!("{}_{}", row.run.task, row.run.id).as_str())
}

fn task(id: &str, project: &str, status: Status) -> Task {
    let mut task = bare_task(id, id, status, jiff::Timestamp::UNIX_EPOCH);
    task.project = Some(project.into());
    Task::new(task_row(task))
}

fn feed(rows: Vec<AgentRow>) -> Feed {
    let mut feed = Feed {
        now: 1_800_000_000,
        link: Link::Up,
        ..Feed::default()
    };
    feed.topics.insert("agents".into(), TopicState::default());
    feed.topics
        .insert("assistant".into(), TopicState::default());
    feed.board.apply_batch(
        1,
        vec![
            task("T-1", "one", Status::Running),
            task("T-2", "two", Status::Running),
        ],
        &[],
    );
    for row in rows {
        feed.agents.insert(key(&row), row);
    }
    feed
}

fn ids(feed: &Feed, filter: &ProjectFilter) -> Vec<String> {
    session_lines(feed, filter)
        .into_iter()
        .map(|line| line.id.to_string())
        .collect()
}

#[test]
fn project_scope_hides_other_projects_runs_but_not_the_quick_chat() {
    let mut feed = feed(vec![
        agent("T-1", "R1", Role::Worker),
        agent("T-2", "R1", Role::Worker),
        agent("T-404", "R1", Role::Worker),
    ]);
    feed.assistants
        .insert("A-7".into(), assistant("A-7", AssistantKind::Quick));
    let mut ids = ids(&feed, &ProjectFilter::Project("one".into()));
    ids.sort_unstable();
    assert_eq!(ids, ["A-7", "T-1_R1"]);
}

/// A build whose host stopped stays listed, with Relaunch among its controls, so the owner can
/// continue it.
#[test]
fn a_cold_build_is_listed_and_can_be_relaunched() {
    let mut feed = feed(Vec::new());
    let mut build = assistant("B-k3x9", AssistantKind::Build);
    build.state = AssistantState::Cold;
    build.live_in = AssistantLocation::None;
    build.build = Some(BuildFacts {
        name: "walk".into(),
        project: Some("one".into()),
        worktree: "/repo/.worktrees/walk".into(),
        repo_root: "/repo".into(),
        branch: Some("build/walk".into()),
        base: Some("main".into()),
        here: false,
        exit: None,
        children: Counts::default(),
        diff: None,
    });
    feed.assistants.insert("B-k3x9".into(), build);
    assert!(ids(&feed, &ProjectFilter::All).contains(&"B-k3x9".to_owned()));
    assert_eq!(controls(&feed, "B-k3x9"), [Control::Relaunch]);
}

/// A build whose host runs in a terminal corrald cannot reach is not offered Resume here.
#[test]
fn a_build_open_in_another_terminal_is_not_resumable() {
    let mut feed = feed(Vec::new());
    let mut build = assistant("B-k3x9", AssistantKind::Build);
    build.state = AssistantState::Cold;
    build.live_in = AssistantLocation::Pane;
    feed.assistants.insert("B-k3x9".into(), build);
    assert!(controls(&feed, "B-k3x9").is_empty());
}

/// A live Pi host whose status read was refused (an older host's oversized `get_state`) is
/// neither waiting on the owner nor quietly idle nor gone: it has its own state and group, and
/// its line says the status is unavailable.
#[test]
fn a_chat_whose_status_is_unavailable_says_so_and_is_not_idle_gone_or_blocked() {
    let mut feed = feed(Vec::new());
    let mut chat = assistant("A-7", AssistantKind::Quick);
    chat.state = AssistantState::Other;
    chat.wait = Some(WaitReason {
        code: "status_unavailable".into(),
        text: "Pi status unavailable: response too large".into(),
        retry_at: None,
    });
    assert!(status_unavailable(&chat));
    feed.assistants.insert("A-7".into(), chat);
    let lines = session_lines(&feed, &ProjectFilter::All);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].state, SessionState::Unavailable);
    let groups = crate::model::sessions::sections(
        &feed,
        &crate::model::projects::Scope::All,
        &crate::model::sessions::State::default(),
    );
    let groups: Vec<_> = groups
        .iter()
        .map(|group| (group.key.as_str(), group.count))
        .collect();
    assert_eq!(groups, [("unavailable", 1)]);
    assert_eq!(
        lines[0].doing.as_deref(),
        Some("Pi status unavailable: response too large")
    );
}

#[test]
fn a_session_waiting_on_the_owner_leads_the_list() {
    let mut blocked = agent("T-2", "R3", Role::Reviewer);
    blocked.status = SessionStatus::Blocked {
        excerpt: "Allow edit? (y/n)".into(),
    };
    let feed = feed(vec![agent("T-1", "R1", Role::Worker), blocked]);
    assert_eq!(ids(&feed, &ProjectFilter::All), ["T-2_R3", "T-1_R1"]);
}
