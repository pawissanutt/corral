use super::*;
use crate::model::timeline::{BurnupScope, burnup_request};
use std::path::PathBuf;

fn second(at: &str) -> u64 {
    at.parse::<Timestamp>().unwrap().as_second() as u64
}

fn at(at: &str) -> Timestamp {
    at.parse().unwrap()
}

fn event(seq: u64, ts: &str, task: &str, kind: EventKind, actor: Actor) -> Event {
    Event {
        seq,
        ts: at(ts),
        task: Some(task.into()),
        run: None,
        kind,
        actor,
        detail: serde_json::Value::Null,
    }
}

#[test]
fn dst_ticks_follow_real_local_instants_and_calendar_midnights() {
    let tz = TimeZone::get("America/New_York").unwrap();
    let spring = window(Range::Today, second("2026-03-08T12:00:00Z"), &tz).unwrap();
    let hours = ticks(&spring, 0, spring.now_minutes());
    assert!(hours[1].label.starts_with("01:00"));
    assert!(hours[2].label.starts_with("03:00"));
    assert_eq!(spring.to.as_second() - spring.from.as_second(), 23 * 3600);
    let fall = window(Range::Today, second("2026-11-01T12:00:00Z"), &tz).unwrap();
    let hours = ticks(&fall, 0, fall.now_minutes());
    assert!(hours[1].label.starts_with("01:00"));
    assert!(hours[2].label.starts_with("01:00"));
    assert_ne!(hours[1].label, hours[2].label);
    let week = window(Range::SevenDays, second("2026-03-10T12:00:00Z"), &tz).unwrap();
    let days = ticks(&week, 0, week.now_minutes());
    assert_eq!(days[5].at - days[4].at, 23 * 60);
    let feed = Feed {
        now: second("2026-11-15T15:00:00Z"),
        time_zone: Some(tz),
        ..Feed::default()
    };
    assert!(when("2026-07-15T15:00:00Z", &feed).ends_with("11:00"));
}

#[test]
fn scope_selection_requests_the_selected_daemon_burnup() {
    let mut state = State::default();
    for scope in [
        BurnupScope::Workspace,
        BurnupScope::Project("app".into()),
        BurnupScope::Plan("T-1".into()),
    ] {
        state.set_burnup_scope(scope.clone());
        assert_eq!(
            burnup_request(&state),
            DetailRef::Burnup { scope: scope.key() }
        );
    }
    state.set_grouping(Grouping::ByAgent);
    let window = window(Range::Today, second("2026-11-15T12:00:00Z"), &TimeZone::UTC).unwrap();
    assert!(matches!(
        request(&state, &window),
        DetailRef::Timeline {
            by: TimelineBy::Agent,
            ..
        }
    ));
}

/// Runs and events become spans: a block waits until the task's next move, an open run is idle
/// from when its output went quiet, and the owner's moves are on their own lane.
#[test]
fn runs_and_events_draw_spans_waits_and_idle_on_the_task_lane() {
    let feed = Feed {
        now: second("2026-11-15T12:00:00Z"),
        time_zone: Some(TimeZone::UTC),
        ..Feed::default()
    };
    let window = window(Range::Today, feed.now, &TimeZone::UTC).unwrap();
    let run = Run {
        id: "R1".into(),
        task: "T-1".into(),
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
        started: at("2026-11-15T09:00:00Z"),
        ended: None,
        outcome: None,
        summary: None,
        head_sha: None,
        quiet_since: Some(at("2026-11-15T10:30:00Z")),
    };
    let page = TimelinePage {
        runs: vec![run],
        events: vec![
            event(1, "2026-11-15T08:00:00Z", "T-1", EventKind::Blocked, Actor::Auto),
            event(2, "2026-11-15T08:20:00Z", "T-1", EventKind::Commented, Actor::Owner),
            event(3, "2026-11-15T08:45:00Z", "T-1", EventKind::Resumed, Actor::Owner),
        ],
    };
    let plot = plot(&page, &window, Grouping::ByTask, &feed);
    let lane = &plot.lanes[0];
    let kinds: Vec<(Kind, i64, i64)> = lane
        .segments
        .iter()
        .map(|s| (s.kind, s.start, s.end))
        .collect();
    assert!(kinds.contains(&(Kind::Ask, 8 * 60, 8 * 60 + 45)));
    assert!(kinds.contains(&(Kind::Run, 9 * 60, 10 * 60 + 30)));
    assert!(kinds.contains(&(Kind::Idle, 10 * 60 + 30, 12 * 60)));
    assert_eq!(plot.owner.len(), 2);
    assert_eq!(plot.auto.len(), 1);
    let gaps = gaps(&plot, &window, &feed);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].kind, Kind::Idle);
}

#[test]
fn burnup_counts_created_and_landed_tasks_once() {
    let feed = Feed {
        now: second("2026-11-15T12:00:00Z"),
        time_zone: Some(TimeZone::UTC),
        ..Feed::default()
    };
    let burnup = WireBurnup {
        scope: "workspace".into(),
        events: vec![
            event(1, "2026-11-10T08:00:00Z", "T-1", EventKind::Created, Actor::Owner),
            event(2, "2026-11-11T08:00:00Z", "T-2", EventKind::Created, Actor::Owner),
            event(3, "2026-11-12T08:00:00Z", "T-1", EventKind::Landed, Actor::Owner),
            event(4, "2026-11-13T08:00:00Z", "T-1", EventKind::Landed, Actor::Owner),
        ],
    };
    let series = burnup_series(&burnup, feed.now, &feed).unwrap();
    assert_eq!(series.points.last().map(|p| (p.1, p.2)), Some((2, 1)));
}
