//! One session as a Sessions row and its inspector read it: who it is, what it does, its model,
//! cost, context, age and its one next step. Every fact is the host's (through the session
//! page's own context) or the session's row; one neither reports is none, never a guess.

use crate::model::agents::{SessionLine, removable, session_line, unix};
use crate::model::daemon::{Feed, age};
use crate::model::effect::Env;
use crate::model::next::{NextStep, session_next_step};
use crate::model::session::{Cx, Facts, Ui, cost_text, quiet, role_word, spent, status};
use crate::model::status::SessionState;
use crate::model::{Id, Live};

/// One session's row.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub line: SessionLine,
    /// `T-12 R1` for a run, the chat's or build's own id.
    pub shown_id: String,
    /// `build`, `worker`, `chat`.
    pub role: String,
    /// The name, or `untitled` when the host recorded none.
    pub title: String,
    /// What it does now, with how long it has been quiet while it works.
    pub doing: String,
    /// The model without its provider: `claude-opus-5-5`.
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `$4.12`, `$4.12+`.
    pub cost: Option<String>,
    /// What it spent, and whether a member's cost is unknown.
    pub spent: (Option<f64>, bool),
    /// `38%` of the model's window.
    pub context: Option<String>,
    /// How long since it started.
    pub age: Option<String>,
    pub activity: Option<String>,
    pub model_id: Option<String>,
    pub quiet: Option<String>,
    pub branch: Option<String>,
    pub next: Option<NextStep>,
    /// `Remove…` (`btn-remove-<id>`): a build stopped here, its worktree, branch and record.
    pub remove: Option<crate::model::agents::Action>,
}

impl SessionRow {
    /// What a screen reader hears for the row.
    pub fn spoken(&self) -> String {
        let mut said = vec![
            self.line.state.word().to_owned(),
            self.shown_id.clone(),
            self.title.clone(),
            self.role.clone(),
        ];
        said.extend((!self.doing.is_empty()).then(|| self.doing.clone()));
        said.join(", ")
    }

    /// `claude-opus-5-5 · high`, when the model is known.
    pub fn model_effort(&self) -> Option<String> {
        let model = self.model.clone()?;
        Some(match &self.effort {
            Some(effort) => format!("{model} · {effort}"),
            None => model,
        })
    }

    /// `Remove…` as the row's step cell shows it: only for a build with no other step (its
    /// host gone), so the cell never holds three buttons. A stopped build whose exit still
    /// reports or asks keeps its step there and `Remove…` in the inspector and its Session menu.
    pub fn remove_in_row(&self) -> Option<crate::model::agents::Action> {
        self.remove.clone().filter(|_| self.next.is_none())
    }

    /// The session's page.
    pub fn live(&self) -> Live {
        Live::Session(self.line.id.clone())
    }
}

/// When the session started, in Unix seconds: a run's start, a chat's or build's creation.
pub(super) fn started(feed: &Feed, id: &str) -> Option<u64> {
    if let Some(run) = feed.agents.get(id) {
        return u64::try_from(run.run.started.as_second()).ok();
    }
    let chat = crate::model::assistant::row(feed, id)?;
    unix(chat.created.as_deref()?)
}

/// Session `id`'s row; none when the daemon lists no such session.
pub fn row(feed: &Feed, id: &Id, env: Env) -> Option<SessionRow> {
    let mut row = from_line(feed, session_line(feed, id.as_str())?, env);
    row.remove =
        removable(feed, id.as_str()).then(|| crate::model::agents::Action::Remove(id.clone()));
    Some(row)
}

pub(super) fn from_line(feed: &Feed, line: SessionLine, env: Env) -> SessionRow {
    let id = &line.id;
    let facts = Facts::of(feed, id);
    let ui = Ui::default();
    let cx = Cx::new(feed, &ui, id, env, &facts);
    let status = status(&cx);
    let spent = spent(&cx);
    let quiet = quiet(&cx);
    let doing = match (&line.doing, &quiet) {
        (Some(doing), Some(quiet)) => format!("{doing} · {quiet}"),
        (Some(doing), None) => doing.clone(),
        (None, Some(quiet)) => quiet.clone(),
        (None, None) => String::new(),
    };
    let branch = cx
        .build()
        .and_then(|build| build.branch.clone())
        .or_else(|| cx.run.map(|run| run.run.branch.clone()))
        .filter(|branch| !branch.is_empty());
    SessionRow {
        shown_id: cx.shown_id(),
        role: role_word(cx.role),
        title: if line.name.is_empty() {
            "untitled".into()
        } else {
            line.name.clone()
        },
        doing,
        model: cx
            .model()
            .map(|model| model.rsplit('/').next().unwrap_or(&model).to_owned()),
        effort: cx.effort(),
        cost: spent.0.map(|cost| cost_text(Some(cost), spent.1)),
        spent,
        context: (line.state != SessionState::Gone)
            .then_some(status.meter.percent)
            .flatten()
            .map(|percent| format!("{percent}%")),
        age: started(feed, id.as_str())
            .filter(|_| feed.now > 0)
            .map(|since| age(Some(since), feed.now)),
        activity: line
            .activity(feed)
            .filter(|_| feed.now > 0)
            .map(|since| age(Some(since), feed.now)),
        model_id: cx.model(),
        quiet,
        branch,
        next: session_next_step(&line),
        remove: None,
        line,
    }
}

/// `3 live · $6.17 spent by them`: the sessions whose host runs, and what those that report a
/// cost spent; no sum when none of them reports one.
pub fn summary(rows: &[SessionRow]) -> String {
    let live: Vec<&SessionRow> = rows
        .iter()
        .filter(|row| row.line.state != SessionState::Gone)
        .collect();
    let spent: Vec<f64> = live.iter().filter_map(|row| row.spent.0).collect();
    let mut said = format!("{} live", live.len());
    if !spent.is_empty() {
        let partial = spent.len() < live.len() || live.iter().any(|row| row.spent.1);
        said.push_str(&format!(
            " · {} spent by them",
            cost_text(Some(spent.iter().sum()), partial)
        ));
    }
    said
}
