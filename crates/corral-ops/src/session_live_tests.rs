use super::*;
use serde_json::json;

#[test]
fn bridge_fleet_maps_and_retains_newest_hundred_with_total() {
    let messages: Vec<_> = (1..=101)
        .map(|seq| {
            json!({
                "seq": seq, "at": seq, "from": "parent", "to": "review",
                "text": "Ready?", "delivery": "delivered"
            })
        })
        .collect();
    let mut page: LivePage = serde_json::from_value(json!({
        "generation": 1, "events": [{"seq": 1, "generation": 1, "at": "now", "kind": "live_fleet"}],
        "last": 1, "next": 1, "more": false,
        "fleet": {
            "children": [{"id": "review", "kind": "review", "title": "Review", "state": "done",
                "model": "provider/model", "tokens": 42, "cost_usd": 0.01, "verdict": "approve"}],
            "messages": messages, "messages_total": 101, "max_parallel": 3
        }
    }))
    .unwrap();
    let fleet = page.fleet.as_mut().unwrap();
    bound_fleet(fleet, LIVE_FLEET_BYTES).unwrap();
    assert_eq!(fleet.children[0].tokens, 42);
    assert_eq!(
        fleet.children[0].verdict,
        Some(corral_protocol::subscription::SessionChildVerdict::Approve)
    );
    assert_eq!(fleet.messages.len(), 100);
    assert_eq!(fleet.messages[0].seq, 2);
    assert_eq!(fleet.messages.last().unwrap().seq, 101);
    assert_eq!(fleet.messages_total, 101);
    validate(&page, 0).unwrap();
    let legacy: LivePage = serde_json::from_value(json!({
        "generation": 1, "events": [], "last": 0, "next": 0, "more": false
    }))
    .unwrap();
    assert!(legacy.fleet.is_none());
}

#[test]
fn fleet_byte_bound_removes_whole_old_messages_before_children() {
    let mut fleet: SessionFleet = serde_json::from_value(json!({
        "children": [{"id": "review", "kind": "review", "title": "Review", "state": "running",
            "model": "provider/model", "tokens": 0}],
        "messages": (1..=3).map(|seq| json!({
            "seq": seq, "at": seq, "from": "parent", "to": "review",
            "text": "💬".repeat(2050), "delivery": "delivered"
        })).collect::<Vec<_>>(),
        "messages_total": 3, "max_parallel": 3
    }))
    .unwrap();
    bound_fleet(&mut fleet, 10 * 1024).unwrap();
    assert_eq!(fleet.children.len(), 1);
    assert_eq!(fleet.messages.len(), 1);
    assert_eq!(fleet.messages[0].seq, 3);
    assert_eq!(fleet.messages[0].text, "💬".repeat(2048));
    assert_eq!(fleet.messages_total, 3);
    assert!(serde_json::to_vec(&fleet).unwrap().len() <= 10 * 1024);
}

#[test]
fn empty_replacements_are_distinct_from_legacy_pages_and_rebinds_reconcile() {
    let mut page: LivePage = serde_json::from_value(json!({
        "generation": 1, "events": [], "last": 0, "next": 0, "more": false,
        "fleet": {"children": [], "messages": [], "messages_total": 0, "max_parallel": 3}
    }))
    .unwrap();
    bound_page_fleet(&mut page).unwrap();
    assert!(page.fleet.is_some());
    reconcile_rebind(&mut page, 42);
    let gap = page.gap.as_ref().unwrap();
    assert_eq!((gap.after, gap.resumed, gap.missing), (42, 1, 0));
    assert!(gap.reconcile);
    validate(&page, 0).unwrap();
    let mut legacy: LivePage = serde_json::from_value(json!({
        "generation": 1, "events": [], "last": 0, "next": 0, "more": false
    }))
    .unwrap();
    bound_page_fleet(&mut legacy).unwrap();
    assert!(legacy.fleet.is_none());
    assert!(legacy.gap.is_none());
}
