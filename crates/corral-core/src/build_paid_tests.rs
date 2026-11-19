use super::*;
use crate::connection::ConnectionId;
fn ledger() -> BuildPaidLedger {
    BuildPaidLedger {
        version: 1,
        store: "fixture".into(),
        session: "B-fixture1".into(),
        profile: "api".into(),
        approval: PaidApproval {
            policy_digest: "11".repeat(32),
            approval: "explicit".into(),
            allowance_nanos: 100,
            candidates: vec![
                ConnectionBinding::new(
                    ConnectionId::OpenaiApi,
                    Some("exact".into()),
                    "fixture".into(),
                    None,
                )
                .unwrap(),
            ],
        },
        generation: 1,
        cap_sha256: "22".repeat(32),
        ledger: BudgetLedger::default(),
        bounds: vec![],
        mixed: None,
        reservation_routes: vec![],
    }
}
fn request(id: &str, nanos: u64) -> BudgetRequest {
    BudgetRequest {
        connection: Some(ConnectionId::OpenaiApi),
        request: id.into(),
        generation: 1,
        tokens: 10,
        usd_nanos: nanos,
        priced: true,
        unbounded: false,
        response: None,
    }
}
#[test]
fn build_uncertain_retries_stay_reserved_without_a_spending_cap() {
    let mut paid = ledger();
    assert!(paid.reserve(request("uncertain", 60)).unwrap());
    assert!(paid.reserve(request("uncertain", 60)).unwrap()); // transport replay, not a second POST
    // Spending caps are disabled: requests past the recorded allowance are still admitted.
    assert!(paid.reserve(request("physical-retry", 60)).unwrap());
    assert!(paid.reserve(request("child", 40)).unwrap());
    assert!(paid.reserve(request("extra", 1)).unwrap());
    assert!(paid.settle("child", "response".into(), 11, 1).is_err());
    paid.settle("child", "response".into(), 2, 5).unwrap();
    paid.settle("child", "response".into(), 2, 5).unwrap();
    assert_eq!(paid.ledger.totals(), (32, 126));
    assert!(paid.reserve(request("child", 40)).unwrap()); // original bound survives settlement
    let before = paid.clone();
    paid.generation += 1;
    paid.cap_sha256 = "33".repeat(32);
    paid.follows(&before).unwrap();
    assert_eq!(paid.ledger.totals().1, 126);
    assert!(paid.reserve(request("stale-parent", 1)).is_err());
    paid.ledger.requests.remove(0);
    assert!(paid.follows(&before).is_err());
    let mut reset = before.clone();
    reset.approval.allowance_nanos += 1;
    assert!(reset.follows(&before).is_err());
}

#[test]
fn mixed_routes_share_one_ledger_and_settle_only_their_own_reservation() {
    let mut paid = ledger();
    let first = paid.route().unwrap().digest.clone();
    let mut second_approval = paid.approval.clone();
    second_approval.candidates[0] = ConnectionBinding::new(
        ConnectionId::OpenaiApi,
        Some("other".into()),
        "second-model".into(),
        None,
    )
    .unwrap();
    let second = second_approval.candidates[0].digest.clone();
    paid.mixed = Some(MixedBuildApproval {
        approval: "explicit shared cap".into(),
        presets: [("child".into(), "second".into())].into(),
        allowance_nanos: 80,
        routes: [
            ("first".into(), paid.approval.clone()),
            ("second".into(), second_approval),
        ]
        .into(),
    });
    assert!(
        paid.reserve_for(request("pending", 50), Some(&first))
            .unwrap()
    );
    assert!(
        paid.reserve_for(request("second", 30), Some(&second))
            .unwrap()
    );
    assert!(
        paid.reserve_for(request("pending", 50), Some(&second))
            .is_err()
    );
    assert!(
        paid.settle_for("second", "id".into(), 5, 20, Some(&first))
            .is_err()
    );
    paid.settle_for("second", "id".into(), 5, 20, Some(&second))
        .unwrap();
    paid.validate().unwrap();
    let before = paid.clone();
    paid.generation += 1;
    paid.cap_sha256 = "33".repeat(32);
    paid.follows(&before).unwrap();
    assert_eq!(paid.ledger.totals().1, 70);
}
