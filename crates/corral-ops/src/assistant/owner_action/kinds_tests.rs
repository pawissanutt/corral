use super::*;
#[test]
fn assistant_gate_diagnosis_cannot_renew_the_owner_investigation_budget() {
    let args = vec!["--file".into(), "diagnosis.json".into()];
    let action = resolve("gate_triage", Some("T-1"), &args, None).unwrap();
    assert_eq!(action.class, Class::Ordinary);
    let mut renewal = args;
    renewal.push("--owner".into());
    assert!(resolve("gate_triage", Some("T-1"), &renewal, None).is_err());
}
