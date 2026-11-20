use super::{Action, Show, State, roster};
use crate::model::daemon::{Feed, Link, Task, TopicState, bare_task, task_row};
use crate::model::projects::ProjectFilter;
use crate::model::Id;
use corral_protocol::rows::{
    AgentRow, AssistantKind, AssistantLocation, AssistantRow, AssistantState, BuildFacts,
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

#[test]
fn project_scope_hides_other_projects_runs_but_not_the_quick_chat_and_persists() {
    let mut feed = feed(vec![
        agent("T-1", "R1", Role::Worker),
        agent("T-2", "R1", Role::Worker),
        agent("T-404", "R1", Role::Worker),
    ]);
    feed.assistants
        .insert("A-7".into(), assistant("A-7", AssistantKind::Quick));
    let filter = ProjectFilter::Project("one".into());
    let model = roster(&State::default(), &feed, &filter);
    let mut ids: Vec<_> = model.rows.iter().map(|row| row.session.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["A-7", "T-1_R1"]);
}

/// A build whose host stopped stays listed under Show All, with its Session menu's Resume, so
/// the owner can continue it; Live leaves it out.
#[test]
fn a_cold_build_is_listed_as_resumable() {
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

    let live = roster(&State::default(), &feed, &ProjectFilter::All);
    assert!(live.rows.iter().all(|row| row.session.as_str() != "B-k3x9"));

    let all = State {
        show: Show::All,
        ..State::default()
    };
    let model = roster(&all, &feed, &ProjectFilter::All);
    let row = model
        .rows
        .iter()
        .find(|row| row.session.as_str() == "B-k3x9")
        .expect("a stopped build is listed under All");
    assert!(row.resumable);
    assert_eq!(row.action, Action::OpenSession(Id::from("B-k3x9")));
    assert!(
        model
            .roles
            .iter()
            .any(|tab| tab.id == "seg-agents-build" && tab.count == 1)
    );
    assert_eq!(
        crate::model::agents::controls(&feed, "B-k3x9"),
        [crate::model::agents::Control::Relaunch]
    );
}

/// A build whose host runs in a terminal corrald cannot reach is not offered Resume here.
#[test]
fn a_build_open_in_another_terminal_is_not_resumable() {
    let mut feed = feed(Vec::new());
    let mut build = assistant("B-k3x9", AssistantKind::Build);
    build.state = AssistantState::Cold;
    build.live_in = AssistantLocation::Pane;
    feed.assistants.insert("B-k3x9".into(), build);
    let all = State {
        show: Show::All,
        ..State::default()
    };
    let model = roster(&all, &feed, &ProjectFilter::All);
    assert!(model.rows.iter().all(|row| !row.resumable));
    assert!(crate::model::agents::controls(&feed, "B-k3x9").is_empty());
}

#[test]
fn a_session_waiting_on_the_owner_leads_the_roster() {
    let mut blocked = agent("T-2", "R3", Role::Reviewer);
    blocked.status = SessionStatus::Blocked {
        excerpt: "Allow edit? (y/n)".into(),
    };
    let feed = feed(vec![agent("T-1", "R1", Role::Worker), blocked]);
    let model = roster(&State::default(), &feed, &ProjectFilter::All);
    let rows: Vec<_> = model.rows.iter().map(|row| row.session.as_str()).collect();
    assert_eq!(rows, ["T-2_R3", "T-1_R1"]);
    assert!(model.rows[0].needs.is_some());
}

#[test]
fn quiet_time_follows_the_runners_quiet_since_and_stays_unknown_without_it() {
    let mut quiet = agent("T-1", "R1", Role::Worker);
    let feed_now = 1_800_000_000;
    quiet.run.quiet_since =
        Some(jiff::Timestamp::from_second(feed_now - 9 * 60).expect("valid timestamp"));
    let feed = feed(vec![quiet, agent("T-2", "R1", Role::Worker)]);
    let model = roster(&State::default(), &feed, &ProjectFilter::All);
    let quiet: Vec<(&str, &str)> = model
        .rows
        .iter()
        .map(|row| (row.session.as_str(), row.quiet.as_str()))
        .collect();
    assert!(quiet.contains(&("T-1_R1", "quiet 9m")));
    assert!(quiet.contains(&("T-2_R1", "–")));
}
