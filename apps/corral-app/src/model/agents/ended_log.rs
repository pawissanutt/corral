//! The page of a headless run that has ended. The run is no longer in the live roster, so it is
//! read from its task's record; corrald still answers its terminal with the tail of the
//! `session.log` it retained, marked as not live, or says `not_found` with a reason when the file
//! is gone.

use corral_protocol::detail::{DetailBody, DetailRef};
use corral_types::{Run, SessionRef};

use crate::model::Id;
use crate::model::daemon::{self, DetailCache};
use crate::model::hue::Hue;
use crate::model::work::{loaded_task, session_task};

use super::live::clock;
use super::live_session::reason;
use super::{Action, Feed, LineClass, Terminal, line};

/// The run behind session `id` when it ended and was a process session, read from its task's
/// loaded record.
pub fn run<'a>(feed: &'a daemon::Feed, id: &str) -> Option<&'a Run> {
    let task = session_task(id)?;
    let number = id.strip_prefix(task)?.strip_prefix('_')?;
    loaded_task(feed, task)?.runs.iter().find(|run| {
        run.id == number
            && run.ended.is_some()
            && matches!(run.session, Some(SessionRef::Process { .. }))
    })
}

/// Corrald's reason the log cannot be read, when it answered not found.
pub(super) fn note(cache: Option<&DetailCache>) -> Option<(Hue, String)> {
    let error = cache?
        .error
        .as_ref()
        .filter(|error| error.code == "not_found")?;
    let reason = reason(&error.message);
    let mut letters = reason.chars();
    let first = letters.next()?;
    Some((
        Hue::Held,
        format!("{}{}.", first.to_uppercase(), letters.as_str()),
    ))
}

/// The retained log's tail under a plain `ended` heading; it takes no input.
pub(super) fn terminal(
    id: &Id,
    run: &Run,
    cache: Option<&DetailCache>,
    feed: &daemon::Feed,
) -> Terminal {
    let capture = match cache.and_then(DetailCache::body) {
        Some(DetailBody::Terminal(capture)) => Some(capture),
        _ => None,
    };
    let error = cache.and_then(|cache| cache.error.as_ref());
    let gone = error.is_some_and(|error| error.code == "not_found");
    let loading = cache.is_none_or(|cache| cache.loading);
    let status = match capture {
        Some(capture) => format!(
            "{} · log tail · read {}",
            run.harness,
            clock(Some(capture.captured_at.as_str()), feed)
                .unwrap_or_else(|| "at an unknown time".into())
        ),
        None if gone => "log gone".to_owned(),
        None if error.is_some() => "not loaded".to_owned(),
        None if loading => "loading".to_owned(),
        None => "no log yet".to_owned(),
    };
    let empty = match capture {
        Some(capture) if capture.lines.is_empty() => Some("The log is empty.".to_owned()),
        Some(_) => None,
        None if gone => Some("No log tail to show.".to_owned()),
        None => Some(match error {
            Some(error) => format!(
                "The log could not be loaded: {} ({}).",
                error.message, error.code
            ),
            None if loading => "Loading the log…".to_owned(),
            None => "No log has arrived; it shows once corrald answers.".to_owned(),
        }),
    };
    Terminal {
        feed: Feed::Snapshot,
        heading: "ended".into(),
        status,
        input: "read-only log".into(),
        lines: capture
            .map(|capture| {
                capture
                    .lines
                    .iter()
                    .map(|text| line(text, LineClass::Plain))
                    .collect()
            })
            .unwrap_or_default(),
        cursor: false,
        verbatim: false,
        styled: Vec::new(),
        empty,
        refresh: Some(Action::Refresh(DetailRef::Terminal { id: id.to_string() })),
    }
}

#[cfg(test)]
mod tests {
    use corral_protocol::detail::{DetailBody, DetailRef, TaskDetail, TerminalSnapshot};
    use corral_types::{Role, SessionRef, Status};

    use super::super::roster_tests::agent;
    use super::super::{State, View, live_session};
    use crate::model::daemon::{self, ClientError, DetailCache, bare_task, detail_key};
    use crate::model::health::daemon_env;
    use crate::model::hue::Hue;
    use crate::model::work::task_ref;
    use crate::model::{Form, Id};

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

    fn page(feed: &daemon::Feed) -> super::super::Session {
        live_session(
            &State::default(),
            &Id::from(SESSION),
            feed,
            daemon_env(feed, Form::Desktop),
            "app:test",
        )
    }

    /// An ended headless run has no roster row and no pi transcript, so its page showed nothing
    /// but an empty transcript; it shows the log's tail under an `ended` heading instead.
    #[test]
    fn an_ended_headless_runs_page_shows_its_log_tail_under_an_ended_heading() {
        let log = TerminalSnapshot {
            lines: vec!["[codex] cd /work/tree".into(), "[codex] exit 0".into()],
            captured_at: "2026-11-14T10:00:00Z".into(),
            live: false,
            rows: Vec::new(),
        };
        let page = page(&feed(None, Some(log)));
        let terminal = page.terminal.expect("an ended headless run has its log");
        let shown: Vec<String> = terminal
            .lines
            .iter()
            .map(|line| line.text.iter().map(|span| span.text.as_str()).collect())
            .collect();
        assert_eq!(shown, ["[codex] cd /work/tree", "[codex] exit 0"]);
        assert_eq!(terminal.heading, "ended");
        assert_eq!(page.view, View::Terminal);
        assert!(page.ended && page.notice.is_none());
    }

    /// Once the log is gone corrald answers not found with its reason; the page says that
    /// reason as a note instead of drawing a blank log.
    #[test]
    fn a_missing_log_shows_the_reason_corrald_gave_as_a_note() {
        let gone = ClientError {
            code: "not_found".into(),
            message: "session log not found".into(),
        };
        let page = page(&feed(Some(gone), None));
        let terminal = page.terminal.expect("the page still has its log view");
        assert!(terminal.lines.is_empty());
        assert_eq!(
            page.notice,
            Some((Hue::Held, "Session log not found.".into()))
        );
    }
}
