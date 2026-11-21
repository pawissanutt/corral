use super::*;

#[test]
fn a_stopped_parent_lists_its_child_and_serves_its_tool_result_and_report() {
    use corral_protocol::detail::{SessionItem, SessionItemPage};

    let fixture = Fixture::new();
    let parent = "B-archive";
    let host = terminal(&fixture, parent, Script::default());
    let dir = dir(&fixture, parent);
    fs::create_dir_all(dir.join("runtime/auxiliary")).unwrap();
    fs::create_dir(dir.join("child-records")).unwrap();
    let relative = "runtime/auxiliary/77ec75cf-7d5d-48bc-822f-9ed151efce4a.jsonl";
    let output = format!("{}final tool line", "é".repeat(1200));
    let entry = |id, parent: Option<&str>, message| json!({"type":"message", "id":id, "parentId":parent, "message":message});
    let transcript = [
        entry("brief", None, json!({"role":"user", "content":"Inspect files"})),
        entry("call", Some("brief"), json!({"role":"assistant", "content":[{"type":"toolCall", "id":"tool", "name":"find"}]})),
        entry("result", Some("call"), json!({"role":"toolResult", "toolCallId":"tool", "toolName":"find", "content":[{"type":"text", "text":output}]})),
        entry("answer", Some("result"), json!({"role":"assistant", "content":[{"type":"text", "text":"Final answer"}]})),
    ].map(|entry| format!("{entry}\n")).concat();
    fs::write(dir.join(relative), &transcript).unwrap();
    fs::write(dir.join("child-records/slice.json"), json!({
        "session_file": relative,
        "detail": {"id":"slice", "kind":"slice", "title":"Inspect", "brief":"Inspect files", "brief_more":false,
            "profile":"default", "model":MODEL, "state":"done", "created_at":1, "usage":{"input":1,"output":2,"turns":1},
            "report":"Harness completion report", "report_more":false, "history":"d", "transcript":true}
    }).to_string()).unwrap();
    fs::write(dir.join("child-records/fleet"), json!({
        "children":[{"id":"slice", "kind":"slice", "title":"Inspect", "state":"done", "model":MODEL, "tokens":3}],
        "messages":[], "messages_total":0, "max_parallel":0
    }).to_string()).unwrap();
    drop(host);
    let socket = fixture.unix_server();
    let mut owner = Client::connect_with(&socket, vec![wire::app::SESSION_FLEET_V1.into()]);
    let id = format!("{parent}/child/slice");
    let child = page(&mut owner, &id);
    assert_eq!(child.host, HostPresence::NotRunning);
    assert!(!child.replyable);
    assert_eq!(
        child.messages.first().unwrap().id,
        format!("{id}/entry/brief")
    );
    assert_eq!(
        child.messages.last().unwrap().id,
        format!("{id}/entry/answer")
    );
    let mut next = child
        .messages
        .iter()
        .flat_map(|row| &row.items)
        .find_map(|item| match item {
            SessionItem::Continuation { token, .. } => Some(token.clone()),
            _ => None,
        });
    let mut full = String::new();
    while let Some(token) = next {
        let served: DetailReply = owner.ok(
            wire::app::DETAIL_GET,
            json!({"kind":"session_item", "id": id, "token":token}),
        );
        let Some(DetailBody::SessionItem(SessionItemPage {
            json, next: more, ..
        })) = served.body
        else {
            panic!("expected fragment")
        };
        full.push_str(&json);
        next = more;
    }
    let item: SessionItem = serde_json::from_str(&full).unwrap();
    assert!(
        matches!(item, SessionItem::ToolCall { output: Some(actual), paired: true, .. } if actual == output)
    );
    assert_eq!(fs::read_to_string(dir.join(relative)).unwrap(), transcript);
    let served: DetailReply = owner.ok(
        wire::app::DETAIL_GET,
        json!({"kind":"session_child", "id":parent, "child":"slice"}),
    );
    let Some(DetailBody::SessionChild(detail)) = served.body else {
        panic!("expected child detail")
    };
    assert_eq!(detail.report.as_deref(), Some("Harness completion report"));

    crate::topics::start_session_live(fixture.shared()).unwrap();
    owner.ok::<()>(wire::SUB_OPEN, json!({"topic":"session", "scope":parent}));
    let frame = read_frame(&mut owner.0).unwrap().unwrap();
    let notice: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
    let SubDelta::Session {
        generation: None,
        fleet: Some(fleet),
        ..
    } = notice.params
    else {
        panic!("expected retained fleet")
    };
    assert_eq!(fleet.children[0].id, "slice");
    assert!(!fleet.children[0].steerable);
}
