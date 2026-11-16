use corral_protocol::board::{RailGlyph, RailStep};
use corral_protocol::detail::{PlanDetail, PlanMember};
use corral_protocol::rows::{
    AttentionActor, AttentionDestination, AttentionMove, InboxKind, InboxRow, InboxState,
    PlanRow as WirePlan, PlanState,
};

use super::{Action, LiveAction, State, page_stats, path_to_done, plan_outline};
use crate::model::board::Mark;
use crate::model::daemon::Feed;
use crate::model::effect::{Conn, Effect, Env};
use crate::model::glance::StatIcon;
use crate::model::nav::Form;
use crate::model::projects::ProjectFilter;
use crate::model::{Id, Live, Selection};

fn member(id: &str, marks: [RailGlyph; 4]) -> PlanMember {
    PlanMember {
        id: id.into(),
        title: id.into(),
        layer: Some(1),
        state: "running".into(),
        rail: marks.map(|glyph| RailStep {
            glyph,
            sha7: None,
            at: None,
        }),
    }
}

fn detail(members: Vec<PlanMember>) -> PlanDetail {
    PlanDetail {
        id: "T-1".into(),
        role: None,
        linked: None,
        revision: None,
        revisions: Vec::new(),
        owner_decisions: Vec::new(),
        state: PlanState::Executing,
        document: None,
        reviews: Vec::new(),
        members,
        edges: Vec::new(),
        scenarios: Some(6),
        mapped: Some(0),
        acceptable: false,
        accept_blockers: Vec::new(),
    }
}

fn row(state: PlanState) -> WirePlan {
    WirePlan {
        id: "T-1".into(),
        title: "Plan".into(),
        role: None,
        linked: None,
        state,
        revision: Some("P2".into()),
        delivery: None,
        members_total: 3,
        members_landed: 0,
        blockers: Vec::new(),
        accepted_at: None,
        projects: Vec::new(),
        rev: 1,
    }
}

fn owner_move(id: &str, task: &str, kind: InboxKind) -> InboxRow {
    InboxRow {
        id: id.into(),
        kind,
        task: Some(task.into()),
        plan: Some("T-1".into()),
        project: None,
        title: "Owner move".into(),
        from: "worker".into(),
        asked_at: None,
        needed_by: None,
        blocking: true,
        state: InboxState::Open,
        question_rev: (kind == InboxKind::WorkerQuestion).then_some(1),
        answerable: true,
        unanswerable_reason: None,
        attention: Some(AttentionMove {
            whose_move: AttentionActor::Owner,
            reason: "Owner decision".into(),
            next_step: "Answer".into(),
            command: None,
            destination: AttentionDestination::Plans,
        }),
        rev: 1,
    }
}

#[test]
fn path_counts_landed_members_not_passed_stages_and_has_one_current_step() {
    use RailGlyph::{Active, Pass, Pending};
    let feed = Feed::default();
    let plan = Id::from("T-1");
    let mut detail = detail(vec![
        member("T-2", [Pass; 4]),
        member("T-3", [Pass, Pass, Pass, Pending]),
        member("T-4", [Pass, Active, Pending, Pending]),
    ]);
    let path = path_to_done(&feed, &plan, &detail);
    assert_eq!((path.landed, path.total, path.current), (1, 3, Some(1)));
    assert_eq!(
        path.steps
            .iter()
            .filter(|step| step.mark == Mark::Active)
            .count(),
        1
    );
    assert_eq!(path.steps[1].items.len(), 2);
    detail.members = vec![
        member("T-2", [Pass; 4]),
        member("T-3", [Pass; 4]),
        member("T-4", [Pass; 4]),
    ];
    let path = path_to_done(&feed, &plan, &detail);
    assert_eq!((path.landed, path.current), (3, Some(2)));
    detail.mapped = Some(6);
    let path = path_to_done(&feed, &plan, &detail);
    assert_eq!(path.current, Some(3));
    assert_eq!(
        path.steps
            .iter()
            .filter(|step| step.mark == Mark::Active)
            .count(),
        1
    );
    detail.state = PlanState::Complete;
    let path = path_to_done(&feed, &plan, &detail);
    assert_eq!(path.current, None);
    assert!(path.steps.iter().all(|step| step.mark == Mark::Done));
}

#[test]
fn awaiting_plan_counts_once_even_with_multiple_owner_attention_rows() {
    let mut feed = Feed::default();
    feed.plans.insert("T-1".into(), row(PlanState::Awaiting));
    feed.inbox.insert(
        "accept".into(),
        owner_move("accept", "T-1", InboxKind::PlanAcceptance),
    );
    feed.inbox.insert(
        "decision".into(),
        owner_move("decision", "T-1", InboxKind::OwnerDecision),
    );
    let stats = page_stats(&feed, &ProjectFilter::All);
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].icon, StatIcon::NeedsYou);
    assert_eq!(stats[0].value, "1");
}

#[test]
fn owner_question_answer_stays_on_the_member_path_item() {
    use RailGlyph::{Active, Pass, Pending};
    let mut feed = Feed::default();
    feed.plans.insert("T-1".into(), row(PlanState::Executing));
    let detail = detail(vec![member("T-2", [Pass, Active, Pending, Pending])]);
    feed.inbox.insert(
        "question-2".into(),
        owner_move("question-2", "T-2", InboxKind::WorkerQuestion),
    );
    let path = path_to_done(&feed, &Id::from("T-1"), &detail);
    let item = &path.steps[1].items[0];
    assert_eq!(
        item.action.as_ref().map(|action| &action.action),
        Some(&Action::Live(LiveAction::Question(Id::from("question-2"))))
    );
    assert_eq!(page_stats(&feed, &ProjectFilter::All)[0].value, "1");
}

#[test]
fn keys_walk_an_unlinked_design_ahead_of_a_linked_one_as_the_list_draws_them() {
    let mut feed = Feed::default();
    let approved = |id: &str, linked: Option<&str>| WirePlan {
        id: id.into(),
        linked: linked.map(Into::into),
        ..row(PlanState::Approved)
    };
    feed.plans
        .insert("T-1".into(), approved("T-1", Some("T-9")));
    feed.plans.insert("T-2".into(), approved("T-2", None));
    let shown: Vec<Id> = plan_outline(&Selection::default(), &feed, &ProjectFilter::All)
        .sections
        .iter()
        .flat_map(|section| &section.rows)
        .map(|row| row.id.clone())
        .collect();
    assert_eq!(shown, [Id::from("T-2"), Id::from("T-1")]);
    let env = Env {
        conn: Conn::Healthy,
        form: Form::Desktop,
    };
    let mut state = State::default();
    let walk = |state: &mut State, key: &str, selection: &Selection| match state.key_live_filtered(
        key,
        None,
        selection,
        &feed,
        env,
        &ProjectFilter::All,
    ) {
        Some(effects) => match effects.as_slice() {
            [Effect::Select(Live::Plan(id))] => Some(id.clone()),
            _ => None,
        },
        None => None,
    };
    assert_eq!(
        walk(&mut state, "j", &Selection::default()),
        Some(Id::from("T-1"))
    );
    let on_last = Selection {
        plan: Some(Id::from("T-1")),
        ..Selection::default()
    };
    assert_eq!(walk(&mut state, "k", &on_last), Some(Id::from("T-2")));
}

#[test]
fn a_long_plan_lists_eight_open_members_first_those_waiting_on_you_then_the_rest_as_one_line() {
    use RailGlyph::Pending;
    let mut feed = Feed::default();
    let members: Vec<PlanMember> = (1..=40)
        .map(|n| member(&format!("T-{}", 100 + n), [Pending; 4]))
        .collect();
    // The last three of forty wait on the owner: urgency, not plan order, brings them up.
    for task in ["T-138", "T-139", "T-140"] {
        let key = format!("q-{task}");
        feed.inbox.insert(
            key.as_str().into(),
            owner_move(&key, task, InboxKind::WorkerQuestion),
        );
    }
    let path = path_to_done(&feed, &Id::from("T-1"), &detail(members));
    let items = &path.steps[1].items;
    assert_eq!(items.len(), 9);
    for (item, task) in items.iter().zip(["T-138", "T-139", "T-140"]) {
        assert!(
            item.text.starts_with(task),
            "{} leads with {task}",
            item.text
        );
    }
    let rest = &items[8];
    assert_eq!(rest.text, "+32 more");
    assert!(rest.action.is_none());
    assert_eq!(rest.hint, "32 more members: 32 waiting");
}
