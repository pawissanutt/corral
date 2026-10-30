//! Cross-task PM request lookup must not hydrate unrelated tasks or settled receipts.
use corral_core::agent::RequestReceipt;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{Status, Store};
use serde_json::json;

fn receipt(request: &str, at: jiff::Timestamp) -> RequestReceipt {
    serde_json::from_value(json!({
        "version":1,"request_id":request,"op":"comment","payload_sha256":"a".repeat(64),
        "identity":{"store":"fixture","project":"app","role":"pm","session_id":"pm-session","generation":1},
        "state":"committed","result":{"ok":true},"at":at,"committed_at":at
    })).unwrap()
}

#[test]
fn pm_request_lookup_is_keyed_for_terminal_tasks_and_missing_requests() {
    let at = "2026-01-01T00:00:00Z".parse().unwrap();
    let wanted = "rq-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        store.fixture_pin_now(at).unwrap();
        let mut target = store
            .create("Target", Some("app"), Status::Done, "")
            .unwrap();
        for n in 0..2 * scale {
            target
                .agent_pm_receipts
                .push(receipt(&format!("rq-{n:032x}"), at));
        }
        let expected = receipt(wanted, at);
        target.agent_pm_receipts.push(expected.clone());
        store.fixture_put_task(&target).unwrap();
        for _ in 0..3 * scale {
            let mut old = store
                .create("Settled", Some("app"), Status::Done, "")
                .unwrap();
            old.agent_pm_receipts
                .push(receipt("rq-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", at));
            store.fixture_put_task(&old).unwrap();
        }
        let _hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(
            store.pm_request_receipt(wanted).unwrap(),
            Some((target.id.clone(), expected))
        );
        assert_eq!(
            store
                .pm_request_receipt("rq-cccccccccccccccccccccccccccccccc")
                .unwrap(),
            None
        );
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
    }
    assert_eq!(measured[0], measured[1]);
}
