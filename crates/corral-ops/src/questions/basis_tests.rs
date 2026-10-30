use super::*;
use corral_core::Status;
use corral_core::work::{self, Counter, HotScope};
use serde_json::json;

#[test]
fn basis_keeps_the_asking_draft_and_keys_other_tasks_without_history_growth() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let fx = super::super::tests_support::fx();
        let mut asking = fx
            .ctx
            .store
            .create("Asking", Some("corral"), Status::Ready, "old spec")
            .unwrap();
        let old = crate::progress_spec_revision(&asking).unwrap();
        asking.body = "uncommitted spec".into();
        let at = "2026-01-01T00:00:00Z".parse::<jiff::Timestamp>().unwrap();
        asking.comments.push(
            serde_json::from_value(json!({"author":"owner","at":at,"text":"draft"})).unwrap(),
        );
        let own = crate::progress_spec_revision(&asking).unwrap();
        let mut target = fx
            .ctx
            .store
            .create("Other", Some("corral"), Status::Ready, "other spec")
            .unwrap();
        target.created = at;
        target.updated = at;
        target.status_since = Some(at);
        target.comments = (0..scale)
            .map(|n| {
                serde_json::from_value(
                    json!({"author":"owner","at":at,"text":format!("comment {n:04}")}),
                )
                .unwrap()
            })
            .collect();
        fx.ctx.store.fixture_put_task(&target).unwrap();
        let other = crate::progress_spec_revision(&target).unwrap();
        let _hot = HotScope::enter();
        let before = work::snapshot();
        assert!(resolve(&fx.ctx, &asking, &format!("spec:{}@{own}", asking.id)).is_ok());
        assert!(resolve(&fx.ctx, &asking, &format!("spec:{}@{old}", asking.id)).is_err());
        assert!(resolve(&fx.ctx, &asking, &format!("comment:{}#1", asking.id)).is_ok());
        assert!(resolve(&fx.ctx, &asking, &format!("spec:{}@{other}", target.id)).is_ok());
        assert!(resolve(&fx.ctx, &asking, &format!("comment:{}#1", target.id)).is_ok());
        assert!(resolve(&fx.ctx, &asking, &format!("plan:{}@missing", target.id)).is_err());
        assert!(
            resolve(
                &fx.ctx,
                &asking,
                &format!("decision:{}/R1/missing", target.id)
            )
            .is_err()
        );
        let after = work::snapshot();
        measured.push(
            [
                Counter::TaskHeadDecodes,
                Counter::NestedRowDecodes,
                Counter::HistoryRowDecodes,
                Counter::FullTaskHydrations,
                Counter::DecodedDocBytes,
                Counter::ViewsOpened,
            ]
            .map(|counter| after.total(counter) - before.total(counter)),
        );
    }
    assert_eq!(measured[0], measured[1]);
    assert_eq!(measured[0][3], 0);
}
