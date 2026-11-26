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

/// Three dependents started at 09:00: one before its dependency landed at 10:00, one after its
/// dependency landed at 08:00, one after a dependency that landed the day before.
fn page() -> TimelinePage {
    TimelinePage {
        runs: vec![
            run("D1", "2026-11-15T07:00:00Z"),
            run("T1", "2026-11-15T09:00:00Z"),
            run("T2", "2026-11-15T09:00:00Z"),
            run("T3", "2026-11-15T09:00:00Z"),
        ],
        events: Vec::new(),
        tasks: vec![
            task("D1", &[], Some("2026-11-15T10:00:00Z")),
            task("D2", &[], Some("2026-11-15T08:00:00Z")),
            task("D3", &[], Some("2026-11-14T12:00:00Z")),
            task("T1", &["D1"], None),
            task("T2", &["D2"], None),
            task("T3", &["D3"], None),
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

#[test]
fn a_dependent_started_before_its_dependency_landed_is_out_of_order_and_others_are_not() {
    let feed = feed();
    let window = window(Range::Today, feed.now, &TimeZone::UTC).unwrap();
    let plot = live_plot(&page(), &window, Grouping::ByTask, &feed);
    let edge = |to: &str| plot.edges.iter().find(|edge| edge.to == to).unwrap();
    assert!(edge("T1").out_of_order);
    assert!(!edge("T2").out_of_order);
    assert!(!edge("T3").out_of_order);
    assert_eq!(edge("T3").landing, Landing::Before);
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

#[test]
fn a_reply_without_tasks_draws_todays_flat_lanes_without_headers_or_edges() {
    let feed = feed();
    let window = window(Range::Today, feed.now, &TimeZone::UTC).unwrap();
    let mut page = page();
    page.tasks.clear();
    let plot = live_plot(&page, &window, Grouping::ByTask, &feed);
    assert_eq!(plot.lanes.len(), 4);
    assert!(plot.groups.is_empty() && plot.edges.is_empty());
    assert!(plot.lanes.iter().all(|lane| lane.warning.is_none()));
    let drawn = layout(&plot, 1190.0, false, LaneView::default());
    assert!(drawn.headers.is_empty() && drawn.edges.is_empty());
}
