//! Daily bars retain connection/profile identity; their coverage is the page's one note.

use std::collections::BTreeMap;

use super::live::{Inputs, provider_label};
use super::totals::Sum;
use super::{CostBar, Costs, NO_PROFILE, Period, coverage};

fn day_label(day: &str, period: Period) -> String {
    let Ok(date) = day.parse::<jiff::civil::Date>() else {
        return day.to_owned();
    };
    match period {
        Period::Month => format!("{:02}-{:02}", date.month(), date.day()),
        Period::Today | Period::Week => date.strftime("%a").to_string(),
    }
}

pub(super) fn costs(input: &Inputs) -> Costs {
    let title = match input.period {
        Period::Month => "Cost per day · 30 days",
        Period::Today | Period::Week => "Cost per day · 7 days",
    }
    .to_owned();
    let Some(series) = input.series else {
        return Costs {
            title,
            legend: vec![],
            bars: vec![],
            note: None,
            empty: Some(
                if input.series_error.is_some() {
                    "Daily costs unavailable."
                } else {
                    "Loading daily costs from corrald…"
                }
                .into(),
            ),
        };
    };
    let note = (input.series_stale || input.series_error.is_some())
        .then(|| "Showing the last series corrald sent; it may be out of date.".to_owned());
    let mut totals: BTreeMap<(&str, &str), f64> = BTreeMap::new();
    let mut zero = Sum::default();
    for usage in series.days.iter().flat_map(|day| &day.by_profile) {
        *totals
            .entry((&usage.connection, &usage.profile))
            .or_default() += usage.cost_usd.unwrap_or(0.0);
        zero.add(usage.cost_usd, usage.cost_basis);
    }
    let mut ranked: Vec<_> = totals
        .into_iter()
        .filter(|(_, value)| *value > 0.0)
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    if ranked.is_empty() {
        let empty = if series.days.iter().all(|day| day.by_profile.is_empty())
            && !coverage::incomplete(&series.completeness)
            && !input.series_stale
            && input.series_error.is_none()
        {
            "corral recorded no usage in this period.".into()
        } else {
            format!("Recorded cost: {}", zero.figure())
        };
        return Costs {
            title,
            legend: vec![],
            bars: vec![],
            empty: Some(empty),
            note,
        };
    }
    let grouped = ranked.len() > 3;
    let named: Vec<_> = ranked
        .iter()
        .take(if grouped { 2 } else { 3 })
        .map(|(key, _)| *key)
        .collect();
    let mut legend: Vec<String> = named
        .iter()
        .map(|(connection, profile)| {
            format!(
                "{} / {}",
                provider_label(connection),
                if profile.is_empty() {
                    NO_PROFILE
                } else {
                    profile
                },
            )
        })
        .collect();
    if grouped {
        legend.push("other connections / profiles".into());
    }
    let days: Vec<_> = series
        .days
        .iter()
        .map(|day| {
            let mut values = [0.0; 3];
            let mut sum = Sum::default();
            for usage in &day.by_profile {
                sum.add(usage.cost_usd, usage.cost_basis);
                let slot = named
                    .iter()
                    .position(|&(connection, profile)| {
                        connection == usage.connection && profile == usage.profile
                    })
                    .unwrap_or(2);
                values[slot] += usage.cost_usd.unwrap_or(0.0);
            }
            (values, sum, day_label(&day.day, input.period))
        })
        .collect();
    let most = days
        .iter()
        .map(|(values, _, _)| values.iter().sum::<f64>())
        .fold(0.0, f64::max)
        .max(f64::MIN_POSITIVE);
    let bars = days
        .into_iter()
        .map(|(values, sum, day)| {
            let mut widths = [0; 3];
            let mut running = 0.0;
            let mut drawn = 0;
            for (slot, value) in values.into_iter().enumerate() {
                running += value;
                let end = (running / most * 100.0).round().min(100.0) as u32;
                widths[slot] = end.saturating_sub(drawn);
                drawn = end;
            }
            CostBar {
                day,
                values: widths,
                total_label: sum.figure(),
            }
        })
        .collect();
    Costs {
        title,
        legend,
        bars,
        empty: None,
        note,
    }
}
