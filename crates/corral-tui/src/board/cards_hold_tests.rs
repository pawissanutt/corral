use super::*;

#[test]
fn held_reason_is_visible_for_all_open_statuses_in_normal_and_compact_cards() {
    let dir = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(dir.path()).unwrap();
    let now = corral_core::clock::now();
    for status in [
        Status::Idea,
        Status::Backlog,
        Status::Ready,
        Status::Running,
        Status::NeedsMe,
        Status::Review,
    ] {
        let mut task = store.create("Held card", None, status, "").unwrap();
        task.hold = Some(corral_core::TaskHold {
            reason: "urgent task first".into(),
            at: now,
            focus: None,
            resume: None,
        });
        for compact in [false, true] {
            let mut config = store.config.clone();
            config.ui.compact_cards = compact;
            let lines = card_lines(&task, &[], &config, 80, false, now, CardExtra::default());
            let text = lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .map(|span| span.content.as_ref())
                .collect::<String>();
            assert!(
                text.contains("Held: urgent task first"),
                "{status:?} compact={compact}: {text}"
            );
        }
    }
}
