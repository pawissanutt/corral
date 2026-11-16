use crate::model::{daemon::Feed, projects::is_open};
use corral_protocol::rows::{AgentStatus, AttentionCounts, InboxState, ProjectRow};

pub(super) fn sample_project(id: &str) -> Option<&'static str> {
    match id {
        "T-883" => None,
        "T-851" | "T-854" | "T-855" | "T-871" | "T-872" | "T-873" | "T-874" | "T-885" | "T-894"
        | "T-895" | "T-896" | "T-897" | "T-898" | "T-857" | "T-859" | "B-peer-roster"
        | "B-fleet-wait" => Some("mitigraph"),
        _ => Some("corral"),
    }
}

pub(super) fn populate(feed: &mut Feed) {
    for id in ["corral", "mitigraph", "mitiplay"] {
        feed.projects.insert(
            id.into(),
            ProjectRow {
                id: id.into(),
                path: format!("/preview/{id}"),
                open: 0,
                running: 0,
                review: 0,
                needs_me: 0,
                in_pm_scope: id != "mitiplay",
                autopilot: false,
                rev: 1,
            },
        );
    }
    refresh(feed);
}

pub(super) fn refresh(feed: &mut Feed) {
    for project in feed.projects.values_mut() {
        project.open = 0;
        project.running = 0;
        project.review = 0;
        project.needs_me = 0;
    }
    for task in feed.board.tasks() {
        if !is_open(task) {
            continue;
        }
        let Some(project) = task
            .project
            .as_deref()
            .and_then(|id| feed.projects.get_mut(id))
        else {
            continue;
        };
        project.open += 1;
        if task
            .agent
            .as_ref()
            .and_then(|agent| agent.session.as_deref())
            .and_then(|id| feed.agents.get(id))
            .is_some_and(|agent| agent.status != AgentStatus::Ended)
        {
            project.running += 1;
        }
        if task.now_code == "review" {
            project.review += 1;
        }
    }
    for item in feed
        .inbox
        .values()
        .filter(|item| item.state == InboxState::Open)
    {
        if item.task.is_none() {
            continue;
        }
        if let Some(project) = item
            .project
            .as_deref()
            .and_then(|id| feed.projects.get_mut(id))
        {
            let count = AttentionCounts::from_rows(std::slice::from_ref(item)).total;
            project.needs_me = project
                .needs_me
                .saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
        }
    }
}
