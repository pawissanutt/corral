use std::path::PathBuf;

use corral_protocol::detail::{TimelinePage, TimelineTask};
use corral_types::{Role, Run, Status};
use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::Landing;
use crate::model::daemon::Feed;
use crate::model::timeline::{Grouping, LaneView, Range, layout, live_plot, window};

fn at(at: &str) -> Timestamp {
    at.parse().unwrap()
}

fn run(task: &str, started: &str) -> Run {
    Run {
        id: "R1".into(),
        task: task.into(),
        role: Role::Worker,
        profile: "pi".into(),
        harness: "pi".into(),
        model: None,
        effort: None,
        worktree: PathBuf::new(),
        branch: String::new(),
        base: String::new(),
        base_sha: String::new(),
        session: None,
        started: at(started),
        ended: None,
        outcome: None,
        summary: None,
        head_sha: None,
        quiet_since: None,
    }
}

fn task(id: &str, depends_on: &[&str], landed: Option<&str>) -> TimelineTask {
    TimelineTask {
        id: id.into(),
        title: String::new(),
        status: Status::Running,
        parent: Some("P".into()),
        depends_on: depends_on.iter().map(|d| (*d).to_owned()).collect(),
        landed: landed.map(at),
    }
}

fn closed(id: &str, status: Status) -> TimelineTask {
    TimelineTask {
        status,
        ..task(id, &[], None)
    }
}

/// Five dependents started at 09:00: one before its dependency landed at 10:00, one after its
/// dependency landed at 08:00, one after a dependency that landed the day before, one on a
/// dropped dependency and one on a dependency done without a recorded landing.
fn page() -> TimelinePage {
    TimelinePage {
        runs: vec![
            run("D1", "2026-11-15T07:00:00Z"),
            run("T1", "2026-11-15T09:00:00Z"),
            run("T2", "2026-11-15T09:00:00Z"),
            run("T3", "2026-11-15T09:00:00Z"),
            run("T4", "2026-11-15T09:00:00Z"),
            run("T5", "2026-11-15T09:00:00Z"),
        ],
        events: Vec::new(),
        tasks: vec![
            task("D1", &[], Some("2026-11-15T10:00:00Z")),
            task("D2", &[], Some("2026-11-15T08:00:00Z")),
            task("D3", &[], Some("2026-11-14T12:00:00Z")),
            closed("D4", Status::Dropped),
            closed("D5", Status::Done),
            task("T1", &["D1"], None),
            task("T2", &["D2"], None),
            task("T3", &["D3"], None),
            task("T4", &["D4"], None),
            task("T5", &["D5"], None),
        ],
    }
}

fn feed() -> Feed {
    Feed {
        now: at("2026-11-15T12:00:00Z").as_second() as u64,
        time_zone: Some(TimeZone::UTC),
        ..Feed::default()
    }
}

/// A dependency that never lands (dropped, or done without a recorded landing) would otherwise
/// leave its dependent out of order forever.
#[test]
fn a_dependent_started_before_its_dependency_landed_is_out_of_order_and_others_are_not() {
    let feed = feed();
    let window = window(Range::Today, feed.now, &TimeZone::UTC).unwrap();
    let plot = live_plot(&page(), &window, Grouping::ByTask, &feed);
    let edge = |to: &str| plot.edges.iter().find(|edge| edge.to == to).unwrap();
    assert!(edge("T1").out_of_order);
    for to in ["T2", "T3", "T4", "T5"] {
        assert!(!edge(to).out_of_order, "{to} is not out of order");
    }
    assert_eq!(edge("T3").landing, Landing::Before);
    assert_eq!(edge("T4").landing, Landing::Dropped);
    assert_eq!(edge("T5").landing, Landing::DoneUnrecorded);
    assert_eq!(edge("T3").from_at, plot.start, "from the chart's left edge");
    let warned: Vec<&str> = plot
        .lanes
        .iter()
        .filter(|lane| lane.warning.is_some())
        .map(|lane| lane.id.as_str())
        .collect();
    assert_eq!(warned, ["T1"]);
    // Only the out-of-order edge is drawn until a lane is selected.
    let drawn = layout(&plot, 1190.0, false, LaneView::default()).edges;
    assert_eq!(drawn.len(), 1);
    assert!(drawn[0].out_of_order);
}

/// P1 → P2 → T with lanes for P2 and T: P2 is a plan of its own, so it heads the group T sits
/// in, its own lane first though T started earlier, and the header has words though corrald
/// served P2 without a title.
#[test]
fn a_nested_plan_heads_its_own_group_with_its_own_lane_first() {
    let feed = feed();
    let window = window(Range::Today, feed.now, &TimeZone::UTC).unwrap();
    let nested = |id: &str, parent: Option<&str>| TimelineTask {
        parent: parent.map(str::to_owned),
        ..task(id, &[], None)
    };
    let page = TimelinePage {
        runs: vec![
            run("T", "2026-11-15T07:00:00Z"),
            run("P2", "2026-11-15T09:00:00Z"),
        ],
        events: Vec::new(),
        tasks: vec![
            nested("P1", None),
            nested("P2", Some("P1")),
            nested("T", Some("P2")),
        ],
    };
    let plot = live_plot(&page, &window, Grouping::ByTask, &feed);
    let groups: Vec<(&str, &[String])> = plot
        .groups
        .iter()
        .map(|group| (group.key.as_str(), group.lanes.as_slice()))
        .collect();
    assert_eq!(
        groups,
        [("P2", ["P2".to_owned(), "T".to_owned()].as_slice())]
    );
    assert!(!plot.groups[0].title.is_empty(), "a header with no title");
}

#[test]
fn a_reply_without_tasks_draws_todays_flat_lanes_without_headers_or_edges() {
    let feed = feed();
    let window = window(Range::Today, feed.now, &TimeZone::UTC).unwrap();
    let mut page = page();
    page.tasks.clear();
    let plot = live_plot(&page, &window, Grouping::ByTask, &feed);
    assert_eq!(plot.lanes.len(), 6);
    assert!(plot.groups.is_empty() && plot.edges.is_empty());
    assert!(plot.lanes.iter().all(|lane| lane.warning.is_none()));
    let drawn = layout(&plot, 1190.0, false, LaneView::default());
    assert!(drawn.headers.is_empty() && drawn.edges.is_empty());
}
