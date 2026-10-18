use super::*;
use corral_core::Store;

fn fixture(id: &str) -> (tempfile::TempDir, TimelineTab) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    std::fs::write(
        store.dir().join("events.jsonl"),
        include_str!("../../../corral-ops/tests/fixtures/timeline-waits.jsonl"),
    )
    .unwrap();
    let entries = corral_ops::timeline(&store, id, TimelineFilter::All).unwrap();
    (
        dir,
        TimelineTab {
            entries: Some(Ok(entries)),
            ..TimelineTab::default()
        },
    )
}

#[test]
fn paired_wait_brackets_and_durations_fit_narrow_and_wide_rows() {
    // Given the same 65-minute fixture as the builder and CLI.
    let (_dir, tab) = fixture("201");
    let now = "2026-10-15T12:35:00Z".parse().unwrap();
    for width in [76, 116] {
        // When rendering the actual rows inside 80- and 120-column detail borders.
        let lines = tab.lines_at(width, now);
        // Then both endpoints retain the duration without wrapping or clipping the bracket.
        assert!(lines.iter().all(|line| line.width() <= width));
        assert_eq!(
            lines[1].to_string().split_whitespace().collect::<Vec<_>>(),
            ["time", "kind", "actor", "via", "run", "summary"]
        );
        assert!(lines[2].to_string().contains("┐ waited 1h 05m"));
        assert!(lines[3].to_string().contains("┘ waited 1h 05m"));
    }
}

#[test]
fn open_wait_age_refreshes_from_render_clock_without_reloading_events() {
    // Given a cached unresolved operation and two fixed render clocks.
    let (_dir, tab) = fixture("204");
    let first = "2026-10-15T12:10:00Z".parse().unwrap();
    let later = "2026-10-15T12:11:00Z".parse().unwrap();
    for width in [76, 116] {
        // When the same cached tab is rendered again.
        let before = tab.lines_at(width, first);
        let after = tab.lines_at(width, later);
        // Then the full open marker survives at both widths and its age advances.
        assert!(after.iter().all(|line| line.width() <= width));
        assert!(before[2].to_string().contains("┐ waiting 2h 10m (open)"));
        assert!(after[2].to_string().contains("┐ waiting 2h 11m (open)"));
        assert!(!after[3].to_string().contains("wait"));
    }
}
