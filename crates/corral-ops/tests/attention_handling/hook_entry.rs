use super::*;
use crate::attention::handling_tests::fixture::{Fx, at};
use crate::owner_notice::{CRASH, CrashPoint};

#[test]
fn real_post_land_publish_is_at_most_once_through_crashes_and_new_incidents_are_distinct() {
    for point in [
        None,
        Some(CrashPoint::AfterClaim),
        Some(CrashPoint::BeforeNotify),
        Some(CrashPoint::AfterNotify),
        Some(CrashPoint::AfterOutcomeSave),
        Some(CrashPoint::AfterAppend),
        Some(CrashPoint::AfterSourceClear),
    ] {
        let fx = Fx::new();
        let task = fx
            .ctx
            .store
            .create("Landed", None, corral_core::Status::Done, "")
            .unwrap();
        let mut op: PostLandOperation = serde_json::from_value(json!({
            "version":1,"key":"fixture-op","project":"app","repo":fx.tmp.path(),
            "task":task.id,"run":"R1","base":"main","base_before":"abc","landed_sha":"def",
            "policy_digest":"policy","created":at(0),"landed":true,"abandoned":false,"notified":false,
            "hooks":[{"hook":{"name":"fixture","argv":["false"],"cwd":".","env":{},
                "timeout_secs":1,"max_attempts":2},"state":"failed","attempts":[],"reason":"fixture failure"}]
        })).unwrap();
        save(&fx.ctx, &op).unwrap();
        CRASH.with(|crash| crash.set(point));
        let result = publish(&fx.ctx, &mut op);
        assert_eq!(result.is_err(), point.is_some(), "{point:?}");
        let calls = fx.calls();
        let reopened = fx.reopen();
        crate::owner_notice::drain(&reopened).unwrap();
        let mut op = post_land_operations(&reopened, None).unwrap().remove(0);
        let first = op.notice.as_ref().unwrap().batch_id.clone();
        publish(&reopened, &mut op).unwrap();
        assert_eq!(fx.calls(), calls, "{point:?}");
        assert_eq!(fx.logical("notification_claimed").len(), 1);
        assert!(
            fx.ctx
                .store
                .get(&task.id)
                .unwrap()
                .attention
                .source_notices
                .is_empty()
        );
        // The existing explicit retry resets this mark. A later failed attempt is a new incident.
        op.notified = false;
        save(&reopened, &op).unwrap();
        publish(&reopened, &mut op).unwrap();
        assert_eq!(fx.calls(), calls + 1);
        assert_ne!(op.notice.as_ref().unwrap().batch_id, first);
        assert_eq!(fx.logical("notification_claimed").len(), 2);
    }
}
