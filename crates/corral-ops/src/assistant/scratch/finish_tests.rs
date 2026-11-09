use super::*;
use corral_core::UsageEntry;

#[test]
fn only_an_explicit_new_task_finish_uses_the_adhoc_lane() {
    let mut options = Finish::default();
    assert_eq!(options.new_task_lane(false).unwrap(), CapacityLane::Normal);
    assert_eq!(options.new_task_lane(true).unwrap(), CapacityLane::Normal);
    options.adhoc = true;
    assert!(options.new_task_lane(false).is_err());
    assert_eq!(options.new_task_lane(true).unwrap(), CapacityLane::Adhoc);
    options.into = Some("T-1");
    assert!(options.new_task_lane(true).is_err());
}

#[test]
fn window_with_only_zero_usage_is_explicitly_unavailable() {
    let from: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
    let to: Timestamp = "2026-01-01T00:00:02Z".parse().unwrap();
    let entry = UsageEntry {
        at: Some(from),
        ..UsageEntry::default()
    };
    let (usage, note) = window_usage("A-1", Usage::from_entries(vec![entry]), from, to);
    assert!(usage.is_none());
    assert!(note.unwrap().contains("no nonzero usage"));

    let entry = UsageEntry {
        at: Some(to),
        input: 32,
        ..UsageEntry::default()
    };
    let (usage, note) = window_usage("A-1", Usage::from_entries(vec![entry.clone()]), from, to);
    assert_eq!(usage.unwrap().input, 32);
    assert!(note.is_none());
    let (usage, note) = window_usage("A-1", Usage::from_entries(vec![entry]), from, from);
    assert!(usage.is_none());
    assert!(note.unwrap().contains("no nonzero usage"));
}
