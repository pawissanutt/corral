use serde_json::Value;

use super::disposition;
use crate::RunProgress;

/// The host's TypeScript mirror reads the same cases (packages/corral-pi/test/request.test.ts).
const CASES: &str =
    include_str!("../../../../packages/corral-pi/test/fixtures/disposition-cases.json");

#[test]
fn disposition_matches_the_shared_host_cases() {
    // Given each shared case's canonical progress.
    let cases: Vec<Value> = serde_json::from_str(CASES).unwrap();
    assert!(cases.len() >= 5);
    for case in cases {
        let progress: Option<RunProgress> =
            serde_json::from_value(case["progress"].clone()).unwrap();
        // When corral derives the request disposition.
        let derived = serde_json::to_value(disposition(progress.as_ref())).unwrap();
        // Then it is exactly the case's expected machine value.
        assert_eq!(derived, case["disposition"], "{}", case["name"]);
    }
}
