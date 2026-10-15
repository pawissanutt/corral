//! Shared cost readback for the Runs tab, kept separate to bound detail view size.

pub(super) fn summary(usage: Option<corral_core::Usage>) -> String {
    let Some(usage) = usage else {
        return "unknown".into();
    };
    let dollars = |cost: Option<f64>| {
        cost.map(|value| {
            if value > 0.0 && value < 0.01 {
                format!("${value:.4}")
            } else {
                format!("${value:.2}")
            }
        })
        .unwrap_or_else(|| "-".into())
    };
    let basis = match usage.cost_basis {
        Some(corral_core::CostBasis::Reported) => "reported",
        Some(corral_core::CostBasis::Estimated) => "estimated",
        Some(corral_core::CostBasis::Mixed) => "mixed",
        None => "unpriced",
    };
    format!(
        "reported {}; est. {}; {}t; {basis}; {} unpriced",
        dollars(usage.reported_usd.or(usage.cost_usd)),
        dollars(usage.estimated_usd),
        usage.total_tokens(),
        usage.unpriced_tokens
    )
}
