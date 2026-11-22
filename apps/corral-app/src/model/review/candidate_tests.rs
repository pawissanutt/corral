use corral_protocol::detail::{DetailBody, ReviewDetail};
use corral_protocol::rows::ProjectRow;
use corral_types::{Actor, Check, CheckKind, CheckResult, Op, Run, Status};
use jiff::Timestamp;

use super::live::review_ref;
use super::*;
use crate::model::Form;
use crate::model::daemon::{DetailCache, Feed, Task, bare_task, detail_key, task_row};
use crate::model::effect::{Conn, Env};
use crate::model::live::Selection;
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
        round: None,
        run: None,
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
            transcript_seams: Default::default(),
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

/// corrald marks a candidate ready only once the pipeline is past every check it runs, so a
/// ready candidate without a review check has no reviewer; it must not read "Review running".
#[test]
fn a_ready_candidate_without_a_review_check_does_not_claim_a_review_is_running() {
    let detail = page(
        &State::default(),
        &feed(Some(gate(CheckResult::Pass)), true),
    )
    .detail;
    let Some(LoopCandidate {
        content: LoopContent::Brief(brief),
        ..
    }) = detail
    else {
        panic!("the ready candidate opens on its brief");
    };
    let review = &brief.readiness[1];
    assert_eq!(review.mark, crate::model::board::Mark::Na);
    assert_ne!(review.title, "Review running");
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
    // A branch typed into the Onto field is where Approve and Land anyway land.
    assert!(submit(Action::Onto("release/0.9".into())).is_empty());
    let onto = Op::Approve {
        id: "T-1".into(),
        onto: Some("release/0.9".into()),
    };
    assert_eq!(sent(submit(Action::Approve)), Some((onto.clone(), false)));
    assert_eq!(sent(submit(Action::LandAnyway)), Some((onto, true)));
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

/// The engine refuses landing a candidate onto its own branch, so the Onto field's branches and
/// the Approve form's must leave it out while offering the project's other branches.
#[test]
fn onto_and_the_approve_form_never_offer_the_candidates_own_branch() {
    let mut task = bare_task("T-1", "Candidate", Status::Review, Timestamp::UNIX_EPOCH);
    task.project = Some("corral".into());
    let mut row = task_row(task.clone());
    row.run = Some(Run {
        id: "R1".into(),
        task: "T-1".into(),
        role: corral_types::Role::Worker,
        profile: "omp-sol".into(),
        harness: "omp".into(),
        model: None,
        effort: None,
        worktree: "/work/T-1".into(),
        branch: "corral/T-1".into(),
        base: "main".into(),
        base_sha: "0".repeat(40),
        session: None,
        started: Timestamp::UNIX_EPOCH,
        ended: None,
        outcome: None,
        summary: None,
        head_sha: None,
        quiet_since: None,
    });
    let mut feed = Feed::default();
    feed.projects.insert(
        "corral".into(),
        ProjectRow {
            id: "corral".into(),
            local_branches: Some(
                ["corral/T-1", "main", "release/0.9"]
                    .map(String::from)
                    .to_vec(),
            ),
            ..ProjectRow::default()
        },
    );
    feed.board_received(1, vec![Task::new(row)], &[], true);

    let own = "corral/T-1".to_owned();
    let onto = page(&State::default(), &feed)
        .detail
        .map(|detail| detail.decide.target.branches)
        .unwrap_or_default();
    assert!(
        onto.contains(&"release/0.9".to_owned()),
        "Onto offers {onto:?}"
    );
    assert!(
        !onto.contains(&own),
        "Onto offers the candidate's own branch"
    );
    let form = crate::model::work::FormContext::new(&feed, Some(&task)).branches;
    assert!(
        form.contains(&"main".to_owned()),
        "the form offers {form:?}"
    );
    assert!(
        !form.contains(&own),
        "the Approve form offers the own branch"
    );
}
