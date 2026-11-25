use super::*;

const LAUNCHER: &[u8] =
    include_bytes!("../../../../../../docs/contracts/lite-analyst-launcher-v1/launcher-v1.json");

fn example() -> Vec<Value> {
    let packet: Value = serde_json::from_slice(LAUNCHER).unwrap();
    packet["provider"]["sse_example"]
        .as_array()
        .unwrap()
        .clone()
}

fn event(value: &Value) -> String {
    format!("data: {}\n\n", value.as_str().unwrap_or(&value.to_string()))
}

fn parse(events: &[Value]) -> Result<Value> {
    assistant(events.iter().map(event).collect::<String>().as_bytes())
}

#[test]
fn normalizes_closed_tool_and_stop_shapes() {
    let mut events = example();
    let call = &events[1]["choices"][0]["delta"]["tool_calls"][0];
    assert_eq!(
        parse(&events).unwrap(),
        json!({"role":"assistant","content":null,"tool_calls":[{"id":call["id"],"type":"function","function":call["function"]}]})
    );
    events[1]["choices"][0]["delta"] = json!({"content":"No authorized evidence."});
    events[2]["choices"][0]["finish_reason"] = json!("stop");
    assert_eq!(
        parse(&events).unwrap(),
        json!({"role":"assistant","content":"No authorized evidence."})
    );
}

#[test]
fn refuses_incomplete_late_and_schema_invalid_sse() {
    let good = example();
    assert!(parse(&good[..3]).is_err());
    assert!(parse(&good[..2]).is_err());
    let mut late = good.clone();
    late.push(good[1].clone());
    assert!(parse(&late).is_err());
    let mut mixed = good.clone();
    let mut text = good[1].clone();
    text["choices"][0]["delta"] = json!({"content":"mixed"});
    mixed.insert(2, text);
    assert!(parse(&mixed).is_err());
    let mut duplicate = good[1].clone();
    duplicate["choices"][0]["delta"]["tool_calls"][0]["index"] = json!(1);
    let mut broken = good.clone();
    broken.insert(2, duplicate);
    assert!(parse(&broken).is_err());
    for (index, pointer, value) in [
        (1, "/id", json!("changed_id")),
        (1, "/choices/0/index", json!(1)),
        (2, "/choices/0/finish_reason", json!("stop")),
        (
            1,
            "/choices/0/delta/tool_calls/0/function/name",
            json!("read_result"),
        ),
        (
            1,
            "/choices/0/delta/tool_calls/0/function/arguments",
            json!("{\"query\":\"x\",\"limit\":1,\"limit\":2}"),
        ),
        (
            1,
            "/choices/0/delta/tool_calls/0/function/arguments",
            json!("{\"query\":\"partial\""),
        ),
    ] {
        let mut bad = good.clone();
        *bad[index].pointer_mut(pointer).unwrap() = value;
        assert!(parse(&bad).is_err(), "accepted changed {pointer}");
    }
    let strict = good.iter().map(event).collect::<String>();
    assert!(
        assistant(
            strict
                .replace("\"model\":", "\"model\":\"fixture-v1\",\"model\":")
                .as_bytes()
        )
        .is_err()
    );
    assert!(assistant(strict.replace('\n', "\r\n").as_bytes()).is_err());
}

#[test]
fn applies_full_event_and_count_bounds_before_returning_content() {
    let mut events = example();
    let mut text = events[1].clone();
    text["choices"][0]["delta"] = json!({"content":"a"});
    events[1] = text.clone();
    events[2]["choices"][0]["finish_reason"] = json!("stop");
    events.splice(2..2, std::iter::repeat_n(text, MAX_EVENTS - 4));
    assert_eq!(events.len(), MAX_EVENTS);
    assert!(parse(&events).is_ok());
    events.insert(2, events[1].clone());
    assert!(parse(&events).is_err());
    events.drain(2..events.len() - 2);
    events[1]["choices"][0]["delta"]["content"] = json!("a".repeat(MAX_EVENT));
    assert!(parse(&events).is_err());
}
