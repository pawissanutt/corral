use corral_protocol::detail::PlanDetail;
use corral_protocol::rows::{PlanRow, StatusCount};
use corral_types::{Blocked, Next, Op, Status};
use jiff::Timestamp;

use super::{LiveAction, State, path_to_done, plan_outline};
use crate::model::board::Mark;
use crate::model::daemon::{Feed, Task, bare_task, task_row};
use crate::model::effect::{Conn, Effect, Env};
use crate::model::nav::Form;
use crate::model::projects::ProjectFilter;
use crate::model::{Id, Selection};

fn task(id: &str, status: Status) -> corral_types::Task {
    let mut task = bare_task(id, id, status, Timestamp::UNIX_EPOCH);
    if id != "T-1" {
        task.parent = Some("T-1".into());
    }
    task
}

fn plan(counts: &[(Status, u64)]) -> PlanRow {
    PlanRow {
        parent: task("T-1", Status::Backlog),
        child_counts: counts
            .iter()
            .map(|(status, count)| StatusCount {
                status: *status,
                count: *count,
            })
            .collect(),
    }
}

fn blocked(id: &str) -> corral_types::Task {
    let mut task = task(id, Status::Blocked);
    task.blocked = Some(Blocked {
        reason: "worker failed".into(),
        detail: None,
        next: vec![Next {
            op: Op::Retry {
                id: id.into(),
                profile: None,
                fresh: false,
                note: None,
            },
            force: false,
        }],
    });
    task
}

#[test]
fn a_plan_with_a_blocked_member_lists_under_needs_you_from_its_counts_alone() {
    let mut feed = Feed::default();
    let row = plan(&[(Status::Done, 1), (Status::Blocked, 1), (Status::Ready, 1)]);
    feed.plans.insert(Id::new("T-1"), row);
    let outline = plan_outline(&Selection::default(), &feed, &ProjectFilter::All);
    assert_eq!(outline.sections[0].label, "Needs you");
    let row = &outline.sections[0].rows[0];
    assert!(row.selected);
    let super::OutlineSecond::Members {
        segments, progress, ..
    } = &row.second
    else {
        panic!("a plan with children draws its member bar");
    };
    assert_eq!(segments.len(), 3);
    assert_eq!(progress, "1/3");
}

#[test]
fn path_to_done_lists_open_members_and_runs_a_blocked_ones_served_next_step() {
    let mut feed = Feed::default();
    let stuck = blocked("T-3");
    feed.board_received(1, vec![Task::new(task_row(stuck.clone()))], &[], true);
    let detail = PlanDetail {
        parent: task("T-1", Status::Backlog),
        children: vec![
            task("T-2", Status::Done),
            stuck,
            task("T-4", Status::Dropped),
        ],
        child_rows: None,
    };
    let path = path_to_done(&feed, &Id::new("T-1"), &detail);
    assert_eq!((path.done, path.total), (1, 2));
    assert_eq!(path.current, Some(0));
    assert_eq!(path.steps[0].mark, Mark::Active);
    let items = &path.steps[0].items;
    assert_eq!(items.len(), 1);
    let Some(next) = &items[0].action else {
        panic!("a blocked member offers its served next step");
    };
    let super::Action::Live(press) = next.action.clone();
    let effects = State::default().reduce_live(
        press,
        &feed,
        Env {
            conn: Conn::Healthy,
            form: Form::Desktop,
        },
    );
    assert!(matches!(
        effects.as_slice(),
        [Effect::Act(act)] if matches!(act.op, Op::Retry { .. }) && !act.force
    ));
    assert!(matches!(
        next.action,
        super::Action::Live(LiveAction::Next { index: 0, .. })
    ));
}

/// A plan page lists its children as the rows corrald serves for them, also those the open board
/// does not carry: a child in Review whose gate failed draws that failure on its card, and a Done
/// one counts as done on the path.
#[test]
fn a_plan_page_lists_its_children_from_their_served_rows() {
    let mut feed = Feed::default();
    feed.board_received(1, vec![], &[], true);
    let mut red = task_row(task("T-2", Status::Review));
    red.gate = Some(corral_types::Check {
        task: "T-2".into(),
        kind: corral_types::CheckKind::Gate,
        key: "4a72ec4f00".into(),
        result: corral_types::CheckResult::Fail,
        notes: None,
        log: None,
        by: corral_types::Actor::Auto,
        at: Timestamp::UNIX_EPOCH,
        round: None,
        run: None,
    });
    let landed = task_row(task("T-3", Status::Done));
    let detail = PlanDetail {
        parent: task("T-1", Status::Backlog),
        children: vec![red.task.clone(), landed.task.clone()],
        child_rows: Some(vec![red, landed]),
    };
    let plan = Id::new("T-1");
    let brief = super::plan_brief(&feed, &plan, &detail);
    let members: Vec<&str> = brief
        .graph
        .members
        .iter()
        .map(|member| member.id.as_str())
        .collect();
    assert_eq!(members, ["T-2", "T-3"]);
    assert_eq!(brief.graph.members[0].stage.marks[1], Mark::Fail);
    assert_eq!((brief.path.done, brief.path.total), (1, 2));
}
