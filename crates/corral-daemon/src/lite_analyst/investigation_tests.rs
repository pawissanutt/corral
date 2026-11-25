use super::*;

#[test]
fn a_forbidden_or_malformed_call_rejects_the_whole_batch_before_dispatch() {
    let mut seen = HashSet::new();
    let item = |id, name, arguments| json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments}});
    let search = item("one", "search_materials", r#"{"query":"public","limit":1}"#);
    for (name, args) in [
        ("read_result", r#"{"result":"known_foreign_handle"}"#),
        ("bash", r#"{"command":"unused"}"#),
        (
            "read_evidence",
            r#"{"source":"ok","start_byte":0,"max_bytes":1,"path":"/state"}"#,
        ),
        (
            "search_materials",
            r#"{"query":"public","limit":1,"limit":2}"#,
        ),
    ] {
        assert!(
            plan_calls(
                &json!({"tool_calls":[search,item("two",name,args)]}),
                &mut seen
            )
            .is_err()
        );
        assert!(seen.is_empty());
    }
    assert!(plan_calls(&json!({"tool_calls":[search]}), &mut seen).is_ok());
    assert!(plan_calls(&json!({"tool_calls":[search]}), &mut seen).is_err());
}

#[test]
fn report_draft_accepts_only_same_generation_citations_at_utf8_boundaries() {
    let issued = HashSet::from(["current".to_owned()]);
    let report = |start, handle| json!({"text":"é!","citations":[{"start_byte":start,"end_byte":2,"handle":handle}]});
    assert!(finish_draft(report(0, "current"), &issued).is_ok());
    assert!(finish_draft(report(1, "current"), &issued).is_err());
    assert!(finish_draft(report(0, "foreign"), &issued).is_err());
}
