//! Quota observations remain evidence: exhaustion does not imply a numerical allowance.

use corral_protocol::detail::UsageQuota;

use super::UNKNOWN;

pub(super) fn fraction(quota: &UsageQuota) -> Option<f32> {
    match (quota.used, quota.limit) {
        (Some(used), Some(limit)) if limit > 0.0 => Some((used / limit) as f32),
        _ => None,
    }
}

/// Whether the observation holds any fact. corrald lists a profile it has no quota source for
/// as an observation of unknown source with nothing in it; that is no observation.
pub(super) fn informative(quota: &UsageQuota) -> bool {
    quota.source != "unknown"
        || quota.window.is_some()
        || quota.used.is_some()
        || quota.limit.is_some()
        || quota.headroom.is_some()
        || quota.resets_at.is_some()
        || quota.exhausted.is_some()
}

pub(super) fn number(value: Option<f64>) -> String {
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return UNKNOWN.into();
    };
    let mut text = format!("{value:.2}");
    let end = text.trim_end_matches('0').trim_end_matches('.').len();
    text.truncate(end);
    if text == "-0" {
        text.remove(0);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{UNKNOWN, number};

    #[test]
    fn quota_numbers_round_float_residue_without_padding() {
        for (value, expected) in [
            (Some(65.99999999999999), "66"),
            (Some(21.999999999999996), "22"),
            (Some(100.0), "100"),
            (Some(0.5), "0.5"),
            (Some(1.234), "1.23"),
            (Some(1.236), "1.24"),
            (Some(-0.001), "0"),
            (None, UNKNOWN),
            (Some(f64::NAN), UNKNOWN),
            (Some(f64::INFINITY), UNKNOWN),
        ] {
            assert_eq!(number(value), expected);
        }
    }
}
