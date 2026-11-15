//! Accounting coverage wording shared by Usage and Timeline. Scan bounds describe examined
//! facts, not a guarantee that every fact between them was included.

use corral_protocol::detail::UsageCompleteness;

pub(super) fn complete() -> UsageCompleteness {
    UsageCompleteness {
        partial: false,
        reason: None,
        scanned_rows: 0,
        scanned_from: None,
        scanned_to: None,
        unpriced_entries: 0,
        undated_entries: 0,
    }
}

/// A visible qualification for an incomplete total, absent only for complete coverage.
pub fn completeness_label(coverage: &UsageCompleteness) -> Option<String> {
    if !coverage.partial && coverage.unpriced_entries == 0 && coverage.undated_entries == 0 {
        return None;
    }
    let mut parts = vec![
        "partial".to_owned(),
        format!("scanned {} rows", coverage.scanned_rows),
    ];
    match (&coverage.scanned_from, &coverage.scanned_to) {
        (Some(from), Some(to)) => parts.push(format!("{from}–{to}")),
        (Some(from), None) => parts.push(format!("from {from}")),
        (None, Some(to)) => parts.push(format!("through {to}")),
        (None, None) => {}
    }
    if coverage.unpriced_entries > 0 {
        parts.push(format!("{} unpriced", coverage.unpriced_entries));
    }
    if coverage.undated_entries > 0 {
        parts.push(format!("{} undated", coverage.undated_entries));
    }
    if let Some(reason) = &coverage.reason {
        parts.push(reason.replace('_', " "));
    }
    Some(parts.join(" · "))
}

/// Row/day totals from the same read share a scan; adding them must not multiply scan counts.
pub(super) fn merge(into: &mut UsageCompleteness, other: &UsageCompleteness) {
    into.partial |= other.partial;
    if into.reason.is_none() {
        into.reason.clone_from(&other.reason);
    }
    into.scanned_rows = into.scanned_rows.max(other.scanned_rows);
    if (other.scanned_from < into.scanned_from || into.scanned_from.is_none())
        && other.scanned_from.is_some()
    {
        into.scanned_from.clone_from(&other.scanned_from);
    }
    if other.scanned_to > into.scanned_to {
        into.scanned_to.clone_from(&other.scanned_to);
    }
    into.unpriced_entries = into.unpriced_entries.saturating_add(other.unpriced_entries);
    // Undated entries can affect every day of the same series.
    into.undated_entries = into.undated_entries.max(other.undated_entries);
}
