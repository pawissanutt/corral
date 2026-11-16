//! Quota observations remain evidence: exhaustion does not imply a numerical allowance.

use corral_protocol::detail::UsageQuota;
use jiff::Timestamp;
use jiff::tz::TimeZone;

use super::{QuotaView, completeness_label};

pub(super) fn fraction(quota: &UsageQuota) -> Option<f32> {
    match (quota.used, quota.limit) {
        (Some(used), Some(limit)) if limit > 0.0 => Some((used / limit) as f32),
        _ => None,
    }
}

pub(super) fn number(value: Option<f64>) -> String {
    let Some(value) = value.filter(|value| value.is_finite()) else {
        return "unknown".into();
    };
    let mut text = format!("{value:.2}");
    let end = text.trim_end_matches('0').trim_end_matches('.').len();
    text.truncate(end);
    if text == "-0" {
        text.remove(0);
    }
    text
}

fn instant(at: Option<&str>, tz: &TimeZone) -> String {
    at.and_then(|at| at.parse::<Timestamp>().ok())
        .map(|at| {
            at.to_zoned(tz.clone())
                .strftime("%Y-%m-%d %H:%M %Z")
                .to_string()
        })
        .unwrap_or_else(|| "unknown".into())
}

pub(super) fn view(quota: &UsageQuota, profile: Option<&str>, tz: &TimeZone) -> QuotaView {
    QuotaView {
        source: quota.source.clone(),
        profile: profile.unwrap_or("no profile recorded").into(),
        scope: quota.scope.clone().unwrap_or_else(|| "unknown".into()),
        window: quota.window.clone().unwrap_or_else(|| "unknown".into()),
        used: number(quota.used),
        limit: number(quota.limit),
        headroom: number(quota.headroom),
        resets_at: instant(quota.resets_at.as_deref(), tz),
        observed_at: instant(quota.observed_at.as_deref(), tz),
        exhausted: match quota.exhausted {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        }
        .into(),
        completeness: completeness_label(&quota.completeness),
    }
}

#[cfg(test)]
mod tests {
    use super::number;

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
            (None, "unknown"),
            (Some(f64::NAN), "unknown"),
            (Some(f64::INFINITY), "unknown"),
        ] {
            assert_eq!(number(value), expected);
        }
    }
}
