use super::*;
use corral_core::Store;

fn fixture(id: &str) -> (tempfile::TempDir, TimelineTab) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    std::fs::write(
        store.dir().join("events.jsonl"),
        [
            include_str!("../../../corral-ops/tests/fixtures/timeline-waits.jsonl"),
            include_str!("../../../corral-ops/tests/fixtures/timeline-wait-review.jsonl"),
        ]
        .concat(),
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

#[test]
fn review_repeated_waits_have_no_false_open_row() {
    // Given repeated questions resolved by the same reply.
    let (_dir, tab) = fixture("205");
    let now = "2026-10-15T12:10:00Z".parse().unwrap();
    for width in [76, 116] {
        // When rendering inside narrow and wide detail borders.
        let lines = tab.lines_at(width, now);
        // Then both closing brackets fit and no already-resolved opener appears open.
        assert!(lines.iter().all(|line| line.width() <= width));
        assert!(
            lines
                .iter()
                .all(|line| !line.to_string().contains("(open)"))
        );
        assert_eq!(lines[4].to_string().matches('┘').count(), 2);
    }
}

#[test]
fn review_dual_role_move_keeps_both_endpoints_at_both_widths() {
    // Given a moved row whose outgoing wait is either closed or still open.
    let now = "2026-10-15T12:10:00Z".parse().unwrap();
    for id in ["206", "207"] {
        let (_dir, tab) = fixture(id);
        for width in [76, 116] {
            // When rendering its two interval endpoints on the one event row.
            let lines = tab.lines_at(width, now);
            let moved = lines[3].to_string();
            // Then neither role is truncated, including the age of an outgoing open wait.
            assert!(lines.iter().all(|line| line.width() <= width));
            assert!(moved.contains('┘') && moved.contains('┐'), "{moved}");
            assert_eq!(moved.contains("(open)"), id == "207", "{moved}");
            if id == "207" {
                assert!(moved.contains("2h 05m"), "{moved}");
            }
        }
    }
}
