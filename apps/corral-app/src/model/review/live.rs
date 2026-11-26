//! The served candidate: the board's Review tasks, the selected task's `Review` and `Task`
//! details, and their evidence, as the shared Review view types. Nothing here is fixture data:
//! a field corrald does not serve is shown as unknown, never filled in.

use corral_protocol::detail::{DetailBody, DetailRef, ReviewDetail, TaskDetail};
use corral_types::{Actor, Check, CheckKind, CheckResult, Run, Status};

use crate::model::daemon::{DetailCache, Feed, Group, Task};
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::live::Selection;
use crate::model::projects::ProjectFilter;
use crate::model::text::span;

use super::Action;
use super::evidence::{TextPage, artifact_text};
use super::view::{Body, ButtonStyle, LogView, Notice, ReportView, button};

pub(super) fn review_ref(task: &Id) -> DetailRef {
    DetailRef::Review {
        task: task.to_string(),
    }
}

pub(super) fn task_ref(task: &Id) -> DetailRef {
    DetailRef::Task {
        id: task.to_string(),
    }
}

pub(super) fn file_ref(task: &Id, path: &str) -> DetailRef {
    DetailRef::ReviewFile {
        task: task.to_string(),
        path: path.to_owned(),
    }
}

pub(super) fn artifact_ref(reference: &str) -> DetailRef {
    DetailRef::Artifact {
        reference: reference.to_owned(),
    }
}

/// Every task in Review on the board, in the project scope, as the list shows them: those
/// waiting for the owner's verdict first, each group in board order. j/k walk and the first
/// candidate shown follow this order.
fn candidates_filtered<'a>(feed: &'a Feed, filter: &ProjectFilter) -> Vec<&'a Task> {
    let filter = filter.resolved(feed);
    let mut candidates: Vec<&Task> = feed
        .board
        .tasks()
        .iter()
        .filter(|task| task.status() == Status::Review && filter.matches(task.project()))
        .collect();
    candidates.sort_by_key(|task| task.group != Group::NeedsYou);
    candidates
}

/// The candidate Review shows: the root's selection while the board still lists that task,
/// otherwise the first candidate.
pub fn selected_filtered<'a>(
    selection: &Selection,
    feed: &'a Feed,
    filter: &ProjectFilter,
) -> Option<&'a Task> {
    let filter = filter.resolved(feed);
    selection
        .review
        .as_ref()
        .and_then(|id| feed.board.task(id))
        .filter(|task| filter.matches(task.project()))
        .or_else(|| candidates_filtered(feed, &filter).into_iter().next())
}

/// The candidate's task row and whatever of its details has arrived.
pub(super) struct Shown<'a> {
    pub task: &'a Task,
    pub review_cache: Option<&'a DetailCache>,
    pub review: Option<&'a ReviewDetail>,
    pub detail: Option<&'a TaskDetail>,
}

impl<'a> Shown<'a> {
    pub fn of(feed: &'a Feed, task: &'a Task) -> Shown<'a> {
        let review_cache = feed.detail(&review_ref(&task.id));
        let review = review_cache.and_then(|cache| match cache.body() {
            Some(DetailBody::Review(review)) => Some(review),
            _ => None,
        });
        let detail = feed
            .detail(&task_ref(&task.id))
            .and_then(|cache| match cache.body() {
                Some(DetailBody::Task(detail)) => Some(detail),
                _ => None,
            });
        Shown {
            task,
            review_cache,
            review,
            detail,
        }
    }

    pub fn id(&self) -> &'a Id {
        &self.task.id
    }

    /// The candidate run recorded at `done`; none while corrald has no completed worker run.
    pub fn run(&self) -> Option<&'a Run> {
        self.review.and_then(|review| review.run.as_ref())
    }

    /// The candidate's commit, from the review detail, else the board row's run.
    pub fn head(&self) -> Option<&'a str> {
        self.run()
            .or(self.task.row.run.as_ref())
            .and_then(|run| run.head_sha.as_deref())
    }

    /// The branch the run started from, which approving lands onto unless told otherwise.
    pub fn base(&self) -> Option<&'a str> {
        self.run()
            .or(self.task.row.run.as_ref())
            .map(|run| run.base.as_str())
    }

    /// The candidate's own branch, which it cannot land onto.
    pub fn branch(&self) -> Option<&'a str> {
        self.run()
            .or(self.task.row.run.as_ref())
            .map(|run| run.branch.as_str())
    }

    /// The check of `kind` that decides now: the board row's gate or review. While Running,
    /// only the row can tell whether a gate still applies; elsewhere, fall back to the detail
    /// for the same key.
    pub fn check(&self, kind: CheckKind) -> Option<&'a Check> {
        let row = match kind {
            CheckKind::Gate => self.task.row.gate.as_ref(),
            CheckKind::Review => self.task.row.review.as_ref(),
        };
        if kind == CheckKind::Gate && self.task.status() == Status::Running {
            return row;
        }
        let round = self.task.task().round.to_string();
        let key = match kind {
            CheckKind::Gate => self.head(),
            CheckKind::Review => Some(round.as_str()),
        };
        row.or_else(|| {
            self.review?
                .checks
                .iter()
                .rev()
                .find(|check| check.kind == kind && Some(check.key.as_str()) == key)
        })
    }

    /// The gate check failed for the task's HEAD.
    pub fn gate_red(&self) -> bool {
        self.check(CheckKind::Gate)
            .is_some_and(|gate| gate.result == CheckResult::Fail)
    }

    /// The agent session of the candidate run: the agents topic's `<task>_<run>` key.
    pub fn session(&self) -> Option<String> {
        self.run().map(|run| format!("{}_{}", self.task.id, run.id))
    }
}

/// A SHA as Review prints it: seven characters.
pub(super) fn sha7(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// The shared local date/time formatter also used by work and plans.
pub(super) fn when(at: jiff::Timestamp, feed: &Feed) -> String {
    crate::model::work::when(&at.to_string(), feed)
}

/// Who recorded a check or an artifact.
pub(crate) const fn actor(actor: Actor) -> &'static str {
    match actor {
        Actor::Owner => "you",
        Actor::Agent => "the worker",
        Actor::Reviewer => "the reviewer",
        Actor::Assistant => "the assistant",
        Actor::Auto => "corrald",
        Actor::Extension => "an extension",
    }
}

pub(super) fn notice(shown: &Shown) -> Option<Notice> {
    let refresh = || Some(button("Refresh", Action::Refresh, ButtonStyle::Quiet));
    let cache = shown.review_cache;
    let error = cache.and_then(|cache| cache.error.as_ref());
    let Some(review) = shown.review else {
        return Some(match error {
            Some(error) => Notice {
                text: format!("Could not load this candidate: {}", error.message),
                hue: Hue::Fail,
                action: refresh(),
            },
            None => Notice {
                text: "Loading the candidate's review detail…".into(),
                hue: Hue::Neutral,
                action: None,
            },
        });
    };
    if review.run.is_none() {
        return Some(Notice {
            text: "corrald records no completed worker run for this task, so there is no stored diff to review yet.".into(),
            hue: Hue::Neutral,
            action: None,
        });
    }
    if let Some(error) = error {
        return Some(Notice {
            text: format!(
                "Showing the last copy; refreshing failed: {}",
                error.message
            ),
            hue: Hue::Held,
            action: refresh(),
        });
    }
    cache.filter(|cache| cache.stale).map(|_| Notice {
        text: "Last known detail · revalidating with corrald".into(),
        hue: Hue::Held,
        action: None,
    })
}

/// The worker run's summary, as it was recorded at `done`.
pub(super) fn report(shown: &Shown) -> Body {
    let Some(run) = shown.run() else {
        return Body::Notice("No worker run has finished for this task.".into());
    };
    let paragraphs: Vec<String> = run
        .summary
        .as_deref()
        .unwrap_or_default()
        .split("\n\n")
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .map(str::to_owned)
        .collect();
    if paragraphs.is_empty() {
        return Body::Notice("The worker recorded no summary for this run.".into());
    }
    let mut source = vec![span(format!("Source: run {} by ", run.id)).dim()];
    source.push(span(run.profile.clone()).mono().dim());
    if let Some(head) = &run.head_sha {
        source.push(span(" at ").dim());
        source.push(span(sha7(head)).mono().dim());
    }
    Body::Report(ReportView {
        heading: format!("{} run summary", shown.id()),
        paragraphs,
        source,
    })
}

/// The gate check's log, from the artifact it names.
pub(super) fn log(feed: &Feed, shown: &Shown) -> Body {
    let Some(gate) = shown.check(CheckKind::Gate) else {
        return Body::Notice("No gate has run on this task's HEAD yet.".into());
    };
    let Some(reference) = &gate.log else {
        return Body::Notice("The gate recorded no log for this result.".into());
    };
    let (text, truncated) = match artifact_text(feed, reference) {
        TextPage::Loading => return Body::Notice("Loading the gate log…".into()),
        TextPage::Failed(why) => return Body::Notice(why),
        TextPage::Text { text, truncated } => (text, truncated),
    };
    let lines: Vec<LogView> = text
        .lines()
        .map(|line| LogView {
            text: line.to_owned(),
            hue: crate::model::artifacts::log_hue(line),
        })
        .collect();
    let result = match gate.result {
        CheckResult::Pass => "passed",
        CheckResult::Fail => "failed",
    };
    let mut note = format!("Gate {result} on {} · log {reference}", sha7(&gate.key));
    if truncated {
        note.push_str(" · corrald serves the first part only");
    }
    Body::Log { lines, note }
}
