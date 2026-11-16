use super::*;
use corral_protocol::detail::{ProfileUsage, UsageCompleteness, UsageDay, UsageQuota};
use corral_protocol::rows::LimitBasis;

fn coverage() -> UsageCompleteness {
    UsageCompleteness {
        scanned_rows: 5,
        ..super::super::completeness::complete()
    }
}

fn row(connection: &str, profile: Option<&str>, cost: Option<f64>) -> UsageRow {
    UsageRow {
        id: format!("{connection}/{profile:?}"),
        connection: connection.into(),
        profile: profile.map(str::to_owned),
        window: None,
        used: None,
        limit: None,
        limit_basis: LimitBasis::None,
        resets_at: None,
        today_usd: cost,
        week_usd: cost,
        cost_basis: Some(CostBasis::Reported),
        today_completeness: coverage(),
        week_completeness: coverage(),
        quotas: vec![],
        rev: 1,
    }
}

fn input<'a>(
    rows: Vec<&'a UsageRow>,
    series: Option<&'a UsageSeries>,
    tz: &'a TimeZone,
) -> Inputs<'a> {
    Inputs {
        rows,
        series,
        rows_error: None,
        rows_loading: false,
        series_error: None,
        series_stale: false,
        agents: vec![],
        assistants: vec![],
        period: Period::Today,
        now: 1_700_000_000,
        tz,
    }
}

fn usage(connection: &str, profile: &str, cost: Option<f64>) -> ProfileUsage {
    ProfileUsage {
        connection: connection.into(),
        profile: profile.into(),
        cost_usd: cost,
        cost_basis: Some(CostBasis::Reported),
        tokens: Some(1),
        completeness: coverage(),
    }
}

#[test]
fn partial_totals_keep_coverage_and_unpriced_contributions_on_every_surface() {
    let known = row("c", Some("known"), Some(2.0));
    let mut unknown = row("c", Some("unknown"), None);
    let partial = UsageCompleteness {
        partial: true,
        reason: Some("scan_budget".into()),
        unpriced_entries: 3,
        ..coverage()
    };
    unknown.today_completeness = partial.clone();
    unknown.week_completeness = partial.clone();
    let mut unpriced = usage("c", "unknown", None);
    unpriced.completeness = partial.clone();
    let series = UsageSeries {
        providers: vec![],
        days: vec![UsageDay {
            day: "2026-11-15".into(),
            by_profile: vec![usage("c", "known", Some(2.0)), unpriced],
        }],
        completeness: partial,
    };
    let tz = TimeZone::UTC;
    let mut inputs = input(vec![&known, &unknown], Some(&series), &tz);
    for period in [Period::Today, Period::Week, Period::Month] {
        inputs.period = period;
        let page = page(&inputs);
        let note = page.providers[0].big_note.as_deref().unwrap();
        assert!(
            note.contains("partial") && note.contains("scanned 5") && note.contains("3 unpriced")
        );
        assert!(page.providers[0].costs.contains("partial"));
        assert!(
            page.profiles[1]
                .cost_note
                .as_deref()
                .unwrap()
                .contains("3 unpriced")
        );
        assert!(
            page.costs.bars[0]
                .total_note
                .as_deref()
                .unwrap()
                .contains("3 unpriced")
        );
        assert!(page.costs.note.as_deref().unwrap().contains("partial"));
    }
}

#[test]
fn monthly_totals_and_series_legend_preserve_connection_and_unknown_profile_keys() {
    let rows = [
        row("a", Some("p"), None),
        row("b", Some("p"), None),
        row("a", None, None),
    ];
    let series = UsageSeries {
        providers: vec![],
        days: vec![UsageDay {
            day: "2026-11-15".into(),
            by_profile: vec![
                usage("a", "p", Some(2.0)),
                usage("b", "p", Some(3.0)),
                usage("a", "", Some(7.0)),
            ],
        }],
        completeness: coverage(),
    };
    let tz = TimeZone::UTC;
    let mut inputs = input(rows.iter().collect(), Some(&series), &tz);
    inputs.period = Period::Month;
    assert_eq!(row_cost(&rows[0], &inputs).total, 2.0);
    assert_eq!(row_cost(&rows[1], &inputs).total, 3.0);
    assert_eq!(row_cost(&rows[2], &inputs).total, 7.0);
    assert_eq!(page(&inputs).costs.legend.len(), 3);
}

#[test]
fn failed_or_partial_usage_never_claims_an_empty_store_and_reports_one_cause() {
    let tz = TimeZone::UTC;
    let mut inputs = input(vec![], None, &tz);
    inputs.rows_error =
        Some("app topic unavailable: app topic unavailable: unavailable: scan failed");
    inputs.series_error = Some("unavailable: scan failed");
    let failed = page(&inputs);
    assert_eq!(failed.notices.len(), 1);
    assert_eq!(failed.notices[0].matches("scan failed").count(), 1);
    assert!(failed.providers_empty.is_none());
    let stored_nothing = UsageSeries {
        providers: vec![],
        days: vec![],
        completeness: coverage(),
    };
    let empty_store = page(&input(vec![], Some(&stored_nothing), &tz));
    assert!(empty_store.costs.empty.is_some());
    assert_ne!(failed.costs.empty, empty_store.costs.empty);
    let series = UsageSeries {
        providers: vec![],
        days: vec![],
        completeness: Default::default(),
    };
    inputs.series = Some(&series);
    inputs.rows_error = None;
    inputs.series_error = None;
    assert!(page(&inputs).providers_empty.is_none());
}

#[test]
fn retained_series_errors_qualify_monthly_costs_as_stale() {
    let row = row("a", Some("p"), Some(1.0));
    let series = UsageSeries {
        providers: vec![],
        days: vec![UsageDay {
            day: "2026-11-15".into(),
            by_profile: vec![usage("a", "p", Some(1.0))],
        }],
        completeness: coverage(),
    };
    let tz = TimeZone::UTC;
    let mut inputs = input(vec![&row], Some(&series), &tz);
    inputs.period = Period::Month;
    inputs.series_error = Some("refresh failed");
    let page = page(&inputs);
    assert!(
        page.providers[0]
            .big_note
            .as_deref()
            .unwrap()
            .contains("stale")
    );
    assert!(
        page.profiles[0]
            .cost_note
            .as_deref()
            .unwrap()
            .contains("stale")
    );
    assert!(page.costs.note.as_deref().unwrap().contains("stale"));
    assert_eq!(page.notices.len(), 1);
}

#[test]
fn exhaustion_observations_do_not_fabricate_numerical_quotas() {
    let mut row = row("a", Some("p"), Some(1.0));
    row.quotas.push(UsageQuota {
        source: "availability_exhaustion".into(),
        scope: Some("account".into()),
        window: None,
        used: None,
        limit: None,
        headroom: None,
        resets_at: Some("2023-11-14T23:00:00Z".into()),
        observed_at: Some("2023-11-14T22:00:00Z".into()),
        exhausted: Some(true),
        completeness: coverage(),
    });
    let tz = TimeZone::UTC;
    let page = page(&input(vec![&row], None, &tz));
    let provider = &page.providers[0];
    assert_eq!(provider.used, None);
    assert!(provider.exhausted);
    // Only the reset clock carries digits; no use, limit or headroom figure is made up.
    assert!(
        provider
            .quota_line
            .replace("23:00", "")
            .chars()
            .all(|c| !c.is_ascii_digit()),
        "{}",
        provider.quota_line
    );
    assert_eq!(page.profiles[0].headroom.as_deref(), Some(UNKNOWN));
}

/// The card header once required an account-scoped fact and said "window unknown", "quota
/// unknown" and "reset unknown" over details listing that very window, use and reset; float
/// headroom also leaked as `66.00000000000001`.
#[test]
fn provider_header_reads_the_quota_its_card_lists() {
    let mut row = row("a", Some("p"), Some(1.0));
    row.quotas.push(UsageQuota {
        source: "provider_api".into(),
        scope: Some("profile".into()),
        window: Some("5 h".into()),
        used: Some(34.0),
        limit: Some(100.0),
        headroom: Some(0.66 * 100.0),
        resets_at: Some("2023-11-14T23:00:00Z".into()),
        observed_at: Some("2023-11-14T22:00:00Z".into()),
        exhausted: Some(false),
        completeness: coverage(),
    });
    let tz = TimeZone::UTC;
    let page = page(&input(vec![&row], None, &tz));
    let provider = &page.providers[0];
    assert_eq!(provider.used, Some(0.34));
    for said in [&provider.window, &provider.quota_line, &provider.headroom] {
        assert!(!said.contains(UNKNOWN) && !said.contains("66.0"), "{said}");
    }
    assert_eq!(page.profiles[0].headroom.as_deref(), Some("66"));
}
