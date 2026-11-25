use super::*;

fn request(question: &str, history: &[Value]) -> Value {
    let mut messages = vec![
        json!({"role":"system","content":prompt().unwrap()}),
        json!({"role":"user","content":question}),
    ];
    messages.extend_from_slice(history);
    let tools: Vec<_> = model_descriptors().unwrap().as_array().unwrap().iter().map(|tool| json!({
        "type":"function", "function":{"name":tool["name"],"description":tool["description"],"parameters":tool["inputSchema"]}
    })).collect();
    json!({"model":"fixture-v1","stream":true,"messages":messages,"tools":tools})
}

#[test]
fn pinned_registry_partitions_and_enforces_closed_schema() {
    assert_eq!(wire::sha256(prompt().unwrap().as_bytes()), PROMPT_SHA256);
    let names = |descs: Value| {
        descs
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(model_descriptors().unwrap()), MODEL_NAMES);
    assert_eq!(names(descriptors(false).unwrap()), MODEL_NAMES[..3]);
    assert_eq!(names(descriptors(true).unwrap()), HOST_NAMES);
    assert!(
        validate_args(
            "search_materials",
            &json!({"query":"🐑".repeat(256),"limit":8})
        )
        .is_ok()
    );
    for input in [
        json!({"query":"x","limit":1,"scope":"extra"}),
        json!({"query":"x","limit":1.0}),
        json!({"query":"🐑".repeat(257),"limit":8}),
    ] {
        assert!(validate_args("search_materials", &input).is_err());
    }
    assert!(
        validate_args(
            "read_evidence",
            &json!({"source":"bad space","start_byte":0,"max_bytes":1})
        )
        .is_err()
    );
    assert!(validate_args("finish_report", &json!({"text":"x","citations":[{"start_byte":0,"end_byte":1,"handle":"a","label":"extra"}]})).is_err());
    assert!(
        validate_reply(
            "calculate_metric",
            &json!({"status":"error","code":"arbitrary"})
        )
        .is_err()
    );
    assert!(
        validate_reply(
            "calculate_metric",
            &json!({"status":"pending","result":"handle"})
        )
        .is_ok()
    );
    assert!(
        validate_reply(
            "read_result",
            &json!({"status":"pending","result":"handle","value_minor":10})
        )
        .is_err()
    );
}

#[test]
fn finalize_requires_exact_body_order_and_supervisor_history() {
    let q = "Approved question?";
    let raw_args = "{\"inputs\":[\"a\"],\"metric\":\"m\" }";
    let result = json!({"status":"ok","value_minor":-7,"unit":"u","limitations":"","result":"res","operation_version":"v1","cell":"c","result_kind":"captured","citation":"cite"});
    let content = String::from_utf8(wire::canonical_signed(&result).unwrap()).unwrap();
    let history = [
        json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"calculate_metric","arguments":raw_args}}]}),
        json!({"role":"tool","tool_call_id":"call_1","content":content}),
    ];
    let good = request(q, &history);
    assert_eq!(
        finalize(q, &good, &history).unwrap(),
        wire::canonical(&good).unwrap()
    );
    assert!(finalize(q, &good, &[]).is_err());
    for (pointer, replacement) in [
        ("/model", json!("not_fixture")),
        ("/stream", json!(false)),
        (
            "/messages/0/content",
            json!(format!("{}<cwd>", prompt().unwrap())),
        ),
        ("/messages/1/content", json!(format!("{q} extra"))),
        (
            "/messages/2/tool_calls/0/function/arguments",
            json!("{\"inputs\":[\"a\"],\"metric\":\"m\"}"),
        ),
        ("/tools/0/function/description", json!("fake description")),
    ] {
        let mut wrong = good.clone();
        *wrong.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            finalize(q, &wrong, &history).is_err(),
            "accepted change at {pointer}"
        );
    }
    let mut reordered = good.clone();
    reordered["tools"].as_array_mut().unwrap().swap(0, 1);
    assert!(finalize(q, &reordered, &history).is_err());

    let mut unclosed_history = history.clone();
    unclosed_history[1]["content"] = json!(format!(" {}", content));
    assert!(finalize(q, &request(q, &unclosed_history), &unclosed_history).is_err());
    let dup = [history.clone(), history].concat();
    assert!(finalize(q, &request(q, &dup), &dup).is_err());
}

#[test]
fn max_body_bound_counts_final_wire_bytes() {
    let q = "𐀀".repeat(MAX_BODY / 4);
    assert!(finalize(&q, &request(&q, &[]), &[]).is_err());
}
