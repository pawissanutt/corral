use super::*;

fn fleet_delta(owner: &mut Client) -> SubDelta {
    let frame = read_frame(&mut owner.0).unwrap().unwrap();
    let notice: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
    notice.params
}

#[test]
fn a_relaunched_parent_still_lists_its_retained_child_and_reopens_its_conversation() {
    use corral_protocol::detail::{SessionItem, SessionItemPage};

    let publish_live = Arc::new(AtomicBool::new(false));
    let publish = Arc::clone(&publish_live);
    let (fixture, _) = launch_fixture_with_resume(move |script| {
        let publish = Arc::clone(&publish);
        script.reply("live_since", move |_, request, _| {
            let after = request["args"]["seq"].as_u64().unwrap();
            let live = publish.load(Ordering::Relaxed);
            let mut events = Vec::new();
            if after == 0 {
                events.push(json!({"seq":1, "at":AT, "kind":"live_text", "turn":1, "message":1, "offset":0, "text":"New host"}));
            }
            if live && after < 2 {
                events.push(json!({"seq":2, "at":AT, "kind":"live_fleet"}));
                return json!({"last":2, "next":2, "more":false, "events":events,
                    "fleet":{
                        "children":[{"id":"overlap", "kind":"slice", "title":"Live row", "state":"running", "model":MODEL,
                            "tokens":8, "steerable":true, "activity":"read"}],
                        "messages":[{"seq":1, "at":1, "from":"parent", "to":"overlap", "text":"Live message", "delivery":"delivered"}],
                        "messages_total":1, "max_parallel":3
                    }});
            }
            let last = if live { 2 } else { 1 };
            json!({"last":last, "next":last, "more":false, "events":events})
        })
    });
    let socket = fixture.unix_server();
    let mut owner = Client::connect_with(
        &socket,
        vec![wire::BUILD_V1.into(), wire::app::SESSION_FLEET_V1.into()],
    );
    let row: AssistantRow = owner.ok(
        BUILD_START,
        json!({"project":"demo", "model":MODEL, "here":false}),
    );
    let parent = &row.id;
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
        "children":[{"id":"slice", "kind":"slice", "title":"Inspect", "state":"done", "model":MODEL, "tokens":3},
            {"id":"overlap", "kind":"slice", "title":"Saved row", "state":"running", "model":MODEL,
                "tokens":4, "steerable":true, "activity":"edit", "question":true, "pending_model":"old"}],
        "messages":[], "messages_total":0, "max_parallel":0
    }).to_string()).unwrap();
    owner.ok::<()>(SESSION_TERMINATE, json!({"id":parent}));
    crate::topics::start_session_live(fixture.shared()).unwrap();
    let watch = json!({"topic":"session", "scope":parent});
    owner.ok::<()>(wire::SUB_OPEN, &watch);
    let SubDelta::Session {
        generation: None,
        fleet: Some(cold),
        ..
    } = fleet_delta(&mut owner)
    else {
        panic!("expected retained fleet while stopped");
    };
    assert_eq!(cold.children[0].id, "slice");
    let interrupted = &cold.children[1];
    assert_eq!(interrupted.state, "failed");
    assert!(!interrupted.steerable);
    assert!(
        interrupted.activity.is_none()
            && interrupted.question.is_none()
            && interrupted.pending_model.is_none()
    );
    owner.ok::<()>(wire::SUB_CLOSE, &watch);

    let resumed: SessionRelaunched = owner.ok(SESSION_RELAUNCH, json!({"id":parent}));
    assert_eq!(&resumed.id, parent);
    owner.ok::<()>(wire::SUB_OPEN, &watch);
    let SubDelta::Session {
        generation: Some(_),
        fleet: initial,
        cursor,
        ..
    } = fleet_delta(&mut owner)
    else {
        panic!("expected a successful live page with no host roster yet");
    };
    publish_live.store(true, Ordering::Relaxed);
    owner.ok::<()>(
        wire::SUB_ACK,
        json!({"topic":"session", "scope":parent, "cursor":cursor}),
    );
    let SubDelta::Session {
        generation: Some(_),
        fleet: Some(fleet),
        ..
    } = fleet_delta(&mut owner)
    else {
        panic!("expected a live roster replacement");
    };
    // Reopen the read-only conversation using the ID discovered after relaunch.
    let reopened = fleet
        .children
        .iter()
        .find(|row| row.id == "slice")
        .map(|row| {
            let id = format!("{parent}/child/{}", row.id);
            let child = page(&mut owner, &id);
            (id, child)
        });
    owner.ok::<()>(SESSION_TERMINATE, json!({"id":parent}));
    let (id, child) = reopened.expect("live roster hid the retained child");
    assert_eq!(initial.unwrap().children, cold.children);
    assert_eq!(fleet.children.len(), 2);
    assert_eq!(fleet.children[0].state, "done");
    assert!(!fleet.children[0].steerable);
    let live = &fleet.children[1];
    assert_eq!(live.id, "overlap");
    assert_eq!(live.title, "Live row");
    assert!(live.steerable);
    assert_eq!((fleet.messages_total, fleet.max_parallel), (1, 3));
    assert_eq!(fleet.messages[0].text, "Live message");
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
}
