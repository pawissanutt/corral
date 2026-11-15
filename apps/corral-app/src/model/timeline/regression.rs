use super::*;
use crate::model::timeline::{BurnupScope, burnup_request};
use corral_protocol::detail::{TimelineLane, TimelineSegment};

fn second(at: &str) -> u64 {
    at.parse::<Timestamp>().unwrap().as_second() as u64
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
    let env = crate::model::effect::Env {
        conn: crate::model::effect::Conn::Healthy,
        mode: crate::model::effect::DataMode::Live,
        form: crate::model::nav::Form::Phone,
    };
    assert!(
        crate::model::timeline::head(&state, env, true)
            .grouping
            .is_some()
    );
    let window = window(Range::Today, second("2026-11-15T12:00:00Z"), &TimeZone::UTC).unwrap();
    assert!(matches!(
        request(&state, &window),
        DetailRef::Timeline {
            by: TimelineBy::Agent,
            ..
        }
    ));
}

#[test]
fn timeline_titles_and_waits_belong_to_the_lane_and_partial_reads_are_disclosed() {
    let feed = Feed {
        now: second("2026-11-15T12:00:00Z"),
        time_zone: Some(TimeZone::UTC),
        ..Feed::default()
    };
    let window = window(Range::Today, feed.now, &TimeZone::UTC).unwrap();
    let page = TimelinePage {
        lanes: vec![TimelineLane {
            id: "T-1".into(),
            title: "T-1 · A title".into(),
            segments: vec![TimelineSegment {
                kind: TimelineKind::Ask,
                from: Some("2026-11-15T09:00:00Z".into()),
                to: None,
                note: None,
            }],
        }],
        owner_actions: vec![],
        pm: vec![],
        last_visit: None,
        completeness: Default::default(),
    };
    let plot = plot(&page, &window, &feed);
    assert_eq!(plot.lanes[0].title, "A title");
    assert!(plot.lanes[0].wait.is_some());
    assert!(plot.lanes[0].segments[0].note.is_none());
    assert!(plot.completeness.is_some());
}
