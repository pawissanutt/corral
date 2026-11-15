//! The served candidate: the board's candidates, the selected task's `Review` and `Task`
//! details, and their evidence, as the shared Review view types. Nothing here is fixture data:
//! a field corrald does not serve is shown as unknown, never filled in.

use corral_protocol::cmd::Expect;
use corral_protocol::detail::{DetailBody, DetailRef, ReviewDetail, ReviewVerdict, TaskDetail};
use corral_protocol::rows::{InboxKind, InboxState};

use crate::model::board::Mark;
use crate::model::daemon::{DetailCache, Feed, Group, Link, Task};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::live::Selection;
use crate::model::nav::Form;
use crate::model::text::span;

use super::decide::decide_bar;
use super::evidence::artifacts;
use super::live_files;
use super::view::{
    AcceptanceView, Body, ButtonStyle, CandidateGroup, CandidateRow, DetailView, Header, LogView,
    Notice, PageView, ReportView, VerdictView, button, chip, effective_layout, layouts, tabs,
    verdict_view,
};
use super::{Action, State, Tab};

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
enum Stage {
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
fn stage(feed: &Feed, task: &Task) -> Option<Stage> {
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
pub fn candidates(feed: &Feed) -> Vec<&Task> {
    let tasks = feed.board.tasks();
    let of = move |wanted: Stage| {
        tasks
            .iter()
            .filter(move |task| stage(feed, task) == Some(wanted))
    };
    of(Stage::YourVerdict).chain(of(Stage::Verifying)).collect()
}

/// The candidate Review shows: the root's selection while the board still lists that task,
/// otherwise the first candidate.
pub fn selected<'a>(selection: &Selection, feed: &'a Feed) -> Option<&'a Task> {
    selection
        .review
        .as_ref()
        .and_then(|id| feed.board.task(id))
        .or_else(|| candidates(feed).into_iter().next())
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

fn stage_word(mark: Mark) -> &'static str {
    match mark {
        Mark::Done => "✓",
        Mark::Active => "running",
        Mark::Fail => "✗",
        Mark::Stale => "stale",
        Mark::Next => "pending",
        Mark::Na => "—",
    }
}

fn candidate_row(feed: &Feed, task: &Task, chosen: Option<&Id>) -> CandidateRow {
    let waiting_on_you = stage(feed, task) == Some(Stage::YourVerdict);
    let troubled = task.rail[..3]
        .iter()
        .any(|step| matches!(step.mark, Mark::Fail | Mark::Stale));
    let mut state = format!(
        "gate {} · review {}",
        stage_word(task.rail[1].mark),
        stage_word(task.rail[2].mark)
    );
    if waiting_on_you {
        state.push_str(" · you");
    }
    CandidateRow {
        task: task.id.to_string(),
        title: task.title.clone(),
        state,
        hue: if troubled {
            Hue::Held
        } else if waiting_on_you {
            Hue::You
        } else {
            Hue::Corral
        },
        identity: vec![
            span(match task.rail[0].sha7.as_deref() {
                Some(sha) => format!("{} · {sha}", task.id),
                None => task.id.to_string(),
            })
            .mono()
            .dim(),
        ],
        selected: chosen == Some(&task.id),
        action: Action::Pick(task.id.clone()),
    }
}

fn candidate_groups(feed: &Feed, chosen: Option<&Id>) -> Vec<CandidateGroup> {
    let all = candidates(feed);
    let group = |label, hue, wanted| {
        let rows: Vec<CandidateRow> = all
            .iter()
            .filter(|task| stage(feed, task) == Some(wanted))
            .map(|task| candidate_row(feed, task, chosen))
            .collect();
        (!rows.is_empty()).then(|| CandidateGroup {
            label,
            count: u32::try_from(rows.len()).unwrap_or(u32::MAX),
            hue,
            rows,
        })
    };
    [
        group("Your verdict", Hue::You, Stage::YourVerdict),
        group("Verifying", Hue::Corral, Stage::Verifying),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn verdict_mark(verdict: &str) -> Mark {
    match verdict.to_ascii_lowercase().as_str() {
        "pass" | "approve" | "approved" => Mark::Done,
        "fail" | "changes" | "request_changes" | "request-changes" | "escalate" | "rejected" => {
            Mark::Fail
        }
        "running" => Mark::Active,
        _ => Mark::Next,
    }
}

fn verdict_row(verdict: &ReviewVerdict, feed: &Feed, phone: bool) -> VerdictView {
    let text = match verdict.text.as_deref().filter(|text| !text.is_empty()) {
        Some(text) => format!("{} · {text}", verdict.verdict),
        None => verdict.verdict.clone(),
    };
    let time = verdict
        .at
        .as_deref()
        .map(|at| when(at, feed))
        .unwrap_or_default();
    verdict_view(
        phone,
        &verdict.by,
        short(&verdict.sha),
        &time,
        verdict_mark(&verdict.verdict),
        &text,
        verdict.stale,
    )
}

fn header(shown: &Shown, phone: bool) -> Header {
    let id = shown.id();
    let sha = shown.sha().map(short).unwrap_or("unknown");
    let mut ident = id.to_string();
    if let Some(run) = shown.run() {
        ident.push_str(" · ");
        ident.push_str(run);
    }
    let metadata = if phone {
        vec![span(format!("{ident} · {sha}")).mono().dim()]
    } else {
        let mut meta = vec![span(format!("{ident} · candidate {sha}")).mono()];
        if let Some(review) = shown.review {
            let (added, deleted) = review.files.iter().fold((0u64, 0u64), |(a, d), file| {
                (a + u64::from(file.added), d + u64::from(file.deleted))
            });
            let count = review.files.len();
            meta.push(span(format!(
                " base {} · {count} file{} · +{added} −{deleted}",
                review.base.as_deref().map(short).unwrap_or("unknown"),
                if count == 1 { "" } else { "s" },
            )));
        }
        meta
    };
    let mut chips = Vec::new();
    if let Some(review) = shown.review.filter(|review| !review.state.is_empty()) {
        chips.push(chip(
            review.state.replace('_', " "),
            if review.approvable {
                Hue::You
            } else {
                Hue::Neutral
            },
        ));
    }
    if shown.superseded() {
        chips.push(chip("newer candidate", Hue::Held));
    }
    Header {
        title: shown.task.title.clone(),
        metadata,
        chips,
    }
}

fn notice(shown: &Shown) -> Option<Notice> {
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

fn report(review: &ReviewDetail, id: &Id, sha: &str) -> Body {
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

fn log(review: &ReviewDetail, sha: &str) -> Body {
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

fn detail(state: &State, feed: &Feed, shown: &Shown, env: Env, width: f32) -> DetailView {
    let phone = env.form == Form::Phone;
    let specs: &[(Tab, &str)] = if phone {
        &[
            (Tab::Diff, "Diff"),
            (Tab::Report, "Report"),
            (Tab::Shot, "Artifacts"),
            (Tab::Log, "Log"),
        ]
    } else {
        &[
            (Tab::Diff, "Diff"),
            (Tab::Report, "Report"),
            (Tab::Shot, "Artifacts"),
            (Tab::Log, "Gate log"),
        ]
    };
    let sha = shown.sha().unwrap_or_default();
    let body = match shown.review {
        None => Body::Notice(match notice(shown) {
            Some(notice) => notice.text,
            None => "Loading the candidate's review detail…".into(),
        }),
        Some(review) => match state.tab {
            Tab::Diff => Body::Diff(live_files::diff(
                state,
                feed,
                shown,
                review,
                effective_layout(state, env, width),
                env,
            )),
            Tab::Report => report(review, shown.id(), sha),
            Tab::Shot => artifacts(state, feed, shown.id(), review, env),
            Tab::Log => log(review, sha),
        },
    };
    let mut verdicts: Vec<VerdictView> = shown
        .review
        .map(|review| {
            review
                .verdicts
                .iter()
                .map(|verdict| verdict_row(verdict, feed, phone))
                .collect()
        })
        .unwrap_or_default();
    if let Some(review) = shown.review.filter(|review| review.approvable) {
        let sha = review
            .candidate_sha
            .as_deref()
            .map(short)
            .unwrap_or("unknown");
        verdicts.push(verdict_view(
            phone,
            "you",
            sha,
            "",
            Mark::Next,
            "your verdict",
            false,
        ));
    }
    DetailView {
        header: header(shown, phone),
        notice: if shown.review.is_some() {
            notice(shown)
        } else {
            None
        },
        verdict_heading: if phone {
            "Verdicts"
        } else {
            "Verdicts · bound to candidate SHA"
        },
        verdict_binding: phone.then_some("bound to SHA"),
        verdicts_empty: (shown.review.is_some() && verdicts.is_empty())
            .then_some("No verdicts are recorded for this candidate yet."),
        verdicts,
        tabs: tabs(state, specs),
        layouts: layouts(state, env),
        body,
    }
}

/// The served Review at `width`. `focus` is the candidate a phone page was pushed for; desktop
/// and terminal show the root's selection, else the first candidate.
pub fn live_page(
    state: &State,
    feed: &Feed,
    selection: &Selection,
    focus: Option<&Id>,
    env: Env,
    width: f32,
) -> PageView {
    let phone = env.form == Form::Phone;
    let task = match focus {
        Some(id) => feed.board.task(id),
        None => selected(selection, feed),
    };
    let shown = task.map(|task| Shown::of(feed, task));
    let chosen = shown.as_ref().map(Shown::id);
    let candidates = candidate_groups(feed, chosen);
    let empty = match (&shown, focus) {
        (Some(_), _) => None,
        (None, Some(id)) => Some(format!(
            "{id} is no longer on the board, so its candidate cannot be shown."
        )),
        (None, None) if !feed.board.complete => Some("Waiting for the board…".into()),
        (None, None) => Some(
            "No candidates are waiting. A task appears here once its run records a candidate."
                .into(),
        ),
    };
    let stale = env.conn.refuses()
        || feed.link != Link::Up
        || shown
            .as_ref()
            .and_then(|shown| shown.review_cache)
            .is_some_and(|cache| cache.stale);
    PageView {
        title: shown
            .as_ref()
            .map_or_else(|| "Review".into(), |shown| shown.id().to_string()),
        back_label: phone.then_some("Back"),
        phone,
        stale,
        conn: env.conn,
        candidates,
        candidate_note: None,
        detail: shown
            .as_ref()
            .map(|shown| detail(state, feed, shown, env, width)),
        empty,
        decide: decide_bar(state, feed, shown.as_ref(), env),
    }
}

/// The served candidate list alone, for the phone's Review destination.
pub fn candidate_list(feed: &Feed, selection: &Selection) -> (Vec<CandidateGroup>, Option<String>) {
    let groups = candidate_groups(feed, selection.review.as_ref());
    let empty = groups.is_empty().then(|| {
        if feed.board.complete {
            "No candidates are waiting. A task appears here once its run records a candidate."
                .to_owned()
        } else {
            "Waiting for the board…".to_owned()
        }
    });
    (groups, empty)
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
        assert!(candidates(&feed).is_empty());
        for (task, state) in [("T-2", InboxState::Open), ("T-3", InboxState::Cancelled)] {
            let row = review_item(task, state);
            feed.inbox.insert(Id::new(&row.id), row);
        }
        let listed: Vec<&str> = candidates(&feed)
            .iter()
            .map(|task| task.id.as_str())
            .collect();
        assert_eq!(listed, ["T-2"]);
        let groups = candidate_groups(&feed, None);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].label, "Your verdict");
        assert_eq!(groups[0].rows[0].task, "T-2");
    }
}
