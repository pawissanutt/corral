use corral_core::Limits;
use corral_ops::SlotUsage;
use unicode_width::UnicodeWidthStr;

/// Slot use (T-053): `"2/3 used: 1 task · 1 review"`. Zero components are left out; review and gate work that
/// `limits.count_reviews` / `count_gates` keeps off the count is shown but marked `uncounted`.
pub fn slot_summary(u: &SlotUsage, limits: &Limits) -> String {
    let parts = slot_parts(u, limits, false);
    let used = format!("{}/{} used", u.total(), limits.running);
    if parts.is_empty() {
        used
    } else {
        format!("{used}: {}", parts.join(" · "))
    }
}

/// The breakdown pieces, long (`"1 review"`, `"1 gate uncounted"`) or short (`"r1"`, `"(g1)"`).
fn slot_parts(u: &SlotUsage, limits: &Limits, short: bool) -> Vec<String> {
    let parts = [
        (u.tasks, "task", "tasks", 't', true),
        (u.reviews, "review", "reviews", 'r', limits.count_reviews),
        (u.gates, "gate", "gates", 'g', limits.count_gates),
        (u.reservations, "reserved", "reserved", 'q', true),
    ];
    parts
        .into_iter()
        .filter(|p| p.0 > 0)
        .map(|(n, one, many, letter, counted)| match (short, counted) {
            (true, true) => format!("{letter}{n}"),
            (true, false) => format!("({letter}{n})"),
            (false, c) => {
                let word = if n == 1 { one } else { many };
                format!("{n} {word}{}", if c { "" } else { " uncounted" })
            }
        })
        .collect()
}

/// The Running column's title badge: the slot fraction plus, when anything besides tasks holds or reserves a
/// slot, the breakdown - long when it fits in `avail` cells, else short, else just the fraction.
pub(super) fn slot_badge(u: &SlotUsage, limits: &Limits, avail: usize) -> String {
    let frac = format!(" {}/{} ", u.total(), limits.running);
    if u.reviews + u.gates + u.reservations == 0 {
        return frac;
    }
    [
        slot_parts(u, limits, false).join(" · "),
        slot_parts(u, limits, true).join(" "),
    ]
    .into_iter()
    .map(|parts| format!(" {parts} ·{frac}"))
    .find(|s| s.width() <= avail)
    .unwrap_or(frac)
}
