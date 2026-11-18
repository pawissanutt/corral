use super::*;
use crate::model::act::{self as acts, Act, ActStatus};
use crate::model::id::Id;
use corral_types::{Op, Status};

#[test]
fn unfinished_dependency_refuses_dispatch_until_forced() {
    let mut feed = feed(1_800_000_000, "2027-01-15".into(), "08:00".into(), None);
    let dispatch = Op::Dispatch {
        id: "T-12".into(),
        profile: None,
        base: None,
    };
    act(
        &mut feed,
        &Act::with_key(Id::new("plain"), dispatch.clone(), false),
    )
    .unwrap();
    let Some(ActStatus::Refused(refusal)) = acts::status(&feed, "plain") else {
        panic!("dispatch with an unfinished dependency was not refused");
    };
    assert!(
        refusal
            .next
            .iter()
            .any(|next| next.force && next.op == dispatch)
    );
    assert_eq!(
        feed.board.task("T-12").map(|task| task.status()),
        Some(Status::Ready)
    );

    act(&mut feed, &Act::with_key(Id::new("forced"), dispatch, true)).unwrap();
    assert_eq!(acts::status(&feed, "forced"), Some(ActStatus::Done));
    assert_eq!(
        feed.board.task("T-12").map(|task| task.status()),
        Some(Status::Running)
    );
    assert!(feed.agents.contains_key("T-12_R1"));
    let forced =
        feed.changes.values().next_back().filter(|event| {
            event.kind == EventKind::Forced && event.task.as_deref() == Some("T-12")
        });
    assert_eq!(
        forced.map(|event| event.detail["skipped"].clone()),
        Some(serde_json::json!(["dependencies"]))
    );
}
