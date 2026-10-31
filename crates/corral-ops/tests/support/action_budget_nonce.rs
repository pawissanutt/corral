//! Pin the start identity, not the work: variable-width nonces are read repeatedly.
//! The site model was independently traced on 83dcc33e and the integrated path; see
//! docs/records/T-337-R2-nonce-model.md. These are derivatives, not history budgets.
use super::action_budget::{measure, unattributed};
use super::action_budget_fixture::{Kind, Variant, build, settle};

fn coefficient(kind: Kind) -> u64 {
    let original: &[(&str, u64)] = match kind {
        Kind::Worker => &[
            ("dispatch", 1),
            ("launch", 11),
            ("tracker target", 3),
            ("operating heads", 8),
            ("PM open heads", 4),
            ("finishing slots", 3),
            ("authorization start load", 1),
            ("authorization submission loads", 4),
        ],
        Kind::AcceptPlanScopes => &[
            ("dispatch", 1),
            ("launch", 10),
            ("plan target", 12),
            ("tracker target", 9),
            ("operating heads", 16),
            ("PM open heads", 4),
            ("finishing slots", 6),
            ("authorization start load", 1),
            ("authorization submission loads", 4),
        ],
        _ => panic!("nonce derivative model covers only worker and plan-scope acceptance"),
    };
    // Each case launches exactly one worker. These are the four added nonce-bearing
    // head decodes, not a tolerance: owning fresh-session update (before/after), the
    // parked-work admission scan, and the truthful role-session audit lookup.
    let integrated = [
        ("fresh session owning update", 2),
        ("parked waiting admission", 1),
        ("role-session audit", 1),
    ];
    original
        .iter()
        .chain(integrated.iter())
        .map(|(_, n)| n)
        .sum()
}

#[test]
fn start_attempt_width_explains_worker_and_plan_scope_byte_drift() {
    for kind in [Kind::Worker, Kind::AcceptPlanScopes] {
        let mut samples = Vec::new();
        // The additional width is a deterministic control, not a new measured expectation.
        let nonces = [
            "start-fixture-000",
            "start-fixture-0000",
            "start-fixture-000000",
        ];
        for nonce in nonces {
            let bench = build(kind, Variant::Base);
            settle(&bench);
            crate::operating::pin_start_attempt(Some(nonce));
            let sample = measure(&bench, kind);
            assert!(sample.failures.is_empty(), "{:?}", sample.failures);
            samples.push(sample);
        }
        let base = &samples[0];
        let key = "decoded_doc_bytes.unattributed";
        let expected = coefficient(kind);
        for (i, larger) in samples.iter().enumerate().skip(1) {
            let width = u64::try_from(nonces[i].len() - nonces[0].len()).unwrap();
            assert_eq!(
                larger.cells[key] - base.cells[key],
                expected * width,
                "{kind:?}: {width} additional nonce bytes"
            );
            // Retain the original exact comparison of every other unattributed/call cell.
            for (cell, value) in unattributed(&base.cells) {
                if cell != key {
                    assert_eq!(Some(value), larger.cells.get(cell), "{kind:?}: {cell}");
                }
            }
            // One nonce-bearing head is read by the real FU-3 sample on both paths.
            // Repeated/cached authorization fetches are all charged, never subtracted.
            for (cell, value) in &base.cells {
                let delta = match cell.as_str() {
                    "decoded_doc_bytes.unattributed" => expected * width,
                    "decoded_doc_bytes.followup.FU-3.sample" => width,
                    "decoded_doc_bytes.total" => (expected + 1) * width,
                    _ => 0,
                };
                assert_eq!(
                    larger.cells.get(cell).copied().unwrap_or(0),
                    value + delta,
                    "{kind:?}: independently predicted {cell}, width {width}"
                );
            }
        }
        println!(
            "{kind:?}: site model {expected} unattributed + 1 FU-3 byte per nonce byte; widths 1 and 3 match, every other cell unchanged"
        );
    }
}
