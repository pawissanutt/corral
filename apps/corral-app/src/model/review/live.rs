//! The served candidate: the board's candidates, the selected task's `Review` and `Task`
//! details, and their evidence, as the shared Review view types. Nothing here is fixture data:
//! a field corrald does not serve is shown as unknown, never filled in.

use corral_protocol::cmd::Expect;
use corral_protocol::detail::{DetailBody, DetailRef, ReviewDetail, TaskDetail};
use corral_protocol::rows::{InboxKind, InboxState};

use crate::model::board::Mark;
use crate::model::daemon::{DetailCache, Feed, Group, Task};
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::live::Selection;
use crate::model::projects::ProjectFilter;
use crate::model::text::span;

use super::Action;
use super::view::{AcceptanceView, Body, ButtonStyle, LogView, Notice, ReportView, button};

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

/// Where a candidate waits: on the owner, or on automated verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    YourVerdict,
    Verifying,
}

/// A task waits on the owner's verdict while corrald's inbox holds an open review item for it:
/// corrald raises one exactly while the task is in Review, whatever else holds its dispatch.
fn awaits_verdict(feed: &Feed, task: &Task) -> bool {
    feed.inbox.values().any(|row| {
        row.kind == InboxKind::Review
            && row.state == InboxState::Open
            && row.task.as_deref() == Some(task.id.as_str())
    })
}

/// A board row is a candidate while it waits on the owner's verdict (corrald's open review
/// item, or the board's needs-you group with a recorded candidate SHA and a finished run), or
/// on gate, review or landing with a recorded candidate SHA.
pub(super) fn stage(feed: &Feed, task: &Task) -> Option<Stage> {
    if awaits_verdict(feed, task) {
        return Some(Stage::YourVerdict);
    }
    task.rail[0].sha7.as_ref()?;
    match task.group {
        Group::NeedsYou if task.rail[0].mark == Mark::Done => Some(Stage::YourVerdict),
        Group::Verifying => Some(Stage::Verifying),
        _ => None,
    }
}

/// Every candidate on the board: the owner's first, then those still verifying.
pub fn candidates_filtered<'a>(feed: &'a Feed, filter: &ProjectFilter) -> Vec<&'a Task> {
    let filter = filter.resolved(feed);
    let filter = &filter;
    let tasks = feed.board.tasks();
    let of = move |wanted: Stage| {
        tasks
            .iter()
            .filter(move |task| stage(feed, task) == Some(wanted))
            .filter(move |task| filter.matches(task.project.as_deref()))
    };
    of(Stage::YourVerdict).chain(of(Stage::Verifying)).collect()
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
        .filter(|task| filter.matches(task.project.as_deref()))
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

    /// The board's newest recorded candidate, seven characters.
    pub fn board_sha(&self) -> Option<&'a str> {
        self.task.rail[0].sha7.as_deref()
    }

    /// The candidate the detail describes, else the board's.
    pub fn sha(&self) -> Option<&'a str> {
        self.review
            .and_then(|review| review.candidate_sha.as_deref())
            .or(self.board_sha())
    }

    /// The detail describes an older candidate than the board now records.
    pub fn superseded(&self) -> bool {
        match (
            self.review.and_then(|r| r.candidate_sha.as_deref()),
            self.board_sha(),
        ) {
            (Some(detail), Some(board)) => !detail.starts_with(board) && !board.starts_with(detail),
            _ => false,
        }
    }

    /// The task's owner-revision precondition, once the task detail has arrived.
    pub fn expect(&self) -> Option<Expect> {
        self.detail.map(|detail| Expect::TaskRevision {
            task: self.task.id.to_string(),
            revision: detail.owner_revision.clone(),
        })
    }

    /// The candidate's run, as the task detail names it (`R1`).
    pub fn run(&self) -> Option<&'a str> {
        self.detail
            .and_then(|detail| detail.run.as_ref())
            .map(|run| run.id.as_str())
    }

    pub fn identity(&self) -> Option<super::CandidateAt> {
        Some(super::CandidateAt {
            task: self.id().clone(),
            candidate: corral_protocol::detail::ReviewCandidate {
                run: self.run()?.to_owned(),
                base: self.review?.base.clone()?,
                head: self.review?.candidate_sha.clone()?,
            },
        })
    }

    pub fn matches(&self, at: &super::CandidateAt) -> bool {
        at.task == *self.id() && self.matches_candidate(&at.candidate)
    }

    pub fn matches_candidate(&self, candidate: &corral_protocol::detail::ReviewCandidate) -> bool {
        self.run() == Some(candidate.run.as_str())
            && self.review.and_then(|review| review.base.as_deref())
                == Some(candidate.base.as_str())
            && self
                .review
                .and_then(|review| review.candidate_sha.as_deref())
                == Some(candidate.head.as_str())
    }
}

/// A SHA as Review prints it: eight characters.
pub(super) fn short(sha: &str) -> &str {
    sha.get(..8).unwrap_or(sha)
}

/// The shared local date/time formatter also used by work and plans.
pub(super) fn when(at: &str, feed: &Feed) -> String {
    crate::model::work::when(at, feed)
}

pub(super) fn notice(shown: &Shown) -> Option<Notice> {
    let refresh = || Some(button("Refresh", Action::Refresh, ButtonStyle::Quiet));
    let cache = shown.review_cache;
    let error = cache.and_then(|cache| cache.error.as_ref());
    if shown.review.is_none() {
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
    }
    if shown.superseded() {
        return Some(Notice {
            text: format!(
                "This detail describes {}; the board now records {}. Decisions wait for the refreshed detail.",
                shown
                    .review
                    .and_then(|r| r.candidate_sha.as_deref())
                    .map(short)
                    .unwrap_or("an older candidate"),
                shown.board_sha().unwrap_or("a newer candidate"),
            ),
            hue: Hue::Held,
            action: refresh(),
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

pub(super) fn report(review: &ReviewDetail, id: &Id, sha: &str) -> Body {
    let paragraphs: Vec<String> = review
        .report
        .as_deref()
        .unwrap_or_default()
        .split("\n\n")
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .map(str::to_owned)
        .collect();
    if paragraphs.is_empty() && review.acceptance.is_empty() {
        return Body::Notice("No implementation report is recorded for this candidate.".into());
    }
    let acceptance = review
        .acceptance
        .iter()
        .map(|item| {
            let item = item.trim();
            let (mark, text) =
                if let Some(rest) = item.strip_prefix("[x] ").or(item.strip_prefix("[X] ")) {
                    (Some(Mark::Done), rest)
                } else if let Some(rest) = item.strip_prefix("[ ] ") {
                    (Some(Mark::Next), rest)
                } else {
                    (None, item)
                };
            AcceptanceView {
                mark,
                text: text.to_owned(),
            }
        })
        .collect();
    Body::Report(ReportView {
        heading: format!("{id} implementation report"),
        paragraphs,
        acceptance_heading: "Acceptance",
        acceptance,
        source: vec![
            span("Source: corrald's review record for candidate ").dim(),
            span(short(sha).to_owned()).mono().dim(),
        ],
    })
}

pub(super) fn log(review: &ReviewDetail, sha: &str) -> Body {
    if review.gate_log_tail.is_empty() {
        return Body::Notice("No gate log is recorded for this candidate.".into());
    }
    let hue = |line: &str| {
        let lower = line.to_ascii_lowercase();
        if lower.contains("fail") || lower.contains("error") {
            Hue::Fail
        } else if lower.contains("pass") {
            Hue::Ok
        } else {
            Hue::Neutral
        }
    };
    Body::Log {
        lines: review
            .gate_log_tail
            .iter()
            .map(|line| LogView {
                text: line.clone(),
                hue: hue(line),
            })
            .collect(),
        note: format!(
            "Last {} lines of the gate log for {}",
            review.gate_log_tail.len(),
            short(sha)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::daemon::Step;
    use corral_protocol::rows::InboxRow;

    fn held_candidate(id: &str) -> Task {
        Task {
            id: Id::new(id),
            title: format!("task {id}"),
            project: None,
            plan: None,
            goal: None,
            group: Group::Held,
            rail: std::array::from_fn(|index| Step {
                mark: if index == 0 { Mark::Done } else { Mark::Next },
                sha7: (index == 0).then(|| "4a72ec4".into()),
                at: None,
            }),
            now: "blocked".into(),
            now_code: "blocked".into(),
            since: None,
            age_since: None,
            held_reason: None,
            questions: 0,
            attention: Vec::new(),
            agent: None,
            cost: None,
            cost_estimated: false,
            rev: 1,
        }
    }

    fn review_item(task: &str, state: InboxState) -> InboxRow {
        InboxRow {
            id: format!("{task}/review"),
            kind: InboxKind::Review,
            task: Some(task.into()),
            plan: None,
            project: None,
            title: format!("task {task}"),
            from: String::new(),
            asked_at: None,
            needed_by: None,
            blocking: true,
            state,
            question_rev: None,
            answerable: false,
            unanswerable_reason: Some("open_review".into()),
            attention: None,
            rev: 1,
        }
    }

    #[test]
    fn an_open_review_item_lists_the_candidate_whatever_holds_its_dispatch() {
        let mut feed = Feed::default();
        feed.board_received(
            1,
            vec![held_candidate("T-2"), held_candidate("T-3")],
            &[],
            true,
        );
        assert!(candidates_filtered(&feed, &ProjectFilter::All).is_empty());
        for (task, state) in [("T-2", InboxState::Open), ("T-3", InboxState::Cancelled)] {
            let row = review_item(task, state);
            feed.inbox.insert(Id::new(&row.id), row);
        }
        let listed: Vec<&str> = candidates_filtered(&feed, &ProjectFilter::All)
            .iter()
            .map(|task| task.id.as_str())
            .collect();
        assert_eq!(listed, ["T-2"]);
        let verdict = feed
            .board
            .task(&Id::new("T-2"))
            .and_then(|task| stage(&feed, task));
        assert_eq!(verdict, Some(Stage::YourVerdict));
    }
}
