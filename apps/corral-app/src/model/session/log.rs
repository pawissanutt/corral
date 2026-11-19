//! The read-only session log of a session without a Pi host: a run whose harness is not Pi (its
//! pane's screen, read as corrald captures it) or a headless run that has ended (the tail of the
//! `session.log` corrald kept). It is read from the terminal snapshot detail and takes no input;
//! a log corrald no longer has is said with corrald's reason.

use corral_protocol::detail::{DetailBody, DetailRef, TerminalSnapshot};
use corral_protocol::rows::AgentRow;
use corral_types::{Run, SessionRef};

use crate::model::Id;
use crate::model::agents::{clock, placement};
use crate::model::daemon::{self, DetailCache, Link};
use crate::model::hue::Hue;
use crate::model::work::{loaded_task, session_task};

use super::Action;
use super::transcript::{Line, LineClass, line};

/// The log pager.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionLog {
    /// `ended`, or `<harness> · <pane>`.
    pub heading: String,
    /// `log tail · read 14:02`, `pane screen · read just now`, `log gone`.
    pub status: String,
    pub lines: Vec<Line>,
    /// What the empty pager says instead of lines.
    pub empty: Option<String>,
    /// corrald's reason the log cannot be read.
    pub note: Option<(Hue, String)>,
    /// Read the capture again.
    pub refresh: Action,
}

/// The run behind session `id` when it ended and was a process session, read from its task's
/// loaded record.
pub fn ended_run<'a>(feed: &'a daemon::Feed, id: &str) -> Option<&'a Run> {
    let task = session_task(id)?;
    let number = id.strip_prefix(task)?.strip_prefix('_')?;
    loaded_task(feed, task)?.runs.iter().find(|run| {
        run.id == number
            && run.ended.is_some()
            && matches!(run.session, Some(SessionRef::Process { .. }))
    })
}

/// Whether a live run's harness speaks no Pi socket, so its page is its log.
pub fn without_pi(row: &AgentRow) -> bool {
    !matches!(row.run.session, Some(SessionRef::Pi { .. }))
}

/// The daemon's reason without its machine prefix: `session has ended` from
/// `terminal_snapshot_unavailable: session has ended`.
pub fn reason(message: &str) -> &str {
    message
        .split_once(": ")
        .filter(|(head, _)| {
            !head.is_empty() && head.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        })
        .map_or(message, |(_, rest)| rest)
        .trim_end_matches('.')
}

/// A recorded line, classed by what it starts with or reports.
fn tail_line(text: &str) -> Line {
    let class = if text.contains("PASS") || text.contains("ok ") || text.contains("pass") {
        LineClass::Ok
    } else if ["$", "Edit", "Bash", "Read", "wake", "read", "Ask"]
        .iter()
        .any(|prefix| text.starts_with(prefix))
    {
        LineClass::Prompt
    } else {
        LineClass::Plain
    };
    line(text, class)
}

fn snapshot(cache: Option<&DetailCache>) -> Option<&TerminalSnapshot> {
    match cache?.body()? {
        DetailBody::Terminal(snapshot) => Some(snapshot),
        _ => None,
    }
}

/// The pane (`w1:p2`) or process (`pid 4120`) a run's session ran in, when recorded.
fn place(session: Option<&SessionRef>) -> Option<String> {
    session.map(|session| match placement(session) {
        SessionRef::Herdr { pane, .. } => pane.clone(),
        SessionRef::Process { pid, .. } => format!("pid {pid}"),
        SessionRef::Pi { .. } => String::new(),
    })
}

/// The log of session `id`, for a live run without a Pi host or an ended headless run; none for
/// any other session.
pub fn log(id: &Id, feed: &daemon::Feed) -> Option<SessionLog> {
    let live = feed.agents.get(id.as_str()).filter(|row| without_pi(row));
    let ended = live
        .is_none()
        .then(|| ended_run(feed, id.as_str()))
        .flatten();
    let run = live.map(|row| &row.run).or(ended)?;
    let target = DetailRef::Terminal { id: id.to_string() };
    let cache = feed.detail(&target);
    let capture = snapshot(cache);
    let error = cache.and_then(|cache| cache.error.as_ref());
    let gone = error.is_some_and(|error| error.code == "not_found");
    let loading = cache.is_none_or(|cache| cache.loading);
    let place = place(run.session.as_ref()).unwrap_or_else(|| "no session recorded".into());
    let heading = if ended.is_some() {
        "ended".to_owned()
    } else {
        format!("{} · {place}", run.harness)
    };
    let read = |capture: &TerminalSnapshot| {
        clock(Some(capture.captured_at.as_str()), feed)
            .unwrap_or_else(|| "at an unknown time".into())
    };
    let status = match capture {
        Some(capture) if capture.live && feed.link == Link::Up => {
            format!("pane screen · read {}", read(capture))
        }
        Some(capture) if capture.live => {
            format!(
                "last screen received · read {} · corrald is not answering",
                read(capture)
            )
        }
        Some(capture) => format!("log tail · read {}", read(capture)),
        None if gone => "log gone".into(),
        None if error.is_some() => "not loaded".into(),
        None if loading => "reading…".into(),
        None => "not read yet".into(),
    };
    let empty = match capture {
        Some(capture) if capture.lines.is_empty() => Some("The log is empty.".to_owned()),
        Some(_) => None,
        None if gone => Some("No log to show.".to_owned()),
        None => Some(match error {
            Some(error) => format!(
                "The log could not be read: {} ({}).",
                error.message, error.code
            ),
            None if loading => "Reading the log…".to_owned(),
            None => "No log has arrived; it shows once corrald answers.".to_owned(),
        }),
    };
    let note = error.filter(|_| gone).and_then(|error| {
        let reason = reason(&error.message);
        let mut letters = reason.chars();
        let first = letters.next()?;
        Some((
            Hue::Held,
            format!("{}{}.", first.to_uppercase(), letters.as_str()),
        ))
    });
    Some(SessionLog {
        heading,
        status,
        lines: capture
            .map(|capture| {
                capture
                    .lines
                    .iter()
                    .map(|text| {
                        if capture.live || ended.is_some() {
                            line(text, LineClass::Plain)
                        } else {
                            tail_line(text)
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        empty,
        note,
        refresh: Action::Refresh(target),
    })
}

#[cfg(test)]
mod tests {
    use corral_protocol::detail::{DetailBody, DetailRef, TaskDetail, TerminalSnapshot};
    use corral_types::{Role, SessionRef, Status};

    use super::log;
    use crate::model::Id;
    use crate::model::agents::roster_tests::agent;
    use crate::model::daemon::{self, ClientError, DetailCache, bare_task, detail_key};
    use crate::model::hue::Hue;
    use crate::model::work::task_ref;

    const SESSION: &str = "T-6_R1";

    /// A feed that holds the record of closed task T-6, whose only run was headless, and the
    /// answer corrald gave to that run's terminal.
    fn feed(error: Option<ClientError>, snapshot: Option<TerminalSnapshot>) -> daemon::Feed {
        let mut run = agent("T-6", "R1", Role::Worker).run;
        run.session = Some(SessionRef::Process {
            pid: 7,
            dir: "/runs/T-6/R1".into(),
        });
        run.ended = Some(jiff::Timestamp::UNIX_EPOCH);
        let record = TaskDetail {
            task: bare_task("T-6", "Landing page copy", Status::Done, run.started),
            runs: vec![run],
            checks: Vec::new(),
            artifacts: Vec::new(),
            events: Vec::new(),
        };
        let mut feed = daemon::Feed::default();
        let task = task_ref("T-6");
        let terminal = DetailRef::Terminal { id: SESSION.into() };
        for (target, error, body) in [
            (task, None, Some(DetailBody::Task(record))),
            (terminal, error, snapshot.map(DetailBody::Terminal)),
        ] {
            feed.details.insert(
                detail_key(&target),
                DetailCache {
                    target,
                    pages: Vec::new(),
                    loading: false,
                    stale: false,
                    error,
                    merged: body,
                },
            );
        }
        feed
    }

    /// An ended headless run has no roster row and no Pi transcript; its page shows the log's
    /// tail under an `ended` heading instead of an empty transcript.
    #[test]
    fn an_ended_headless_runs_page_shows_its_log_tail_under_an_ended_heading() {
        let tail = TerminalSnapshot {
            lines: vec!["[codex] cd /work/tree".into(), "[codex] exit 0".into()],
            captured_at: "2026-11-14T10:00:00Z".into(),
            live: false,
            rows: Vec::new(),
        };
        let shown = log(&Id::from(SESSION), &feed(None, Some(tail))).expect("its log");
        let lines: Vec<String> = shown
            .lines
            .iter()
            .map(|line| line.text.iter().map(|span| span.text.as_str()).collect())
            .collect();
        assert_eq!(lines, ["[codex] cd /work/tree", "[codex] exit 0"]);
        assert_eq!(shown.heading, "ended");
        assert!(shown.note.is_none());
    }

    /// Once the log is gone corrald answers not found with its reason; the pager says that
    /// reason as a note instead of drawing a blank log.
    #[test]
    fn a_missing_log_shows_the_reason_corrald_gave_as_a_note() {
        let gone = ClientError {
            code: "not_found".into(),
            message: "session log not found".into(),
        };
        let shown = log(&Id::from(SESSION), &feed(Some(gone), None)).expect("its log");
        assert!(shown.lines.is_empty());
        assert_eq!(
            shown.note,
            Some((Hue::Held, "Session log not found.".into()))
        );
    }
}
