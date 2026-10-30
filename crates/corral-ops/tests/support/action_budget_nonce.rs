//! Pin the start identity, not the work: variable-width process nonces are read repeatedly.
use super::action_budget::{measure, unattributed};
use super::action_budget_fixture::{Kind, Variant, build, settle};

#[test]
fn start_attempt_width_explains_worker_and_plan_scope_byte_drift() {
    for (kind, expected) in [(Kind::Worker, 35), (Kind::AcceptPlanScopes, 63)] {
        let mut samples = Vec::new();
        for nonce in ["start-fixture-000", "start-fixture-0000"] {
            let bench = build(kind, Variant::Base);
            settle(&bench);
            crate::operating::pin_start_attempt(Some(nonce));
            let sample = measure(&bench, kind);
            assert!(sample.failures.is_empty(), "{:?}", sample.failures);
            samples.push(sample);
        }
        let [short, long] = samples.try_into().unwrap();
        let key = "decoded_doc_bytes.unattributed";
        let difference = long.cells[key] - short.cells[key];
        assert_eq!(difference, expected, "{kind:?}: one nonce byte");
        for (cell, value) in unattributed(&short.cells) {
            if cell != key {
                assert_eq!(Some(value), long.cells.get(cell), "{kind:?}: {cell}");
            }
        }
        println!(
            "{kind:?}: one action.attempt nonce byte adds {difference} unattributed decoded bytes; other cells unchanged"
        );
    }
}
