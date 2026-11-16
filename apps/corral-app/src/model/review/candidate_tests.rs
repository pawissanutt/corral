use super::live::{Shown, file_ref, review_ref, task_ref};
use super::live_files::{diff, draft_key};
use super::*;
use crate::model::Form;
use crate::model::board::Mark;
use crate::model::daemon::{DetailCache, Feed, Group, Link, Step, Task, detail_key};
use crate::model::effect::{Conn, Effect};
use crate::model::projects::ProjectFilter;
use corral_protocol::detail::{DetailBody, PatchPage, ReviewCandidate, ReviewFile};
use serde_json::json;

fn env() -> Env {
    Env {
        conn: Conn::Healthy,
        form: Form::Desktop,
    }
}

fn candidate(head: &str) -> CandidateAt {
    CandidateAt {
        task: Id::new("T-1"),
        candidate: ReviewCandidate {
            run: "R1".into(),
            base: "base".into(),
            head: head.into(),
        },
    }
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

fn show(feed: &mut Feed, at: &CandidateAt) {
    feed.board_received(
        1,
        vec![Task {
            id: at.task.clone(),
            title: "Candidate".into(),
            project: None,
            plan: None,
            goal: None,
            group: Group::NeedsYou,
            rail: std::array::from_fn(|_| Step {
                mark: Mark::Done,
                sha7: Some(at.candidate.head.clone()),
                at: None,
            }),
            now: "review".into(),
            now_code: "review".into(),
            since: None,
            age_since: None,
            held_reason: None,
            questions: 0,
            attention: vec![],
            agent: None,
            cost: None,
            cost_estimated: false,
            rev: 1,
        }],
        &[],
        true,
    );
    cache(feed, review_ref(&at.task), DetailBody::Review(serde_json::from_value(json!({
        "task": "T-1", "candidate_sha": at.candidate.head, "base": at.candidate.base,
        "state": "review", "verdicts": [], "files": [{"path": "x", "added": 1, "deleted": 1}],
        "report": null, "acceptance": [], "gate_log_tail": [], "artifacts": [],
        "annotations": [], "approvable": true
    })).unwrap()));
    cache(feed, task_ref(&at.task), DetailBody::Task(serde_json::from_value(json!({
        "id": "T-1", "owner_revision": "rev", "title": "Candidate", "status": "review",
        "body_excerpt": "", "stages": [
            {"glyph":"pass","sha7":null,"at":null}, {"glyph":"pass","sha7":null,"at":null},
            {"glyph":"pass","sha7":null,"at":null}, {"glyph":"pass","sha7":null,"at":null}
        ], "dependencies": [], "progress": null, "owner_next": null, "hold": null, "park": null,
        "artifacts": [], "run": {"id":"R1", "session":null, "profile":null, "model":null,
            "effort":null,"base":"base","head":at.candidate.head,"started_at":null,"ended_at":null},
        "plan_context": null, "log_tail": []
    })).unwrap()));
}

fn patch(feed: &mut Feed, at: &CandidateAt) {
    cache(
        feed,
        file_ref(&at.task, "x"),
        DetailBody::ReviewFile(PatchPage {
            path: "x".into(),
            patch: "@@ -1 +1 @@\n-old\n+new\n".into(),
            truncated: false,
            candidate: Some(at.candidate.clone()),
        }),
    );
}

fn file(state: &State, feed: &Feed) -> FileView {
    let shown = Shown::of(feed, feed.board.task(&Id::new("T-1")).unwrap());
    let review = shown.review.unwrap();
    assert_eq!(
        review.files,
        [ReviewFile {
            path: "x".into(),
            added: 1,
            deleted: 1
        }]
    );
    diff(state, feed, &shown, review, Layout::Unified, env())
        .files
        .remove(0)
}

#[test]
fn stale_patches_and_open_composers_never_rebind_to_a_new_candidate() {
    let mut feed = Feed {
        link: Link::Up,
        session: Some(serde_json::from_value(json!({
            "device": {"id":"d", "name":"device", "platform":"other", "app_version":"test",
                "enrolled_at":"2026-11-14T00:00:00Z", "revoked_at":null},
            "authority": {"reply":true,"act":true,"assistant":false,"assistant_act":false,"admin":false},
            "seen": {"through":0,"at":null}, "server_time":"2026-11-14T00:00:00Z"
        })).unwrap()),
        ..Feed::default()
    };
    let old = candidate("oldhead");
    let new = candidate("newhead");
    let at = LineAt {
        path: "x".into(),
        old_side: false,
        line: 1,
    };
    let selection = Selection {
        review: Some(old.task.clone()),
        ..Selection::default()
    };
    let mut state = State::default();
    state.expanded.insert((old.task.clone(), "x".into()));
    show(&mut feed, &old);
    patch(&mut feed, &old);
    assert!(file(&state, &feed).annotatable);
    state.apply_live_filtered(
        Action::Annotate {
            candidate: old.clone(),
            at: at.clone(),
        },
        &feed,
        &selection,
        None,
        env(),
        &ProjectFilter::All,
    );
    assert!(state.composing.is_some());

    show(&mut feed, &new);
    assert!(!file(&state, &feed).annotatable);
    assert!(file(&state, &feed).candidate.is_none());
    for target in [
        Compose::New {
            candidate: old.clone(),
            at: at.clone(),
        },
        Compose::New {
            candidate: new.clone(),
            at: at.clone(),
        },
        Compose::Changes(old.clone()),
    ] {
        let effects = state.apply_live_filtered(
            Action::Submit {
                target,
                text: "owner note".into(),
            },
            &feed,
            &selection,
            None,
            env(),
            &ProjectFilter::All,
        );
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::Act(_)))
        );
    }
    assert!(state.sent.is_empty());
    assert_ne!(draft_key(&old, &at), draft_key(&new, &at));
    assert_ne!(old.draft("changes"), new.draft("changes"));
    let mut other = old.clone();
    other.task = Id::new("T-2");
    assert_ne!(draft_key(&old, &at), draft_key(&other, &at));

    patch(&mut feed, &new);
    feed.details
        .get_mut(&detail_key(&file_ref(&new.task, "x")))
        .unwrap()
        .stale = true;
    assert!(!file(&state, &feed).annotatable);
    patch(&mut feed, &new);
    assert!(file(&state, &feed).annotatable);
    state.composing = None;
    state.apply_live_filtered(
        Action::Annotate {
            candidate: old,
            at: at.clone(),
        },
        &feed,
        &selection,
        None,
        env(),
        &ProjectFilter::All,
    );
    assert!(state.composing.is_none());
    let effects = state.apply_live_filtered(
        Action::Submit {
            target: Compose::New {
                candidate: new.clone(),
                at,
            },
            text: "owner note".into(),
        },
        &feed,
        &selection,
        None,
        env(),
        &ProjectFilter::All,
    );
    assert!(
        matches!(&effects[..], [Effect::Act(act)] if matches!(&act.command,
        corral_protocol::cmd::Command::Annotation(corral_protocol::cmd::annotation::AnnotationCommand::Open {
            expected_run, base, head, ..
        }) if expected_run == &new.candidate.run && base == &new.candidate.base && head == &new.candidate.head))
    );
}

#[test]
fn narrow_review_content_is_unified_and_gate_receipts_are_visible() {
    let state = State::default();
    assert_eq!(effective_layout(&state, env(), 1099.0), Layout::Unified);
    assert_eq!(effective_layout(&state, env(), 1100.0), Layout::Split);
    let mut feed = Feed::default();
    let at = candidate("head");
    show(&mut feed, &at);
    let cache = feed
        .details
        .get_mut(&detail_key(&review_ref(&at.task)))
        .unwrap();
    let Some(DetailBody::Review(review)) = cache.merged.as_mut() else {
        panic!("review fixture")
    };
    review.gates.push(corral_protocol::detail::ReviewGate {
        by: "gate".into(),
        sha: "old-head".into(),
        verdict: "fail".into(),
        at: None,
        text: None,
        stale: true,
    });
    let selection = Selection {
        review: Some(at.task),
        ..Selection::default()
    };
    let page = |state: &State| {
        loop_page_filtered(
            state,
            &feed,
            &selection,
            None,
            env(),
            1099.0,
            &ProjectFilter::All,
        )
        .detail
        .unwrap()
        .content
    };
    let LoopContent::Brief(brief) = page(&state) else {
        panic!("Brief is the first tab")
    };
    assert_eq!(brief.readiness[0].stats[0].key, "earlier-gates");
    let diff_tab = State {
        tab: Tab::Diff,
        ..State::default()
    };
    assert!(matches!(
        page(&diff_tab),
        LoopContent::Evidence(Body::Diff(DiffView {
            layout: Layout::Unified,
            ..
        }))
    ));
}

#[test]
fn pushed_review_actions_do_not_target_a_candidate_in_the_list_project_filter() {
    use corral_protocol::cmd::{Command, review::ReviewCommand, task::TaskCommand};

    let mut feed = Feed {
        link: Link::Up,
        session: Some(serde_json::from_value(json!({
            "device": {"id":"d", "name":"device", "platform":"other", "app_version":"test",
                "enrolled_at":"2026-11-14T00:00:00Z", "revoked_at":null},
            "authority": {"reply":true,"act":true,"assistant":false,"assistant_act":false,"admin":false},
            "seen": {"through":0,"at":null}, "server_time":"2026-11-14T00:00:00Z"
        })).unwrap()),
        ..Feed::default()
    };
    let focused = candidate("head");
    show(&mut feed, &focused);
    let mut b = feed.board.task(&focused.task).unwrap().clone();
    b.project = Some("B".into());
    let mut a = b.clone();
    a.id = Id::new("T-2");
    a.project = Some("A".into());
    let selection = Selection {
        review: Some(a.id.clone()),
        ..Selection::default()
    };
    feed.board_received(2, vec![a.clone(), b], &[], true);
    let filter = ProjectFilter::Project(Id::new("A"));
    let env = Env {
        form: Form::Phone,
        ..env()
    };
    let mut state = State::default();
    let page = loop_page_filtered(
        &state,
        &feed,
        &selection,
        Some(&focused.task),
        env,
        400.0,
        &filter,
    );
    assert_eq!(page.detail.unwrap().header.task, focused.task.to_string());
    assert_eq!(
        selected_filtered(&selection, &feed, &filter).unwrap().id,
        a.id
    );

    for action in [Action::Approve, Action::Retry] {
        let effects =
            state.apply_live_filtered(action, &feed, &selection, Some(&focused.task), env, &filter);
        assert!(
            matches!(&effects[..], [Effect::Act(act)] if matches!(&act.command,
                Command::Review(ReviewCommand::Approve { id, .. })
                    | Command::Task(TaskCommand::Retry { id, .. })
                    if id == focused.task.as_str()
            ))
        );
    }
    assert_eq!(state.sent.len(), 2);
    assert!(state.sent.iter().all(|sent| sent.task == focused.task));

    state.apply_live_filtered(
        Action::Expand("x".into()),
        &feed,
        &selection,
        Some(&focused.task),
        env,
        &filter,
    );
    assert!(state.expanded.contains(&(focused.task.clone(), "x".into())));
    assert!(!state.expanded.contains(&(a.id.clone(), "x".into())));
    state.apply_live_filtered(
        Action::Expand("x".into()),
        &feed,
        &selection,
        None,
        env,
        &filter,
    );
    assert!(state.expanded.contains(&(a.id, "x".into())));
    assert!(
        state
            .apply_live_filtered(
                Action::Retry,
                &feed,
                &selection,
                Some(&Id::new("T-missing")),
                env,
                &filter,
            )
            .is_empty()
    );
}
