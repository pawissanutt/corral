//! What a task's page lists under its band: the plan's children, what the task depends on, its
//! runs with the session each opens, its artifacts, the worker's summary from `done` and the
//! task's events; and the comment the History tab sends. Each reads the served row, the plan's
//! detail or the task's detail; what none of them serves is left out.

use corral_protocol::detail::{DetailBody, DetailRef, PlanDetail};
use corral_types::{Op, Outcome, Role, Run, Status};

use crate::model::act::Act;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::id::IdSupply;
use crate::model::status::status_word;
use crate::model::{Id, Live, work};

use super::{closed, task_at};

/// The blocks of the page and the inspector, by their headings.
pub const DESCRIPTION: &str = "Description";
pub const CHILDREN: &str = "Children";
pub const DEPENDS: &str = "Depends on";
pub const PIPELINE: &str = "Pipeline";
pub const CHECKS: &str = "Checks";
pub const RUNS: &str = "Runs";
pub const ARTIFACTS: &str = "Artifacts";
pub const FACTS: &str = "Facts";
pub const CHANGES: &str = "Changes";
pub const SUMMARY: &str = "Summary from done";
pub const DIFF: &str = "Diff";
pub const EVENTS: &str = "Events";
pub const HISTORY: &str = "History";

/// What a block says while it has nothing to list.
pub const NO_DESCRIPTION: &str = "No description.";
pub const NO_CHECKS: &str = "None yet.";
pub const NO_RUNS: &str = "No run yet.";
pub const NO_ARTIFACTS: &str = "None recorded.";
pub const NO_SUMMARY: &str = "No summary yet: the worker writes one when it runs `corral done`.";

/// The controls' words: the artifact finder narrowed to the task, the inspector's way to the
/// page, the Comment box's send, and Ask opened with the task as its context.
pub const FIND: &str = "Find in Artifacts";
pub const OPEN_PAGE: &str = "Open page";
pub const SEND: &str = "Comment";
pub const ASK: &str = "Ask about this";

/// `2 files`: the Changes block's count.
pub fn file_count(files: usize) -> String {
    if files == 1 {
        "1 file".to_owned()
    } else {
        format!("{files} files")
    }
}

/// The plan detail of a task with children.
pub fn plan_ref(id: &str) -> DetailRef {
    DetailRef::Plan { id: id.to_owned() }
}

fn plan_detail<'a>(feed: &'a Feed, id: &str) -> Option<&'a PlanDetail> {
    match feed.detail(&plan_ref(id))?.body()? {
        DetailBody::Plan(detail) => Some(detail),
        _ => None,
    }
}

/// Another task the page links to: a child or a dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    pub id: Id,
    /// Its status as served; none when corrald does not serve it.
    pub status: Option<Status>,
    pub title: String,
    /// `Done · after T-4`, `not open`.
    pub note: String,
}

impl Linked {
    /// Opens the linked task's page.
    pub fn open(&self) -> Live {
        Live::Task(self.id.clone())
    }
}

fn after(depends_on: &[String]) -> String {
    if depends_on.is_empty() {
        String::new()
    } else {
        format!(" · after {}", depends_on.join(", "))
    }
}

/// A plan's children: every one the plan's detail serves, Done ones included, else the open
/// ones the board holds. Empty for a task without children.
pub fn children(feed: &Feed, id: &str) -> Vec<Linked> {
    let linked = |task: &corral_types::Task| Linked {
        id: Id::from(task.id.as_str()),
        status: Some(task.status),
        title: task.title.clone(),
        note: format!("{}{}", status_word(task.status), after(&task.depends_on)),
    };
    match plan_detail(feed, id) {
        Some(detail) => detail.children.iter().map(linked).collect(),
        None => feed
            .board
            .children(id)
            .map(|child| linked(child.task()))
            .collect(),
    }
}

/// Whether the page lists children, and so keeps the plan's detail loaded.
pub fn is_plan(feed: &Feed, id: &str) -> bool {
    super::is_plan(feed, id) || plan_detail(feed, id).is_some()
}

/// What the task depends on, each with its status as corrald serves it.
pub fn depends(feed: &Feed, id: &str) -> Vec<Linked> {
    let Some(task) = task_at(feed, id) else {
        return Vec::new();
    };
    let served = task.row.dependencies.as_ref();
    task.task()
        .depends_on
        .iter()
        .map(|dependency| {
            let status = served
                .and_then(|served| served.iter().find(|row| row.id == *dependency))
                .and_then(|row| row.status)
                .or_else(|| feed.board.task(dependency).map(|task| task.status()));
            let title = feed
                .board
                .task(dependency)
                .map(|task| task.title().to_owned())
                .or_else(|| closed::find(feed, dependency).map(|task| task.title.clone()))
                .unwrap_or_default();
            Linked {
                id: Id::from(dependency.as_str()),
                note: status.map_or_else(|| "not open".to_owned(), |s| status_word(s).to_owned()),
                status,
                title,
            }
        })
        .collect()
}

/// One run of the task, which opens its session's page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunLine {
    /// `R2 · worker`.
    pub name: String,
    /// `pi-terra · started Nov 12 18:40 · done`.
    pub words: String,
    /// Whether corrald lists the run as live.
    pub live: bool,
    /// The session page it opens (`<task>_<run>`).
    pub session: Id,
    /// `btn-open-reviewer` for the live reviewer, which stays across rounds; else
    /// `btn-open-run-<run>`.
    pub test_id: String,
}

const fn role_word(role: Role) -> &'static str {
    match role {
        Role::Worker => "worker",
        Role::Reviewer => "reviewer",
    }
}

const fn outcome_word(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Done => "done",
        Outcome::Landed => "landed",
        Outcome::Failed => "failed",
        Outcome::Stopped => "stopped",
        Outcome::Superseded => "superseded",
    }
}

/// The task's runs, newest first: every one its detail serves, else the worker and reviewer
/// its row names.
pub fn runs(feed: &Feed, id: &str) -> Vec<RunLine> {
    let row = task_at(feed, id).map(|task| task.row.clone());
    let reviewer = row
        .as_ref()
        .and_then(|row| row.reviewer.as_ref())
        .map(|run| run.id.clone());
    let mut runs: Vec<Run> = match work::loaded_task(feed, id) {
        Some(detail) => detail.runs.clone(),
        None => row
            .iter()
            .flat_map(|row| row.run.iter().chain(row.reviewer.iter()))
            .cloned()
            .collect(),
    };
    if let Some(live) = row.as_ref().and_then(|row| row.reviewer.as_ref())
        && !runs.iter().any(|run| run.id == live.id)
    {
        runs.push(live.clone());
    }
    runs.sort_by(|a, b| b.started.cmp(&a.started));
    runs.iter()
        .map(|run| {
            let session = Id::from(format!("{id}_{}", run.id));
            let live = feed.agents.contains_key(&session);
            let mut words = vec![
                run.profile.clone(),
                format!("started {}", work::when_at(run.started, feed)),
            ];
            words.extend(run.outcome.map(|outcome| outcome_word(outcome).to_owned()));
            if live {
                words.push("live".to_owned());
            }
            RunLine {
                name: format!("{} · {}", run.id, role_word(run.role)),
                words: words.join(" · "),
                live,
                test_id: if reviewer.as_deref() == Some(run.id.as_str()) {
                    "btn-open-reviewer".to_owned()
                } else {
                    format!("btn-open-run-{}", run.id)
                },
                session,
            }
        })
        .collect()
}

/// One artifact the task recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactLine {
    pub kind: String,
    pub title: String,
    pub at: String,
}

/// The artifacts the task's detail lists, newest first.
pub fn artifacts(feed: &Feed, id: &str) -> Vec<ArtifactLine> {
    let Some(detail) = work::loaded_task(feed, id) else {
        return Vec::new();
    };
    let mut artifacts: Vec<_> = detail.artifacts.iter().collect();
    artifacts.sort_by(|a, b| b.at.cmp(&a.at));
    artifacts
        .into_iter()
        .map(|artifact| ArtifactLine {
            kind: artifact.kind.clone().unwrap_or_else(|| "file".to_owned()),
            title: artifact.title.clone(),
            at: work::when_at(artifact.at, feed),
        })
        .collect()
}

/// One changed file of the stored diff: its path and `+12 −3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLine {
    pub path: String,
    pub counts: String,
}

/// The files the review detail lists for the task's candidate, once it has arrived.
pub fn files(feed: &Feed, id: &str) -> Vec<FileLine> {
    let target = DetailRef::Review {
        task: id.to_owned(),
    };
    match feed.detail(&target).and_then(|cache| cache.body()) {
        Some(DetailBody::Review(review)) => review
            .files
            .iter()
            .map(|file| FileLine {
                path: file.path.clone(),
                counts: format!("+{} −{}", file.added, file.deleted),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// What the worker said at `done`: the newest worker run's summary.
pub fn summary(feed: &Feed, id: &str) -> Option<String> {
    let detail_runs = work::loaded_task(feed, id).map(|detail| detail.runs.clone());
    let row_run = task_at(feed, id).and_then(|task| task.row.run.clone());
    detail_runs
        .unwrap_or_default()
        .into_iter()
        .chain(row_run)
        .filter(|run| run.role == Role::Worker && run.summary.is_some())
        .max_by_key(|run| run.started)
        .and_then(|run| run.summary)
        .filter(|summary| !summary.trim().is_empty())
}

/// One event of the task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLine {
    pub at: String,
    /// What it says, from its kind and detail: `moved backlog → ready`, `gate failed`.
    pub text: String,
    pub by: &'static str,
}

const fn actor_word(actor: corral_types::Actor) -> &'static str {
    match actor {
        corral_types::Actor::Owner => "owner",
        corral_types::Actor::Agent => "agent",
        corral_types::Actor::Reviewer => "reviewer",
        corral_types::Actor::Assistant => "assistant",
        corral_types::Actor::Auto => "auto",
        corral_types::Actor::Extension => "extension",
    }
}

/// The task's events from its detail, newest first.
pub fn history(feed: &Feed, id: &str) -> Vec<EventLine> {
    let Some(detail) = work::loaded_task(feed, id) else {
        return Vec::new();
    };
    detail
        .events
        .iter()
        .rev()
        .map(|event| EventLine {
            at: work::when_at(event.ts, feed),
            text: work::event_text(event),
            by: actor_word(event.actor),
        })
        .collect()
}

/// What the History tab says while it lists nothing.
pub const NO_EVENTS: &str = "No events recorded for this task.";
/// The Comment box's words before anything is typed.
pub const COMMENT: &str = "Comment on this task…";

/// The `comment` operation with the box's words; nothing for an empty box.
pub fn comment(ids: &mut IdSupply, task: &Id, text: &str) -> Vec<Effect> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    vec![Effect::Act(Act::new(
        ids,
        Op::Comment {
            id: task.to_string(),
            text: text.to_owned(),
        },
        false,
    ))]
}

#[cfg(test)]
mod tests {
    use corral_types::Role;

    use super::*;
    use crate::model::daemon::{Task, bare_task, task_row};

    fn run(id: &str, role: Role, minute: i64) -> Run {
        Run {
            id: id.into(),
            task: "T-1".into(),
            role,
            profile: "pi".into(),
            harness: "pi".into(),
            model: None,
            effort: None,
            worktree: "/w".into(),
            branch: "t-1".into(),
            base: "main".into(),
            base_sha: "0000000".into(),
            session: None,
            started: jiff::Timestamp::from_second(minute * 60).unwrap_or_default(),
            ended: None,
            outcome: None,
            summary: None,
            head_sha: None,
            quiet_since: None,
        }
    }

    /// The live reviewer, which the row names beside the worker, was missing from the runs, so
    /// its session could not be opened from the task: it is listed, as the reviewer.
    #[test]
    fn the_live_reviewer_is_a_run_that_opens_its_session() {
        let mut row = task_row(bare_task(
            "T-1",
            "t",
            Status::Review,
            jiff::Timestamp::UNIX_EPOCH,
        ));
        row.run = Some(run("R1", Role::Worker, 1));
        row.reviewer = Some(run("R2", Role::Reviewer, 2));
        let mut feed = Feed::default();
        feed.board.apply_batch(1, vec![Task::new(row)], &[]);
        let runs = runs(&feed, "T-1");
        let ids: Vec<(&str, &str)> = runs
            .iter()
            .map(|run| (run.test_id.as_str(), run.session.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                ("btn-open-reviewer", "T-1_R2"),
                ("btn-open-run-R1", "T-1_R1")
            ]
        );
    }
}
