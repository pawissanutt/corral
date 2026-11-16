//! A provider's window as one quota observation says it. The card's header (window, use,
//! headroom, reset) and the first provenance line under it read the same observation, so the
//! header never calls unknown a fact its details hold. Amounts are corral's evidence only: an
//! exhausted window says "exhausted", not a guessed allowance.

use corral_protocol::detail::UsageQuota;
use corral_protocol::rows::UsageRow;
use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::{NO_PROFILE, QuotaLine, UNKNOWN, coverage, quota};

/// The source corrald names for an observation it has no quota source for.
const UNKNOWN_SOURCE: &str = "unknown";

/// The observation a provider's card is drawn from: an account-wide one before a narrower
/// scope, one with a known use fraction before one without, then the latest observed. Facts of
/// an unknown source never head a card.
pub(super) fn header<'a>(
    rows: impl Iterator<Item = &'a UsageRow>,
) -> Option<(&'a UsageRow, &'a UsageQuota)> {
    rows.flat_map(|row| row.quotas.iter().map(move |fact| (row, fact)))
        .filter(|(_, fact)| fact.source != UNKNOWN_SOURCE && quota::informative(fact))
        .max_by_key(|&(_, fact)| {
            (
                fact.scope.as_deref() == Some("account"),
                quota::fraction(fact).is_some(),
                fact.observed_at.as_deref(),
            )
        })
}

/// A recorded instant in the viewer's zone, as the app's other attributions read: `20:39` today,
/// `Nov 16 09:00` on another day.
pub(super) fn when(at: Option<&str>, now: Timestamp, tz: &TimeZone) -> Option<String> {
    let at = at?.parse::<Timestamp>().ok()?.to_zoned(tz.clone());
    let today = now.to_zoned(tz.clone()).date();
    Some(if at.date() == today {
        at.strftime("%H:%M").to_string()
    } else {
        at.strftime("%b %-d %H:%M").to_string()
    })
}

/// "5 h window", when the observation names one.
pub(super) fn window(fact: Option<&UsageQuota>) -> Option<String> {
    fact.and_then(|fact| fact.window.as_deref())
        .map(|window| format!("{window} window"))
}

/// The observation's amounts in one line: use against the limit, headroom, the reset.
pub(super) fn amounts(fact: &UsageQuota, now: Timestamp, tz: &TimeZone) -> String {
    let exhausted = fact.exhausted == Some(true);
    let mut parts = Vec::new();
    match (fact.used, fact.limit) {
        (Some(_), Some(_)) => parts.push(format!(
            "{} of {} used",
            quota::number(fact.used),
            quota::number(fact.limit)
        )),
        (Some(_), None) => parts.push(format!(
            "{} used · limit {UNKNOWN}",
            quota::number(fact.used)
        )),
        (None, Some(_)) => parts.push(format!("use {UNKNOWN} of {}", quota::number(fact.limit))),
        (None, None) if fact.headroom.is_none() && !exhausted => {
            parts.push("no amounts recorded".into());
        }
        (None, None) => {}
    }
    if exhausted {
        parts.push("exhausted".into());
    }
    match fact.headroom {
        Some(_) => parts.push(format!("{} left", quota::number(fact.headroom))),
        None if !exhausted && (fact.used.is_some() || fact.limit.is_some()) => {
            parts.push(format!("headroom {UNKNOWN}"));
        }
        None => {}
    }
    parts.push(
        when(fact.resets_at.as_deref(), now, tz)
            .map_or_else(|| "no reset recorded".into(), |at| format!("resets {at}")),
    );
    parts.join(" · ")
}

/// The headroom alone, for a line with no room for the amounts.
pub(super) fn headroom(fact: Option<&UsageQuota>) -> String {
    match fact {
        None => "no quota".into(),
        Some(fact) if fact.headroom.is_some() => {
            format!("{} left", quota::number(fact.headroom))
        }
        Some(fact) if fact.exhausted == Some(true) => "exhausted".into(),
        Some(_) => format!("headroom {UNKNOWN}"),
    }
}

/// One observation's provenance line; `with_amounts` for every observation but the header's.
/// What the observation does not name is left out rather than called unknown field by field.
pub(super) fn line(
    row: &UsageRow,
    fact: &UsageQuota,
    with_amounts: bool,
    now: Timestamp,
    tz: &TimeZone,
) -> QuotaLine {
    let mut parts = vec![row.profile.clone().unwrap_or_else(|| NO_PROFILE.into())];
    if with_amounts {
        parts.extend(window(Some(fact)));
        parts.push(amounts(fact, now, tz));
    }
    if fact.source != UNKNOWN_SOURCE {
        parts.push(fact.source.replace('_', " "));
    }
    parts.extend(fact.scope.as_deref().map(|scope| format!("{scope} scope")));
    parts.extend(when(fact.observed_at.as_deref(), now, tz).map(|at| format!("observed {at}")));
    QuotaLine {
        text: parts.join(" · "),
        exhausted: fact.exhausted == Some(true),
        completeness: coverage::observation(&fact.completeness, now, tz),
    }
}
