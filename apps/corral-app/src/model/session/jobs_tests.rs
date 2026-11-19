use corral_protocol::detail::{DetailBody, DetailReply, JobOutputPage, SessionJob};

use super::{JobOutputView, buttons, output, output_target};
use crate::model::Id;
use crate::model::daemon::{self, DetailCache, Feed, Link};
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::session::{Cx, Facts, SessionRole, Ui};

const SESSION: &str = "B-k3x9";
const JOB: &str = "j1-4";

fn job(state: &str, exit_code: Option<i64>, actions: &[&str]) -> SessionJob {
    SessionJob {
        id: JOB.into(),
        command: "cargo test -p auth".into(),
        dir: None,
        state: state.into(),
        started_at: Some("2026-11-19T10:00:00Z".into()),
        ended_at: None,
        exit_code,
        output_bytes: 2_048,
        tail: vec!["running 12 tests".into()],
        monitors: Vec::new(),
        actions: actions.iter().map(|action| (*action).to_owned()).collect(),
    }
}

fn ids(job: &SessionJob) -> Vec<(String, bool)> {
    buttons(&Id::from(SESSION), job, None, false, false)
        .into_iter()
        .map(|button| (button.id, button.unavailable.is_none()))
        .collect()
}

/// The opened output of the job as the view reads it, when corrald served `text` from byte
/// `offset` of `bytes`, with `next` the token of the bytes before it.
fn opened(text: &str, offset: u64, bytes: u64, next: Option<&str>) -> JobOutputView {
    let id = Id::from(SESSION);
    let target = output_target(&id, JOB);
    let mut feed = Feed {
        link: Link::Up,
        ..Feed::default()
    };
    feed.details.insert(
        daemon::detail_key(&target),
        DetailCache {
            target,
            pages: vec![DetailReply {
                etag: "e1".into(),
                as_of: "2026-11-19T10:00:00Z".into(),
                seq: 1,
                next: next.map(str::to_owned),
                body: None,
            }],
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::SessionJobOutput(JobOutputPage {
                text: text.into(),
                offset,
                bytes,
                live: true,
            })),
        },
    );
    let ui = Ui::default();
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
    output(&cx, JOB)
}

#[test]
fn a_running_job_offers_stop_and_a_finished_one_does_not() {
    let running = ids(&job("running", None, &["stop", "background"]));
    assert!(
        running.contains(&("btn-job-stop".to_owned(), true)),
        "{running:?}"
    );
    assert!(
        running.contains(&("btn-job-background".to_owned(), true)),
        "{running:?}"
    );
    for finished in [job("exited", Some(0), &[]), job("exited", Some(1), &[])] {
        let shown = ids(&finished);
        assert!(
            shown
                .iter()
                .all(|(id, _)| id != "btn-job-stop" && id != "btn-job-background"),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|(id, _)| id == "btn-job-open"),
            "{shown:?}"
        );
    }
}

#[test]
fn a_search_counts_matches_only_in_the_loaded_output_and_says_when_earlier_output_is_not_searched()
{
    let text = "compiling\nerror: one\nwarning\nError: two\n";
    let bytes = 5_000 + text.len() as u64;
    let partial = opened(text, 5_000, bytes, Some("older"));
    let found = partial
        .search("error", usize::MAX)
        .expect("a typed query searches");
    assert_eq!(found.total, 2);
    assert_eq!(
        found.current,
        Some(1),
        "the search stands on the newest match"
    );
    assert_eq!(found.line, Some(3));
    assert!(found.note.is_some(), "the unloaded earlier output is said");
    assert!(
        found.earlier.is_some(),
        "the earlier output can be searched"
    );
    let whole = opened(text, 0, text.len() as u64, None);
    let found = whole.search("error", 0).expect("a typed query searches");
    assert_eq!(found.total, 2);
    assert!(found.note.is_none(), "{:?}", found.note);
    assert!(found.earlier.is_none());
    assert!(whole.search("", 0).is_none());
}

#[test]
fn new_lines_while_paused_are_counted_not_scrolled_to() {
    let before = opened("a\nb\n", 0, 4, None);
    let paused_at = before.end;
    let after = opened("a\nb\nc\nd\ne\n", 0, 10, None);
    let following = after.follow(None).expect("a live output follows");
    assert!(!following.paused);
    let paused = after
        .follow(Some(paused_at))
        .expect("a live output can be paused");
    assert!(paused.paused, "new lines do not resume following");
    assert_eq!(paused.new_lines, 3);
}
