use super::live::{Shown, review_ref, task_ref};
use super::*;
use crate::model::board::Mark;
use crate::model::daemon::{DetailCache, Feed, Group, Link, Step, Task, detail_key};
use crate::model::effect::{Conn, Effect, Env};
use crate::model::hue::Hue;
use crate::model::projects::ProjectFilter;
use corral_protocol::detail::{
    ArtifactRef, DetailBody, ReviewDetail, ReviewGate, ReviewVerdict, TaskDetail, TaskProgress,
};
use serde_json::json;

fn env() -> Env {
    Env {
        conn: Conn::Healthy,
        form: crate::model::Form::Desktop,
    }
}

fn task(id: &str, group: Group, gate: Mark) -> Task {
    Task {
        id: Id::new(id),
        title: "Candidate\nwith a title".into(),
        project: Some("A".into()),
        plan: None,
        goal: None,
        group,
        rail: std::array::from_fn(|index| Step {
            mark: match index {
                0 => Mark::Done,
                1 => gate,
                _ => Mark::Next,
            },
            sha7: (index == 0).then(|| "85b45e1".into()),
            at: None,
        }),
        now: "gating".into(),
        now_code: "gating".into(),
        age_since: None,
        since: None,
        held_reason: None,
        questions: 0,
        attention: vec![],
        agent: None,
        cost: None,
        cost_estimated: false,
        rev: 1,
    }
}

fn review() -> ReviewDetail {
    ReviewDetail {
        task: "T-1".into(),
        candidate_sha: Some("85b45e1e".into()),
        base: Some("base".into()),
        state: "gating".into(),
        verdicts: vec![],
        gates: vec![],
        files: vec![],
        report: None,
        acceptance: vec![],
        gate_log_tail: vec![],
        artifacts: vec![],
        annotations: vec![],
        approvable: false,
    }
}

fn detail() -> TaskDetail {
    serde_json::from_value(json!({
        "id":"T-1", "owner_revision":"revision", "title":"Candidate", "status":"gating",
        "body_excerpt":"", "stages":[
            {"glyph":"pass","sha7":null,"at":null}, {"glyph":"active","sha7":null,"at":null},
            {"glyph":"pending","sha7":null,"at":null}, {"glyph":"pending","sha7":null,"at":null}
        ], "dependencies":[], "progress":null, "owner_next":null, "hold":null, "park":null,
        "artifacts":[], "run":{"id":"R1","session":"worker","profile":null,"model":null,
            "effort":null,"base":"base","head":"85b45e1e","started_at":null,"ended_at":null},
        "plan_context":null,"log_tail":[]
    }))
    .unwrap()
}

fn cache(feed: &mut Feed, target: corral_protocol::detail::DetailRef, body: DetailBody) {
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            target,
            pages: vec![],
            loading: false,
            stale: false,
            error: None,
            merged: Some(body),
        },
    );
}

fn feed(review: ReviewDetail) -> Feed {
    let mut feed = Feed {
        link: Link::Up,
        session: Some(serde_json::from_value(json!({
            "device":{"id":"d","name":"device","platform":"other","app_version":"test",
                "enrolled_at":"2026-11-16T00:00:00Z","revoked_at":null},
            "authority":{"reply":true,"act":true,"assistant":true,"assistant_act":true,"admin":false},
            "seen":{"through":0,"at":null},"server_time":"2026-11-16T00:00:00Z"
        })).unwrap()),
        ..Feed::default()
    };
    feed.board_received(
        1,
        vec![task("T-1", Group::Verifying, Mark::Active)],
        &[],
        true,
    );
    cache(
        &mut feed,
        review_ref(&Id::new("T-1")),
        DetailBody::Review(review),
    );
    cache(
        &mut feed,
        task_ref(&Id::new("T-1")),
        DetailBody::Task(detail()),
    );
    feed
}

#[test]
fn older_candidate_receipts_never_enter_the_readiness_chain() {
    let mut review = review();
    review.approvable = true;
    for (sha, verdict, stale) in [("f34f3df6", "fail", false), ("85b45e1e", "pass", false)] {
        review.gates.push(ReviewGate {
            by: "quick tier".into(),
            sha: sha.into(),
            verdict: verdict.into(),
            at: Some("2026-11-16T04:45:00Z".into()),
            text: Some("gate detail".into()),
            stale,
        });
    }
    for (sha, verdict, text) in [
        ("11111111", "changes", "older finding"),
        ("85b45e1e", "approve", "current finding"),
    ] {
        review.verdicts.push(ReviewVerdict {
            by: "reviewer".into(),
            sha: sha.into(),
            verdict: verdict.into(),
            at: None,
            text: Some(text.into()),
            stale: false,
        });
    }
    let task = task("T-1", Group::NeedsYou, Mark::Done);
    let chain = readiness_chain(&Feed::default(), &task, &review);
    assert_eq!(chain.len(), 3);
    assert!(
        chain
            .iter()
            .all(|link| link.sha7.as_deref() == Some("85b45e1"))
    );
    assert_eq!(
        chain.iter().map(|link| link.mark).collect::<Vec<_>>(),
        [Mark::Done, Mark::Done, Mark::Next]
    );
    assert_eq!(chain[0].stats[0].value, "1");
    assert!(chain[0].stats[0].hint.contains("f34f3df"));
    assert_eq!(chain[1].stats[0].value, "1");
    assert!(chain[1].stats[0].hint.contains("1111111"));
    assert_eq!(chain[1].quote.as_ref().unwrap().text, "current finding");
    assert_eq!(chain[2].hue, Hue::You);

    review.gates[1].stale = true;
    assert_eq!(
        readiness_chain(&Feed::default(), &task, &review)[0].mark,
        Mark::Stale
    );
}

#[test]
fn verifying_approval_is_disabled_with_the_same_refusal_as_the_command_guard() {
    let feed = feed(review());
    let task = feed.board.task("T-1").unwrap();
    let mut state = State::default();
    let bar = candidate_decide_bar(&state, &feed, Some(task), env());
    assert!(matches!(bar.buttons[0].action, Action::Approve));
    assert!(matches!(bar.buttons[1].action, Action::RequestChanges));
    assert!(matches!(bar.buttons[2].action, Action::AskCandidate));
    let approve = &bar.buttons[0];
    assert!(!approve.enabled);
    let expected = super::outline::approval_unavailable(&Shown::of(&feed, task));
    assert_eq!(approve.refusal.as_deref(), Some(expected.as_str()));
    assert_eq!(
        state.apply_live_filtered(
            Action::Approve,
            &feed,
            &Selection::default(),
            Some(&task.id),
            env(),
            &ProjectFilter::All
        ),
        [Effect::Toast(expected.clone())]
    );
    assert!(state.sent.is_empty());
    let mut finished_gate = task.clone();
    finished_gate.rail[1].mark = Mark::Done;
    assert_ne!(
        expected,
        super::outline::approval_unavailable(&Shown::of(&feed, &finished_gate))
    );
}

#[test]
fn absent_optional_evidence_never_leaves_a_hidden_tab_selected() {
    let mut review = review();
    review.report = Some(" \n ".into());
    let mut state = State {
        tab: Tab::Shot,
        ..State::default()
    };
    let tabs = candidate_tabs(&state, Some(&review));
    assert_eq!(
        tabs.iter().map(|tab| tab.tab).collect::<Vec<_>>(),
        [Tab::Brief, Tab::Diff, Tab::Log]
    );
    assert_eq!(
        tabs.iter().find(|tab| tab.selected).unwrap().tab,
        Tab::Brief
    );
    review.acceptance.push("A recorded criterion".into());
    review.artifacts.push(ArtifactRef {
        source_ref: "run/report.txt".into(),
        title: "Report".into(),
        media_type: None,
    });
    let tabs = candidate_tabs(&state, Some(&review));
    assert_eq!(
        tabs.iter().map(|tab| tab.tab).collect::<Vec<_>>(),
        [Tab::Brief, Tab::Diff, Tab::Log, Tab::Report, Tab::Shot]
    );
    assert_eq!(tabs.iter().find(|tab| tab.selected).unwrap().tab, Tab::Shot);
    for tab in [Tab::Diff, Tab::Log, Tab::Report, Tab::Shot] {
        state.rtab(tab);
        assert_eq!(
            candidate_tabs(&state, Some(&review))
                .into_iter()
                .find(|tab| tab.selected)
                .unwrap()
                .tab,
            tab
        );
    }
    assert_eq!(State::default().tab, Tab::Brief);
}

#[test]
fn page_counts_equal_the_scoped_outline_even_without_candidate_details() {
    let mut feed = feed(review());
    let mut outside = task("T-4", Group::NeedsYou, Mark::Done);
    outside.project = Some("B".into());
    feed.board_received(
        2,
        vec![
            task("T-1", Group::NeedsYou, Mark::Done),
            task("T-2", Group::NeedsYou, Mark::Done),
            task("T-3", Group::Verifying, Mark::Active),
            outside,
        ],
        &[],
        true,
    );
    let page = loop_page_filtered(
        &State::default(),
        &feed,
        &Selection::default(),
        None,
        env(),
        900.0,
        &ProjectFilter::Project(Id::new("A")),
    );
    assert_eq!(
        page.outline
            .iter()
            .map(|group| group.count)
            .collect::<Vec<_>>(),
        [2, 1]
    );
    assert_eq!(
        page.stats
            .iter()
            .map(|stat| stat.value.as_str())
            .collect::<Vec<_>>(),
        ["2", "1"]
    );
    assert_eq!(page.outline[0].rows[0].test_id, "task-T-1");
    assert!(!page.outline[0].rows[0].title.contains('\n'));
    assert!(
        page.outline
            .iter()
            .flat_map(|group| &group.rows)
            .all(|row| row.project_chip.is_none())
    );
}

#[test]
fn asked_for_ignores_steps_from_other_runs_and_preserves_aggregate_progress() {
    let mut feed = Feed::default();
    let review = review();
    let mut detail = detail();
    detail.progress = Some(TaskProgress {
        completed: 2,
        total: 3,
        text: None,
    });
    let mut row: corral_protocol::rows::AgentRow = serde_json::from_value(json!({
        "id":"worker", "role":"worker", "task":"T-1", "run":"R0", "title":"Worker",
        "profile":null,"runtime":null,"machine":null,"pane":null,"status":"working",
        "now":"working","since":null,"tail":[],"cost_usd":null,"cost_basis":null,
        "tokens":null,"rev":1,"steps":[{"title":"First","state":"done"},{"title":"Next","state":"pending"}]
    })).unwrap();
    feed.agents.insert(Id::new("worker"), row.clone());
    let side = super::brief_side::side(&feed, &review, Some(&detail));
    let asked = side.asked_for.unwrap();
    assert!(asked.items.is_empty());
    assert_eq!(asked.count, "2/3");
    row.run = Some("R1".into());
    feed.agents.insert(Id::new("worker"), row);
    let side = super::brief_side::side(&feed, &review, Some(&detail));
    let asked = side.asked_for.unwrap();
    assert_eq!(
        asked
            .items
            .iter()
            .map(|(mark, _)| *mark)
            .collect::<Vec<_>>(),
        [Mark::Done, Mark::Next]
    );
    assert_eq!(asked.count, "1/2");
    assert!(matches!(
        side.open_session.unwrap().action,
        Action::OpenSession
    ));
    feed.agents.clear();
    detail.progress = None;
    assert!(
        super::brief_side::side(&feed, &review, Some(&detail))
            .asked_for
            .is_none()
    );
}
