use super::*;
use crate::model::daemon::detail_key;

pub(crate) fn attempt(state: GateAttemptState) -> GateAttempt {
    GateAttempt {
        id: "gate-one".into(),
        task: "T-1".into(),
        run: "R1".into(),
        sha: "0123456789abcdef".into(),
        round: 0,
        started: jiff::Timestamp::from_second(100).unwrap(),
        heartbeat: Some(jiff::Timestamp::from_second(102).unwrap()),
        last_output: Some(jiff::Timestamp::from_second(101).unwrap()),
        command: Some("just gate".into()),
        step: Some(1),
        steps: 3,
        state,
        finished: None,
        check: None,
    }
}

fn show(feed: &mut Feed, open: &GateOpen, page: &GateAttemptPage) {
    let target = target(open.task.as_str(), &open.attempt, None);
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            target,
            transcript_seams: Default::default(),
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::GateAttempt(page.clone())),
        },
    );
}

#[test]
fn the_log_catches_up_after_reconnect_without_moving_a_held_window() {
    let open = GateOpen {
        task: Id::new("T-1"),
        attempt: "gate-one".into(),
    };
    let mut feed = Feed {
        link: Link::Up,
        now: 104,
        ..Feed::default()
    };
    let mut first = GateAttemptPage {
        attempt: attempt(GateAttemptState::Running),
        start: 0,
        end: 5,
        total: 5,
        text: "first".into(),
    };
    show(&mut feed, &open, &first);
    assert!(due(&feed, &open, 102));
    let held = GateUi {
        before: None,
        held: Some(first.clone()),
    };
    feed.link = Link::Down;
    assert!(!due(&feed, &open, 102));
    assert_eq!(view(&feed, &open, &held).standing.0, Hue::Held);

    feed.link = Link::Up;
    feed.now = 107;
    let selector = target(open.task.as_str(), &open.attempt, None);
    feed.details.get_mut(&detail_key(&selector)).unwrap().error =
        Some(crate::model::daemon::ClientError {
            code: "unavailable".to_owned(),
            message: "temporary read failure".to_owned(),
        });
    assert_eq!(view(&feed, &open, &held).standing.0, Hue::Held);
    assert!(!due(&feed, &open, 105));
    assert!(due(&feed, &open, 102));

    first.attempt.state = GateAttemptState::Passed;
    first.attempt.finished = Some(jiff::Timestamp::from_second(109).unwrap());
    first.text.push_str("\nresumed");
    first.end = first.text.len() as u64;
    first.total = first.end;
    show(&mut feed, &open, &first);
    let pinned = view(&feed, &open, &held);
    assert_eq!(pinned.end, 5);
    assert_eq!(pinned.standing.0, Hue::Ok);
    assert_eq!(view(&feed, &open, &GateUi::default()).end, first.total);
    assert!(!due(&feed, &open, 102));
}
