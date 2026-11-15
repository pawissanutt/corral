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

fn number(value: Option<f64>) -> String {
    value.map_or_else(|| "unknown".into(), |value| value.to_string())
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

pub(super) fn unknown() -> QuotaView {
    QuotaView {
        source: "unknown".into(),
        scope: "unknown".into(),
        window: "unknown".into(),
        used: "unknown".into(),
        limit: "unknown".into(),
        headroom: "unknown".into(),
        resets_at: "unknown".into(),
        observed_at: "unknown".into(),
        exhausted: "unknown".into(),
        profile: "no profile recorded".into(),
        completeness: Some("partial · quota coverage unknown".into()),
    }
}
