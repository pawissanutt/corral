//! What the page's totals cover, said once and in words: how many records corral read, between
//! which of the viewer's calendar days, why it stopped, then what it could not price or date and
//! how many profiles have no recorded cost. A provider, profile or quota observation repeats
//! none of it; each says only where its own coverage differs.

use corral_protocol::detail::UsageCompleteness;
use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::count;

/// Whether the coverage leaves anything out of the totals it qualifies.
pub(super) fn incomplete(coverage: &UsageCompleteness) -> bool {
    coverage.partial || coverage.unpriced_entries > 0 || coverage.undated_entries > 0
}

/// `n` and the noun phrase that agrees with it.
fn counted(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", count(n), if n == 1 { one } else { many })
}

/// A recorded instant as the viewer's calendar day, `Oct 25`, with its year when that is not
/// this year.
fn day(at: &str, now: Timestamp, tz: &TimeZone) -> Option<String> {
    let at = at.parse::<Timestamp>().ok()?.to_zoned(tz.clone());
    let this_year = now.to_zoned(tz.clone()).year();
    let format = if at.year() == this_year {
        "%b %-d"
    } else {
        "%b %-d, %Y"
    };
    Some(at.strftime(format).to_string())
}

/// The scan in words, after "corral": `read 98,249 usage records from Oct 25 to Nov 16 and
/// stopped at its row limit`.
fn scan(coverage: &UsageCompleteness, now: Timestamp, tz: &TimeZone) -> String {
    let mut words = if coverage.scanned_rows == 0 {
        "did not report how much it read".to_owned()
    } else {
        format!(
            "read {}",
            counted(coverage.scanned_rows, "usage record", "usage records")
        )
    };
    let from = coverage
        .scanned_from
        .as_deref()
        .and_then(|at| day(at, now, tz));
    let to = coverage
        .scanned_to
        .as_deref()
        .and_then(|at| day(at, now, tz));
    match (from, to) {
        (Some(from), Some(to)) if from == to => words.push_str(&format!(" on {from}")),
        (Some(from), Some(to)) => words.push_str(&format!(" from {from} to {to}")),
        (Some(from), None) => words.push_str(&format!(" from {from}")),
        (None, Some(to)) => words.push_str(&format!(" through {to}")),
        (None, None) => {}
    }
    match coverage.reason.as_deref() {
        None | Some("coverage_unknown") => {}
        Some("row_limit") => words.push_str(" and stopped at its row limit"),
        Some("scan_budget") => words.push_str(" and stopped at its scan budget"),
        Some(other) => words.push_str(&format!(" ({})", other.replace('_', " "))),
    }
    words
}

/// What a total leaves out besides the scan: `439 entries have no price`.
fn gaps(coverage: &UsageCompleteness) -> Vec<String> {
    let mut gaps = Vec::new();
    if coverage.unpriced_entries > 0 {
        gaps.push(counted(
            coverage.unpriced_entries,
            "entry has no price",
            "entries have no price",
        ));
    }
    if coverage.undated_entries > 0 {
        gaps.push(counted(
            coverage.undated_entries,
            "entry has no date",
            "entries have no date",
        ));
    }
    gaps
}

/// The page's one coverage note: the scan when partial, what is unpriced or undated, how many
/// profiles have no recorded cost, and whether 30-day costs come from a retained series. None
/// when the totals are whole.
pub(super) fn page_note(
    coverage: Option<&UsageCompleteness>,
    missing: u64,
    stale_series: bool,
    now: Timestamp,
    tz: &TimeZone,
) -> Option<String> {
    let mut sentences = Vec::new();
    let mut gaps = coverage.map(gaps).unwrap_or_default();
    if missing > 0 {
        gaps.push(counted(
            missing,
            "profile has no recorded cost",
            "profiles have no recorded cost",
        ));
    }
    match coverage {
        Some(coverage) if coverage.partial => sentences.push(format!(
            "Totals are partial: corral {}.",
            scan(coverage, now, tz)
        )),
        _ if !gaps.is_empty() => sentences.push("Totals count recorded, priced costs only.".into()),
        _ => {}
    }
    if !gaps.is_empty() {
        sentences.push(format!("{}.", gaps.join("; ")));
    }
    if stale_series {
        sentences.push("30-day costs come from the last series corrald sent.".into());
    }
    (!sentences.is_empty()).then(|| sentences.join(" "))
}

/// Whether two coverages describe the same scan: rows, bounds and reason.
fn same_scan(a: &UsageCompleteness, b: &UsageCompleteness) -> bool {
    a.partial == b.partial
        && a.scanned_rows == b.scanned_rows
        && a.scanned_from == b.scanned_from
        && a.scanned_to == b.scanned_to
        && a.reason == b.reason
}

/// A provider's own scan, said only where it differs from the page's.
pub(super) fn differing_scan(
    own: &UsageCompleteness,
    page: Option<&UsageCompleteness>,
    now: Timestamp,
    tz: &TimeZone,
) -> Option<String> {
    (own.partial && !page.is_some_and(|page| same_scan(own, page)))
        .then(|| format!("Partial here: corral {}.", scan(own, now, tz)))
}

/// A profile's own unpriced and undated entries, for its row: `12 unpriced`.
pub(super) fn row_gaps(coverage: &UsageCompleteness) -> Option<String> {
    let mut parts = Vec::new();
    if coverage.unpriced_entries > 0 {
        parts.push(format!("{} unpriced", count(coverage.unpriced_entries)));
    }
    if coverage.undated_entries > 0 {
        parts.push(format!("{} undated", count(coverage.undated_entries)));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// A quota observation's coverage, when partial: the scan and its gaps on one line.
pub(super) fn observation(
    coverage: &UsageCompleteness,
    now: Timestamp,
    tz: &TimeZone,
) -> Option<String> {
    if !incomplete(coverage) {
        return None;
    }
    let mut parts = vec![format!("partial: corral {}", scan(coverage, now, tz))];
    parts.extend(gaps(coverage));
    Some(parts.join("; "))
}
