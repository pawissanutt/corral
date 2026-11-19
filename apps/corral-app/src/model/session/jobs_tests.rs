use corral_protocol::detail::SessionJob;

use super::buttons;
use crate::model::Id;

fn job(state: &str, exit_code: Option<i64>, actions: &[&str]) -> SessionJob {
    SessionJob {
        id: "j1-4".into(),
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
    buttons(&Id::from("B-k3x9"), job, None, false, false)
        .into_iter()
        .map(|button| (button.id, button.unavailable.is_none()))
        .collect()
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
