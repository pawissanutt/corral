use super::{Action, RosterPanelTab, RosterRole, State, roster, roster_panel};
use crate::model::board::Mark;
use crate::model::daemon::{Feed, Group, Link, Step, Task, TopicState};
use crate::model::health::daemon_env;
use crate::model::projects::{FilterScreen, Filters, ProjectFilter};
use crate::model::{Form, Id, Selection};
use corral_protocol::rows::{
    AgentActivity, AgentRow, AgentStatus, AssistantKind, AssistantLocation, AssistantRow,
    AssistantState,
};

fn agent(id: &str, role: &str, task: Option<&str>) -> AgentRow {
    AgentRow {
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
        now: "recorded work".into(),
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
    }
}

fn task(id: &str, project: &str) -> Task {
    Task {
        id: id.into(),
        title: id.into(),
        project: Some(project.into()),
        plan: None,
        goal: None,
        group: Group::Producing,
        rail: std::array::from_fn(|_| Step {
            mark: Mark::Active,
            sha7: None,
            at: None,
        }),
        now: "working".into(),
        now_code: "working".into(),
        since: None,
        age_since: None,
        held_reason: None,
        questions: 0,
        attention: vec![],
        agent: None,
        cost: None,
        cost_estimated: false,
        rev: 1,
    }
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

fn feed() -> Feed {
    let mut feed = Feed {
        now: 1_800_000_000,
        link: Link::Up,
        ..Feed::default()
    };
    feed.topics.insert("agents".into(), TopicState::default());
    feed.topics
        .insert("assistant".into(), TopicState::default());
    feed.board
        .apply_batch(1, vec![task("T-1", "one"), task("T-2", "two")], &[]);
    feed
}

#[test]
fn project_scope_hides_other_tasks_but_not_workspace_sessions_and_persists() {
    let mut feed = feed();
    for row in [
        agent("w1", "worker", Some("T-1")),
        agent("w2", "worker", Some("T-2")),
        agent("pm", "pm", Some("T-2")),
        agent("A-7", "assistant", Some("T-2")),
        agent("unknown-task", "worker", Some("T-404")),
    ] {
        feed.agents.insert(row.id.clone().into(), row);
    }
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
    assert!(ids.contains(&"w1"));
    assert!(!ids.contains(&"w2") && !ids.contains(&"unknown-task"));
    assert!(ids.contains(&"pm") && ids.contains(&"A-7"));
    assert!(
        model
            .rows
            .iter()
            .filter(|row| row.session.as_str() == "pm" || row.session.as_str() == "A-7")
            .all(|row| row.project == "workspace")
    );
    let mut filters = Filters::default();
    filters.set("store", FilterScreen::Agents, filter.clone());
    let restored =
        Filters::parse(&filters.encode().expect("filters encode")).expect("filters parse");
    assert_eq!(restored.get("store", FilterScreen::Agents, &feed), filter);
    assert_eq!(
        restored.get("store", FilterScreen::Now, &feed),
        ProjectFilter::All
    );
}

#[test]
fn quick_thread_is_pinned_before_needs_you_and_never_duplicated() {
    let mut feed = feed();
    let mut worker = agent("worker", "worker", Some("T-1"));
    worker.status = AgentStatus::Asking;
    feed.agents.insert("worker".into(), worker);
    feed.assistants
        .insert("A-7".into(), assistant("A-7", AssistantKind::Quick));
    feed.assistants
        .insert("A-8".into(), assistant("A-8", AssistantKind::Session));
    feed.assistants
        .insert("A-9".into(), assistant("A-9", AssistantKind::Session));
    feed.agents
        .insert("A-8".into(), agent("A-8", "assistant", None));
    let env = daemon_env(&feed, Form::Desktop);
    let model = roster(&State::default(), &feed, env, &ProjectFilter::All);
    assert_eq!(
        model.rows.first().map(|row| row.session.as_str()),
        Some("A-7")
    );
    assert!(model.rows[0].pinned);
    assert!(
        !model
            .rows
            .iter()
            .any(|row| matches!(row.session.as_str(), "A-8" | "A-9"))
    );
    assert_eq!(
        model
            .roles
            .iter()
            .find(|tab| tab.id == "seg-agents-assistant")
            .map(|tab| tab.count),
        Some(1)
    );
    assert_eq!(
        model
            .stats
            .iter()
            .find(|stat| stat.key == "sessions")
            .map(|stat| stat.value.as_str()),
        Some("2")
    );
    let named = State {
        roster_session: Some("A-8".into()),
        ..State::default()
    };
    assert!(roster_panel(&named, &feed, env, &ProjectFilter::All, "device").is_none());
    feed.agents
        .insert("A-7".into(), agent("A-7", "assistant", None));
    let merged = roster(&State::default(), &feed, env, &ProjectFilter::All);
    assert_eq!(
        merged
            .rows
            .iter()
            .filter(|row| row.session.as_str() == "A-7")
            .count(),
        1
    );
}

#[test]
fn absent_session_steps_do_not_become_board_stages() {
    let mut feed = feed();
    feed.agents
        .insert("worker".into(), agent("worker", "worker", Some("T-1")));
    let state = State {
        roster_session: Some("worker".into()),
        ..State::default()
    };
    let panel = roster_panel(
        &state,
        &feed,
        daemon_env(&feed, Form::Desktop),
        &ProjectFilter::All,
        "device",
    )
    .expect("selected session panel");
    assert!(panel.steps.is_none());
    assert_eq!(
        panel.wants,
        vec![corral_protocol::detail::DetailRef::Session {
            id: "worker".into()
        }]
    );
}

#[test]
fn old_output_is_quiet_and_missing_or_future_output_stays_unknown() {
    let mut feed = feed();
    let at = "2027-01-15T08:00:00Z";
    let seconds = super::live::unix(at).expect("valid timestamp");
    feed.now = seconds + 38;
    let recent = super::roster::last_output(Some(at), &feed);
    assert!(!recent.starts_with("quiet"));
    feed.now = seconds + 9 * 60;
    assert!(super::roster::last_output(Some(at), &feed).starts_with("quiet"));
    assert_eq!(super::roster::last_output(None, &feed), "—");
    feed.now = seconds - 1;
    assert_eq!(super::roster::last_output(Some(at), &feed), "—");
}

#[test]
fn unnegotiated_activity_is_not_shown_and_keys_follow_only_visible_rows() {
    let mut feed = feed();
    let mut worker = agent("worker", "worker", Some("T-1"));
    worker.activity = Some(AgentActivity {
        until: "2027-01-15T08:00:00Z".into(),
        per_minute: vec![1; 20],
    });
    feed.agents.insert("worker".into(), worker);
    feed.agents.insert(
        "other-project".into(),
        agent("other-project", "worker", Some("T-2")),
    );
    let mut state = State {
        roster_tab: RosterPanelTab::Terminal,
        ..State::default()
    };
    let env = daemon_env(&feed, Form::Desktop);
    let filter = ProjectFilter::Project("one".into());
    let model = roster(&state, &feed, env, &filter);
    assert!(model.rows.iter().all(|row| row.activity.is_none()));
    state.key_live_filtered("j", None, &Selection::default(), &feed, env, &filter);
    assert_eq!(state.roster_session, Some(Id::from("worker")));
    assert_eq!(state.roster_tab, RosterPanelTab::Summary);
    state.act_roster(Action::SetRosterRole(RosterRole::Gate), &feed, env);
    assert!(roster_panel(&state, &feed, env, &filter, "device").is_none());
    state.key_live_filtered("escape", None, &Selection::default(), &feed, env, &filter);
    assert!(state.roster_session.is_none());
}

#[test]
fn retained_activity_names_its_recorded_window_instead_of_the_current_clock() {
    let mut feed = feed();
    let until = "2027-01-15T09:54:00Z";
    let end = super::live::unix(until).expect("valid timestamp");
    feed.now = end + 10 * 60;
    feed.link = Link::Down;
    feed.time_zone = Some(jiff::tz::TimeZone::UTC);
    feed.capabilities
        .insert(corral_protocol::AGENTS_ACTIVITY_V1.into());
    let mut worker = agent("worker", "worker", Some("T-1"));
    worker.activity = Some(AgentActivity {
        until: until.into(),
        per_minute: vec![1; 20],
    });
    feed.agents.insert("worker".into(), worker);
    let state = State {
        roster_session: Some("worker".into()),
        ..State::default()
    };
    let panel = roster_panel(
        &state,
        &feed,
        daemon_env(&feed, Form::Desktop),
        &ProjectFilter::All,
        "device",
    )
    .expect("selected retained session");
    assert!(
        panel
            .activity
            .as_ref()
            .expect("recorded activity")
            .hint
            .contains("09:54")
    );
    assert_eq!(panel.activity_axis[0], "09:34");
    assert_eq!(panel.activity_axis[2], "09:54");
    feed.now = end + 2 * 60;
    let current = roster_panel(
        &state,
        &feed,
        daemon_env(&feed, Form::Desktop),
        &ProjectFilter::All,
        "device",
    )
    .expect("selected recent session");
    assert_eq!(current.activity_axis[2], "now");
}

#[test]
fn preview_running_pm_uses_the_launch_predicates_canonical_session_id() {
    let feed =
        crate::model::preview::feed(1_800_000_000, "2027-01-15".into(), "08:00".into(), None);
    let model = roster(
        &State::default(),
        &feed,
        daemon_env(&feed, Form::Desktop),
        &ProjectFilter::All,
    );
    assert_eq!(
        feed.health.as_ref().map(|health| health.pm),
        Some(crate::model::daemon::PmLevel::Live)
    );
    assert_eq!(
        model
            .rows
            .iter()
            .find(|row| row.session.as_str() == "pm")
            .map(|row| row.shown_id.as_str()),
        Some("PM")
    );
    assert!(model.launch.is_none());
}
