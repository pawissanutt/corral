use super::{Action, RosterPanelTab, RosterRole, State, roster, roster_panel};
use crate::model::daemon::{Feed, Link, Task, TopicState, bare_task, task_row};
use crate::model::health::daemon_env;
use crate::model::projects::{FilterScreen, Filters, ProjectFilter};
use crate::model::{Form, Id, Selection};
use corral_protocol::rows::{
    AgentRow, AssistantKind, AssistantLocation, AssistantRow, AssistantState,
};
use corral_types::{Blocked, Role, Run, SessionRef, SessionStatus, Status};

/// A live run of `task`, working in a herdr pane.
pub(super) fn agent(task: &str, run: &str, role: Role) -> AgentRow {
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

fn assistant(id: &str, kind: AssistantKind) -> AssistantRow {
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
        rev: 1,
    }
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
fn project_scope_hides_other_projects_runs_but_not_the_quick_thread_and_persists() {
    let mut feed = feed(vec![
        agent("T-1", "R1", Role::Worker),
        agent("T-2", "R1", Role::Worker),
        agent("T-404", "R1", Role::Worker),
    ]);
    feed.assistants
        .insert("A-7".into(), assistant("A-7", AssistantKind::Quick));
    let filter = ProjectFilter::Project("one".into());
    let model = roster(
        &State::default(),
        &feed,
        daemon_env(&feed, Form::Desktop),
        &filter,
    );
    let ids: Vec<_> = model.rows.iter().map(|row| row.session.as_str()).collect();
    assert_eq!(ids, ["A-7", "T-1_R1"]);
    assert_eq!(model.rows[0].project, "workspace");
    let mut filters = Filters::default();
    filters.set("store", FilterScreen::Agents, filter.clone());
    let restored =
        Filters::parse(&filters.encode().expect("filters encode")).expect("filters parse");
    assert_eq!(restored.get("store", FilterScreen::Agents, &feed), filter);
}

#[test]
fn quick_thread_leads_a_blocked_run_and_each_run_row_carries_its_task_menu() {
    let mut blocked = agent("T-2", "R3", Role::Reviewer);
    blocked.status = SessionStatus::Blocked {
        excerpt: "Allow edit? (y/n)".into(),
    };
    let mut feed = feed(vec![agent("T-1", "R1", Role::Worker), blocked]);
    feed.assistants
        .insert("A-7".into(), assistant("A-7", AssistantKind::Quick));
    feed.assistants
        .insert("A-8".into(), assistant("A-8", AssistantKind::Session));
    let env = daemon_env(&feed, Form::Desktop);
    let model = roster(&State::default(), &feed, env, &ProjectFilter::All);
    let rows: Vec<_> = model
        .rows
        .iter()
        .map(|row| (row.session.as_str(), row.task.as_ref().map(Id::as_str)))
        .collect();
    assert_eq!(
        rows,
        [
            ("A-7", None),
            ("T-2_R3", Some("T-2")),
            ("T-1_R1", Some("T-1"))
        ]
    );
    assert!(model.rows[0].pinned);
    assert_eq!(model.rows[1].doing, "Allow edit? (y/n)");
    assert!(
        model
            .roles
            .iter()
            .any(|tab| tab.id == "seg-agents-reviewer" && tab.count == 1)
    );
}

#[test]
fn a_blocked_task_waits_on_the_owner_from_the_runs_panel() {
    let mut feed = feed(vec![agent("T-1", "R1", Role::Worker)]);
    let mut stuck = bare_task("T-1", "T-1", Status::Blocked, jiff::Timestamp::UNIX_EPOCH);
    stuck.project = Some("one".into());
    stuck.blocked = Some(Blocked {
        reason: "gate failed twice".into(),
        detail: None,
        next: vec![],
    });
    feed.board
        .apply_batch(2, vec![Task::new(task_row(stuck))], &[]);
    let state = State {
        roster_session: Some("T-1_R1".into()),
        ..State::default()
    };
    let panel = roster_panel(
        &state,
        &feed,
        daemon_env(&feed, Form::Desktop),
        &ProjectFilter::All,
        "device",
    )
    .expect("selected run panel");
    assert_eq!(panel.task, Some(Id::from("T-1")));
    assert_eq!(
        panel.waits.map(|wait| wait.text),
        Some("gate failed twice".to_owned())
    );
}

#[test]
fn quiet_time_follows_the_runners_quiet_since_and_stays_unknown_without_it() {
    let mut quiet = agent("T-1", "R1", Role::Worker);
    let feed_now = 1_800_000_000;
    quiet.run.quiet_since =
        Some(jiff::Timestamp::from_second(feed_now - 9 * 60).expect("valid timestamp"));
    let feed = feed(vec![quiet, agent("T-2", "R1", Role::Worker)]);
    let model = roster(
        &State::default(),
        &feed,
        daemon_env(&feed, Form::Desktop),
        &ProjectFilter::All,
    );
    let quiet: Vec<_> = model.rows.iter().map(|row| row.quiet.as_str()).collect();
    assert_eq!(quiet, ["quiet 9m", "—"]);
}

#[test]
fn keys_follow_only_visible_rows() {
    let feed = feed(vec![
        agent("T-1", "R1", Role::Worker),
        agent("T-2", "R1", Role::Worker),
    ]);
    let mut state = State {
        roster_tab: RosterPanelTab::Terminal,
        ..State::default()
    };
    let env = daemon_env(&feed, Form::Desktop);
    let filter = ProjectFilter::Project("one".into());
    state.key_live_filtered("j", None, &Selection::default(), &feed, env, &filter);
    assert_eq!(state.roster_session, Some(Id::from("T-1_R1")));
    assert_eq!(state.roster_tab, RosterPanelTab::Summary);
    state.act_roster(Action::SetRosterRole(RosterRole::Reviewer), &feed, env);
    assert!(roster_panel(&state, &feed, env, &filter, "device").is_none());
    state.key_live_filtered("escape", None, &Selection::default(), &feed, env, &filter);
    assert!(state.roster_session.is_none());
}
