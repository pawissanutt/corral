//! The session's background jobs on the Agents tab, read from the host's `SessionJobs` part:
//! their group (`Jobs · 1 running`, each job with its last output line and a peek at its last
//! lines in place), the selected job's detail (command, directory, state and elapsed time,
//! output size, exit code, armed watches, the last lines of output) and its controls, and the
//! opened job's output (`SessionJobOutput`): the newest bytes first, re-read as the host
//! announces a change while it runs, paged toward older bytes on request, searched and followed
//! as `find` reads it. A running job offers Stop (asked once) and Background; a finished one
//! offers neither.

use corral_protocol::detail::{
    DetailBody, DetailGet, DetailRef, JobOutputPage, SessionJob, SessionJobs,
};

use crate::model::Id;
use crate::model::agents::unix;
use crate::model::board::Mark;
use crate::model::daemon::DetailCache;
use crate::model::hue::Hue;

use super::super::read_state::ReadState;
use super::super::tabs::load;
use super::super::transcript::{Line, LineClass, line, size};
use super::super::{Action, Can, Cx, SubView};
use super::{FleetAction, FleetButton, control, ops};

#[path = "jobs_find.rs"]
mod find;

pub use find::{FollowLine, JobSearch, Peek};

/// The host capability that lists the jobs and serves their output.
const JOBS: &str = "jobs";
/// The host capability that stops jobs and sends them to the background.
const JOB_CONTROL: &str = "job_control";

/// The jobs' group on the tab.
#[derive(Debug, Clone, PartialEq)]
pub struct JobList {
    /// `Jobs · 1 running`.
    pub heading: String,
    pub rows: Vec<JobRowView>,
    /// Why the list is missing or partial: unavailable, reading, failed.
    pub note: Option<(Hue, String)>,
    pub retry: Option<Action>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobRowView {
    /// `job-<id>`.
    pub test_id: String,
    pub job: String,
    pub mark: Mark,
    pub hue: Hue,
    /// The command's first line, clipped: the job's name.
    pub command: String,
    /// `running`, `exited 1`; how long it ran is the clock's (`AgentsClock::job`).
    pub state: String,
    /// Its last output line, as the list carries it.
    pub last: Option<String>,
    /// Its last lines, while the owner peeks at them.
    pub peek: Option<Peek>,
    /// Opens or closes the peek.
    pub peek_action: Action,
    pub selected: bool,
    /// Selects the job; on the selected row, opens its output.
    pub action: Action,
}

/// The selected job's facts and controls.
#[derive(Debug, Clone, PartialEq)]
pub struct JobDetail {
    pub job: String,
    /// The id's first eight characters, beside the name.
    pub short: String,
    /// `$ cargo test -p auth`: the command's first line, clipped.
    pub command: String,
    pub dir: Option<String>,
    /// `running`, `exited 1`, in its tone; how long it ran is the clock's.
    pub state: (Hue, String),
    /// Output size and watches.
    pub chips: Vec<(Hue, String)>,
    /// The last lines of its output.
    pub tail: Vec<Line>,
    pub buttons: Vec<FleetButton>,
}

/// An opened job's output, in place of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct JobOutputView {
    pub job: String,
    /// The id's first eight characters, beside the name.
    pub short: String,
    pub back: Action,
    /// `$ cargo test -p auth`, or `a job no longer listed` when the list no longer has it.
    pub command: String,
    /// `▼ following the newest output`, `‖ showing older output`, `■ exited 0`.
    pub status: (Hue, String),
    /// `last 40.0 KB of 1.2 MB`.
    pub span: Option<String>,
    pub lines: Vec<Line>,
    /// The start byte of each line in the job's output.
    pub at: Vec<u64>,
    /// The byte after the last one shown.
    pub end: u64,
    /// A read of the output is on its way.
    pub loading: bool,
    /// Where the read stands while it is not simply read.
    pub read: Option<(Hue, String)>,
    pub retry: Option<Action>,
    /// What an output without lines says.
    pub empty: Option<String>,
    /// The bytes before the shown ones.
    pub older: Option<Action>,
    /// Back to the newest bytes, while older ones show.
    pub newest: Option<Action>,
    /// The output grows while it shows: the view keeps to its end.
    pub live: bool,
    pub buttons: Vec<FleetButton>,
}

impl JobOutputView {
    /// The search of `query` in the loaded output, standing on match `at` (clamped to the last,
    /// so `usize::MAX` stands on the newest); none for an empty query.
    pub fn search(&self, query: &str, at: usize) -> Option<JobSearch> {
        find::search(self, query, at)
    }

    /// Following the newest lines, or paused since byte `paused_since`; none once the output
    /// does not grow here.
    pub fn follow(&self, paused_since: Option<u64>) -> Option<FollowLine> {
        find::follow_line(self, paused_since)
    }
}

/// The peek's key among the tab's opened texts.
fn peek_key(job: &str) -> String {
    format!("peek:{job}")
}

/// The read output of `job` when it is the newest the host has.
fn newest_text<'f>(cx: &Cx<'f>, job: &str) -> Option<&'f str> {
    page(cx.feed.detail(&output_target(cx.id, job)))
        .filter(|page| page.offset.saturating_add(page.text.len() as u64) >= page.bytes)
        .map(|page| page.text.as_str())
}

fn target(session: &Id) -> DetailRef {
    DetailRef::SessionJobs {
        id: session.to_string(),
    }
}

pub(super) fn output_target(session: &Id, job: &str) -> DetailRef {
    DetailRef::SessionJobOutput {
        id: session.to_string(),
        job: job.to_owned(),
    }
}

fn served<'f>(cache: Option<&'f DetailCache>) -> Option<&'f SessionJobs> {
    match cache?.body()? {
        DetailBody::SessionJobs(jobs) => Some(jobs),
        _ => None,
    }
}

/// The session's jobs as last read.
pub(super) fn all<'f>(cx: &Cx<'f>) -> &'f [SessionJob] {
    served(cx.feed.detail(&target(cx.id))).map_or(&[][..], |jobs| jobs.jobs.as_slice())
}

/// How long a job's name may run before it is clipped.
const NAME_CHARS: usize = 80;

/// The first eight characters of a job's id: where an id helps, a job is named by it.
pub(in crate::model::session) fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// A job's name: its command's first line, clipped, with `…` where anything was cut.
pub(super) fn name(command: &str) -> String {
    let first = command.lines().next().unwrap_or_default().trim_end();
    let mut name: String = first.chars().take(NAME_CHARS).collect();
    if name.len() < command.trim_end().len() {
        name.push('…');
    }
    name
}

/// The name of job `job` of `session` as last listed, else its short id.
pub(super) fn named(feed: &crate::model::daemon::Feed, session: &Id, job: &str) -> String {
    served(feed.detail(&target(session)))
        .and_then(|jobs| jobs.jobs.iter().find(|found| found.id == job))
        .map_or_else(
            || format!("job {}", short_id(job)),
            |found| name(&found.command),
        )
}

fn running(job: &SessionJob) -> bool {
    job.state == "running"
}

/// A job's state as a mark, its hue and its words.
fn job_state(job: &SessionJob) -> (Mark, Hue, String) {
    match (job.state.as_str(), job.exit_code) {
        ("running", _) => (Mark::Active, Hue::Worker, "running".into()),
        (_, Some(0)) => (Mark::Done, Hue::Ok, "exited 0".into()),
        (_, Some(code)) => (Mark::Fail, Hue::Fail, format!("exited {code}")),
        ("lost", _) => (Mark::Na, Hue::Held, "lost · its outcome is unknown".into()),
        ("killed" | "stopped" | "aborted", _) => (Mark::Na, Hue::Neutral, "stopped".into()),
        ("released" | "background", _) => (Mark::Na, Hue::Neutral, "in the background".into()),
        (other, _) => (Mark::Na, Hue::Neutral, other.replace('_', " ")),
    }
}

/// How long it ran, or runs; none without a start.
fn elapsed(job: &SessionJob, now: u64) -> Option<String> {
    let start = unix(job.started_at.as_deref()?)?;
    let end = match job.ended_at.as_deref() {
        Some(end) => unix(end)?,
        None if now > 0 => now,
        None => return None,
    };
    Some(super::super::activity::duration(end.saturating_sub(start)))
}

/// How long each listed job ran, or runs, by job id; a job without a start has none.
pub(super) fn elapsed_by_job(cx: &Cx<'_>) -> std::collections::BTreeMap<String, String> {
    all(cx)
        .iter()
        .filter_map(|job| Some((job.id.clone(), elapsed(job, cx.feed.now)?)))
        .collect()
}

/// The job's controls: open its output (unless it shows), and while it runs Stop (asked once)
/// and Background. `blocked` is why the host cannot control jobs now; `sending` whether a
/// request for this job is on its way.
pub(super) fn buttons(
    session: &Id,
    job: &SessionJob,
    blocked: Option<String>,
    sending: bool,
    opened: bool,
) -> Vec<FleetButton> {
    let mut out = Vec::new();
    if !opened {
        out.push(FleetButton::new(
            "job-open",
            "Open output",
            Some("o"),
            Action::Sub {
                session: session.clone(),
                sub: Some(SubView::Job(job.id.clone())),
            },
        ));
    }
    if !running(job) {
        return out;
    }
    let sending = sending.then(|| "your last request is on its way".to_owned());
    let refused = |action: &str| {
        (!job.actions.iter().any(|offered| offered == action))
            .then(|| "the host does not offer this for the job".to_owned())
    };
    out.push(
        FleetButton::new(
            "job-stop",
            "Stop",
            Some("x"),
            Action::Fleet(FleetAction::Stop {
                session: session.clone(),
                job: job.id.clone(),
            }),
        )
        .danger()
        .unless(blocked.clone())
        .unless(sending.clone())
        .unless(refused("stop")),
    );
    out.push(
        FleetButton::new(
            "job-background",
            "Background",
            Some("b"),
            Action::Fleet(FleetAction::Background {
                session: session.clone(),
                job: job.id.clone(),
            }),
        )
        .unless(blocked)
        .unless(sending)
        .unless(refused("background")),
    );
    out
}

fn tail(job: &SessionJob) -> Vec<Line> {
    job.tail
        .iter()
        .map(|text| line(text, LineClass::Plain))
        .collect()
}

/// The jobs' group; none when the session serves no jobs, or has none and nothing to say.
pub(super) fn list(cx: &Cx<'_>, selected: Option<&str>) -> Option<JobList> {
    let can = cx.can(JOBS);
    if can == Can::Absent {
        return None;
    }
    let target = target(cx.id);
    let cache = cx.feed.detail(&target);
    let read = ReadState::of(cache, cx.feed);
    let served = served(cache);
    let jobs = served.map_or(&[][..], |jobs| jobs.jobs.as_slice());
    let counted = cx.state.map_or(0, |state| state.jobs.total);
    let unavailable = served
        .and_then(|jobs| jobs.unavailable.clone())
        .or(match can {
            Can::Unavailable(why) => Some(why),
            _ => None,
        });
    let failed = matches!(read, ReadState::Failed(_));
    if jobs.is_empty() && counted == 0 && unavailable.is_none() && !failed {
        return None;
    }
    let note = if let Some(why) = unavailable {
        Some((Hue::Held, format!("unavailable: {why}")))
    } else if failed || matches!(read, ReadState::Stale { .. }) {
        Some((read.hue(), read.words()))
    } else if served.is_none() {
        Some((Hue::Neutral, "Reading the jobs…".to_owned()))
    } else {
        None
    };
    let live = if served.is_some() {
        u32::try_from(jobs.iter().filter(|job| running(job)).count()).unwrap_or(u32::MAX)
    } else {
        cx.state.map_or(0, |state| state.jobs.live)
    };
    Some(JobList {
        heading: match live {
            0 => "Jobs · none running".into(),
            n => format!("Jobs · {n} running"),
        },
        rows: jobs
            .iter()
            .map(|job| {
                let (mark, hue, state) = job_state(job);
                let chosen = selected == Some(job.id.as_str());
                JobRowView {
                    test_id: format!("job-{}", job.id),
                    job: job.id.clone(),
                    mark,
                    hue,
                    command: name(&job.command),
                    state,
                    last: job
                        .tail
                        .iter()
                        .rev()
                        .find(|text| !text.trim().is_empty())
                        .cloned(),
                    peek: cx
                        .ui
                        .fleet
                        .opened
                        .contains(&peek_key(&job.id))
                        .then(|| find::peek(job, newest_text(cx, &job.id))),
                    peek_action: Action::Fleet(FleetAction::More {
                        session: cx.id.clone(),
                        key: peek_key(&job.id),
                    }),
                    selected: chosen,
                    action: if chosen {
                        Action::Sub {
                            session: cx.id.clone(),
                            sub: Some(SubView::Job(job.id.clone())),
                        }
                    } else {
                        Action::Fleet(FleetAction::SelectJob {
                            session: cx.id.clone(),
                            job: job.id.clone(),
                        })
                    },
                }
            })
            .collect(),
        note,
        retry: failed.then_some(Action::Refresh(target)),
    })
}

/// The selected job's detail.
pub(super) fn detail(cx: &Cx<'_>, id: &str) -> Option<JobDetail> {
    let job = all(cx).iter().find(|job| job.id == id)?;
    let (_, hue, words) = job_state(job);
    let mut chips = vec![(
        Hue::Neutral,
        format!("{} of output", size(job.output_bytes)),
    )];
    chips.extend(
        job.monitors
            .iter()
            .map(|watch| (Hue::Neutral, format!("⌁ watch {watch} armed"))),
    );
    Some(JobDetail {
        job: job.id.clone(),
        short: short_id(&job.id),
        command: format!("$ {}", name(&job.command)),
        dir: job.dir.clone(),
        state: (hue, words),
        chips,
        tail: tail(job),
        buttons: buttons(
            cx.id,
            job,
            control(cx, JOB_CONTROL),
            ops::sending(cx, &job.id),
            false,
        ),
    })
}

fn page(cache: Option<&DetailCache>) -> Option<&JobOutputPage> {
    match cache?.body()? {
        DetailBody::SessionJobOutput(page) => Some(page),
        _ => None,
    }
}

/// The opened job's output.
pub(super) fn output(cx: &Cx<'_>, id: &str) -> JobOutputView {
    let session = cx.id;
    let job = all(cx).iter().find(|job| job.id == id);
    let target = output_target(session, id);
    let cache = cx.feed.detail(&target);
    let read = ReadState::of(cache, cx.feed);
    let shown = page(cache);
    let loading = cache.is_none_or(|cache| cache.loading);
    let end = shown.map(|page| page.offset.saturating_add(page.text.len() as u64));
    let older_shown = shown.zip(end).is_some_and(|(page, end)| end < page.bytes);
    let live = shown.is_some_and(|page| page.live) && !older_shown;
    let status = match (shown, job) {
        (_, Some(job)) if !running(job) => {
            let (_, hue, words) = job_state(job);
            (hue, format!("■ {words}"))
        }
        (Some(page), _) if !page.live => (Hue::Neutral, "■ ended".to_owned()),
        _ if older_shown => (
            Hue::Neutral,
            "‖ showing older output · Newest returns to the live tail".to_owned(),
        ),
        (Some(_), _) => (Hue::Worker, "▼ following the newest output".to_owned()),
        (None, _) => (Hue::Neutral, "not read yet".to_owned()),
    };
    let span = shown.zip(end).and_then(|(page, end)| {
        (page.bytes > page.text.len() as u64).then(|| {
            if end >= page.bytes {
                format!(
                    "last {} of {}",
                    size(page.text.len() as u64),
                    size(page.bytes)
                )
            } else {
                format!(
                    "{} from byte {} of {}",
                    size(page.text.len() as u64),
                    page.offset,
                    size(page.bytes)
                )
            }
        })
    });
    let next = cache
        .and_then(|cache| cache.pages.first())
        .and_then(|first| first.next.clone());
    JobOutputView {
        job: id.to_owned(),
        short: short_id(id),
        back: Action::Sub {
            session: session.clone(),
            sub: None,
        },
        command: job.map_or_else(
            || "a job no longer listed".to_owned(),
            |job| format!("$ {}", name(&job.command)),
        ),
        status,
        span,
        lines: shown.map_or_else(Vec::new, |page| {
            page.text
                .lines()
                .map(|text| line(text, LineClass::Plain))
                .collect()
        }),
        at: shown.map_or_else(Vec::new, |page| find::starts(&page.text, page.offset)),
        end: end.unwrap_or_default(),
        loading,
        read: match &read {
            ReadState::Failed(_) | ReadState::Stale { .. } => Some((read.hue(), read.words())),
            ReadState::NotRead | ReadState::Reading => Some(match control(cx, JOBS) {
                Some(why) if shown.is_none() => {
                    (Hue::Held, format!("The output is not read: {why}"))
                }
                _ => (Hue::Neutral, "Reading the output…".to_owned()),
            }),
            _ => None,
        },
        retry: matches!(read, ReadState::Failed(_)).then(|| Action::Refresh(target.clone())),
        empty: shown
            .filter(|page| page.text.is_empty())
            .map(|_| "No output yet.".to_owned()),
        older: next.filter(|_| !loading).map(|token| {
            Action::Fleet(FleetAction::Older {
                session: session.clone(),
                job: id.to_owned(),
                token,
            })
        }),
        newest: older_shown.then(|| {
            Action::Fleet(FleetAction::Newest {
                session: session.clone(),
                job: id.to_owned(),
            })
        }),
        live,
        buttons: job.map_or_else(Vec::new, |job| {
            buttons(
                session,
                job,
                control(cx, JOB_CONTROL),
                ops::sending(cx, &job.id),
                true,
            )
        }),
    }
}

/// The request that pages job output: older bytes under `token`, or the newest with none.
pub(super) fn page_request(session: &Id, job: &str, token: Option<String>) -> DetailGet {
    DetailGet {
        target: output_target(session, job),
        etag: None,
        page: token,
    }
}

/// What the jobs read while the tab shows: the list, and an opened job's output.
pub(super) fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    if cx.can(JOBS) == Can::Absent {
        return Vec::new();
    }
    let mut wanted: Vec<Action> = load(cx, Ok(target(cx.id)), Some(JOBS))
        .into_iter()
        .collect();
    if let Some(SubView::Job(job)) = &cx.ui.sub {
        wanted.extend(load(cx, Ok(output_target(cx.id, job)), Some(JOBS)));
    }
    // A peeked job's newest lines come from its output, read once and then as it changes.
    for job in all(cx)
        .iter()
        .filter(|job| cx.ui.fleet.opened.contains(&peek_key(&job.id)))
    {
        wanted.extend(load(cx, Ok(output_target(cx.id, &job.id)), Some(JOBS)));
    }
    wanted
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
