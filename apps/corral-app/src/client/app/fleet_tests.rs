use super::*;

fn page(fleet: Value) -> SubDelta {
    let mut page = json!({
        "topic": "session", "scope": "T-1", "epoch": 1, "generation": 1,
        "events": [], "cursor": {"rev": 0, "after": null}, "more": false, "gap": null
    });
    if !fleet.is_null() {
        page["fleet"] = fleet;
    }
    serde_json::from_value(page).unwrap()
}

fn child(id: &str) -> Value {
    json!({
        "id": id, "kind": "slice", "title": id, "state": "running",
        "model": "provider/model", "tokens": 1
    })
}

fn fleet(children: Vec<Value>) -> Value {
    json!({"children": children, "messages": [], "messages_total": 0, "max_parallel": 3})
}

fn children(feed: Signal<Feed>) -> Option<Vec<String>> {
    feed.with_untracked(|feed| {
        feed.live.get("T-1")?.fleet.as_ref().map(|fleet| {
            fleet
                .children
                .iter()
                .map(|child| child.id.clone())
                .collect()
        })
    })
}

fn watch(app: &mut AppState, feed: Signal<Feed>, on: bool) {
    app.request(
        feed,
        ClientRequest::Watch {
            session: "T-1".into(),
            on,
        },
    );
}

fn capable() -> Feed {
    let mut feed = Feed::default();
    feed.capabilities.insert(SESSION_V1.into());
    feed.capabilities.insert(SESSION_FLEET_V1.into());
    feed
}

#[test]
fn a_fleet_page_replaces_the_previous_fleet_instead_of_appending() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(capable());
    let mut app = AppState::new();
    watch(&mut app, feed, true);
    app.rows(feed, page(fleet(vec![child("a"), child("b")])), false)
        .unwrap();
    app.rows(feed, page(fleet(vec![child("c")])), false)
        .unwrap();
    assert_eq!(children(feed), Some(vec!["c".to_owned()]));
    // corrald omits the fleet once the session has neither children nor messages.
    app.rows(feed, page(Value::Null), false).unwrap();
    assert_eq!(children(feed), None);
}

#[test]
fn ending_the_watch_clears_the_fleet() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(capable());
    let mut app = AppState::new();
    watch(&mut app, feed, true);
    app.rows(feed, page(fleet(vec![child("a")])), false)
        .unwrap();
    assert!(children(feed).is_some());
    watch(&mut app, feed, false);
    assert_eq!(children(feed), None);
}
