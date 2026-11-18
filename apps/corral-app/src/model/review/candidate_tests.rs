use corral_protocol::detail::{DetailBody, ReviewDetail};
use corral_types::{Actor, Check, CheckKind, CheckResult, Op, Status};
use jiff::Timestamp;

use super::live::review_ref;
use super::*;
use crate::model::Form;
use crate::model::daemon::{DetailCache, Feed, Task, bare_task, detail_key, task_row};
use crate::model::effect::Conn;
use crate::model::projects::ProjectFilter;

fn env() -> Env {
    Env {
        conn: Conn::Healthy,
        form: Form::Desktop,
    }
}

fn gate(result: CheckResult) -> Check {
    Check {
        task: "T-1".into(),
        kind: CheckKind::Gate,
        key: "4a72ec4f00".into(),
        result,
        notes: None,
        log: None,
        by: Actor::Auto,
        at: Timestamp::UNIX_EPOCH,
    }
}

/// `T-1` in Review with `gate`, `ready` as given, and `T-2` still running.
fn feed(gate: Option<Check>, ready: bool) -> Feed {
    let mut row = task_row(bare_task(
        "T-1",
        "Candidate",
        Status::Review,
        Timestamp::UNIX_EPOCH,
    ));
    row.gate = gate;
    row.ready = ready;
    let running = task_row(bare_task(
        "T-2",
        "Running",
        Status::Running,
        Timestamp::UNIX_EPOCH,
    ));
    let mut feed = Feed::default();
    feed.board_received(1, vec![Task::new(row), Task::new(running)], &[], true);
    feed
}

fn serve(feed: &mut Feed, review: ReviewDetail) {
    let target = review_ref(&Id::new("T-1"));
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            target,
            pages: vec![],
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Review(review)),
        },
    );
}

fn page(state: &State, feed: &Feed) -> LoopPage {
    loop_page_filtered(
        state,
        feed,
        &Selection::default(),
        None,
        env(),
        0.0,
        &ProjectFilter::All,
    )
}

#[test]
fn only_review_tasks_are_candidates_and_ready_ones_wait_on_your_verdict() {
    let verifying = page(&State::default(), &feed(None, false));
    assert_eq!(verifying.outline.len(), 1);
    assert_eq!(verifying.outline[0].label, "In verification");
    assert_eq!(verifying.outline[0].rows[0].id, "T-1");
    let ready = page(&State::default(), &feed(None, true));
    assert_eq!(ready.outline[0].label, "Your verdict");
}

#[test]
fn land_anyway_appears_only_while_the_gate_is_red() {
    let labels = |feed: &Feed| -> Vec<Action> {
        let page = page(&State::default(), feed);
        page.detail
            .map(|detail| {
                detail
                    .decide
                    .secondary
                    .into_iter()
                    .map(|b| b.action)
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(!labels(&feed(Some(gate(CheckResult::Pass)), false)).contains(&Action::LandAnyway));
    assert!(labels(&feed(Some(gate(CheckResult::Fail)), false)).contains(&Action::LandAnyway));
}

#[test]
fn deciding_submits_the_engine_operation_without_predicting_a_refusal() {
    let feed = feed(Some(gate(CheckResult::Fail)), false);
    let mut state = State::default();
    let mut submit = |action: Action| {
        state.apply_live_filtered(
            action,
            &feed,
            &Selection::default(),
            None,
            env(),
            &ProjectFilter::All,
        )
    };
    let sent = |effects: Vec<Effect>| match effects.as_slice() {
        [Effect::Act(act)] => Some((act.op.clone(), act.force)),
        _ => None,
    };
    let approve = Op::Approve {
        id: "T-1".into(),
        onto: None,
    };
    assert_eq!(
        sent(submit(Action::Approve)),
        Some((approve.clone(), false))
    );
    assert_eq!(sent(submit(Action::LandAnyway)), Some((approve, true)));
    assert_eq!(
        sent(submit(Action::Gate)),
        Some((Op::Gate { id: "T-1".into() }, false))
    );
}

#[test]
fn a_review_body_without_a_finished_run_says_so_instead_of_an_empty_diff() {
    let mut feed = feed(None, false);
    serve(
        &mut feed,
        ReviewDetail {
            task: "T-1".into(),
            run: None,
            diff: None,
            checks: vec![],
            max_rounds: 3,
            files: vec![],
        },
    );
    let state = State {
        tab: Tab::Diff,
        ..State::default()
    };
    let detail = page(&state, &feed).detail;
    let Some(LoopCandidate {
        notice: Some(notice),
        content: LoopContent::Evidence(Body::Diff(diff)),
        ..
    }) = detail
    else {
        panic!("the candidate shows its diff tab with a notice");
    };
    assert!(notice.text.contains("no completed worker run"));
    assert!(diff.files.is_empty());
    assert!(
        diff.empty
            .is_some_and(|line| line.contains("No worker run"))
    );
}
